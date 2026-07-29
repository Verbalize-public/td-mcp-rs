//! Tool descriptors, dispatch, and bridge-coupled execution.
//!
//! `dispatch_tool` is async: it enqueues a task on the per-pid queue, delegates
//! the live bridge RPC to the daemon-supplied [`BridgeRpc`] impl, then records
//! the task outcome on the registry. Diagnostic mapping lives in
//! [`crate::outcomes`].

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tdmcp_core::{PidRegistry, TaskMode};
use thiserror::Error;
use tokio::sync::Mutex;

use crate::bridge_rpc::{BridgeRpc, BridgeRpcError};
use crate::fleet::{fleet_summary, FleetParams};
use crate::outcomes::{map_inspect_outcome, map_perception_outcome, map_script_outcome};

/// Per-call bridge wait budget. A timeout fails the **wait** — it does not
/// claim TD cancelled the work.
pub const BRIDGE_TIMEOUT: Duration = Duration::from_secs(30);

/// Static tool descriptor for `describe_tools` / MCP list.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDescriptor {
    /// Tool name.
    pub name: String,
    /// One-line description.
    pub description: String,
}

/// Tool call failures mapped to diagnostics.
#[derive(Debug, Error)]
pub enum ToolCallError {
    /// Unknown tool name.
    #[error("unknown tool: {0}")]
    UnknownTool(String),
    /// JSON args parse failure.
    #[error("invalid arguments: {0}")]
    InvalidArgs(String),
    /// Domain / queue / bridge failure with diagnostics.
    #[error("{summary}")]
    Failed {
        /// Short summary.
        summary: String,
        /// Structured diagnostics.
        diagnostics: tdmcp_diagnostics::Diagnostics,
    },
}

/// Catalogue of v1 tools (provisional names from README).
#[must_use]
pub fn tool_descriptors() -> Vec<ToolDescriptor> {
    vec![
        ToolDescriptor {
            name: "fleet".into(),
            description: "Fleet view — TD processes by pid, bridge, tasks, cancelled traces".into(),
        },
        ToolDescriptor {
            name: "execute_python".into(),
            description: "Run Python in TD; OpPath-exempt with tdmcp_resolve helper".into(),
        },
        ToolDescriptor {
            name: "inspect".into(),
            description: "Structural subtree read (nodes/params/errors)".into(),
        },
        ToolDescriptor {
            name: "capture".into(),
            description: "Perception capture (top/preview/…)".into(),
        },
        ToolDescriptor {
            name: "describe_tools".into(),
            description: "Manifest of available tools".into(),
        },
    ]
}

/// Args for execute_python.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutePythonParams {
    /// Target pid.
    pub pid: u32,
    /// Script body.
    pub script: String,
    /// Exclusive enqueue.
    #[serde(default)]
    pub exclusive: bool,
    /// Optional context path (exposed to script as helper; not enforced).
    #[serde(default)]
    pub context_path: Option<String>,
}

/// Args for capture (perception).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureParams {
    /// Target pid.
    pub pid: u32,
    /// Operator path (OpPath; relative to contextPath or /project1).
    pub path: String,
    /// Capture mode: `top` | `preview` | `auto`.
    #[serde(default = "default_capture_mode")]
    pub mode: String,
    /// Resolution base for relative `path`.
    #[serde(default)]
    pub context_path: Option<String>,
}

fn default_capture_mode() -> String {
    "auto".into()
}

/// Args for inspect.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InspectParams {
    /// Target pid.
    pub pid: u32,
    /// Operator path (OpPath).
    pub path: String,
    /// Resolution base for relative `path`.
    #[serde(default)]
    pub context_path: Option<String>,
    /// Sections to include: `nodes` | `params` | `errors`.
    #[serde(default)]
    pub include: Vec<String>,
    /// Structural detail level: `summary` | `detailed`.
    #[serde(default = "default_detail_level")]
    pub detail_level: String,
}

fn default_detail_level() -> String {
    "summary".into()
}

/// Outcome of a bridge-driven tool call, as reported to the mapper.
#[derive(Debug)]
pub enum BridgeOutcome {
    /// Bridge returned a result value (may encode a soft failure).
    Ok(Value),
    /// Queue rejected the enqueue (exclusive-while-busy).
    QueueBusy,
    /// Transport / timeout / disconnect failure.
    Transport(BridgeRpcError),
}

/// Dispatch a named tool call to a JSON result.
///
/// Never holds the registry lock across a bridge await.
pub async fn dispatch_tool(
    registry: &Arc<Mutex<PidRegistry>>,
    catalog: &tdmcp_diagnostics::Catalog,
    bridge: &dyn BridgeRpc,
    name: &str,
    args: Value,
) -> Result<Value, ToolCallError> {
    match name {
        "fleet" => {
            let params: FleetParams = serde_json::from_value(args)
                .map_err(|e| ToolCallError::InvalidArgs(e.to_string()))?;
            let reg = registry.lock().await;
            Ok(serde_json::to_value(fleet_summary(&reg, &params))
                .map_err(|e| ToolCallError::InvalidArgs(e.to_string()))?)
        }
        "describe_tools" => Ok(serde_json::json!({ "tools": tool_descriptors() })),
        "execute_python" => {
            let params: ExecutePythonParams = serde_json::from_value(args)
                .map_err(|e| ToolCallError::InvalidArgs(e.to_string()))?;
            let outcome = enqueue_and_call(
                registry,
                bridge,
                params.pid,
                "PythonEval",
                mode_of(params.exclusive),
                "execute_python",
                serde_json::json!({
                    "script": params.script,
                    "contextPath": params.context_path,
                }),
            )
            .await;
            map_script_outcome(catalog, params.pid, outcome)
        }
        "capture" => {
            let params: CaptureParams = serde_json::from_value(args)
                .map_err(|e| ToolCallError::InvalidArgs(e.to_string()))?;
            let outcome = enqueue_and_call(
                registry,
                bridge,
                params.pid,
                "Capture",
                TaskMode::Shared,
                "capture",
                serde_json::json!({
                    "path": params.path,
                    "mode": params.mode,
                    "contextPath": params.context_path,
                }),
            )
            .await;
            map_perception_outcome(
                catalog,
                params.pid,
                params.path,
                params.context_path,
                outcome,
            )
        }
        "inspect" => {
            let params: InspectParams = serde_json::from_value(args)
                .map_err(|e| ToolCallError::InvalidArgs(e.to_string()))?;
            let outcome = enqueue_and_call(
                registry,
                bridge,
                params.pid,
                "Inspect",
                TaskMode::Shared,
                "inspect",
                serde_json::json!({
                    "path": params.path,
                    "contextPath": params.context_path,
                    "include": params.include,
                    "detailLevel": params.detail_level,
                }),
            )
            .await;
            map_inspect_outcome(
                catalog,
                params.pid,
                params.path,
                params.context_path,
                outcome,
            )
        }
        other => Err(ToolCallError::UnknownTool(other.to_owned())),
    }
}

fn mode_of(exclusive: bool) -> TaskMode {
    if exclusive {
        TaskMode::Exclusive
    } else {
        TaskMode::Shared
    }
}

/// Enqueue (eager — preserves exclusive-while-busy semantics), then call the
/// bridge with a timeout. The daemon actor owns queue progression
/// (`start_next` / `complete_task`) so it stays coupled to the wire.
async fn enqueue_and_call(
    registry: &Arc<Mutex<PidRegistry>>,
    bridge: &dyn BridgeRpc,
    pid: u32,
    task_name: &str,
    mode: TaskMode,
    method: &str,
    params: Value,
) -> BridgeOutcome {
    {
        let mut reg = registry.lock().await;
        if let Err(e) = reg.enqueue(pid, task_name, mode) {
            return match &e {
                tdmcp_core::EnqueueError::Queue(_) => BridgeOutcome::QueueBusy,
                _ => BridgeOutcome::Transport(BridgeRpcError::NotConnected { pid }),
            };
        }
    }

    let call = bridge.call(pid, method, params);
    match tokio::time::timeout(BRIDGE_TIMEOUT, call).await {
        Ok(Ok(value)) => BridgeOutcome::Ok(value),
        Ok(Err(err)) => BridgeOutcome::Transport(err),
        Err(_) => BridgeOutcome::Transport(BridgeRpcError::Timeout {
            pid,
            budget_ms: BRIDGE_TIMEOUT.as_millis() as u64,
        }),
    }
}

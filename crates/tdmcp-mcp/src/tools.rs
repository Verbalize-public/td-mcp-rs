//! Tool descriptors, dispatch, and bridge-coupled execution.
//!
//! `dispatch_tool` is async: it enqueues a task on the per-pid queue, delegates
//! the live bridge RPC to the daemon-supplied [`BridgeRpc`] impl, then records
//! the task outcome on the registry. Diagnostic mapping lives in
//! [`crate::outcomes`].

use std::sync::Arc;
use std::time::Duration;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tdmcp_core::{BridgeMethod, OpPath, Pid, PidRegistry, TaskMode};
use thiserror::Error;
use tokio::sync::Mutex;

use crate::bridge_rpc::{BridgeRpc, BridgeRpcError};
use crate::fleet::{fleet_summary, FleetParams};
use crate::outcomes::{map_inspect_outcome, map_perception_outcome, map_script_outcome};
use crate::schema::input_schema_for;

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
    /// JSON Schema for arguments (derived from param types).
    pub input_schema: Map<String, Value>,
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
        /// Optional JPEG (base64) when perception failed but a frame was captured
        /// (e.g. black-frame) — agents still need to see the pixels.
        image_jpeg_base64: Option<String>,
    },
}

/// Catalogue of v1 tools with derived schemas.
#[must_use]
pub fn tool_descriptors() -> Vec<ToolDescriptor> {
    vec![
        ToolDescriptor {
            name: "fleet".into(),
            description: "Fleet view — TD processes by pid, bridge, tasks, cancelled traces".into(),
            input_schema: input_schema_for("fleet"),
        },
        ToolDescriptor {
            name: "execute_python".into(),
            description: "Run Python in TD; prints tee to op.Debug.op('debug') and the COMP face LOGS section; response includes logs (disable with includeLogs: false). OpPath-exempt with tdmcp_resolve helper.".into(),
            input_schema: input_schema_for("execute_python"),
        },
        ToolDescriptor {
            name: "inspect".into(),
            description: "Structural subtree read (nodes/params/errors). Default summary includes a direct-child roster ({name, opType}); detailed adds path+family. Roster capped at 64 — when truncated see node.truncation (detailLevel does not raise the cap).".into(),
            input_schema: input_schema_for("inspect"),
        },
        ToolDescriptor {
            name: "capture".into(),
            description: "Perception capture (top/preview/…)".into(),
            input_schema: input_schema_for("capture"),
        },
        ToolDescriptor {
            name: "describe_tools".into(),
            description: "Manifest of available tools".into(),
            input_schema: input_schema_for("describe_tools"),
        },
    ]
}

/// Args for execute_python.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutePythonParams {
    /// Target pid.
    pub pid: Pid,
    /// Script body.
    pub script: String,
    /// Exclusive enqueue.
    #[serde(default)]
    pub exclusive: bool,
    /// Optional context path (exposed to script as helper; not enforced).
    #[serde(default)]
    pub context_path: Option<OpPath>,
    /// When true (default), capture stdout/stderr during exec and return as `logs`.
    #[serde(default = "default_true")]
    pub include_logs: bool,
}

fn default_true() -> bool {
    true
}

/// Capture mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CaptureMode {
    /// TOP → JPEG.
    Top,
    /// COMP face fallback chain.
    Preview,
    /// TOP → top; COMP → preview.
    #[default]
    Auto,
    /// CHOP → capped JSON (P1).
    ChopData,
}

impl CaptureMode {
    /// Wire string for the bridge.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Top => "top",
            Self::Preview => "preview",
            Self::Auto => "auto",
            Self::ChopData => "chop_data",
        }
    }
}

/// Default longer-side cap for perception JPEGs (token / wire discipline).
pub const CAPTURE_DEFAULT_MAX_SIZE: u32 = 256;

/// Args for capture (perception).
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CaptureParams {
    /// Target pid.
    pub pid: Pid,
    /// Operator path (OpPath; relative to contextPath or /project1).
    pub path: OpPath,
    /// Capture mode.
    #[serde(default)]
    pub mode: CaptureMode,
    /// Resolution base for relative `path`.
    #[serde(default)]
    pub context_path: Option<OpPath>,
    /// Longer-side pixel cap before JPEG encode. `null` = native resolution.
    /// Defaults to 256.
    #[serde(default = "default_capture_max_size")]
    pub max_size: Option<u32>,
}

fn default_capture_max_size() -> Option<u32> {
    Some(CAPTURE_DEFAULT_MAX_SIZE)
}

/// Sections to include in an inspect response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum InspectInclude {
    /// Node tree.
    Nodes,
    /// Parameters.
    Params,
    /// TD errors.
    Errors,
}

/// Structural detail level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum DetailLevel {
    /// Direct-child roster as `{name, opType}` (capped at 64; see `node.truncation`).
    #[default]
    Summary,
    /// Direct-child roster as `{path, family, opType}` (same 64 cap — does not uncap).
    Detailed,
}

impl DetailLevel {
    /// Wire string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Summary => "summary",
            Self::Detailed => "detailed",
        }
    }
}

/// Args for inspect.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InspectParams {
    /// Target pid.
    pub pid: Pid,
    /// Operator path (OpPath).
    pub path: OpPath,
    /// Resolution base for relative `path`.
    #[serde(default)]
    pub context_path: Option<OpPath>,
    /// Sections to include.
    #[serde(default)]
    pub include: Vec<InspectInclude>,
    /// Structural detail level.
    #[serde(default)]
    pub detail_level: DetailLevel,
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
            let method = BridgeMethod::ExecutePython;
            let outcome = enqueue_and_call(
                registry,
                bridge,
                params.pid,
                method,
                mode_of(params.exclusive),
                serde_json::json!({
                    "script": params.script,
                    "contextPath": params.context_path,
                    "includeLogs": params.include_logs,
                }),
            )
            .await;
            map_script_outcome(catalog, params.pid, outcome)
        }
        "capture" => {
            let params: CaptureParams = serde_json::from_value(args)
                .map_err(|e| ToolCallError::InvalidArgs(e.to_string()))?;
            let method = BridgeMethod::Capture;
            let outcome = enqueue_and_call(
                registry,
                bridge,
                params.pid,
                method,
                TaskMode::Shared,
                serde_json::json!({
                    "path": params.path,
                    "mode": params.mode.as_str(),
                    "contextPath": params.context_path,
                    "maxSize": params.max_size,
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
            let method = BridgeMethod::Inspect;
            let include: Vec<&str> = params
                .include
                .iter()
                .map(|i| match i {
                    InspectInclude::Nodes => "nodes",
                    InspectInclude::Params => "params",
                    InspectInclude::Errors => "errors",
                })
                .collect();
            let outcome = enqueue_and_call(
                registry,
                bridge,
                params.pid,
                method,
                TaskMode::Shared,
                serde_json::json!({
                    "path": params.path,
                    "contextPath": params.context_path,
                    "include": include,
                    "detailLevel": params.detail_level.as_str(),
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
    pid: Pid,
    method: BridgeMethod,
    mode: TaskMode,
    params: Value,
) -> BridgeOutcome {
    let raw_pid = pid.get();
    {
        let mut reg = registry.lock().await;
        if let Err(e) = reg.enqueue(raw_pid, method.queue_label(), mode) {
            return match &e {
                tdmcp_core::EnqueueError::Queue(_) => BridgeOutcome::QueueBusy,
                _ => BridgeOutcome::Transport(BridgeRpcError::NotConnected { pid: raw_pid }),
            };
        }
    }

    let call = bridge.call(raw_pid, method.wire_str(), params);
    match tokio::time::timeout(BRIDGE_TIMEOUT, call).await {
        Ok(Ok(value)) => BridgeOutcome::Ok(value),
        Ok(Err(err)) => BridgeOutcome::Transport(err),
        Err(_) => BridgeOutcome::Transport(BridgeRpcError::Timeout {
            pid: raw_pid,
            budget_ms: BRIDGE_TIMEOUT.as_millis() as u64,
        }),
    }
}

//! Tool descriptors and execute_python stub (bridge RPC later).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tdmcp_core::{PidRegistry, TaskMode};
use tdmcp_diagnostics::{
    Catalog, DiagnosticLayer, DiagnosticSeverity, DiagnosticSpan, Diagnostics, Diagnostics as Diag,
};
use thiserror::Error;

use crate::fleet::{fleet_summary, FleetParams};

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
        diagnostics: Diagnostics,
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
            description: "Run Python in TD; result = … (OpPath-exempt)".into(),
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

/// Result of execute_python when the bridge is not yet connected (stub).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutePythonResult {
    /// Whether the script was accepted onto the queue.
    pub queued: bool,
    /// Echoed context path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_path: Option<String>,
    /// Note for agents until live bridge RPC lands.
    pub note: String,
}

/// Enqueue an execute_python task (live bridge RPC is daemon-owned).
pub fn execute_python_stub(
    registry: &mut PidRegistry,
    catalog: &Catalog,
    params: ExecutePythonParams,
) -> Result<ExecutePythonResult, ToolCallError> {
    let mode = if params.exclusive {
        TaskMode::Exclusive
    } else {
        TaskMode::Shared
    };
    match registry.enqueue(params.pid, "PythonEval", mode) {
        Ok(()) => Ok(ExecutePythonResult {
            queued: true,
            context_path: params.context_path,
            note: "queued; bridge RPC executes when peer is connected".into(),
        }),
        Err(e) => {
            let code = match &e {
                tdmcp_core::EnqueueError::UnknownPid { .. } => "tdmcp.bridge.unknown_pid",
                tdmcp_core::EnqueueError::BridgeDisconnected { .. } => "tdmcp.bridge.lost",
                tdmcp_core::EnqueueError::Queue(_) => "tdmcp.bridge.queue_busy",
            };
            let span = DiagnosticSpan {
                tool: "execute_python".into(),
                mutation_index: None,
                field: None,
                line: None,
                column: None,
                snippet: None,
            };
            let mut item = catalog
                .build_error(code, span, Some(e.to_string()))
                .unwrap_or_else(|_| tdmcp_diagnostics::DiagnosticItem {
                    severity: DiagnosticSeverity::Error,
                    code: code.into(),
                    layer: DiagnosticLayer::Fleet,
                    message: e.to_string(),
                    span: DiagnosticSpan {
                        tool: "execute_python".into(),
                        mutation_index: None,
                        field: None,
                        line: None,
                        column: None,
                        snippet: None,
                    },
                    context: Default::default(),
                    lints: Vec::new(),
                    mitigation: Vec::new(),
                    references: Vec::new(),
                    raw_traceback: None,
                });
            item.context.pid = Some(params.pid);
            let mut diagnostics = Diag {
                summary: e.to_string(),
                items: vec![item],
            };
            diagnostics.summary = diagnostics.recount_summary();
            let _ = params.script;
            Err(ToolCallError::Failed {
                summary: diagnostics.summary.clone(),
                diagnostics,
            })
        }
    }
}

/// Dispatch a named tool call to JSON.
pub fn dispatch_tool(
    registry: &mut PidRegistry,
    catalog: &Catalog,
    name: &str,
    args: Value,
) -> Result<Value, ToolCallError> {
    match name {
        "fleet" => {
            let params: FleetParams = serde_json::from_value(args)
                .map_err(|e| ToolCallError::InvalidArgs(e.to_string()))?;
            Ok(serde_json::to_value(fleet_summary(registry, &params))
                .map_err(|e| ToolCallError::InvalidArgs(e.to_string()))?)
        }
        "execute_python" => {
            let params: ExecutePythonParams = serde_json::from_value(args)
                .map_err(|e| ToolCallError::InvalidArgs(e.to_string()))?;
            let result = execute_python_stub(registry, catalog, params)?;
            Ok(serde_json::to_value(result)
                .map_err(|e| ToolCallError::InvalidArgs(e.to_string()))?)
        }
        "describe_tools" => Ok(serde_json::json!({ "tools": tool_descriptors() })),
        other => Err(ToolCallError::UnknownTool(other.to_owned())),
    }
}

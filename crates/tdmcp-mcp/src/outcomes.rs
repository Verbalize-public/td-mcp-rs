//! Outcome → diagnostic mapping for bridge-driven tools.
//!
//! The daemon actor reports a raw [`BridgeOutcome`]; this module interprets it
//! into the uniform `diagnostics` envelope (script / perception / inspect /
//! bridge transport layers). Catalog-backed codes + mitigation only — no
//! free-string-only failures on the MCP surface.

use serde::Deserialize;
use serde_json::Value;
use tdmcp_core::{OpPath, Pid};
use tdmcp_diagnostics::codes;
use tdmcp_diagnostics::{
    Catalog, DiagnosticContext, DiagnosticItem, DiagnosticLayer, DiagnosticSeverity,
    DiagnosticSpan, Diagnostics,
};

use crate::bridge_rpc::BridgeRpcError;
use crate::tools::{BridgeOutcome, ToolCallError};

/// Typed soft-failure / success shell from the bridge (not transport errors).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeResultEnvelope {
    /// Whether the bridge handler succeeded.
    #[serde(default)]
    pub ok: Option<bool>,
    /// Success payload.
    #[serde(default)]
    #[allow(dead_code, reason = "deserialized for typed shell completeness")]
    pub result: Option<Value>,
    /// Human error string (script failures).
    #[serde(default)]
    pub error: Option<String>,
    /// Stable `tdmcp.*` code when the bridge supplied one.
    #[serde(default)]
    pub code: Option<String>,
    /// Human message (perception / inspect failures).
    #[serde(default)]
    pub message: Option<String>,
    /// Optional traceback.
    #[serde(default)]
    pub traceback: Option<String>,
}

impl BridgeResultEnvelope {
    /// Parse a bridge JSON value into a typed envelope.
    pub fn from_value(value: &Value) -> Self {
        serde_json::from_value(value.clone()).unwrap_or(Self {
            ok: None,
            result: None,
            error: None,
            code: None,
            message: None,
            traceback: None,
        })
    }

    /// True when the bridge reported a soft failure (`ok: false`).
    #[must_use]
    pub fn is_error(&self) -> bool {
        self.ok == Some(false)
    }

    /// Best available human message.
    #[must_use]
    pub fn message_or(&self, fallback: &str) -> String {
        self.message
            .clone()
            .or_else(|| self.error.clone())
            .unwrap_or_else(|| fallback.to_owned())
    }
}

/// Build a single-item `Failed` tool error.
pub fn failed_one(item: DiagnosticItem) -> ToolCallError {
    failed_one_with_image(item, None)
}

/// Build a single-item `Failed` tool error, optionally attaching a JPEG frame.
pub fn failed_one_with_image(
    item: DiagnosticItem,
    image_jpeg_base64: Option<String>,
) -> ToolCallError {
    let summary = item.message.clone();
    let diagnostics = Diagnostics {
        summary,
        items: vec![item],
    };
    ToolCallError::Failed {
        summary: diagnostics.recount_summary(),
        diagnostics,
        image_jpeg_base64,
    }
}

/// Map a script (`execute_python`) outcome.
pub fn map_script_outcome(
    catalog: &Catalog,
    pid: Pid,
    outcome: BridgeOutcome,
) -> Result<Value, ToolCallError> {
    let span = span("execute_python", None);
    match outcome {
        BridgeOutcome::Ok(value) => {
            let env = BridgeResultEnvelope::from_value(&value);
            if env.is_error() {
                let msg = env.message_or("script execution failed");
                let mut item = build_diag(
                    catalog,
                    codes::SCRIPT_EXECUTION_FAILED,
                    span,
                    Some(msg),
                    ctx(pid, None, None),
                );
                item.raw_traceback = env.traceback;
                Err(failed_one(item))
            } else {
                Ok(serde_json::json!({ "ok": true, "result": value.get("result") }))
            }
        }
        BridgeOutcome::QueueBusy => Err(queue_busy(catalog, "execute_python", pid)),
        BridgeOutcome::Transport(err) => Err(transport(catalog, "execute_python", pid, err)),
    }
}

/// Map a perception (`capture`) outcome.
pub fn map_perception_outcome(
    catalog: &Catalog,
    pid: Pid,
    path: OpPath,
    context_path: Option<OpPath>,
    outcome: BridgeOutcome,
) -> Result<Value, ToolCallError> {
    let span = span("capture", Some("path".into()));
    match outcome {
        BridgeOutcome::Ok(value) => {
            let env = BridgeResultEnvelope::from_value(&value);
            if env.is_error() {
                let code = env.code.as_deref().unwrap_or(codes::PERCEPTION_NO_PATH);
                let msg = env.message_or("perception capture failed");
                let item = build_diag(
                    catalog,
                    code,
                    span,
                    Some(msg),
                    ctx(pid, Some(path), context_path),
                );
                let jpeg = value
                    .get("jpegBase64")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned);
                Err(failed_one_with_image(item, jpeg))
            } else {
                Ok(serde_json::json!({ "ok": true, "capture": value }))
            }
        }
        BridgeOutcome::QueueBusy => Err(queue_busy(catalog, "capture", pid)),
        BridgeOutcome::Transport(err) => Err(transport(catalog, "capture", pid, err)),
    }
}

/// Map an `inspect` outcome.
pub fn map_inspect_outcome(
    catalog: &Catalog,
    pid: Pid,
    path: OpPath,
    context_path: Option<OpPath>,
    outcome: BridgeOutcome,
) -> Result<Value, ToolCallError> {
    let span = span("inspect", Some("path".into()));
    match outcome {
        BridgeOutcome::Ok(value) => {
            let env = BridgeResultEnvelope::from_value(&value);
            if env.is_error() {
                let code = env.code.as_deref().unwrap_or(codes::OP_NOT_FOUND);
                let msg = env.message_or("inspect failed");
                let item = build_diag(
                    catalog,
                    code,
                    span,
                    Some(msg),
                    ctx(pid, Some(path), context_path),
                );
                Err(failed_one(item))
            } else {
                Ok(serde_json::json!({ "ok": true, "inspect": value }))
            }
        }
        BridgeOutcome::QueueBusy => Err(queue_busy(catalog, "inspect", pid)),
        BridgeOutcome::Transport(err) => Err(transport(catalog, "inspect", pid, err)),
    }
}

fn span(tool: &str, field: Option<String>) -> DiagnosticSpan {
    DiagnosticSpan {
        tool: tool.into(),
        mutation_index: None,
        field,
        line: None,
        column: None,
        snippet: None,
    }
}

fn ctx(pid: Pid, op_path: Option<OpPath>, context_path: Option<OpPath>) -> DiagnosticContext {
    DiagnosticContext {
        pid: Some(pid.get()),
        op_path: op_path.map(|p| p.0),
        context_path: context_path.map(|p| p.0),
    }
}

fn queue_busy(catalog: &Catalog, tool: &str, pid: Pid) -> ToolCallError {
    let item = build_diag(
        catalog,
        codes::BRIDGE_QUEUE_BUSY,
        span(tool, None),
        Some(format!(
            "exclusive request rejected — queue non-empty (pid {pid})"
        )),
        ctx(pid, None, None),
    );
    failed_one(item)
}

fn transport(catalog: &Catalog, tool: &str, pid: Pid, err: BridgeRpcError) -> ToolCallError {
    let code = match &err {
        BridgeRpcError::NotConnected { .. } | BridgeRpcError::Disconnected { .. } => {
            codes::BRIDGE_LOST
        }
        BridgeRpcError::Timeout { .. } => codes::BRIDGE_TIMEOUT,
        BridgeRpcError::BridgeReturned { .. } => codes::BRIDGE_LOST,
    };
    let item = build_diag(
        catalog,
        code,
        span(tool, None),
        Some(err.to_string()),
        ctx(pid, None, None),
    );
    failed_one(item)
}

/// Build a catalog-backed error item, falling back to a minimal hand-built item
/// if the code is unknown (never a free-string-only bag).
pub fn build_diag(
    catalog: &Catalog,
    code: &str,
    span: DiagnosticSpan,
    message: Option<String>,
    context: DiagnosticContext,
) -> DiagnosticItem {
    match catalog.build_error(code, span.clone(), message.clone()) {
        Ok(mut item) => {
            item.context = context;
            item
        }
        Err(_) => DiagnosticItem {
            severity: DiagnosticSeverity::Error,
            code: code.to_owned(),
            layer: DiagnosticLayer::Fleet,
            message: message.unwrap_or_else(|| code.to_owned()),
            span,
            context,
            lints: Vec::new(),
            mitigation: Vec::new(),
            references: Vec::new(),
            raw_traceback: None,
        },
    }
}

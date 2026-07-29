//! Outcome → diagnostic mapping for bridge-driven tools.
//!
//! The daemon actor reports a raw [`BridgeOutcome`]; this module interprets it
//! into the uniform `diagnostics` envelope (script / perception / inspect /
//! bridge transport layers). Catalog-backed codes + mitigation only — no
//! free-string-only failures on the MCP surface.

use serde_json::Value;
use tdmcp_diagnostics::{
    Catalog, DiagnosticContext, DiagnosticItem, DiagnosticLayer, DiagnosticSeverity,
    DiagnosticSpan, Diagnostics,
};

use crate::bridge_rpc::BridgeRpcError;
use crate::tools::{BridgeOutcome, ToolCallError};

/// Build a single-item `Failed` tool error.
pub fn failed_one(item: DiagnosticItem) -> ToolCallError {
    let summary = item.message.clone();
    let diagnostics = Diagnostics {
        summary,
        items: vec![item],
    };
    ToolCallError::Failed {
        summary: diagnostics.recount_summary(),
        diagnostics,
    }
}

/// Map a script (`execute_python`) outcome.
pub fn map_script_outcome(
    catalog: &Catalog,
    pid: u32,
    outcome: BridgeOutcome,
) -> Result<Value, ToolCallError> {
    let span = span("execute_python", None);
    match outcome {
        BridgeOutcome::Ok(value) => {
            if is_bridge_error(&value) {
                let msg = value
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("script execution failed")
                    .to_owned();
                let traceback = value
                    .get("traceback")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                let mut item = build_diag(
                    catalog,
                    "tdmcp.script.execution_failed",
                    span,
                    Some(msg.clone()),
                    ctx(pid, None, None),
                );
                item.raw_traceback = traceback;
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
    pid: u32,
    path: String,
    context_path: Option<String>,
    outcome: BridgeOutcome,
) -> Result<Value, ToolCallError> {
    let span = span("capture", Some("path".into()));
    match outcome {
        BridgeOutcome::Ok(value) => {
            if is_bridge_error(&value) {
                let code = value
                    .get("code")
                    .and_then(Value::as_str)
                    .unwrap_or("tdmcp.perception.no_path");
                let msg = value
                    .get("message")
                    .and_then(Value::as_str)
                    .or_else(|| value.get("error").and_then(Value::as_str))
                    .unwrap_or("perception capture failed")
                    .to_owned();
                let item = build_diag(
                    catalog,
                    code,
                    span,
                    Some(msg.clone()),
                    ctx(pid, Some(path), context_path),
                );
                Err(failed_one(item))
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
    pid: u32,
    path: String,
    context_path: Option<String>,
    outcome: BridgeOutcome,
) -> Result<Value, ToolCallError> {
    let span = span("inspect", Some("path".into()));
    match outcome {
        BridgeOutcome::Ok(value) => {
            if is_bridge_error(&value) {
                let code = value
                    .get("code")
                    .and_then(Value::as_str)
                    .unwrap_or("tdmcp.op.not_found");
                let msg = value
                    .get("message")
                    .and_then(Value::as_str)
                    .or_else(|| value.get("error").and_then(Value::as_str))
                    .unwrap_or("inspect failed")
                    .to_owned();
                let item = build_diag(
                    catalog,
                    code,
                    span,
                    Some(msg.clone()),
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

fn is_bridge_error(value: &Value) -> bool {
    value
        .get("ok")
        .and_then(Value::as_bool)
        .is_some_and(|ok| !ok)
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

fn ctx(pid: u32, op_path: Option<String>, context_path: Option<String>) -> DiagnosticContext {
    DiagnosticContext {
        pid: Some(pid),
        op_path,
        context_path,
    }
}

fn queue_busy(catalog: &Catalog, tool: &str, pid: u32) -> ToolCallError {
    let item = build_diag(
        catalog,
        "tdmcp.bridge.queue_busy",
        span(tool, None),
        Some(format!(
            "exclusive request rejected — queue non-empty (pid {pid})"
        )),
        ctx(pid, None, None),
    );
    failed_one(item)
}

fn transport(catalog: &Catalog, tool: &str, pid: u32, err: BridgeRpcError) -> ToolCallError {
    let code = match &err {
        BridgeRpcError::NotConnected { .. } | BridgeRpcError::Disconnected { .. } => {
            "tdmcp.bridge.lost"
        }
        BridgeRpcError::Timeout { .. } => "tdmcp.bridge.timeout",
        BridgeRpcError::BridgeReturned { .. } => "tdmcp.bridge.lost",
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

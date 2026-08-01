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
    Catalog, DiagnosticContext, DiagnosticItem, DiagnosticLayer, DiagnosticLevel,
    DiagnosticSeverity, DiagnosticSpan, Diagnostics, LintItem, Suggestion,
};

use crate::bridge_rpc::BridgeRpcError;
use crate::tools::{BridgeOutcome, FormatMode, ToolCallError};

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
    /// Captured stdout/stderr when includeLogs was enabled.
    #[serde(default)]
    pub logs: Option<String>,
    /// Structured execute_python exception report.
    #[serde(default)]
    pub exception: Option<Value>,
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
            logs: None,
            exception: None,
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
    failed_one_with_image_and_data(item, image_jpeg_base64, None)
}

/// Build a single-item `Failed` tool error with optional JPEG + structured data.
pub fn failed_one_with_image_and_data(
    item: DiagnosticItem,
    image_jpeg_base64: Option<String>,
    data: Option<Value>,
) -> ToolCallError {
    let summary = item.message.clone();
    let diagnostics = Diagnostics {
        summary,
        items: vec![item],
    };
    crate::tools::ToolFailPayload {
        summary: diagnostics.recount_summary(),
        diagnostics,
        image_jpeg_base64,
        data,
    }
    .into_error()
}

/// Map a script (`execute_python`) outcome.
pub fn map_script_outcome(
    catalog: &Catalog,
    pid: Pid,
    outcome: BridgeOutcome,
    diagnostic_level: DiagnosticLevel,
    format_mode: FormatMode,
    context_path: Option<OpPath>,
) -> Result<Value, ToolCallError> {
    let mut span = span("execute_python", None);
    match outcome {
        BridgeOutcome::Ok(value) => {
            let env = BridgeResultEnvelope::from_value(&value);
            if env.is_error() {
                let msg = env.message_or("script execution failed");
                let mut context = ctx(pid, None, context_path);
                context.logs = env.logs.clone();
                let exception = reduce_exception(env.exception.clone(), format_mode);
                fill_span_from_exception(&mut span, exception.as_ref());
                let mut item = build_diag(
                    catalog,
                    codes::SCRIPT_EXECUTION_FAILED,
                    span,
                    Some(msg),
                    context,
                );
                item.raw_traceback = raw_traceback_for(
                    diagnostic_level,
                    env.traceback.or_else(|| {
                        exception
                            .as_ref()
                            .and_then(|e| e.get("raw"))
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    }),
                );
                item.exception = exception;
                if let Some(lint) = none_op_lint(item.exception.as_ref()) {
                    item.lints = vec![lint];
                }
                Err(failed_one(item))
            } else {
                let mut body = serde_json::json!({ "ok": true, "result": value.get("result") });
                if let Some(logs) = env.logs {
                    body["logs"] = Value::String(logs);
                } else if let Some(logs) = value.get("logs") {
                    body["logs"] = logs.clone();
                }
                Ok(body)
            }
        }
        BridgeOutcome::QueueBusy => Err(queue_busy(catalog, "execute_python", pid)),
        BridgeOutcome::Transport(err) => Err(transport(catalog, "execute_python", pid, err)),
    }
}

fn reduce_exception(exception: Option<Value>, format_mode: FormatMode) -> Option<Value> {
    let mut exception = exception?;
    if format_mode == FormatMode::Debug {
        return Some(exception);
    }
    if let Some(frames) = exception.get_mut("frames").and_then(Value::as_array_mut) {
        for frame in frames {
            if let Some(obj) = frame.as_object_mut() {
                obj.remove("locals");
            }
        }
    }
    Some(exception)
}

fn fill_span_from_exception(span: &mut DiagnosticSpan, exception: Option<&Value>) {
    let Some(exception) = exception else {
        return;
    };
    if let Some(syntax) = exception.get("syntax") {
        if !syntax.is_null() {
            span.line = syntax
                .get("lineno")
                .and_then(Value::as_u64)
                .map(|n| n as u32);
            span.column = syntax
                .get("offset")
                .and_then(Value::as_u64)
                .map(|n| n as u32);
            span.snippet = syntax
                .get("text")
                .and_then(Value::as_str)
                .map(|s| s.trim_end_matches('\n').to_owned());
            if span.line.is_some() {
                return;
            }
        }
    }
    let Some(frames) = exception.get("frames").and_then(Value::as_array) else {
        return;
    };
    let last_user = frames.iter().rev().find(|f| {
        f.get("filename")
            .and_then(Value::as_str)
            .is_some_and(|p| p == "<string>")
    });
    if let Some(frame) = last_user {
        span.line = frame
            .get("lineno")
            .and_then(Value::as_u64)
            .map(|n| n as u32);
        span.snippet = frame.get("line").and_then(Value::as_str).map(str::to_owned);
    }
}

fn none_op_lint(exception: Option<&Value>) -> Option<LintItem> {
    let exception = exception?;
    let ty = exception.get("type").and_then(Value::as_str)?;
    let message = exception
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("");
    if ty != "AttributeError" || !message.contains("NoneType") {
        return None;
    }
    Some(LintItem {
        severity: DiagnosticSeverity::Lint,
        code: codes::SCRIPT_NONE_OP.to_owned(),
        message: "Attribute access on None — likely a missing op() / bad path".to_owned(),
        confidence: Some("high".to_owned()),
        suggestion: None,
    })
}

fn raw_traceback_for(level: DiagnosticLevel, traceback: Option<String>) -> Option<String> {
    match level {
        DiagnosticLevel::Detailed => traceback,
        DiagnosticLevel::Summary => None,
    }
}

/// Map a perception (`capture`) outcome.
pub fn map_perception_outcome(
    catalog: &Catalog,
    pid: Pid,
    path: OpPath,
    context_path: Option<OpPath>,
    outcome: BridgeOutcome,
    _diagnostic_level: DiagnosticLevel,
) -> Result<Value, ToolCallError> {
    let span = span("capture", Some("path".into()));
    match outcome {
        BridgeOutcome::Ok(value) => {
            let env = BridgeResultEnvelope::from_value(&value);
            if env.is_error() {
                let code = env.code.as_deref().unwrap_or(codes::PERCEPTION_NO_PATH);
                // Prefer bridge message/error; otherwise let catalog text win
                // (avoids stomping e.g. black_frame with a generic fallback).
                let msg = env.message.clone().or_else(|| env.error.clone());
                let item = build_diag(catalog, code, span, msg, ctx(pid, Some(path), context_path));
                let jpeg = value
                    .get("jpegBase64")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned);
                Err(failed_one_with_image(item, jpeg))
            } else {
                // Bridge already returns flat {ok, path, bytes, mimeType, jpegBase64?, …}.
                Ok(value)
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
    path: Option<OpPath>,
    context_path: Option<OpPath>,
    outcome: BridgeOutcome,
    _diagnostic_level: DiagnosticLevel,
) -> Result<Value, ToolCallError> {
    let span = span("inspect", Some("paths".into()));
    match outcome {
        BridgeOutcome::Ok(value) => {
            let env = BridgeResultEnvelope::from_value(&value);
            if env.is_error() {
                let code = env.code.as_deref().unwrap_or(codes::OP_NOT_FOUND);
                let msg = env.message_or("inspect failed");
                let item = build_diag(catalog, code, span, Some(msg), ctx(pid, path, context_path));
                Err(failed_one(item))
            } else {
                // Bridge already returns flat {ok, nodes: [...]}.
                Ok(value)
            }
        }
        BridgeOutcome::QueueBusy => Err(queue_busy(catalog, "inspect", pid)),
        BridgeOutcome::Transport(err) => Err(transport(catalog, "inspect", pid, err)),
    }
}

/// Map a `mutate_nodes` outcome.
pub fn map_mutate_outcome(
    catalog: &Catalog,
    pid: Pid,
    context_path: Option<OpPath>,
    outcome: BridgeOutcome,
    _diagnostic_level: DiagnosticLevel,
) -> Result<Value, ToolCallError> {
    match outcome {
        BridgeOutcome::Ok(value) => {
            let env = BridgeResultEnvelope::from_value(&value);
            // Soft failure: bridge returns applied/failedAt/steps even when ok:false.
            if env.is_error() || value.get("ok") == Some(&Value::Bool(false)) {
                let failed_at = value
                    .get("failedAt")
                    .and_then(Value::as_u64)
                    .map(|i| i as u32);
                let failure = mutate_failure_from_steps(&value);
                let mut item = build_diag(
                    catalog,
                    failure.code,
                    DiagnosticSpan {
                        tool: "mutate_nodes".into(),
                        mutation_index: failed_at,
                        field: failure.field,
                        line: None,
                        column: None,
                        snippet: None,
                    },
                    Some(failure.message),
                    ctx(pid, failure.op_path, context_path),
                );
                // Best-effort: never let malformed bridge lints drop the hard error.
                item.lints = failure.lints;
                let data = Some(serde_json::json!({
                    "applied": value.get("applied").cloned().unwrap_or(Value::from(0)),
                    "failedAt": value.get("failedAt").cloned().unwrap_or(Value::Null),
                    "steps": value.get("steps").cloned().unwrap_or_else(|| Value::Array(vec![])),
                }));
                return Err(failed_one_with_image_and_data(item, None, data));
            }
            Ok(serde_json::json!({
                "ok": true,
                "applied": value.get("applied").cloned().unwrap_or(Value::from(0)),
                "failedAt": value.get("failedAt").cloned().unwrap_or(Value::Null),
                "steps": value.get("steps").cloned().unwrap_or_else(|| Value::Array(vec![])),
            }))
        }
        BridgeOutcome::QueueBusy => Err(queue_busy(catalog, "mutate_nodes", pid)),
        BridgeOutcome::Transport(err) => Err(transport(catalog, "mutate_nodes", pid, err)),
    }
}

/// First hard-failure fields pulled from a mutate envelope (fail-soft extras).
struct MutateStepFailure {
    code: &'static str,
    message: String,
    op_path: Option<OpPath>,
    field: Option<String>,
    lints: Vec<LintItem>,
}

/// Pull the first hard-failure code/message/path from a mutate envelope.
fn mutate_failure_from_steps(value: &Value) -> MutateStepFailure {
    let steps = value.get("steps").and_then(Value::as_array);
    if let Some(steps) = steps {
        for step in steps {
            if step.get("ok") == Some(&Value::Bool(false))
                && step.get("skipped") != Some(&Value::Bool(true))
            {
                let code = step
                    .get("code")
                    .and_then(Value::as_str)
                    .unwrap_or(codes::MUTATE_STEP_FAILED);
                // Leak-free: map known codes to static; unknown → MUTATE_STEP_FAILED.
                let code = match code {
                    codes::OP_NOT_FOUND => codes::OP_NOT_FOUND,
                    codes::OP_UNKNOWN_TYPE => codes::OP_UNKNOWN_TYPE,
                    codes::PAR_UNKNOWN => codes::PAR_UNKNOWN,
                    codes::FLAG_UNKNOWN => codes::FLAG_UNKNOWN,
                    codes::BATCH_SKIPPED_DEPENDENT => codes::BATCH_SKIPPED_DEPENDENT,
                    codes::MUTATE_STEP_FAILED => codes::MUTATE_STEP_FAILED,
                    codes::WIRE_BAD_INDEX => codes::WIRE_BAD_INDEX,
                    codes::WIRE_CONNECT_FAILED => codes::WIRE_CONNECT_FAILED,
                    _ => codes::MUTATE_STEP_FAILED,
                };
                let message = step
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("mutate step failed")
                    .to_owned();
                let op_path = step.get("path").and_then(Value::as_str).map(OpPath::from);
                let field = step.get("field").and_then(Value::as_str).map(str::to_owned);
                let lints = parse_step_lints(step);
                return MutateStepFailure {
                    code,
                    message,
                    op_path,
                    field,
                    lints,
                };
            }
        }
    }
    let env = BridgeResultEnvelope::from_value(value);
    MutateStepFailure {
        code: codes::MUTATE_STEP_FAILED,
        message: env.message_or("mutate_nodes failed"),
        op_path: None,
        field: None,
        lints: Vec::new(),
    }
}

/// Best-effort parse of bridge step `lints` (cap 1). Malformed entries are skipped.
fn parse_step_lints(step: &Value) -> Vec<LintItem> {
    let Some(arr) = step.get("lints").and_then(Value::as_array) else {
        return Vec::new();
    };
    for entry in arr {
        let Some(obj) = entry.as_object() else {
            continue;
        };
        let Some(code) = obj.get("code").and_then(Value::as_str) else {
            continue;
        };
        let Some(message) = obj.get("message").and_then(Value::as_str) else {
            continue;
        };
        let confidence = obj
            .get("confidence")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let suggestion = obj.get("suggestion").and_then(|s| {
            let replace = s.get("replace").and_then(Value::as_str)?;
            Some(Suggestion {
                op_path: s.get("opPath").and_then(Value::as_str).map(str::to_owned),
                replace: Some(replace.to_owned()),
            })
        });
        return vec![LintItem {
            severity: DiagnosticSeverity::Lint,
            code: code.to_owned(),
            message: message.to_owned(),
            confidence,
            suggestion,
        }];
    }
    Vec::new()
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
        logs: None,
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
            exception: None,
        },
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "unit tests"
)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fail_item(value: Value) -> DiagnosticItem {
        let catalog = Catalog::fallback();
        let err = map_mutate_outcome(
            &catalog,
            Pid::new(1),
            None,
            BridgeOutcome::Ok(value),
            DiagnosticLevel::Summary,
        )
        .expect_err("expected mutate soft failure");
        match err {
            ToolCallError::Failed(payload) => payload.diagnostics.items[0].clone(),
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn inspect_empty_errors_warnings_is_success() {
        let catalog = Catalog::fallback();
        let ok = map_inspect_outcome(
            &catalog,
            Pid::new(1),
            Some(OpPath::new("/project1")),
            None,
            BridgeOutcome::Ok(json!({
                "ok": true,
                "nodes": [{
                    "ok": true,
                    "path": "/project1",
                    "errors": [],
                    "warnings": []
                }]
            })),
            DiagnosticLevel::Summary,
        )
        .expect("empty TD message arrays must not fail the tool");
        assert_eq!(ok["ok"], true);
        assert_eq!(ok["nodes"][0]["errors"], json!([]));
        assert_eq!(ok["nodes"][0]["warnings"], json!([]));
    }

    fn script_fail(
        bridge_val: Value,
        level: DiagnosticLevel,
        format_mode: FormatMode,
    ) -> DiagnosticItem {
        let catalog = Catalog::fallback();
        let err = map_script_outcome(
            &catalog,
            Pid::new(1),
            BridgeOutcome::Ok(bridge_val),
            level,
            format_mode,
            Some(OpPath("/project1".into())),
        )
        .expect_err("expected script failure");
        match err {
            ToolCallError::Failed(payload) => payload.diagnostics.items[0].clone(),
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn script_summary_omits_traceback_detailed_keeps_it() {
        let bridge_val = json!({
            "ok": false,
            "error": "boom",
            "traceback": "Traceback (most recent call last):\n  File \"<td>\", line 1",
            "exception": {
                "type": "RuntimeError",
                "message": "boom",
                "frames": [{
                    "filename": "<string>",
                    "lineno": 1,
                    "name": "<module>",
                    "line": "raise RuntimeError('boom')",
                    "locals": {"x": {"type": "int", "repr": "1"}}
                }],
                "syntax": null,
                "raw": "Traceback (most recent call last):\n  File \"<td>\", line 1"
            }
        });
        let summary = script_fail(
            bridge_val.clone(),
            DiagnosticLevel::Summary,
            FormatMode::Normal,
        );
        assert!(summary.raw_traceback.is_none());
        assert!(summary.exception.is_some());
        assert_eq!(summary.exception.as_ref().unwrap()["type"], "RuntimeError");
        assert!(summary.exception.as_ref().unwrap()["frames"][0]
            .get("locals")
            .is_none());
        assert_eq!(summary.span.line, Some(1));
        assert_eq!(summary.context.context_path.as_deref(), Some("/project1"));

        let detailed = script_fail(bridge_val, DiagnosticLevel::Detailed, FormatMode::Normal);
        assert!(detailed
            .raw_traceback
            .as_deref()
            .is_some_and(|t| t.contains("Traceback")));
    }

    #[test]
    fn script_debug_keeps_locals() {
        let bridge_val = json!({
            "ok": false,
            "error": "boom",
            "traceback": "tb",
            "exception": {
                "type": "ValueError",
                "message": "boom",
                "frames": [{
                    "filename": "<string>",
                    "lineno": 1,
                    "name": "<module>",
                    "line": "raise ValueError('boom')",
                    "locals": {"x": {"type": "int", "repr": "1"}}
                }],
                "syntax": null,
                "raw": "tb"
            }
        });
        let item = script_fail(bridge_val, DiagnosticLevel::Detailed, FormatMode::Debug);
        assert_eq!(
            item.exception.as_ref().unwrap()["frames"][0]["locals"]["x"]["repr"],
            "1"
        );
    }

    #[test]
    fn script_none_op_lint() {
        let bridge_val = json!({
            "ok": false,
            "error": "'NoneType' object has no attribute 'name'",
            "traceback": "tb",
            "exception": {
                "type": "AttributeError",
                "message": "'NoneType' object has no attribute 'name'",
                "frames": [],
                "syntax": null,
                "raw": "tb"
            }
        });
        let item = script_fail(bridge_val, DiagnosticLevel::Detailed, FormatMode::Normal);
        assert_eq!(item.code, codes::SCRIPT_EXECUTION_FAILED);
        assert_eq!(item.lints.len(), 1);
        assert_eq!(item.lints[0].code, codes::SCRIPT_NONE_OP);
        assert_eq!(item.lints[0].confidence.as_deref(), Some("high"));
    }

    #[test]
    fn mutate_forwards_field_and_wrong_collection_lint() {
        let item = fail_item(json!({
            "ok": false,
            "applied": 0,
            "failedAt": 0,
            "steps": [{
                "ok": false,
                "code": "tdmcp.par.unknown",
                "path": "/project1/noise1",
                "field": "viewer",
                "message": "unknown parameter: viewer (exists as flag — use flags)",
                "lints": [{
                    "severity": "lint",
                    "code": "tdmcp.par.wrong_collection",
                    "message": "'viewer' is an OP flag; use flags, not values",
                    "confidence": "high",
                    "suggestion": {"replace": "flags.viewer"}
                }]
            }]
        }));
        assert_eq!(item.code, codes::PAR_UNKNOWN);
        assert_eq!(item.span.field.as_deref(), Some("viewer"));
        assert_eq!(item.lints.len(), 1);
        assert_eq!(item.lints[0].code, codes::PAR_WRONG_COLLECTION);
        assert_eq!(
            item.lints[0]
                .suggestion
                .as_ref()
                .and_then(|s| s.replace.as_deref()),
            Some("flags.viewer")
        );
    }

    #[test]
    fn mutate_malformed_lints_still_emit_hard_error() {
        let item = fail_item(json!({
            "ok": false,
            "applied": 0,
            "failedAt": 0,
            "steps": [{
                "ok": false,
                "code": "tdmcp.flag.unknown",
                "path": "/project1/noise1",
                "field": "selected",
                "message": "unknown flag: selected",
                "lints": "not-an-array"
            }]
        }));
        assert_eq!(item.code, codes::FLAG_UNKNOWN);
        assert_eq!(item.span.field.as_deref(), Some("selected"));
        assert!(item.lints.is_empty());
        assert!(!item.mitigation.is_empty());
    }

    #[test]
    fn mutate_lint_entries_missing_code_are_skipped() {
        let item = fail_item(json!({
            "ok": false,
            "applied": 0,
            "failedAt": 0,
            "steps": [{
                "ok": false,
                "code": "tdmcp.par.unknown",
                "path": "/project1/noise1",
                "field": "nope",
                "message": "unknown parameter: nope",
                "lints": [{"message": "missing code"}, {"severity": "lint"}]
            }]
        }));
        assert_eq!(item.code, codes::PAR_UNKNOWN);
        assert!(item.lints.is_empty());
    }

    #[test]
    fn mutate_forwards_similar_name_and_similar_type_lints() {
        let par = fail_item(json!({
            "ok": false,
            "applied": 0,
            "failedAt": 0,
            "steps": [{
                "ok": false,
                "code": "tdmcp.par.unknown",
                "path": "/project1/hsv1",
                "field": "satmult",
                "message": "unknown parameter: satmult (did you mean: saturationmult?)",
                "lints": [{
                    "severity": "lint",
                    "code": "tdmcp.par.similar_name",
                    "message": "similar parameter 'saturationmult' found on node",
                    "confidence": "medium",
                    "suggestion": {"replace": "saturationmult"}
                }]
            }]
        }));
        assert_eq!(par.code, codes::PAR_UNKNOWN);
        assert_eq!(par.lints[0].code, codes::PAR_SIMILAR_NAME);

        let op = fail_item(json!({
            "ok": false,
            "applied": 0,
            "failedAt": 0,
            "steps": [{
                "ok": false,
                "code": "tdmcp.op.unknown_type",
                "path": "/project1/x",
                "field": "hsvAdjustTOP",
                "message": "unknown opType: hsvAdjustTOP (did you mean: hsvadjustTOP?)",
                "lints": [{
                    "severity": "lint",
                    "code": "tdmcp.op.similar_type",
                    "message": "similar opType 'hsvadjustTOP' found",
                    "confidence": "medium",
                    "suggestion": {"replace": "hsvadjustTOP"}
                }]
            }]
        }));
        assert_eq!(op.code, codes::OP_UNKNOWN_TYPE);
        assert_eq!(op.lints[0].code, codes::OP_SIMILAR_TYPE);
    }

    fn perception_fail_item(value: Value) -> DiagnosticItem {
        let catalog = Catalog::fallback();
        let err = map_perception_outcome(
            &catalog,
            Pid::new(1),
            OpPath::new("/project1/out1"),
            None,
            BridgeOutcome::Ok(value),
            DiagnosticLevel::Summary,
        )
        .expect_err("expected perception soft failure");
        match err {
            ToolCallError::Failed(payload) => payload.diagnostics.items[0].clone(),
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn perception_black_frame_uses_catalog_message_when_bridge_omits_it() {
        let item = perception_fail_item(json!({
            "ok": false,
            "code": "tdmcp.perception.black_frame",
            "jpegBase64": "aaaa"
        }));
        assert_eq!(item.code, codes::PERCEPTION_BLACK_FRAME);
        assert!(
            item.message.to_lowercase().contains("black"),
            "expected catalog black-frame message, got {:?}",
            item.message
        );
        assert!(
            !item.message.contains("perception capture failed"),
            "generic fallback must not stomp catalog message"
        );
    }

    #[test]
    fn perception_uniform_frame_uses_catalog_message_when_bridge_omits_it() {
        let item = perception_fail_item(json!({
            "ok": false,
            "code": "tdmcp.perception.uniform_frame",
            "jpegBase64": "aaaa"
        }));
        assert_eq!(item.code, codes::PERCEPTION_UNIFORM_FRAME);
        assert!(
            item.message.to_lowercase().contains("uniform"),
            "expected catalog uniform-frame message, got {:?}",
            item.message
        );
        assert!(!item.message.contains("perception capture failed"));
    }

    #[test]
    fn perception_prefers_bridge_message_over_catalog() {
        let item = perception_fail_item(json!({
            "ok": false,
            "code": "tdmcp.perception.black_frame",
            "message": "Captured TOP frame is black (mean rgb≈0.00,0.00,0.00)",
            "jpegBase64": "aaaa"
        }));
        assert_eq!(
            item.message,
            "Captured TOP frame is black (mean rgb≈0.00,0.00,0.00)"
        );
    }

    #[test]
    fn perception_chop_data_success_passes_through_without_image() {
        let catalog = Catalog::fallback();
        let value = json!({
            "ok": true,
            "path": "/project1/zone/const1",
            "mode": "chop_data",
            "family": "CHOP",
            "numChans": 1,
            "numSamples": 2,
            "rate": 60.0,
            "channels": [{"name": "chan1", "samples": [0.1, 0.2]}]
        });
        let out = map_perception_outcome(
            &catalog,
            Pid::new(1),
            OpPath::new("/project1/zone/const1"),
            None,
            BridgeOutcome::Ok(value.clone()),
            DiagnosticLevel::Summary,
        )
        .expect("chop_data success should pass through");
        assert_eq!(out, value);
        assert!(out.get("jpegBase64").is_none());
        assert_eq!(out["mode"], "chop_data");
    }

    #[test]
    fn perception_empty_chop_fails_without_image() {
        let catalog = Catalog::fallback();
        let err = map_perception_outcome(
            &catalog,
            Pid::new(1),
            OpPath::new("/project1/zone/const1"),
            None,
            BridgeOutcome::Ok(json!({
                "ok": false,
                "code": "tdmcp.perception.empty_chop",
                "message": "CHOP has no channels or samples (numChans=0, numSamples=0)",
                "path": "/project1/zone/const1",
                "mode": "chop_data",
                "family": "CHOP"
            })),
            DiagnosticLevel::Summary,
        )
        .expect_err("expected empty_chop failure");
        match err {
            ToolCallError::Failed(payload) => {
                assert_eq!(
                    payload.diagnostics.items[0].code,
                    codes::PERCEPTION_EMPTY_CHOP
                );
                assert!(payload.image_jpeg_base64.is_none());
            }
            other => panic!("unexpected error: {other}"),
        }
    }
}

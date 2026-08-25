//! Curated argument-error diagnostics.
//!
//! Wraps typed argument deserialization so no raw serde string reaches an
//! agent: every schema-level failure becomes a catalog-backed
//! [`DiagnosticItem`] delivered as a structured tool failure (same wire shape
//! as bridge failures). Expected fields / variants come from the tool's own
//! derived schema — never hand-maintained lists.

use serde::de::DeserializeOwned;
use serde_json::Value;

use tdmcp_diagnostics::codes;
use tdmcp_diagnostics::{
    Catalog, DiagnosticContext, DiagnosticItem, DiagnosticSeverity, DiagnosticSpan, LintItem,
    Suggestion,
};

use crate::outcomes::build_diag;
use crate::schema::input_schema_for;
use crate::tools::{ToolCallError, ToolFailPayload, ToolName};

/// Deserialize typed args, mapping any failure to a curated diagnostic payload.
pub fn parse_args<T: DeserializeOwned>(
    catalog: &Catalog,
    tool: ToolName,
    args: Value,
) -> Result<T, ToolCallError> {
    match serde_path_to_error::deserialize::<Value, T>(args) {
        Ok(params) => Ok(params),
        Err(e) => Err(args_error(catalog, tool, e.path().to_string(), e.inner())),
    }
}

/// A required-list pre-check (e.g. non-empty `paths`) as a coded failure.
///
/// Reuses existing catalog codes (`tdmcp.op.paths_required`,
/// `tdmcp.api_help.queries_required`) instead of free strings.
pub fn coded_failure(
    catalog: &Catalog,
    tool: ToolName,
    code: &str,
    field: &str,
    message: impl Into<String>,
) -> ToolCallError {
    let item = build_diag(
        catalog,
        code,
        DiagnosticSpan {
            tool: tool.wire_str().into(),
            mutation_index: None,
            field: Some(field.to_owned()),
            line: None,
            column: None,
            snippet: None,
        },
        Some(message.into()),
        DiagnosticContext::default(),
        tdmcp_diagnostics::DiagnosticLayer::Args,
    );
    payload(item)
}

/// Reclassify an internal result-serialization failure (never the caller's fault).
pub fn serialize_failed(
    catalog: &Catalog,
    tool: ToolName,
    what: &str,
    err: &serde_json::Error,
) -> ToolCallError {
    let item = build_diag(
        catalog,
        codes::MCP_SERIALIZE_FAILED,
        DiagnosticSpan {
            tool: tool.wire_str().into(),
            mutation_index: None,
            field: None,
            line: None,
            column: None,
            snippet: None,
        },
        Some(format!(
            "{}: failed to serialize {what}: {err}",
            tool.wire_str()
        )),
        DiagnosticContext::default(),
        tdmcp_diagnostics::DiagnosticLayer::Fleet,
    );
    payload(item)
}

/// Classify one deserialization error and build its curated failure.
fn args_error(
    catalog: &Catalog,
    tool: ToolName,
    path: String,
    err: &serde_json::Error,
) -> ToolCallError {
    let msg = strip_position(&err.to_string());
    let name = tool.wire_str();
    let schema = input_schema_for(tool);

    let (code, field_ref, message, lint) = if let Some(field) = missing_field(&msg) {
        let field_ref = join_path(&path, &field);
        let allowed = allowed_values(&schema, &field);
        let mut m = format!(
            "{name}: {ref_txt} is missing required field \"{field}\"",
            ref_txt = display_ref(&field_ref)
        );
        if let Some(values) = &allowed {
            m.push_str(&format!(" (one of {})", join_values(values)));
        }
        (codes::ARGS_MISSING_FIELD, field_ref, m, None)
    } else if let Some((field, expected)) = unknown_field(&msg) {
        // serde's path already sits on the offending key for root-level
        // structs but on the containing object for nested ones — normalize.
        let parent = if last_segment(&path) == field {
            match path.rfind('.') {
                Some(i) => path[..i].to_owned(),
                None => String::new(),
            }
        } else {
            path.clone()
        };
        let mut m = format!(
            "{name}: unknown field \"{field}\" at {};",
            display_ref(&parent)
        );
        if !expected.is_empty() {
            m.push_str(&format!(" expected one of {}", join_values(&expected)));
        }
        let lint = similar(&field, &expected).map(|(cand, confidence)| LintItem {
            severity: DiagnosticSeverity::Lint,
            code: codes::ARGS_SIMILAR_FIELD.to_owned(),
            message: format!("did you mean `{cand}`?"),
            confidence: Some(confidence.to_owned()),
            suggestion: Some(Suggestion {
                op_path: None,
                replace: Some(cand.to_owned()),
            }),
        });
        // Span points at the exact offending key.
        (
            codes::ARGS_UNKNOWN_FIELD,
            join_path(&parent, &field),
            m,
            lint,
        )
    } else if let Some((value, expected)) = unknown_variant(&msg) {
        let target = last_segment(&path);
        let mut m = format!("{name}: \"{value}\" is not a valid value for {target}");
        if !expected.is_empty() {
            m.push_str(&format!("; one of {}", join_values(&expected)));
        }
        (codes::ARGS_UNKNOWN_VARIANT, path, m, None)
    } else {
        (
            codes::ARGS_WRONG_TYPE,
            path.clone(),
            format!("{name}: {} — {msg}", display_ref(&path)),
            None,
        )
    };

    let mut item = build_diag(
        catalog,
        code,
        DiagnosticSpan {
            tool: name.into(),
            mutation_index: None,
            field: (!field_ref.is_empty()).then_some(field_ref),
            line: None,
            column: None,
            snippet: None,
        },
        Some(message),
        DiagnosticContext::default(),
        tdmcp_diagnostics::DiagnosticLayer::Args,
    );
    if let Some(lint) = lint {
        item.lints.push(lint);
    }
    payload(item)
}

fn payload(item: DiagnosticItem) -> ToolCallError {
    ToolFailPayload {
        summary: item.message.clone(),
        diagnostics: tdmcp_diagnostics::Diagnostics {
            summary: item.message.clone(),
            items: vec![item],
        },
        image_base64: None,
        image_mime_type: None,
        data: None,
    }
    .into_error()
}

// --- serde message classification -----------------------------------------

fn missing_field(msg: &str) -> Option<String> {
    let rest = msg.strip_prefix("missing field ")?;
    backticked(rest).first().cloned()
}

fn unknown_field(msg: &str) -> Option<(String, Vec<String>)> {
    let rest = msg.strip_prefix("unknown field ")?;
    let field = backticked(rest).first()?.clone();
    // Everything serde quotes after "expected" is the allowed-field list.
    let expected: Vec<String> = msg
        .split("expected")
        .nth(1)
        .map(backticked)
        .unwrap_or_default();
    Some((field, expected))
}

fn unknown_variant(msg: &str) -> Option<(String, Vec<String>)> {
    let rest = msg.strip_prefix("unknown variant ")?;
    let tokens = backticked(rest);
    let value = tokens.first()?.clone();
    let expected: Vec<String> = msg
        .split("expected one of")
        .nth(1)
        .map(backticked)
        .unwrap_or_default();
    Some((value, expected))
}

/// All `` `x` ``-quoted tokens in `s`, in order.
fn backticked(s: &str) -> Vec<String> {
    let mut it = s.split('`');
    let mut out = Vec::new();
    while let Some(_outside) = it.next() {
        match it.next() {
            Some(tok) if !tok.is_empty() => out.push(tok.to_owned()),
            Some(_) => {}
            None => break,
        }
    }
    out
}

/// Drop serde_json's trailing ` at line N column M` (meaningless for `Value` input).
fn strip_position(msg: &str) -> String {
    match msg.find(" at line ") {
        Some(i) => msg[..i].to_owned(),
        None => msg.to_owned(),
    }
}

fn join_path(path: &str, field: &str) -> String {
    if path.is_empty() {
        field.to_owned()
    } else {
        format!("{path}.{field}")
    }
}

/// Human rendering of a JSON reference ("arguments root" when empty).
fn display_ref(path: &str) -> String {
    if path.is_empty() {
        "arguments root".to_owned()
    } else {
        path.to_owned()
    }
}

fn last_segment(path: &str) -> String {
    match path.rfind('.') {
        Some(i) => path[i + 1..].to_owned(),
        None => path.to_owned(),
    }
}

fn join_values(values: &[String]) -> String {
    const CAP: usize = 10;
    if values.len() <= CAP {
        values.join(", ")
    } else {
        format!(
            "{}, … (+{} more)",
            values[..CAP].join(", "),
            values.len() - CAP
        )
    }
}

// --- schema-driven hints ----------------------------------------------------

/// Nearest shipped tool name for an unknown tool call (silence over wrong hints).
pub fn suggest_tool(name: &str) -> Option<String> {
    let candidates: Vec<String> = ToolName::ALL
        .iter()
        .map(|t| t.wire_str().to_owned())
        .collect();
    similar(name, &candidates).map(|(cand, _)| cand.to_owned())
}

/// Allowed values for `field`, resolved from the derived schema: top-level
/// enums and `$defs` variant tables (internally-tagged enums like `MutateStep`).
/// Tolerant by design — returns `None` whenever the shape is unrecognized.
fn allowed_values(schema: &serde_json::Map<String, Value>, field: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    // Top-level enum on the property itself.
    if let Some(prop) = schema.get("properties").and_then(|p| p.get(field)) {
        collect_enum_values(prop, &mut out);
    }
    if out.is_empty() {
        let defs = schema.get("$defs")?.as_object()?;
        for def in defs.values() {
            let Some(obj) = def.as_object() else {
                continue;
            };
            for variants in ["oneOf", "anyOf"] {
                if let Some(list) = obj.get(variants).and_then(Value::as_array) {
                    for v in list {
                        if let Some(c) = v
                            .get("properties")
                            .and_then(|p| p.get(field))
                            .and_then(|t| t.get("const"))
                            .and_then(Value::as_str)
                        {
                            out.push(c.to_owned());
                        }
                    }
                }
            }
        }
    }
    (!out.is_empty()).then_some(out)
}

fn collect_enum_values(v: &Value, out: &mut Vec<String>) {
    if let Some(list) = v.get("enum").and_then(Value::as_array) {
        for item in list {
            if let Some(s) = item.as_str() {
                out.push(s.to_owned());
            }
        }
    }
}

// --- did-you-mean ------------------------------------------------------------

/// Nearest candidate for `name`: casefold-exact wins (`high`), otherwise the
/// best normalized-edit-distance match at ≥0.6 (`high` at ≥0.85). Silence over
/// wrong hints — mirrors `bridge/tdmcp_bridge/suggest.py`.
fn similar<'a>(name: &str, candidates: &'a [String]) -> Option<(&'a str, &'static str)> {
    let key: String = name.chars().flat_map(|c| c.to_lowercase()).collect();
    let mut best: Option<(f32, &str)> = None;
    for cand in candidates {
        let lower: String = cand.chars().flat_map(|c| c.to_lowercase()).collect();
        if lower == key {
            return Some((cand.as_str(), "high"));
        }
        let score = similarity(&key, &lower);
        if score >= 0.6 && best.is_none_or(|(b, _)| score > b) {
            best = Some((score, cand.as_str()));
        }
    }
    best.map(|(score, cand)| {
        let confidence = if score >= 0.85 { "high" } else { "medium" };
        (cand, confidence)
    })
}

/// `1 - levenshtein/max_len` over chars.
fn similarity(a: &str, b: &str) -> f32 {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let max = a.len().max(b.len()) as f32;
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur.push((prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    1.0 - prev[b.len()] as f32 / max
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
    use tdmcp_diagnostics::Catalog;

    fn fail_payload(value: Value, tool: ToolName) -> ToolFailPayload {
        let catalog = Catalog::fallback();
        let err = parse_args::<crate::tools::MutateNodesParams>(&catalog, tool, value)
            .expect_err("expected arg failure");
        match err {
            ToolCallError::Failed(payload) => *payload,
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn missing_step_op_gets_curated_item_with_allowed_ops() {
        let payload = fail_payload(
            json!({"pid": 1, "steps": [{"path": "/project1"}]}),
            ToolName::MutateNodes,
        );
        assert!(payload.summary.contains("missing required field \"op\""));
        let item = &payload.diagnostics.items[0];
        assert_eq!(item.code, "tdmcp.args.missing_field");
        assert_eq!(item.layer, tdmcp_diagnostics::DiagnosticLayer::Args);
        assert_eq!(item.span.field.as_deref(), Some("steps[0].op"));
        assert!(item
            .message
            .contains("one of create, set, delete, connect, disconnect"));
    }

    #[test]
    fn typo_top_level_field_gets_similar_field_lint() {
        let payload = fail_payload(
            json!({"pid": 1, "steps": [], "contextpath": "/project1"}),
            ToolName::MutateNodes,
        );
        let item = &payload.diagnostics.items[0];
        assert_eq!(item.code, "tdmcp.args.unknown_field");
        // Root-level unknown field: no path duplication in span or message.
        assert_eq!(item.span.field.as_deref(), Some("contextpath"));
        assert!(!item.message.contains("at contextpath"), "{}", item.message);
        let lint = &item.lints[0];
        assert_eq!(lint.code, "tdmcp.args.similar_field");
        assert_eq!(
            lint.suggestion.as_ref().and_then(|s| s.replace.as_deref()),
            Some("contextPath")
        );
        assert_eq!(lint.confidence.as_deref(), Some("high"));
    }

    #[test]
    fn bad_include_value_reports_unknown_variant() {
        let catalog = Catalog::fallback();
        let err = parse_args::<crate::fleet::FleetParams>(
            &catalog,
            ToolName::Fleet,
            json!({"include": ["typo"]}),
        )
        .expect_err("expected arg failure");
        match err {
            ToolCallError::Failed(payload) => {
                let item = &payload.diagnostics.items[0];
                assert_eq!(item.code, "tdmcp.args.unknown_variant");
                assert!(
                    item.message
                        .contains("\"typo\" is not a valid value for include"),
                    "{}",
                    item.message
                );
                assert!(item.message.contains("tasks"), "{}", item.message);
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn wrong_type_carries_serde_detail_without_position() {
        let catalog = Catalog::fallback();
        let err = parse_args::<crate::tools::ExecutePythonParams>(
            &catalog,
            ToolName::ExecutePython,
            json!({"pid": 1, "script": 5}),
        )
        .expect_err("expected arg failure");
        match err {
            ToolCallError::Failed(payload) => {
                let item = &payload.diagnostics.items[0];
                assert_eq!(item.code, "tdmcp.args.wrong_type");
                assert!(!item.message.contains("line "), "{}", item.message);
                assert!(item.message.contains("script"), "{}", item.message);
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn valid_args_pass_through() {
        let catalog = Catalog::fallback();
        let params = parse_args::<crate::tools::ExecutePythonParams>(
            &catalog,
            ToolName::ExecutePython,
            json!({"pid": 7, "script": "print(1)"}),
        )
        .expect("valid args");
        assert_eq!(params.pid.get(), 7);
    }

    #[test]
    fn similar_prefers_exact_casefold_then_distance() {
        let cands = vec![
            "contextPath".to_owned(),
            "detailLevel".to_owned(),
            "diagnosticLevel".to_owned(),
        ];
        assert_eq!(
            similar("ContextPath", &cands),
            Some(("contextPath", "high"))
        );
        let (cand, conf) = similar("diagnosicLevel", &cands).expect("near miss");
        assert_eq!(cand, "diagnosticLevel");
        assert_eq!(conf, "high");
        assert!(similar("zzzz", &cands).is_none());
    }
}

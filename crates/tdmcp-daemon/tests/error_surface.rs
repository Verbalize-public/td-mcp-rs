//! Integration: the complete agent-facing error surface, locked.
//!
//! Every argument-shape failure must return the curated `{ok:false,
//! summary, items[]}` envelope with catalog codes, precise spans, and
//! self-correcting hints — never raw serde strings. Unknown tools stay
//! protocol errors (-32602 / HTTP 400) but carry suggestions.

#![allow(clippy::unwrap_used, reason = "test setup/assertions may panic")]
#![allow(clippy::expect_used, reason = "test setup/assertions may panic")]
#![allow(clippy::panic, reason = "test assertions may panic")]

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tdmcp_core::{PidRegistry, ProcessAttrs, ProcessFingerprint};
use tdmcp_diagnostics::Catalog;
use tdmcp_mcp::testing::{test_resource_provider, FakeBridgeRpc};
use tdmcp_mcp::{build_mcp_router, AppState, BridgeRpc};
use tower::ServiceExt;

fn app_with(bridge: Arc<dyn BridgeRpc>) -> axum::Router {
    let mut registry = PidRegistry::new();
    registry.handshake(
        34,
        ProcessAttrs {
            title: Some("test".into()),
            fingerprint: ProcessFingerprint {
                title: Some("test".into()),
                ..Default::default()
            },
            ..Default::default()
        },
        Some("1".into()),
    );
    let state = AppState::new(
        registry,
        Catalog::fallback(),
        bridge,
        test_resource_provider().expect("resource provider"),
    );
    build_mcp_router(state)
}

async fn call_tool(app: axum::Router, name: &str, arguments: Value) -> (StatusCode, Value) {
    let body = json!({"name": name, "arguments": arguments}).to_string();
    let req = Request::builder()
        .method("POST")
        .uri("/mcp/tools/call")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: Value = serde_json::from_slice(&bytes).expect("JSON body on every response");
    (status, v)
}

/// Shared invariants for every curated arg-error payload.
fn assert_curated(v: &Value, expected_code: &str) {
    assert_eq!(v["ok"], json!(false), "{v}");
    let item = &v["items"][0];
    assert_eq!(item["code"], expected_code, "{item}");
    assert_eq!(item["severity"], "error", "{item}");
    assert!(
        item["span"]["tool"].is_string(),
        "span must name the tool: {item}"
    );
    assert!(
        item["mitigation"].as_array().is_some_and(|m| !m.is_empty()),
        "curated errors must carry mitigation: {item}"
    );
}

#[tokio::test]
async fn missing_step_op_is_curated_with_allowed_ops() {
    let app = app_with(Arc::new(FakeBridgeRpc::responding(json!({}))));
    let (status, v) = call_tool(
        app,
        "mutate_nodes",
        json!({"pid": 34, "steps": [{"path": "/project1/x"}]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_curated(&v, "tdmcp.args.missing_field");
    let item = &v["items"][0];
    assert_eq!(item["span"]["field"], "steps[0].op");
    assert!(item["message"].as_str().unwrap().contains("one of create"));
    // summary mirrors the single item's message (one-fact payloads).
    assert_eq!(v["summary"], item["message"]);
    assert_eq!(item["references"][0]["id"], "describe_tools");
}

#[tokio::test]
async fn unknown_top_level_field_gets_did_you_mean_lint() {
    let app = app_with(Arc::new(FakeBridgeRpc::responding(json!({}))));
    let (_, v) = call_tool(
        app,
        "mutate_nodes",
        json!({"pid": 34, "steps": [], "contextpath": "/project1"}),
    )
    .await;
    assert_curated(&v, "tdmcp.args.unknown_field");
    let lint = &v["items"][0]["lints"][0];
    assert_eq!(lint["code"], "tdmcp.args.similar_field");
    assert_eq!(lint["suggestion"]["replace"], "contextPath");
    assert_eq!(lint["confidence"], "high");
}

#[tokio::test]
async fn bad_enum_value_lists_allowed_variants() {
    let app = app_with(Arc::new(FakeBridgeRpc::responding(json!({}))));
    let (_, v) = call_tool(app, "fleet", json!({"include": ["typo"]})).await;
    assert_curated(&v, "tdmcp.args.unknown_variant");
    let msg = v["items"][0]["message"].as_str().unwrap();
    for allowed in ["process", "bridge", "tasks", "cancelled", "popups"] {
        assert!(msg.contains(allowed), "`{allowed}` missing from: {msg}");
    }
}

#[tokio::test]
async fn wrong_type_has_no_serde_position_noise() {
    let app = app_with(Arc::new(FakeBridgeRpc::responding(json!({}))));
    let (_, v) = call_tool(app, "execute_python", json!({"pid": 34, "script": 5})).await;
    assert_curated(&v, "tdmcp.args.wrong_type");
    let msg = v["items"][0]["message"].as_str().unwrap();
    assert!(!msg.contains("line "), "serde position leaked: {msg}");
    assert!(msg.contains("script"), "{msg}");
}

#[tokio::test]
async fn null_arguments_are_curated_not_protocol_errors() {
    let app = app_with(Arc::new(FakeBridgeRpc::responding(json!({}))));
    // No `arguments` key at all -> Value::Null -> typed parse failure.
    let (status, v) = call_tool(app, "execute_python", json!(null)).await;
    assert_eq!(status, StatusCode::OK);
    assert_curated(&v, "tdmcp.args.wrong_type");
}

#[tokio::test]
async fn inspect_empty_paths_reuses_bridge_catalog_code() {
    let app = app_with(Arc::new(FakeBridgeRpc::responding(json!({}))));
    let (_, v) = call_tool(app, "inspect", json!({"pid": 34, "paths": []})).await;
    assert_curated(&v, "tdmcp.op.paths_required");
    assert_eq!(v["items"][0]["span"]["field"], "paths");
}

#[tokio::test]
async fn api_help_empty_queries_uses_catalog_code() {
    let app = app_with(Arc::new(FakeBridgeRpc::responding(json!({}))));
    let (_, v) = call_tool(app, "api_help", json!({"pid": 34, "queries": []})).await;
    assert_curated(&v, "tdmcp.api_help.queries_required");
}

#[tokio::test]
async fn near_miss_tool_name_gets_suggestion() {
    let app = app_with(Arc::new(FakeBridgeRpc::responding(json!({}))));
    let (status, v) = call_tool(app, "fleets", json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let summary = v["summary"].as_str().unwrap();
    assert!(summary.contains("did you mean `fleet`"), "{summary}");
    assert!(summary.contains("describe_tools"), "{summary}");
}

#[tokio::test]
async fn unrelated_tool_name_gets_no_wrong_hint() {
    let app = app_with(Arc::new(FakeBridgeRpc::responding(json!({}))));
    let (status, v) = call_tool(app, "zzzzzzz", json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let summary = v["summary"].as_str().unwrap();
    assert!(!summary.contains("did you mean"), "{summary}");
    assert!(summary.contains("unknown tool: zzzzzzz"), "{summary}");
}

#[tokio::test]
async fn success_envelope_is_untouched() {
    let bridge = FakeBridgeRpc::responding(json!({}));
    let app = app_with(Arc::new(bridge));
    let (status, v) = call_tool(app, "fleet", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["ok"], json!(true));
    assert!(v["data"]["processes"].is_array(), "{v}");
    assert!(
        v.get("items").is_none(),
        "success must not carry items: {v}"
    );
}

#[tokio::test]
async fn malformed_json_body_gets_curated_envelope_not_raw_text() {
    // A body-extraction rejection (malformed JSON, wrong content-type, or —
    // in production, over the DefaultBodyLimit layer main.rs adds — too
    // large) must not let axum answer with its own raw text body: every
    // failure on this route carries the same {ok:false, summary} envelope.
    // See docs/LIMITS_AUDIT.md §4.5 / §5 Phase 1.3.
    let app = app_with(Arc::new(FakeBridgeRpc::responding(json!({}))));
    let req = Request::builder()
        .method("POST")
        .uri("/mcp/tools/call")
        .header("content-type", "application/json")
        .body(Body::from("{not valid json"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_ne!(resp.status(), StatusCode::OK);
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    assert!(
        content_type.contains("json"),
        "expected a JSON response, got content-type {content_type:?}"
    );
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: Value = serde_json::from_slice(&bytes).expect("body must be JSON, not raw text");
    assert_eq!(v["ok"], json!(false), "{v}");
    assert!(v["summary"].is_string(), "{v}");
}

#[test]
fn unknown_resource_uri_lists_available_ids() {
    let provider = test_resource_provider().expect("resource provider");
    let err = provider
        .read_resource("tdmcp://docs/definitely-not-a-doc")
        .unwrap_err();
    assert!(err.contains("unknown skill id"), "{err}");
    assert!(err.contains("available:"), "must list available ids: {err}");
    assert!(err.contains("resources/list"), "{err}");
}

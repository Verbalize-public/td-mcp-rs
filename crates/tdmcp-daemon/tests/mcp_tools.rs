//! Integration: MCP JSON tool call against in-process router + fake bridge.

#![allow(clippy::unwrap_used, reason = "test setup/assertions may panic")]
#![allow(clippy::expect_used, reason = "test setup/assertions may panic")]

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::json;
use tdmcp_core::{PidRegistry, ProcessAttrs, ProcessFingerprint};
use tdmcp_diagnostics::Catalog;
use tdmcp_mcp::testing::FakeBridgeRpc;
use tdmcp_mcp::{build_mcp_router, AppState, BridgeRpc};
use tower::ServiceExt;

fn registry_with_pid() -> PidRegistry {
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
        chrono::Utc::now(),
    );
    registry
}

#[tokio::test]
async fn tools_list_includes_derived_input_schema() {
    let bridge: Arc<dyn BridgeRpc> = Arc::new(FakeBridgeRpc::responding(json!({})));
    let state = AppState::new(registry_with_pid(), Catalog::fallback(), bridge);
    let app = build_mcp_router(state);

    let req = Request::builder()
        .method("GET")
        .uri("/mcp/tools/list")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let tools = v["tools"].as_array().expect("tools array");
    let exec = tools
        .iter()
        .find(|t| t["name"] == "execute_python")
        .expect("execute_python");
    assert!(
        exec["inputSchema"]["properties"]["pid"].is_object(),
        "derived inputSchema must advertise pid: {exec}"
    );
    assert!(exec["inputSchema"]["properties"]["script"].is_object());
}

#[tokio::test]
async fn unknown_field_rejected() {
    let bridge: Arc<dyn BridgeRpc> =
        Arc::new(FakeBridgeRpc::responding(json!({"ok": true, "result": 1})));
    let state = AppState::new(registry_with_pid(), Catalog::fallback(), bridge);
    let app = build_mcp_router(state);

    let req = Request::builder()
        .method("POST")
        .uri("/mcp/tools/call")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"name":"execute_python","arguments":{"pid":34,"script":"result=1","nope":true}})
                .to_string(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn execute_python_happy_path() {
    let bridge: Arc<dyn BridgeRpc> =
        Arc::new(FakeBridgeRpc::responding(json!({"ok": true, "result": 1})));
    let state = AppState::new(registry_with_pid(), Catalog::fallback(), bridge);
    let app = build_mcp_router(state);

    let req = Request::builder()
        .method("POST")
        .uri("/mcp/tools/call")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"name":"execute_python","arguments":{"pid":34,"script":"result=1","exclusive":false}})
                .to_string(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["data"]["result"], 1);
}

#[tokio::test]
async fn exclusive_fails_while_shared_in_flight() {
    // Gated bridge: the first (shared) call stays in-flight on the bridge, so
    // the second (exclusive) call must hit queue_busy at enqueue time.
    let fake = FakeBridgeRpc::gated(json!({"ok": true, "result": 1}));
    let gate_handle = fake.gate_handle();
    let bridge: Arc<dyn BridgeRpc> = Arc::new(fake);
    let gate = gate_handle.lock().await;

    let state = AppState::new(registry_with_pid(), Catalog::fallback(), bridge);
    let app = build_mcp_router(state.clone());

    let app1 = app.clone();
    let first = tokio::spawn(async move {
        let req = Request::builder()
            .method("POST")
            .uri("/mcp/tools/call")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({"name":"execute_python","arguments":{"pid":34,"script":"result=1","exclusive":false}})
                    .to_string(),
            ))
            .unwrap();
        app1.oneshot(req).await.unwrap()
    });

    // Let the first call enqueue + reach the gated bridge.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let req2 = Request::builder()
        .method("POST")
        .uri("/mcp/tools/call")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"name":"execute_python","arguments":{"pid":34,"script":"result=2","exclusive":true}})
                .to_string(),
        ))
        .unwrap();
    let resp2 = app.oneshot(req2).await.unwrap();
    assert_eq!(resp2.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp2.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["ok"], false);
    assert_eq!(
        v["diagnostics"]["items"][0]["code"],
        "tdmcp.bridge.queue_busy"
    );

    drop(gate);
    let _ = first.await;
}

#[tokio::test]
async fn script_failure_returns_diagnostics() {
    let bridge: Arc<dyn BridgeRpc> = Arc::new(FakeBridgeRpc::responding(
        json!({"ok": false, "error": "NameError: 'x'", "traceback": "  File \"<td>\", line 1"}),
    ));
    let state = AppState::new(registry_with_pid(), Catalog::fallback(), bridge);
    let app = build_mcp_router(state);

    let req = Request::builder()
        .method("POST")
        .uri("/mcp/tools/call")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"name":"execute_python","arguments":{"pid":34,"script":"x"}}).to_string(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["ok"], false);
    assert_eq!(
        v["diagnostics"]["items"][0]["code"],
        "tdmcp.script.execution_failed"
    );
    assert!(v["diagnostics"]["items"][0]["rawTraceback"].is_string());
}

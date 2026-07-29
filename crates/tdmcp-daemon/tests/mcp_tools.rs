//! Integration: MCP JSON tool call against in-process router.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::json;
use tdmcp_core::{PidRegistry, ProcessAttrs, ProcessFingerprint};
use tdmcp_diagnostics::Catalog;
use tdmcp_mcp::{build_mcp_router, AppState};
use tower::ServiceExt;

#[tokio::test]
async fn fleet_and_exclusive_queue_busy() {
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
    let state = AppState::new(registry, Catalog::fallback());
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
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

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
}

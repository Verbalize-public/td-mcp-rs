//! Admin MCP session list + annotate.

#![allow(clippy::unwrap_used, reason = "test setup/assertions may panic")]
#![allow(clippy::expect_used, reason = "test setup/assertions may panic")]

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};
use tdmcp_core::PidRegistry;
use tdmcp_daemon::admin::{build_admin_router, RestartArgs};
use tdmcp_diagnostics::Catalog;
use tdmcp_mcp::testing::FakeBridgeRpc;
use tdmcp_mcp::{AppState, McpHandler};
use tower::ServiceExt;

fn restart_args() -> RestartArgs {
    RestartArgs {
        exe: PathBuf::from("tdmcp-daemon"),
        port: 9860,
        data_dir: PathBuf::from("."),
        bridge_dir: PathBuf::from("."),
        catalog_path: PathBuf::from("."),
        no_gui: true,
    }
}

#[tokio::test]
async fn mcp_sessions_list_and_annotate() {
    let state = AppState::new(
        PidRegistry::new(),
        Catalog::fallback(),
        Arc::new(FakeBridgeRpc::responding(json!({}))),
    );
    let handler = McpHandler::new(state.clone());
    assert_eq!(state.mcp_session_count(), 1);

    // Simulate initialize filling clientInfo.
    state
        .mcp_sessions
        .set_client_info(handler.session_id(), "tdmcp-stdio-proxy", "0.1.0");

    let app = build_admin_router(state.clone(), restart_args());

    let response = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .uri("/admin/mcp-sessions")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: Value = serde_json::from_slice(&body).unwrap();
    let sessions = v["sessions"].as_array().expect("sessions array");
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["clientName"], "tdmcp-stdio-proxy");
    assert_eq!(sessions[0]["clientVersion"], "0.1.0");
    assert!(sessions[0]["id"].as_str().is_some());
    assert!(sessions[0]["connectedAt"].as_u64().unwrap() > 0);

    let annotate = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/admin/mcp-sessions/annotate")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    json!({
                        "matchClientName": "tdmcp-stdio-proxy",
                        "clientName": "Cursor",
                        "clientVersion": "0.42.0"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(annotate.status(), 200);
    let annotate_body = axum::body::to_bytes(annotate.into_body(), usize::MAX)
        .await
        .unwrap();
    let av: Value = serde_json::from_slice(&annotate_body).unwrap();
    assert_eq!(av["ok"], true);

    let status = app
        .oneshot(
            axum::http::Request::builder()
                .uri("/admin/status")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status_body = axum::body::to_bytes(status.into_body(), usize::MAX)
        .await
        .unwrap();
    let sv: Value = serde_json::from_slice(&status_body).unwrap();
    assert_eq!(sv["ok"], true);
    assert_eq!(sv["mcpSessionCount"], 1);
    assert_eq!(sv["bridgeCount"], 0);

    let list = state.mcp_sessions.list();
    assert_eq!(list[0].client_name, "Cursor");
    assert_eq!(list[0].client_version, "0.42.0");

    drop(handler);
    assert_eq!(state.mcp_session_count(), 0);
}

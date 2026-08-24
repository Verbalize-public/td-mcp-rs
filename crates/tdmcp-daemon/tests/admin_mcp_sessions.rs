//! Admin MCP session list + annotate.

#![allow(clippy::unwrap_used, reason = "test setup/assertions may panic")]
#![allow(clippy::expect_used, reason = "test setup/assertions may panic")]

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use serde_json::{json, Value};
use tdmcp_core::{DaemonId, PidRegistry, SlaveRegistry};
use tdmcp_daemon::admin::{build_admin_router, RestartArgs};
use tdmcp_daemon::federation::FederationRuntime;
use tdmcp_daemon::{LogRing, LogSink};
use tdmcp_diagnostics::Catalog;
use tdmcp_mcp::testing::{test_resource_provider, FakeBridgeRpc};
use tdmcp_mcp::{AppState, McpHandler};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

fn restart_args() -> RestartArgs {
    RestartArgs {
        exe: PathBuf::from("tdmcp-daemon"),
        port: 9860,
        bind_address: "127.0.0.1".into(),
        data_dir: PathBuf::from("."),
        bridge_dir: PathBuf::from("."),
        catalog_path: PathBuf::from("."),
        no_gui: true,
    }
}

fn test_federation() -> FederationRuntime {
    FederationRuntime {
        role: "standalone".into(),
        daemon_id: DaemonId::new("test-daemon"),
        hostname: "localhost".into(),
        port: 9860,
        bind_address: "127.0.0.1".into(),
        config_path: PathBuf::from("."),
        slaves: Arc::new(Mutex::new(SlaveRegistry::new())),
        local_auth_token: String::new(),
        master_url: String::new(),
        master_psk: String::new(),
        version: "0.0.0".into(),
    }
}

/// A [`LogSink`] backed by a leaked temp-dir file appender — fine for a
/// short-lived test process, and lets tests push straight into the returned
/// ring while `admin_router_with_logs` serves reads from the same ring
/// through the sink.
fn test_log_sink() -> (LogSink, Arc<LogRing>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let appender = tracing_appender::rolling::never(dir.path(), "test.log");
    let (writer, guard) = tracing_appender::non_blocking(appender);
    std::mem::forget(guard);
    std::mem::forget(dir);
    let ring = Arc::new(LogRing::new(2048));
    (LogSink::new(ring.clone(), writer), ring)
}

fn admin_router(state: AppState) -> axum::Router {
    let (sink, _ring) = test_log_sink();
    admin_router_with_logs(state, sink)
}

fn admin_router_with_logs(state: AppState, logs: LogSink) -> axum::Router {
    build_admin_router(
        state,
        restart_args(),
        CancellationToken::new(),
        Arc::new(AtomicBool::new(false)),
        test_federation(),
        logs,
        PathBuf::from("/tmp/tdmcp-test-logs"),
    )
}

#[tokio::test]
async fn mcp_sessions_list_and_annotate() {
    let state = AppState::new(
        PidRegistry::new(),
        Catalog::fallback(),
        Arc::new(FakeBridgeRpc::responding(json!({}))),
        test_resource_provider().expect("resource provider"),
    );
    let handler = McpHandler::new(state.clone());
    assert_eq!(state.mcp_session_count(), 1);

    // Simulate initialize filling clientInfo.
    state
        .mcp_sessions
        .set_client_info(handler.session_id(), "tdmcp-stdio-proxy", "0.1.0");

    let app = admin_router(state.clone());

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
    assert_eq!(sv["noGui"], true);

    let list = state.mcp_sessions.list();
    assert_eq!(list[0].client_name, "Cursor");
    assert_eq!(list[0].client_version, "0.42.0");

    drop(handler);
    assert_eq!(state.mcp_session_count(), 0);
}

fn test_state() -> AppState {
    AppState::new(
        PidRegistry::new(),
        Catalog::fallback(),
        Arc::new(FakeBridgeRpc::responding(json!({}))),
        test_resource_provider().expect("resource provider"),
    )
}

fn rec(msg: &str, level: tdmcp_daemon::Level, src: tdmcp_daemon::Src) -> tdmcp_daemon::Record {
    tdmcp_daemon::Record {
        seq: 0,
        ts: "2026-01-01T00:00:00.000Z".into(),
        level,
        src,
        pid: 1,
        target: "test".into(),
        msg: msg.into(),
        code: None,
        kvs: Default::default(),
    }
}

async fn get_json(app: &axum::Router, uri: &str) -> (axum::http::StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .uri(uri)
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}

#[tokio::test]
async fn admin_logs_cursor_resume_loses_nothing() {
    let (sink, ring) = test_log_sink();
    for i in 0..5 {
        ring.push(rec(
            &format!("line {i}"),
            tdmcp_daemon::Level::Info,
            tdmcp_daemon::Src::Daemon,
        ));
    }
    let app = admin_router_with_logs(test_state(), sink);

    let (status, first) = get_json(&app, "/admin/logs?after=0&limit=3").await;
    assert_eq!(status, 200);
    let first_records = first["records"].as_array().expect("records");
    assert_eq!(first_records.len(), 3);
    assert_eq!(first_records[0]["msg"], "line 0");
    let cursor = first_records[2]["seq"].as_u64().expect("seq");
    assert_eq!(first["next"], first["next"], "next present");

    let (status2, second) = get_json(&app, &format!("/admin/logs?after={cursor}&limit=8")).await;
    assert_eq!(status2, 200);
    let second_records = second["records"].as_array().expect("records");
    assert_eq!(second_records.len(), 2, "the remaining 2 of 5 pushed");
    assert_eq!(second_records[0]["msg"], "line 3");
    assert_eq!(second_records[1]["msg"], "line 4");
}

#[tokio::test]
async fn admin_logs_limit_is_clamped_server_side() {
    let (sink, ring) = test_log_sink();
    for i in 0..600 {
        ring.push(rec(
            &format!("m{i}"),
            tdmcp_daemon::Level::Info,
            tdmcp_daemon::Src::Daemon,
        ));
    }
    let app = admin_router_with_logs(test_state(), sink);
    let (status, body) = get_json(&app, "/admin/logs?limit=10000").await;
    assert_eq!(status, 200);
    assert_eq!(body["records"].as_array().expect("records").len(), 512);
}

#[tokio::test]
async fn admin_logs_filters_by_level_and_src() {
    let (sink, ring) = test_log_sink();
    ring.push(rec("info-daemon", tdmcp_daemon::Level::Info, tdmcp_daemon::Src::Daemon));
    ring.push(rec("warn-bridge", tdmcp_daemon::Level::Warn, tdmcp_daemon::Src::Bridge));
    ring.push(rec("error-mcp", tdmcp_daemon::Level::Error, tdmcp_daemon::Src::Mcp));
    let app = admin_router_with_logs(test_state(), sink);

    let (_, warn_up) = get_json(&app, "/admin/logs?level=warn").await;
    let records = warn_up["records"].as_array().expect("records");
    assert_eq!(records.len(), 2, "warn and error, not info");

    let (_, bridge_only) = get_json(&app, "/admin/logs?src=bridge").await;
    let records = bridge_only["records"].as_array().expect("records");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["msg"], "warn-bridge");

    let (_, multi_src) = get_json(&app, "/admin/logs?src=bridge,mcp").await;
    assert_eq!(multi_src["records"].as_array().expect("records").len(), 2);
}

#[tokio::test]
async fn admin_logs_bad_level_and_src_are_400() {
    let (sink, _ring) = test_log_sink();
    let app = admin_router_with_logs(test_state(), sink);

    let (status, body) = get_json(&app, "/admin/logs?level=catastrophic").await;
    assert_eq!(status, 400);
    assert_eq!(body["ok"], false);

    let (status2, _) = get_json(&app, "/admin/logs?src=not_a_src").await;
    assert_eq!(status2, 400);
}

#[tokio::test]
async fn admin_logs_path_returns_configured_dir() {
    let (sink, _ring) = test_log_sink();
    let app = admin_router_with_logs(test_state(), sink);
    let (status, body) = get_json(&app, "/admin/logs/path").await;
    assert_eq!(status, 200);
    assert!(body["dir"].as_str().expect("dir").contains("tdmcp-test-logs"));
}

async fn post_json(app: &axum::Router, uri: &str, body: Value) -> (axum::http::StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(axum::body::Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let resp_body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&resp_body).unwrap())
}

/// M5: acceptance shape — a proxy POST to `/admin/logs/ingest` lands
/// `src:"proxy"` records centrally, pid 0 with the proxy pid carried in
/// `kvs.proxyPid` (never trusting the payload's own claimed identity).
#[tokio::test]
async fn admin_logs_ingest_stamps_proxy_src_and_pid_zero() {
    let (sink, ring) = test_log_sink();
    let app = admin_router_with_logs(test_state(), sink);

    let (status, body) = post_json(
        &app,
        "/admin/logs/ingest",
        json!({
            "pid": 4242,
            "lines": [
                {"level": "error", "target": "stdio_proxy", "msg": "heal failed",
                 "pid": 999999, "src": "daemon"}
            ]
        }),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body["ok"], true);
    assert_eq!(body["ingested"], 1);

    let (recs, _) = ring.snapshot_after(0, 8, None, &[]);
    assert_eq!(recs.len(), 1);
    assert_eq!(recs[0].pid, 0, "record pid must never be the proxy's own claim");
    assert_eq!(recs[0].src, tdmcp_daemon::Src::Proxy);
    assert_eq!(recs[0].kvs.get("proxyPid").map(String::as_str), Some("4242"));
    assert_eq!(recs[0].msg, "heal failed");
}

#[tokio::test]
async fn admin_logs_ingest_ignores_payload_without_lines() {
    let (sink, ring) = test_log_sink();
    let app = admin_router_with_logs(test_state(), sink);

    let (status, body) = post_json(&app, "/admin/logs/ingest", json!({"pid": 1})).await;
    assert_eq!(status, 200);
    assert_eq!(body["ingested"], 0);
    assert!(ring.snapshot_after(0, 8, None, &[]).0.is_empty());
}


//! Stdio MCP proxy → Streamable HTTP daemon (in-process), fleet round-trip
//! and reconnect-after-daemon-restart coverage.

#![allow(clippy::unwrap_used, reason = "test setup/assertions may panic")]
#![allow(clippy::expect_used, reason = "test setup/assertions may panic")]
#![allow(clippy::panic, reason = "test setup/assertions may panic")]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::routing::get;
use axum::Json;
use rmcp::model::{CallToolRequestParams, ErrorCode};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ServiceError, ServiceExt};
use serde_json::json;
use tdmcp_core::{PidRegistry, ProcessAttrs, ProcessFingerprint};
use tdmcp_diagnostics::codes;
use tdmcp_diagnostics::Catalog;
use tdmcp_mcp::testing::FakeBridgeRpc;
use tdmcp_mcp::{
    run_stdio_proxy_rw, run_stdio_proxy_rw_config, AppState, BridgeRpc, McpHandler, ReconnectConfig,
};
use tokio_util::sync::CancellationToken;

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

/// Spawn HTTP daemon with `/mcp/rpc` + `/mcp/health` on an ephemeral port.
async fn spawn_http_daemon(bridge: Arc<dyn BridgeRpc>) -> (String, SocketAddr, CancellationToken) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ct = spawn_http_daemon_on(bridge, listener).await;
    (format!("http://{addr}/mcp/rpc"), addr, ct)
}

/// Spawn on an already-bound listener (for same-port restart tests).
async fn spawn_http_daemon_on(
    bridge: Arc<dyn BridgeRpc>,
    listener: tokio::net::TcpListener,
) -> CancellationToken {
    let state = AppState::new(registry_with_pid(), Catalog::fallback(), bridge);
    let ct = CancellationToken::new();
    let service: StreamableHttpService<McpHandler, LocalSessionManager> =
        StreamableHttpService::new(
            move || Ok(McpHandler::new(state.clone())),
            Default::default(),
            StreamableHttpServerConfig::default()
                .with_sse_keep_alive(None)
                .with_cancellation_token(ct.child_token()),
        );
    let router = axum::Router::new()
        .route(
            "/mcp/health",
            get(|| async { Json(serde_json::json!({"ok": true})) }),
        )
        .nest_service("/mcp/rpc", service);
    tokio::spawn({
        let ct = ct.clone();
        async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move { ct.cancelled_owned().await })
                .await;
        }
    });
    // Brief yield so the accept loop is live before clients connect.
    tokio::task::yield_now().await;
    ct
}

async fn wait_port_free(addr: SocketAddr) {
    for _ in 0..50 {
        if tokio::net::TcpListener::bind(addr).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("port {addr} did not become free");
}

fn fast_reconnect_config() -> ReconnectConfig {
    ReconnectConfig {
        recent: Duration::from_millis(3_000),
        stale: Duration::from_millis(15_000),
        debounce: Duration::from_millis(50),
        probe_interval: Duration::from_millis(100),
        probe_max: Duration::from_millis(500),
    }
}

#[tokio::test]
async fn stdio_proxy_fleet_round_trip() {
    let bridge: Arc<dyn BridgeRpc> = Arc::new(FakeBridgeRpc::responding(json!({})));
    let (url, _addr, ct) = spawn_http_daemon(bridge).await;

    let (client_side, server_side) = tokio::io::duplex(64 * 1024);
    let (server_read, server_write) = tokio::io::split(server_side);
    let proxy_task =
        tokio::spawn(async move { run_stdio_proxy_rw(&url, server_read, server_write).await });

    let client = ().serve(client_side).await.expect("stdio client initialize");

    let tools = client.list_tools(None).await.expect("list_tools");
    let names: Vec<_> = tools.tools.iter().map(|t| t.name.as_ref()).collect();
    assert!(names.contains(&"fleet"), "tools={names:?}");
    assert!(names.contains(&"describe_tools"), "tools={names:?}");

    let result = client
        .call_tool(
            CallToolRequestParams::new("fleet")
                .with_arguments(json!({}).as_object().cloned().unwrap_or_default()),
        )
        .await
        .expect("call fleet");

    let structured = result.structured_content.expect("structured_content");
    let processes = structured
        .get("processes")
        .and_then(|p| p.as_array())
        .expect("processes array");
    assert_eq!(processes.len(), 1);
    assert_eq!(processes[0].get("pid"), Some(&json!(34)));

    let _ = client.cancel().await;
    let proxy_result = proxy_task.await.expect("join proxy");
    assert!(
        proxy_result.is_ok(),
        "proxy should exit cleanly: {proxy_result:?}"
    );
    ct.cancel();
}

#[tokio::test]
async fn stdio_proxy_preserves_invalid_params_code() {
    let bridge: Arc<dyn BridgeRpc> = Arc::new(FakeBridgeRpc::responding(json!({})));
    let (url, _addr, ct) = spawn_http_daemon(bridge).await;

    let (client_side, server_side) = tokio::io::duplex(64 * 1024);
    let (server_read, server_write) = tokio::io::split(server_side);
    let proxy_task =
        tokio::spawn(async move { run_stdio_proxy_rw(&url, server_read, server_write).await });

    let client = ().serve(client_side).await.expect("stdio client initialize");

    let err = client
        .call_tool(
            CallToolRequestParams::new("fleet").with_arguments(
                json!({"include": ["typo"]})
                    .as_object()
                    .cloned()
                    .unwrap_or_default(),
            ),
        )
        .await
        .expect_err("typo include must fail");

    match err {
        ServiceError::McpError(data) => {
            assert_eq!(
                data.code,
                ErrorCode::INVALID_PARAMS,
                "stdio proxy must forward -32602, not remap to internal_error: {data}"
            );
            let msg = data.message.to_string();
            assert!(msg.contains("typo"), "message should mention typo: {msg}");
        }
        other => panic!("expected ServiceError::McpError, got {other:?}"),
    }

    let _ = client.cancel().await;
    let proxy_result = proxy_task.await.expect("join proxy");
    assert!(
        proxy_result.is_ok(),
        "proxy should exit cleanly: {proxy_result:?}"
    );
    ct.cancel();
}

#[tokio::test]
async fn stdio_proxy_unreachable_after_daemon_kill() {
    let bridge: Arc<dyn BridgeRpc> = Arc::new(FakeBridgeRpc::responding(json!({})));
    let (url, _addr, ct) = spawn_http_daemon(bridge).await;

    let (client_side, server_side) = tokio::io::duplex(64 * 1024);
    let (server_read, server_write) = tokio::io::split(server_side);
    let cfg = fast_reconnect_config();
    let proxy_task = tokio::spawn(async move {
        run_stdio_proxy_rw_config(&url, server_read, server_write, cfg).await
    });

    let client = ().serve(client_side).await.expect("stdio client initialize");
    client
        .call_tool(
            CallToolRequestParams::new("fleet")
                .with_arguments(json!({}).as_object().cloned().unwrap_or_default()),
        )
        .await
        .expect("fleet before kill");

    ct.cancel();
    tokio::time::sleep(Duration::from_millis(200)).await;

    let err = client
        .call_tool(
            CallToolRequestParams::new("fleet")
                .with_arguments(json!({}).as_object().cloned().unwrap_or_default()),
        )
        .await
        .expect_err("fleet after kill must fail");

    match err {
        ServiceError::McpError(data) => {
            assert_eq!(data.code, ErrorCode::INTERNAL_ERROR);
            let msg = data.message.to_string();
            assert!(
                msg.contains("daemon") || msg.contains("unreachable") || msg.contains("lost"),
                "message should be informative: {msg}"
            );
            let data = data.data.expect("error data payload");
            assert_eq!(
                data.get("code").and_then(|c| c.as_str()),
                Some(codes::DAEMON_UNREACHABLE)
            );
            assert_eq!(data.get("healed").and_then(|h| h.as_bool()), Some(false));
        }
        other => panic!("expected McpError, got {other:?}"),
    }

    let _ = client.cancel().await;
    let _ = proxy_task.await;
}

#[tokio::test]
async fn stdio_proxy_recovers_after_daemon_restart() {
    let bridge: Arc<dyn BridgeRpc> = Arc::new(FakeBridgeRpc::responding(json!({})));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{addr}/mcp/rpc");
    let ct = spawn_http_daemon_on(Arc::clone(&bridge), listener).await;

    let (client_side, server_side) = tokio::io::duplex(64 * 1024);
    let (server_read, server_write) = tokio::io::split(server_side);
    let cfg = fast_reconnect_config();
    let url_clone = url.clone();
    let proxy_task = tokio::spawn(async move {
        run_stdio_proxy_rw_config(&url_clone, server_read, server_write, cfg).await
    });

    let client = ().serve(client_side).await.expect("stdio client initialize");
    client
        .call_tool(
            CallToolRequestParams::new("fleet")
                .with_arguments(json!({}).as_object().cloned().unwrap_or_default()),
        )
        .await
        .expect("fleet before restart");

    ct.cancel();
    wait_port_free(addr).await;

    // First post-kill call: informative unreachable (heal cannot succeed yet).
    let err = client
        .call_tool(
            CallToolRequestParams::new("fleet")
                .with_arguments(json!({}).as_object().cloned().unwrap_or_default()),
        )
        .await
        .expect_err("must fail while down");
    match &err {
        ServiceError::McpError(data) => {
            assert_eq!(
                data.data
                    .as_ref()
                    .and_then(|d| d.get("code"))
                    .and_then(|c| c.as_str()),
                Some(codes::DAEMON_UNREACHABLE)
            );
        }
        other => panic!("expected McpError, got {other:?}"),
    }

    // Bring daemon back on the same port.
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    let ct2 = spawn_http_daemon_on(bridge, listener).await;

    // Watcher (or the next call's heal) should reconnect. No silent retry on the
    // failed call — but a subsequent call must succeed on the same stdio session.
    let mut recovered = false;
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        match client
            .call_tool(
                CallToolRequestParams::new("fleet")
                    .with_arguments(json!({}).as_object().cloned().unwrap_or_default()),
            )
            .await
        {
            Ok(result) => {
                let structured = result.structured_content.expect("structured");
                let processes = structured
                    .get("processes")
                    .and_then(|p| p.as_array())
                    .expect("processes");
                assert_eq!(processes.len(), 1);
                recovered = true;
                break;
            }
            Err(ServiceError::McpError(data)) => {
                let code = data
                    .data
                    .as_ref()
                    .and_then(|d| d.get("code"))
                    .and_then(|c| c.as_str());
                assert_eq!(code, Some(codes::DAEMON_UNREACHABLE));
                // If healed mid-call, next iteration should succeed.
                if data
                    .data
                    .as_ref()
                    .and_then(|d| d.get("healed"))
                    .and_then(|h| h.as_bool())
                    == Some(true)
                {
                    continue;
                }
            }
            Err(other) => panic!("unexpected error: {other:?}"),
        }
    }
    assert!(recovered, "stdio proxy should recover after daemon restart");

    let _ = client.cancel().await;
    let _ = proxy_task.await;
    ct2.cancel();
}

#[tokio::test]
async fn stdio_proxy_watcher_heals_without_intervening_success() {
    // Kill → fail one call (marks unhealthy) → restart → wait for watcher →
    // next call succeeds (heal already done; may still return healed error once).
    let bridge: Arc<dyn BridgeRpc> = Arc::new(FakeBridgeRpc::responding(json!({})));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{addr}/mcp/rpc");
    let ct = spawn_http_daemon_on(Arc::clone(&bridge), listener).await;

    let (client_side, server_side) = tokio::io::duplex(64 * 1024);
    let (server_read, server_write) = tokio::io::split(server_side);
    let cfg = fast_reconnect_config();
    let url_clone = url.clone();
    let proxy_task = tokio::spawn(async move {
        run_stdio_proxy_rw_config(&url_clone, server_read, server_write, cfg).await
    });

    let client = ().serve(client_side).await.expect("stdio client initialize");
    client
        .call_tool(
            CallToolRequestParams::new("fleet")
                .with_arguments(json!({}).as_object().cloned().unwrap_or_default()),
        )
        .await
        .expect("fleet before kill");

    ct.cancel();
    wait_port_free(addr).await;

    let _ = client
        .call_tool(
            CallToolRequestParams::new("fleet")
                .with_arguments(json!({}).as_object().cloned().unwrap_or_default()),
        )
        .await
        .expect_err("mark unhealthy");

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    let ct2 = spawn_http_daemon_on(bridge, listener).await;

    // Give the background watcher time to heal with no successful tool call yet.
    tokio::time::sleep(Duration::from_millis(800)).await;

    let result = client
        .call_tool(
            CallToolRequestParams::new("fleet")
                .with_arguments(json!({}).as_object().cloned().unwrap_or_default()),
        )
        .await
        .expect("fleet after watcher heal");
    assert!(result.structured_content.is_some());

    let _ = client.cancel().await;
    let _ = proxy_task.await;
    ct2.cancel();
}

#[tokio::test]
async fn concurrent_calls_during_heal_share_outcome() {
    // After restart, parallel tool calls must wait for the in-flight heal rather
    // than thundering-herd `healed: false` from try_lock fail-open.
    let bridge: Arc<dyn BridgeRpc> = Arc::new(FakeBridgeRpc::responding(json!({})));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{addr}/mcp/rpc");
    let ct = spawn_http_daemon_on(Arc::clone(&bridge), listener).await;

    let (client_side, server_side) = tokio::io::duplex(64 * 1024);
    let (server_read, server_write) = tokio::io::split(server_side);
    let cfg = fast_reconnect_config();
    let url_clone = url.clone();
    let proxy_task = tokio::spawn(async move {
        run_stdio_proxy_rw_config(&url_clone, server_read, server_write, cfg).await
    });

    let client = ().serve(client_side).await.expect("stdio client initialize");
    client
        .call_tool(
            CallToolRequestParams::new("fleet")
                .with_arguments(json!({}).as_object().cloned().unwrap_or_default()),
        )
        .await
        .expect("fleet before restart");

    ct.cancel();
    wait_port_free(addr).await;

    let _ = client
        .call_tool(
            CallToolRequestParams::new("fleet")
                .with_arguments(json!({}).as_object().cloned().unwrap_or_default()),
        )
        .await
        .expect_err("mark unhealthy");

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    let ct2 = spawn_http_daemon_on(bridge, listener).await;

    // Brief pause so health is up, then stampede.
    tokio::time::sleep(Duration::from_millis(100)).await;

    let args = json!({}).as_object().cloned().unwrap_or_default();
    let results = futures::future::join_all((0..8).map(|_| {
        client.call_tool(CallToolRequestParams::new("fleet").with_arguments(args.clone()))
    }))
    .await;

    let mut ok = 0usize;
    let mut healed_err = 0usize;
    let mut other_err = 0usize;
    for result in results {
        match result {
            Ok(_) => ok += 1,
            Err(ServiceError::McpError(data)) => {
                let healed = data
                    .data
                    .as_ref()
                    .and_then(|d| d.get("healed"))
                    .and_then(|h| h.as_bool());
                if healed == Some(true) {
                    healed_err += 1;
                } else {
                    other_err += 1;
                }
            }
            Err(_) => other_err += 1,
        }
    }

    assert!(
        ok + healed_err >= 6,
        "most parallel calls should succeed or report healed reconnect (ok={ok} healed_err={healed_err} other_err={other_err})"
    );

    // Follow-up call must succeed on the shared session.
    client
        .call_tool(
            CallToolRequestParams::new("fleet")
                .with_arguments(json!({}).as_object().cloned().unwrap_or_default()),
        )
        .await
        .expect("fleet after concurrent heal");

    let _ = client.cancel().await;
    let _ = proxy_task.await;
    ct2.cancel();
}

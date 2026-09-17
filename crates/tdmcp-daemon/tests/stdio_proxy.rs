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
use tdmcp_mcp::testing::{test_resource_provider, FakeBridgeRpc};
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
    let state = AppState::new(
        registry_with_pid(),
        Catalog::fallback(),
        bridge,
        test_resource_provider().expect("resource provider"),
    );
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
        ..Default::default()
    }
}

#[tokio::test]
async fn stdio_proxy_tools_list_and_fleet_on_wire() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    for version in ["2025-06-18", "2026-07-28"] {
        let bridge: Arc<dyn BridgeRpc> = Arc::new(FakeBridgeRpc::responding(json!({
            "ok": true, "path": "/project1/out1", "bytes": 1,
            "mimeType": "image/png", "imageBase64": "eA=="
        })));
        let (url, _addr, ct) = spawn_http_daemon(bridge).await;
        let (client_side, server_side) = tokio::io::duplex(64 * 1024);
        let (server_read, server_write) = tokio::io::split(server_side);
        let url = url.clone();
        let proxy_task =
            tokio::spawn(async move { run_stdio_proxy_rw(&url, server_read, server_write).await });
        let (read, mut write) = tokio::io::split(client_side);
        let mut read = BufReader::new(read);
        let init = json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "protocolVersion": version,
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "1.0"}
            }
        });
        write
            .write_all(format!("{init}\n").as_bytes())
            .await
            .unwrap();
        let mut line = String::new();
        tokio::time::timeout(Duration::from_secs(10), read.read_line(&mut line))
            .await
            .expect("initialize timeout")
            .unwrap();
        let init: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(init["result"]["protocolVersion"], version);
        let initialized = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
        let list = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}});
        write
            .write_all(format!("{initialized}\n{list}\n").as_bytes())
            .await
            .unwrap();
        line.clear();
        tokio::time::timeout(Duration::from_secs(10), read.read_line(&mut line))
            .await
            .expect("tools/list timeout")
            .unwrap();
        let response: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(response["id"], 2);
        assert!(response.get("error").is_none(), "{response}");
        let result = &response["result"];
        assert!(result["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["name"] == "fleet"));
        if version == "2026-07-28" {
            assert_eq!(result["resultType"], "complete");
            assert_eq!(result["ttlMs"], 0);
            assert_eq!(result["cacheScope"], "private");
        } else {
            assert!(result.get("resultType").is_none());
        }
        for (id, method, params, field) in [
            (
                10,
                "resources/read",
                json!({"uri": "tdmcp://docs/operate"}),
                "contents",
            ),
            (11, "resources/list", json!({}), "resources"),
            (
                12,
                "resources/templates/list",
                json!({}),
                "resourceTemplates",
            ),
        ] {
            let request = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
            write
                .write_all(format!("{request}\n").as_bytes())
                .await
                .unwrap();
            line.clear();
            tokio::time::timeout(Duration::from_secs(10), read.read_line(&mut line))
                .await
                .expect("resource response timeout")
                .unwrap();
            let response: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(response["id"], id);
            assert!(response.get("error").is_none(), "{response}");
            let result = &response["result"];
            assert!(result[field].is_array(), "{response}");
            if method == "resources/read" {
                assert_eq!(result[field][0]["uri"], "tdmcp://docs/operate");
                assert_eq!(result[field][0]["mimeType"], "text/markdown");
                assert!(!result[field][0]["text"].as_str().unwrap().is_empty());
            } else if method == "resources/list" {
                assert!(result[field]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|resource| resource["uri"] == "tdmcp://docs/operate"));
            } else {
                assert_eq!(result[field], json!([]));
            }
            if version == "2026-07-28" {
                assert_eq!(result["resultType"], "complete", "{method}");
                assert_eq!(result["ttlMs"], 0, "{method}: missing cache TTL");
                assert_eq!(
                    result["cacheScope"], "private",
                    "{method}: missing cache scope"
                );
            } else {
                assert!(result.get("resultType").is_none(), "{method}");
            }
        }
        let call = json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": {"name": "fleet", "arguments": {}}});
        write
            .write_all(format!("{call}\n").as_bytes())
            .await
            .unwrap();
        line.clear();
        tokio::time::timeout(Duration::from_secs(10), read.read_line(&mut line))
            .await
            .expect("fleet timeout")
            .unwrap();
        let response: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(response["id"], 3);
        assert!(response.get("error").is_none(), "{response}");
        let result = &response["result"];
        assert_eq!(result["isError"], false);
        assert_eq!(result["structuredContent"]["processes"][0]["pid"], 34);
        assert_eq!(result["content"][0]["type"], "text");
        let text: serde_json::Value =
            serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(text, result["structuredContent"]);
        if version == "2026-07-28" {
            assert_eq!(result["resultType"], "complete", "{response}");
        } else {
            assert!(result.get("resultType").is_none(), "{response}");
        }
        for (id, name, arguments) in [
            (4, "fleet", json!({"include": ["typo"]})),
            (5, "no_such_tool", json!({})),
            (
                6,
                "capture",
                json!({"pid": 34, "path": "/project1/out1", "mode": "top"}),
            ),
        ] {
            let call = json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
                "params": {"name": name, "arguments": arguments}});
            write
                .write_all(format!("{call}\n").as_bytes())
                .await
                .unwrap();
            line.clear();
            tokio::time::timeout(Duration::from_secs(10), read.read_line(&mut line))
                .await
                .expect("tool response timeout")
                .unwrap();
            let response: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(response["id"], id, "{response}");
            if name == "no_such_tool" {
                assert!(response.get("result").is_none(), "{response}");
                assert_eq!(response["error"]["code"], -32602, "{response}");
                assert!(response["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains(name));
                continue;
            }
            assert!(response.get("error").is_none(), "{response}");
            let result = &response["result"];
            if version == "2026-07-28" {
                assert_eq!(result["resultType"], "complete", "{response}");
            } else {
                assert!(result.get("resultType").is_none(), "{response}");
            }
            let content = result["content"].as_array().unwrap();
            let text = content.iter().find(|item| item["type"] == "text").unwrap();
            if name == "fleet" {
                let text: serde_json::Value =
                    serde_json::from_str(text["text"].as_str().unwrap()).unwrap();
                assert_eq!(text, result["structuredContent"]);
                assert_eq!(result["isError"], true, "{response}");
                let item = &result["structuredContent"]["items"][0];
                assert_eq!(item["code"], "tdmcp.args.unknown_variant");
                assert_eq!(item["span"]["field"], "include[0]");
            } else {
                assert_eq!(result["isError"], false, "{response}");
                assert_eq!(result["structuredContent"]["path"], "/project1/out1");
                assert!(result["structuredContent"].get("imageBase64").is_none());
                let images: Vec<_> = content
                    .iter()
                    .filter(|item| item["type"] == "image")
                    .collect();
                assert_eq!(images.len(), 1, "{response}");
                assert_eq!(images[0]["data"], "eA==");
                assert_eq!(images[0]["mimeType"], "image/png");
            }
        }
        drop(write);
        drop(read);
        tokio::time::timeout(Duration::from_secs(10), proxy_task)
            .await
            .expect("proxy shutdown timeout")
            .expect("join proxy")
            .expect("proxy exit");
        ct.cancel();
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

    // Protocol-level errors stay protocol errors end-to-end: unknown tool
    // remains -32602 through the proxy.
    let err = client
        .call_tool(
            CallToolRequestParams::new("no_such_tool")
                .with_arguments(json!({}).as_object().cloned().unwrap_or_default()),
        )
        .await
        .expect_err("unknown tool must fail");

    match err {
        ServiceError::McpError(data) => {
            assert_eq!(
                data.code,
                ErrorCode::INVALID_PARAMS,
                "stdio proxy must forward -32602, not remap to internal_error: {data}"
            );
            let msg = data.message.to_string();
            assert!(
                msg.contains("no_such_tool"),
                "message should mention the tool: {msg}"
            );
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
async fn stdio_proxy_forwards_curated_arg_errors() {
    let bridge: Arc<dyn BridgeRpc> = Arc::new(FakeBridgeRpc::responding(json!({})));
    let (url, _addr, ct) = spawn_http_daemon(bridge).await;

    let (client_side, server_side) = tokio::io::duplex(64 * 1024);
    let (server_read, server_write) = tokio::io::split(server_side);
    let proxy_task =
        tokio::spawn(async move { run_stdio_proxy_rw(&url, server_read, server_write).await });

    let client = ().serve(client_side).await.expect("stdio client initialize");

    // Schema-level arg failures arrive as structured isError results —
    // catalog-backed, hinted, untouched by the proxy.
    let result = client
        .call_tool(
            CallToolRequestParams::new("fleet").with_arguments(
                json!({"include": ["typo"]})
                    .as_object()
                    .cloned()
                    .unwrap_or_default(),
            ),
        )
        .await
        .expect("bad-include returns an isError result, not a protocol error");

    assert_eq!(result.is_error, Some(true), "curated arg error: {result:?}");
    let structured = result.structured_content.expect("structured_content");
    let item = &structured["items"][0];
    assert_eq!(item["code"], "tdmcp.args.unknown_variant", "{item}");
    assert_eq!(item["span"]["field"], "include[0]", "{item}");
    assert!(
        item["message"]
            .as_str()
            .unwrap_or_default()
            .contains("tasks"),
        "allowed variants must be listed: {item}"
    );

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

/// A daemon session that never answers must not hang the MCP client forever:
/// the proxy bounds every forwarded call, and on budget expiry heals the link
/// (fresh session) and returns a structured `tdmcp.daemon.unreachable` error.
/// A follow-up call on the healed link then succeeds.
#[tokio::test]
async fn stdio_proxy_call_timeout_heals_and_returns_budget_error() {
    // Bridge that never answers while the gate is held (simulates a wedged
    // session whose response channel stalls).
    let fake = FakeBridgeRpc::gated(json!({}));
    let gate_handle = fake.gate_handle();
    let held_gate = gate_handle.lock().await;
    let bridge: Arc<dyn BridgeRpc> = Arc::new(fake);

    let (url, _addr, ct) = spawn_http_daemon(bridge.clone()).await;
    let (client_side, server_side) = tokio::io::duplex(64 * 1024);
    let (server_read, server_write) = tokio::io::split(server_side);
    // Short ceilings for both budget classes so the test completes quickly;
    // everything else uses the fast reconnect defaults.
    let cfg = ReconnectConfig {
        call_timeout: Duration::from_millis(800),
        script_timeout: Duration::from_millis(800),
        ..fast_reconnect_config()
    };
    let url_clone = url.clone();
    let proxy_task = tokio::spawn(async move {
        run_stdio_proxy_rw_config(&url_clone, server_read, server_write, cfg).await
    });

    let client = ().serve(client_side).await.expect("stdio client initialize");

    // First call: a bridged call the daemon never answers → proxy must time
    // out (~800ms), heal the link, and surface a budget error — not hang.
    let inspect_args = json!({ "pid": 34, "paths": ["/project1"] })
        .as_object()
        .cloned()
        .unwrap_or_default();
    let started = std::time::Instant::now();
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        client
            .call_tool(CallToolRequestParams::new("inspect").with_arguments(inspect_args.clone())),
    )
    .await
    .expect("proxy must not hang past the bounded budget");

    let elapsed = started.elapsed();
    let err = result.expect_err("call must fail after budget expiry");
    let ServiceError::McpError(data) = err else {
        panic!("expected McpError, got {err:?}");
    };
    let payload = data.data.expect("structured payload");
    assert_eq!(
        payload.get("code").and_then(|c| c.as_str()),
        Some(codes::DAEMON_UNREACHABLE)
    );
    assert_eq!(
        payload.get("healed").and_then(|h| h.as_bool()),
        Some(true),
        "proxy must have healed the link on timeout"
    );
    assert!(
        data.message.contains("budget"),
        "message should mention the budget: {}",
        data.message
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "timeout must fire near the budget, took {elapsed:?}"
    );

    // Release the bridge gate and confirm the healed link serves new calls.
    drop(held_gate);
    client
        .call_tool(CallToolRequestParams::new("inspect").with_arguments(inspect_args))
        .await
        .expect("bridged call succeeds on the healed session");

    let _ = client.cancel().await;
    let _ = proxy_task.await;
    ct.cancel();
}

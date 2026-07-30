//! Stdio MCP proxy → Streamable HTTP daemon (in-process), fleet round-trip.

#![allow(clippy::unwrap_used, reason = "test setup/assertions may panic")]
#![allow(clippy::expect_used, reason = "test setup/assertions may panic")]

use std::sync::Arc;

use rmcp::model::CallToolRequestParams;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::ServiceExt;
use serde_json::json;
use tdmcp_core::{PidRegistry, ProcessAttrs, ProcessFingerprint};
use tdmcp_diagnostics::Catalog;
use tdmcp_mcp::testing::FakeBridgeRpc;
use tdmcp_mcp::{run_stdio_proxy_rw, AppState, BridgeRpc, McpHandler};
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

async fn spawn_http_daemon(bridge: Arc<dyn BridgeRpc>) -> (String, CancellationToken) {
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
    let router = axum::Router::new().nest_service("/mcp/rpc", service);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn({
        let ct = ct.clone();
        async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move { ct.cancelled_owned().await })
                .await;
        }
    });
    (format!("http://{addr}/mcp/rpc"), ct)
}

#[tokio::test]
async fn stdio_proxy_fleet_round_trip() {
    let bridge: Arc<dyn BridgeRpc> = Arc::new(FakeBridgeRpc::responding(json!({})));
    let (url, ct) = spawn_http_daemon(bridge).await;

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

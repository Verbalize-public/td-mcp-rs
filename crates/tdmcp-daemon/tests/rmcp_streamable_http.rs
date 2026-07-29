//! Integration: real `rmcp` Streamable HTTP transport end-to-end.
//!
//! Exercises the exact wiring `main.rs` uses (`McpHandler` over `AppState`,
//! `StreamableHttpService` + `LocalSessionManager`) against a live TCP
//! listener, with a fake bridge standing in for TouchDesigner. Covers the
//! legacy session handshake (`initialize` -> `notifications/initialized` ->
//! `tools/call`) since that is what real MCP clients (Claude Desktop, etc.)
//! speak by default.

#![allow(clippy::unwrap_used, reason = "test setup/assertions may panic")]
#![allow(clippy::expect_used, reason = "test setup/assertions may panic")]

use std::sync::Arc;

use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::session::SessionId;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use serde_json::{json, Value};
use tdmcp_core::{PidRegistry, ProcessAttrs, ProcessFingerprint};
use tdmcp_diagnostics::Catalog;
use tdmcp_mcp::testing::FakeBridgeRpc;
use tdmcp_mcp::{AppState, BridgeRpc, McpHandler};
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

/// Spawn the real streamable-http router (same shape as `main.rs`) on an
/// ephemeral port, backed by `bridge`. Returns the base URL and a shutdown
/// token.
async fn spawn(bridge: Arc<dyn BridgeRpc>) -> (reqwest::Client, String, CancellationToken) {
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
    let client = reqwest::Client::new();
    (client, format!("http://{addr}/mcp/rpc"), ct)
}

/// Parse an SSE body into its `data:` payloads (JSON-decoded), in order.
fn sse_data_events(body: &str) -> Vec<Value> {
    body.split("\n\n")
        .filter(|e| !e.is_empty())
        .filter_map(|event| {
            event
                .lines()
                .find_map(|line| line.strip_prefix("data: "))
                .and_then(|data| serde_json::from_str::<Value>(data).ok())
        })
        .collect()
}

async fn initialize(client: &reqwest::Client, url: &str) -> SessionId {
    let response = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": { "name": "test", "version": "1.0" }
                }
            })
            .to_string(),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let session_id: SessionId = response.headers()["mcp-session-id"]
        .to_str()
        .unwrap()
        .into();

    let status = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("mcp-session-id", session_id.to_string())
        .header("Mcp-Protocol-Version", "2025-06-18")
        .body(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}).to_string())
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, 202);
    session_id
}

async fn call_tool(
    client: &reqwest::Client,
    url: &str,
    session_id: &SessionId,
    id: i64,
    name: &str,
    arguments: Value,
) -> Value {
    let body = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("mcp-session-id", session_id.to_string())
        .header("Mcp-Protocol-Version", "2025-06-18")
        .body(
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "tools/call",
                "params": { "name": name, "arguments": arguments }
            })
            .to_string(),
        )
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    sse_data_events(&body)
        .into_iter()
        .find(|v| v.get("id") == Some(&json!(id)))
        .expect("response event for this request id")
}

#[tokio::test]
async fn initialize_advertises_tools_capability() {
    let bridge: Arc<dyn BridgeRpc> = Arc::new(FakeBridgeRpc::responding(json!({})));
    let (client, url, ct) = spawn(bridge).await;

    let response = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": { "name": "test", "version": "1.0" }
                }
            })
            .to_string(),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body = response.text().await.unwrap();
    let events = sse_data_events(&body);
    let init_result = events
        .into_iter()
        .find(|v| v.get("id") == Some(&json!(1)))
        .expect("initialize response event");
    assert!(init_result["result"]["capabilities"]["tools"].is_object());

    ct.cancel();
}

#[tokio::test]
async fn fleet_tool_call_round_trips_over_real_transport() {
    let bridge: Arc<dyn BridgeRpc> = Arc::new(FakeBridgeRpc::responding(json!({})));
    let (client, url, ct) = spawn(bridge).await;

    let session_id = initialize(&client, &url).await;
    let result = call_tool(&client, &url, &session_id, 2, "fleet", json!({})).await;

    assert_eq!(result["result"]["isError"], false);
    assert!(result["result"]["structuredContent"]["processes"].is_array());

    ct.cancel();
}

#[tokio::test]
async fn execute_python_tool_call_reaches_fake_bridge() {
    let bridge: Arc<dyn BridgeRpc> =
        Arc::new(FakeBridgeRpc::responding(json!({"ok": true, "result": 42})));
    let (client, url, ct) = spawn(bridge).await;

    let session_id = initialize(&client, &url).await;
    let result = call_tool(
        &client,
        &url,
        &session_id,
        2,
        "execute_python",
        json!({"pid": 34, "script": "result=42"}),
    )
    .await;

    assert_eq!(result["result"]["isError"], false);
    assert_eq!(result["result"]["structuredContent"]["result"], 42);

    ct.cancel();
}

#[tokio::test]
async fn script_failure_surfaces_as_tool_error_with_diagnostics() {
    let bridge: Arc<dyn BridgeRpc> = Arc::new(FakeBridgeRpc::responding(
        json!({"ok": false, "error": "NameError: 'x'"}),
    ));
    let (client, url, ct) = spawn(bridge).await;

    let session_id = initialize(&client, &url).await;
    let result = call_tool(
        &client,
        &url,
        &session_id,
        2,
        "execute_python",
        json!({"pid": 34, "script": "x"}),
    )
    .await;

    assert_eq!(result["result"]["isError"], true);
    assert_eq!(
        result["result"]["structuredContent"]["items"][0]["code"],
        "tdmcp.script.execution_failed"
    );

    ct.cancel();
}

#[tokio::test]
async fn unknown_tool_is_a_protocol_error() {
    let bridge: Arc<dyn BridgeRpc> = Arc::new(FakeBridgeRpc::responding(json!({})));
    let (client, url, ct) = spawn(bridge).await;

    let session_id = initialize(&client, &url).await;
    let result = call_tool(&client, &url, &session_id, 2, "not_a_tool", json!({})).await;

    assert!(result["error"]["code"].is_number());

    ct.cancel();
}

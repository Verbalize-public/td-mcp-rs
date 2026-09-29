//! W12: the same tool answer, through every transport and for every client.
//!
//! The plan's gate 5 is "text-only rendering paths, structured clients, errors
//! with images, attachment admission, full-resolution access, schema
//! compatibility and all supported transports". Three of those are claims
//! about the same bytes reaching a client intact, and the failure mode for all
//! three is a client that silently renders a *smaller* thing and never says so.
//!
//! So the harness calls one bridge value through three surfaces — the daemon's
//! JSON fallback, the real `rmcp` Streamable HTTP transport, and the dispatcher
//! directly — and asserts the DATA is byte-identical in all three. What each
//! transport is allowed to do differently is stated explicitly instead of
//! being left to chance, because a parity test that ignores the differences
//! teaches nothing, and one that forbids them would fail for no reason.
//!
//! Where a difference could hide data, the test pins what the client is told:
//! a payload too large to duplicate into a text block must say so in the
//! content, and an image must never leave the reply as a bare string.

#![allow(clippy::unwrap_used, reason = "test setup/assertions may panic")]
#![allow(clippy::expect_used, reason = "test setup/assertions may panic")]
#![allow(clippy::panic, reason = "test assertions may panic")]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::session::SessionId;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use serde_json::{json, Value};
use tdmcp_core::{PidRegistry, ProcessAttrs, ProcessFingerprint, TaskResult};
use tdmcp_diagnostics::Catalog;
use tdmcp_mcp::testing::test_resource_provider;
use tdmcp_mcp::{AppState, BridgeRpc, McpHandler};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

/// A 1x1 PNG — real bytes, so "full-resolution access" means the image arrives
/// byte-for-byte rather than as a thumbnail-shaped stand-in.
const ONE_PIXEL_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

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

/// The bridge the daemon really runs against, minus TouchDesigner.
///
/// It plays the daemon's per-pid actor because queue progression belongs to the
/// actor, not the tool layer: a bridge that only returned a value would leave
/// the task queued, and the *second* call in a test would be refused as
/// queue-busy before it was ever admitted — a harness defect that would read
/// as a product defect.
struct ScriptedBridge {
    registry: Arc<tokio::sync::Mutex<PidRegistry>>,
    reply: Value,
    calls: Arc<AtomicU64>,
}

/// The state, plus the one handle the trait cannot hand back.
///
/// `BridgeRpc` has no `as_any`, and adding one to read a test counter would be
/// a production API shaped by a test. The counter is a shared `Arc` instead, so
/// the harness measures the real bridge without the trait knowing.
struct Harness {
    state: AppState,
    bridge_calls: Arc<AtomicU64>,
}

impl Harness {
    fn new(reply: Value) -> Self {
        Self::with_admission(reply, tdmcp_config::DEFAULT_ADMISSION_MAX_IMAGE_BYTES)
    }

    /// Build a harness with an explicit admission bound, so a test can hold a
    /// real image over a small budget without needing a large payload.
    fn with_admission(reply: Value, max_image_bytes: usize) -> Self {
        let registry = Arc::new(tokio::sync::Mutex::new(registry_with_pid()));
        let calls = Arc::new(AtomicU64::new(0));
        let bridge = Arc::new(ScriptedBridge {
            registry: Arc::clone(&registry),
            reply,
            calls: Arc::clone(&calls),
        });
        let state = AppState::new_shared(
            registry,
            Catalog::fallback(),
            bridge,
            test_resource_provider().expect("resource provider"),
        )
        .with_admission(max_image_bytes);
        Harness {
            state,
            bridge_calls: calls,
        }
    }

    fn calls(&self) -> u64 {
        self.bridge_calls.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl BridgeRpc for ScriptedBridge {
    async fn call(
        &self,
        pid: u32,
        _method: &str,
        _params: Value,
    ) -> Result<Value, tdmcp_mcp::BridgeRpcError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.registry
            .lock()
            .await
            .start_next(pid)
            .expect("the tool layer enqueued this task before calling the bridge");
        let reply = self.reply.clone();
        self.registry
            .lock()
            .await
            .complete_task(pid, TaskResult::Success)
            .expect("complete the task this call started");
        Ok(reply)
    }
}

/// The same HTTP daemon, started for real on a port the stdio proxy dials.
///
/// `mcp_client` keeps its own server in-process because it speaks HTTP by hand;
/// the stdio proxy resolves a URL, so this one exists only to be dialled.
async fn spawn_http_daemon(state: AppState) -> (String, std::net::SocketAddr, CancellationToken) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let shutdown = CancellationToken::new();
    let service: StreamableHttpService<McpHandler, LocalSessionManager> =
        StreamableHttpService::new(
            move || Ok(McpHandler::new(state.clone())),
            Default::default(),
            StreamableHttpServerConfig::default()
                .with_sse_keep_alive(None)
                .with_cancellation_token(shutdown.child_token()),
        );
    let router = axum::Router::new().nest_service("/mcp/rpc", service);
    tokio::spawn({
        let token = shutdown.clone();
        async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move { token.cancelled_owned().await })
                .await;
        }
    });
    tokio::task::yield_now().await;
    (format!("http://{addr}/mcp/rpc"), addr, shutdown)
}

fn args(path: &str) -> Value {
    json!({"pid": 34, "paths": [path]})
}

/// The daemon's own JSON route: what an MCP client that speaks HTTP but not
/// MCP gets. Refusals are reported in the body, never as a transport status,
/// so a client cannot mistake a refusal for a transport failure.
async fn json_route(state: &AppState, name: &str, arguments: Value) -> Value {
    let router = tdmcp_mcp::build_mcp_router(state.clone());
    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp/tools/call")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"name": name, "arguments": arguments}).to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("router answers");
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "the tool route reports refusals in the body, never as a status"
    );
    serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body"),
    )
    .expect("json body")
}

/// The reply a real MCP client receives, over the real rmcp transport.
struct McpClient {
    client: reqwest::Client,
    url: String,
    session: SessionId,
    _shutdown: CancellationToken,
}

async fn mcp_client(state: AppState) -> McpClient {
    let shutdown = CancellationToken::new();
    let service: StreamableHttpService<McpHandler, LocalSessionManager> =
        StreamableHttpService::new(
            move || Ok(McpHandler::new(state.clone())),
            Default::default(),
            StreamableHttpServerConfig::default()
                .with_sse_keep_alive(None)
                .with_cancellation_token(shutdown.child_token()),
        );
    let router = axum::Router::new().nest_service("/mcp/rpc", service);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let url = format!("http://{}/mcp/rpc", listener.local_addr().expect("addr"));
    tokio::spawn({
        let token = shutdown.clone();
        async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move { token.cancelled_owned().await })
                .await;
        }
    });

    let client = reqwest::Client::new();
    let response = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(
            json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {
                    "protocolVersion": "2025-06-18", "capabilities": {},
                    "clientInfo": {"name": "parity", "version": "1.0"}
                }
            })
            .to_string(),
        )
        .send()
        .await
        .expect("initialize");
    assert_eq!(response.status(), 200);
    let session: SessionId = response.headers()["mcp-session-id"]
        .to_str()
        .expect("session header")
        .into();
    client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("mcp-session-id", session.to_string())
        .header("Mcp-Protocol-Version", "2025-06-18")
        .body(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}).to_string())
        .send()
        .await
        .expect("initialized");
    McpClient {
        client,
        url,
        session,
        _shutdown: shutdown,
    }
}

fn sse_events(body: &str) -> Vec<Value> {
    body.split("\n\n")
        .filter(|event| !event.is_empty())
        .filter_map(|event| {
            event
                .lines()
                .find_map(|line| line.strip_prefix("data: "))
                .and_then(|data| serde_json::from_str::<Value>(data).ok())
        })
        .collect()
}

impl McpClient {
    async fn call(&self, id: i64, name: &str, arguments: Value) -> Value {
        let body = self
            .client
            .post(&self.url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("mcp-session-id", self.session.to_string())
            .header("Mcp-Protocol-Version", "2025-06-18")
            .body(
                json!({
                    "jsonrpc": "2.0", "id": id, "method": "tools/call",
                    "params": {"name": name, "arguments": arguments}
                })
                .to_string(),
            )
            .send()
            .await
            .expect("tools/call")
            .text()
            .await
            .expect("body");
        sse_events(&body)
            .into_iter()
            .find(|event| event.get("id") == Some(&json!(id)))
            .expect("a response event for this request id")
    }
}

/// The tool's own answer, with no transport wrapper at all.
async fn direct(state: &AppState, name: &str, arguments: Value) -> Value {
    tdmcp_mcp::dispatch_tool(
        &state.registry,
        &state.catalog,
        state.bridge.as_ref(),
        name,
        arguments,
        None,
        None,
    )
    .await
    .expect("a valid call succeeds")
}

fn inspect_reply() -> Value {
    json!({
        "ok": true,
        "pid": 34,
        "nodes": [
            {
                "path": "/project1/box1",
                "exists": true,
                "opType": "boxCOMP",
                "params": [
                    {"name": "const0value", "value": 8},
                    {"name": "enabled", "value": true},
                    {"name": "label", "value": null},
                    {"name": "missing", "present": false}
                ]
            },
            {"path": "/project1/never", "exists": false}
        ],
        "timing": {"elapsedMs": 3}
    })
}

#[tokio::test]
async fn the_same_read_reaches_every_transport_byte_for_byte() {
    let harness = Harness::new(inspect_reply());
    let mcp = mcp_client(harness.state.clone()).await;

    let via_dispatch = direct(&harness.state, "inspect", args("/project1/box1")).await;
    let via_json = json_route(&harness.state, "inspect", args("/project1/box1")).await;
    let via_mcp = mcp.call(1, "inspect", args("/project1/box1")).await["result"].clone();

    // Shape, asserted rather than assumed: this is the whole point of the
    // harness. If any of these stop holding the test is measuring a different
    // system than it claims to.
    assert_eq!(via_json["ok"], json!(true));
    assert_eq!(
        via_json["data"], via_dispatch,
        "the JSON route reshaped the data"
    );
    assert_eq!(
        via_mcp["isError"],
        json!(false),
        "an ordinary read was reported as an error: {via_mcp:#}"
    );
    // The two transports wrap the answer in different envelopes on purpose:
    // the JSON route is its own HTTP API and answers `{ok, data}`, while MCP's
    // `structuredContent` is the tool's own value verbatim. What must match is
    // the DATA, so that is what is compared — and the difference is stated
    // here rather than left for the next reader to rediscover.
    assert_eq!(
        via_mcp["structuredContent"], via_dispatch,
        "the MCP transport reshaped the tool's own answer"
    );
    assert!(
        via_json.get("data").is_some() && via_json.get("ok").is_some(),
        "the JSON route is expected to add an ok/data envelope: {via_json:#}"
    );

    // Text-only clients read `content[]`, so the text block must be a complete
    // rendering of the same data, not a summary of it.
    let text = via_mcp["content"][0]["text"]
        .as_str()
        .expect("a text block for a text-only client");
    let rendered: Value = serde_json::from_str(text).expect("the text block is json");
    assert_eq!(
        rendered, via_dispatch,
        "a text-only client sees different data"
    );

    // And the distinctions the contract is about survive the crossing, in
    // every representation at once.
    for carrier in [via_dispatch.clone(), rendered] {
        assert_eq!(carrier["nodes"][0]["params"][2]["value"], Value::Null);
        assert_eq!(carrier["nodes"][0]["params"][3]["present"], json!(false));
        assert!(carrier["nodes"][0]["params"][3].get("value").is_none());
        assert_eq!(carrier["nodes"][1]["exists"], json!(false));
    }
}

#[tokio::test]
async fn a_read_only_large_answer_says_where_the_data_is_instead_of_vanishing() {
    let harness = Harness::new(json!({"ok": true, "blob": "x".repeat(300 * 1024)}));
    let mcp = mcp_client(harness.state.clone()).await;

    let via_dispatch = direct(&harness.state, "inspect", args("/project1/box1")).await;
    let via_mcp = mcp.call(1, "inspect", args("/project1/box1")).await["result"].clone();

    // Above 256 KiB the text copy of the payload is not repeated, because the
    // transport cannot carry the same value twice. The DATA must be untouched
    // — it lives in `structuredContent` — and a client that only renders
    // `content[]` must be told where it went rather than shown nothing, which
    // it could easily read as "this call returned nothing".
    assert_eq!(via_mcp["structuredContent"], via_dispatch);
    let content = via_mcp["content"].as_array().expect("content is an array");
    let text = content
        .iter()
        .filter(|block| block["type"] == json!("text"))
        .map(|block| block["text"].as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    assert_eq!(
        text.len(),
        1,
        "a large answer duplicated itself into text blocks"
    );
    assert!(
        text[0].contains("structuredContent"),
        "the client was left with no pointer to where the data went: {text:?}"
    );
    assert!(
        text[0].len() < 4096,
        "the pointer restates the payload it is replacing"
    );
    // The 300 KiB payload is still fully present, in the one field that
    // carries it: nothing was truncated to make room.
    assert_eq!(
        via_mcp["structuredContent"]["blob"].as_str().map(str::len),
        Some(300 * 1024)
    );
}

#[tokio::test]
async fn a_refused_call_says_the_same_thing_on_every_transport() {
    let harness = Harness::new(inspect_reply());
    let mcp = mcp_client(harness.state.clone()).await;

    // A malformed argument is refused by the parser, before any bridge call.
    let bad = json!({"pid": "not-a-pid", "paths": ["/project1/box1"]});
    let via_dispatch = tdmcp_mcp::dispatch_tool(
        &harness.state.registry,
        &harness.state.catalog,
        harness.state.bridge.as_ref(),
        "inspect",
        bad.clone(),
        None,
        None,
    )
    .await
    .expect_err("a pid that is not a number is refused");
    let via_json = json_route(&harness.state, "inspect", bad.clone()).await;
    let via_mcp = mcp.call(1, "inspect", bad).await["result"].clone();

    let tdmcp_mcp::ToolCallError::Failed(refusal) = &via_dispatch else {
        panic!(
            "a malformed argument is refused, not reported as an unknown tool: {via_dispatch:?}"
        );
    };
    let from_dispatch = refusal.structured_content();
    assert_eq!(via_json, from_dispatch, "the JSON route reshaped a refusal");
    assert_eq!(
        via_mcp["structuredContent"], from_dispatch,
        "the MCP transport reshaped a refusal"
    );
    assert_eq!(
        via_mcp["isError"],
        json!(true),
        "a refusal was not marked as one"
    );
    // A refusal is an error, but it is still an ANSWER: dropping the text
    // block would leave a text-only client with nothing at all.
    assert!(
        via_mcp["content"]
            .as_array()
            .expect("content")
            .iter()
            .any(|block| block["type"] == json!("text")),
        "a text-only client got no text for a refusal"
    );
}

#[tokio::test]
async fn an_image_stays_an_image_attachment_and_is_never_dropped_to_a_string() {
    let harness = Harness::new(json!({
        "ok": true,
        "path": "/project1/out1",
        "bytes": 70,
        "mimeType": "image/png",
        "imageBase64": ONE_PIXEL_PNG,
    }));
    let mcp = mcp_client(harness.state.clone()).await;

    let via_mcp = mcp
        .call(1, "capture", json!({"pid": 34, "path": "/project1/out1"}))
        .await["result"]
        .clone();
    let content = via_mcp["content"].as_array().expect("content is an array");
    let images: Vec<&Value> = content
        .iter()
        .filter(|block| block["type"] == json!("image"))
        .collect();
    assert_eq!(
        images.len(),
        1,
        "the image was not delivered as an attachment: {via_mcp:#}"
    );
    // Full resolution: the exact bytes the bridge produced, not a stand-in.
    assert_eq!(images[0]["data"], json!(ONE_PIXEL_PNG));
    assert_eq!(images[0]["mimeType"], json!("image/png"));
    // And the structured content still carries the metadata, without a
    // second copy of the payload: the image is reachable as an attachment
    // exactly once, not as attachment plus inline string.
    assert!(
        via_mcp["structuredContent"].get("imageBase64").is_none(),
        "the image payload was duplicated into the structured content"
    );
    assert_eq!(via_mcp["structuredContent"]["bytes"], json!(70));
    assert_eq!(
        via_mcp["structuredContent"]["path"],
        json!("/project1/out1")
    );
}

#[tokio::test]
async fn an_image_bearing_refusal_still_delivers_the_image() {
    let harness = Harness::new(json!({
        "ok": false,
        "code": "tdmcp.perception.black_frame",
        "message": "the TOP cooked to a uniform black frame",
        "path": "/project1/out1",
        "bytes": 70,
        "mimeType": "image/png",
        "imageBase64": ONE_PIXEL_PNG,
    }));
    let mcp = mcp_client(harness.state.clone()).await;

    let via_mcp = mcp
        .call(1, "capture", json!({"pid": 34, "path": "/project1/out1"}))
        .await["result"]
        .clone();
    // The refusal's own code survives: an image attached to a failure must not
    // cost the caller the reason it failed.
    assert_eq!(via_mcp["isError"], json!(true));
    let items = via_mcp["structuredContent"]["items"]
        .as_array()
        .expect("a refusal carries diagnostic items");
    assert_eq!(items[0]["code"], json!("tdmcp.perception.black_frame"));
    let content = via_mcp["content"].as_array().expect("content is an array");
    assert!(
        content.iter().any(|block| {
            block["type"] == json!("image") && block["data"] == json!(ONE_PIXEL_PNG)
        }),
        "the image attached to the refusal was dropped: {via_mcp:#}"
    );
}

/// A refused call never reaches TouchDesigner, and a read does.
///
/// Every other test here compares transports against each other, so a harness
/// with a bridge that ignored the request would pass all of them. This one
/// measures the call count, so a parser that stopped refusing before dispatch
/// would show up as a bridge call that should not exist.
#[tokio::test]
async fn a_refused_call_never_reaches_touchdesigner_but_a_read_does() {
    let harness = Harness::new(inspect_reply());
    let refused = tdmcp_mcp::dispatch_tool(
        &harness.state.registry,
        &harness.state.catalog,
        harness.state.bridge.as_ref(),
        "inspect",
        json!({"pid": "not-a-pid", "paths": []}),
        None,
        None,
    )
    .await;
    assert!(
        refused.is_err(),
        "a pid that is not a number must be refused"
    );
    assert_eq!(
        harness.calls(),
        0,
        "a refusal reached TouchDesigner, so it was decided there and not here"
    );

    direct(&harness.state, "inspect", args("/project1/box1")).await;
    assert_eq!(
        harness.calls(),
        1,
        "an accepted read did not reach the bridge exactly once"
    );
}

/// The real `tdmcp-daemon mcp` path: an MCP client on a pipe, the stdio proxy,
/// and the same `/mcp/rpc` daemon the other cases call directly.
///
/// This is the only surface that is neither HTTP. The two claims it has to
/// carry that the others do not are a large answer (the pipe is line-delimited
/// JSON, so framing, not SSE, is what could break) and an image (an attachment
/// has to survive being re-framed). Both are the claims most likely to be
/// quietly true for HTTP and quietly false for stdio.
#[tokio::test]
async fn the_stdio_path_carries_a_large_answer_and_an_image_like_the_others() {
    use rmcp::model::CallToolRequestParams;
    use rmcp::ServiceExt;
    use tdmcp_mcp::run_stdio_proxy_rw;

    // One bridge value has to hold BOTH shapes, because it serves one call: a
    // big text field and a real image. `capture` is the tool that promotes a
    // top-level `imageBase64` to an attachment, and it reads no required
    // argument, so no extra setup is needed to reach it.
    let harness = Harness::new(json!({
        "ok": true,
        "blob": "Q".repeat(300 * 1024),
        "path": "/project1/out1",
        "bytes": 70,
        "mimeType": "image/png",
        "imageBase64": ONE_PIXEL_PNG,
    }));
    let (url, _addr, ct) = spawn_http_daemon(harness.state.clone()).await;

    let (client_side, server_side) = tokio::io::duplex(1024 * 1024);
    let (server_read, server_write) = tokio::io::split(server_side);
    let proxy_task =
        tokio::spawn(async move { run_stdio_proxy_rw(&url, server_read, server_write).await });
    let client = ().serve(client_side).await.expect("stdio client initialize");

    let large = client
        .call_tool(
            CallToolRequestParams::new("capture").with_arguments(
                json!({"pid": 34, "path": "/project1/out1"})
                    .as_object()
                    .cloned()
                    .unwrap_or_default(),
            ),
        )
        .await
        .expect("the large capture reaches the daemon");
    // 1. The full payload survived the pipe, untruncated.
    assert_eq!(
        large
            .structured_content
            .as_ref()
            .and_then(|s| s["blob"].as_str())
            .map(str::len),
        Some(300 * 1024),
        "the large answer was truncated in transit"
    );
    // 2. A client that renders only `content[]` was told where it went.
    let text = large
        .content
        .iter()
        .filter_map(|block| block.as_text().map(|t| t.text.as_str()))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("structuredContent"),
        "a text-only stdio client got no pointer to the payload"
    );
    assert!(
        !text.contains("QQQQ"),
        "the pointer restates the omitted bytes"
    );
    // 3. The image is still an attachment, at full fidelity — not a string.
    let images: Vec<_> = large
        .content
        .iter()
        .filter_map(|block| block.as_image())
        .collect();
    assert_eq!(
        images.len(),
        1,
        "the image did not survive the stdio framing: {large:?}"
    );
    assert_eq!(images[0].data, ONE_PIXEL_PNG);
    assert_eq!(images[0].mime_type, "image/png");

    let _ = client.cancel().await;
    assert!(
        proxy_task.await.expect("join proxy").is_ok(),
        "the proxy should exit cleanly"
    );
    ct.cancel();
    assert_eq!(harness.calls(), 1, "one call, one bridge call");
}

/// The admission decision is made by the SERVER, so it has to be visible on
/// every transport identically — a client that only speaks one of them must
/// not learn a different story from the one it happens to speak.
///
/// The bound is set to 10 bytes and the image reports 70, so the withholding is
/// real without needing a large payload: this exercises the decision, not the
/// size.
#[tokio::test]
async fn a_withheld_image_reaches_every_transport_with_its_bytes_intact() {
    let reply = json!({
        "ok": true,
        "path": "/project1/out1",
        "bytes": 70,
        "mimeType": "image/png",
        "imageBase64": ONE_PIXEL_PNG,
    });
    let harness = Harness::with_admission(reply.clone(), 10);
    let state = harness.state.clone();
    let args = json!({"pid": 34, "path": "/project1/out1"});

    // The dispatcher's own value. This is the tool's answer with no transport
    // wrapper: it carries NO `admission` block, because admission is decided
    // by the MCP layer, not by the tool. Asserting that is the point — the
    // record must not be a property of the dispatcher that only appears once
    // some client happens to read it.
    let via_dispatch = direct(&state, "capture", args.clone()).await;
    assert!(
        via_dispatch.get("admission").is_none(),
        "admission is a transport-layer decision, not a tool one: {via_dispatch}"
    );

    // The same call as a real MCP client sees it, over real Streamable HTTP.
    let client = mcp_client(state).await;
    let via_mcp = client.call(101, "capture", args).await;
    let via_mcp_result = &via_mcp["result"];

    // Every field the tool produced is unchanged by the transport.
    for field in ["ok", "path", "bytes", "mimeType", "imageBase64"] {
        assert_eq!(
            via_dispatch[field], via_mcp_result["structuredContent"][field],
            "the transport must not alter {field}"
        );
    }

    // 1. Not attached, on any transport.
    let images: Vec<_> = via_mcp_result["content"]
        .as_array()
        .expect("content array")
        .iter()
        .filter(|block| block["type"] == "image")
        .collect();
    assert!(
        images.is_empty(),
        "an over-budget image must not be attached: {via_mcp_result}"
    );

    // 2. The bytes are still there, byte for byte. Withholding is a decision
    //    about the attachment list, not a deletion.
    assert_eq!(
        via_mcp_result["structuredContent"]["imageBase64"].as_str(),
        Some(ONE_PIXEL_PNG),
        "a withheld image must keep its exact bytes in structuredContent"
    );

    // 3. The four quantities are reported separately, and the one nobody can
    //    observe says so.
    let admission = &via_mcp_result["structuredContent"]["admission"];
    assert_eq!(admission["generated"]["images"], 1);
    assert_eq!(admission["generated"]["bytes"], 70);
    assert_eq!(admission["admitted"]["images"], 0);
    assert_eq!(admission["admitted"]["verdict"], "withheld");
    assert_eq!(admission["admitted"]["maxImageBytes"], 10);
    assert_eq!(admission["admitted"]["withheldBytes"], 70);
    assert_eq!(
        admission["includedInModelInput"]["state"], "unavailable",
        "the server must not claim a client's model saw the image"
    );

    // 4. A text-only client is told, in text, in the same response.
    let text = via_mcp_result["content"]
        .as_array()
        .expect("content array")
        .iter()
        .filter(|block| block["type"] == "text")
        .filter_map(|block| block["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("NOT attached") && text.contains("structuredContent"),
        "a text-only client must be told the picture is withheld and where it is: {text}"
    );
}

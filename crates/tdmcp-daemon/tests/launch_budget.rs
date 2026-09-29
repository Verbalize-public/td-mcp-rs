//! W2 B3 live acceptance: a launch that outlives the proxy's deadline.
//!
//! The unit tests pin the budget arithmetic and the error text. What they
//! cannot reach is the thing B3 is actually about — a real proxy, forwarding
//! over a real JSON-RPC transport, meeting a daemon that does not answer in
//! time. That is what this drives.
//!
//! No TouchDesigner is needed, and deliberately so: `spawn_td` is a LOCAL tool
//! (`ToolName::SpawnTd` dispatches into `crate::lifecycle::spawn_td` and never
//! crosses the bridge), so what B3 fixes is entirely on the proxy side. The
//! daemon-side ownership of the child is covered by `lifecycle_reaper_tests`,
//! which spawns real child processes.
//!
//! Scope note: only the LAUNCH cases live here. The converse — that a slow
//! non-launch call still gets the stalled-link diagnosis and still heals — is
//! already covered by `stdio_proxy_call_timeout_heals_and_returns_budget_error`
//! in `stdio_proxy.rs`, which drives the real Streamable-HTTP transport and
//! asserts `DAEMON_UNREACHABLE` with `healed: true`. That test had to run on
//! the real transport rather than a stub, because the heal re-dials the daemon
//! and a hand-rolled JSON-RPC endpoint does not serve that. Rebuilding it here
//! would have been a worse copy of a test that already passes.

#![allow(clippy::unwrap_used, reason = "test setup/assertions may panic")]
#![allow(clippy::expect_used, reason = "test setup/assertions may panic")]
#![allow(clippy::panic, reason = "test assertions may panic")]

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use tdmcp_diagnostics::codes;
use tdmcp_mcp::{run_stdio_proxy_rw_config, ReconnectConfig};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// How long the stub daemon takes to answer any `tools/call`.
const STALL: Duration = Duration::from_secs(30);

/// A minimal JSON-RPC endpoint: enough to initialize and to accept one call —
/// and the call takes [`STALL`], which is the shape of a launch still coming up.
async fn spawn_stalling_daemon() -> (String, String, Arc<AtomicU64>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr: SocketAddr = listener.local_addr().expect("local addr");

    let calls = Arc::new(AtomicU64::new(0));
    let counted = calls.clone();
    let router = Router::new()
        .route("/mcp/health", get(|| async { Json(json!({"ok": true})) }))
        .route(
            "/mcp/rpc",
            post(move |body: String| {
                let counted = counted.clone();
                async move {
                    let request: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
                    let Some(id) = request.get("id").cloned() else {
                        // A notification expects no body.
                        return Json(Value::Null);
                    };
                    let method = request
                        .get("method")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    let result = match method.as_str() {
                        "initialize" => json!({
                            "protocolVersion": "2025-06-18",
                            "capabilities": {"tools": {}},
                            "serverInfo": {"name": "launch-budget-stub", "version": "1"},
                        }),
                        "tools/call" => {
                            // The launch is still coming up. Counted, so a
                            // re-sent launch shows up as a second call rather
                            // than only a slower answer.
                            counted.fetch_add(1, Ordering::SeqCst);
                            tokio::time::sleep(STALL).await;
                            json!({"content": [{"type": "text", "text": "{}"}], "isError": false})
                        }
                        _ => json!({}),
                    };
                    Json(json!({"jsonrpc": "2.0", "id": id, "result": result}))
                }
            }),
        );

    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    tokio::task::yield_now().await;
    (
        format!("http://{addr}/mcp/rpc"),
        format!("http://{addr}"),
        calls,
    )
}

fn launch_config(margin: Duration, max: Duration) -> ReconnectConfig {
    ReconnectConfig {
        recent: Duration::from_millis(3_000),
        stale: Duration::from_secs(15),
        debounce: Duration::from_millis(50),
        probe_interval: Duration::from_millis(100),
        probe_max: Duration::from_millis(500),
        launch_margin: margin,
        launch_max: max,
        ..Default::default()
    }
}

/// Drive one call through a real proxy and return the JSON-RPC response.
async fn call_through_proxy(
    url: &str,
    cfg: ReconnectConfig,
    tool: &str,
    arguments: Value,
) -> Value {
    let (client_side, server_side) = tokio::io::duplex(64 * 1024);
    let (server_read, server_write) = tokio::io::split(server_side);
    let proxy_url = url.to_owned();
    let proxy = tokio::spawn(async move {
        run_stdio_proxy_rw_config(&proxy_url, server_read, server_write, cfg).await
    });
    let (read, mut write) = tokio::io::split(client_side);
    let mut read = BufReader::new(read);
    let mut line = String::new();

    let init = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "launch-budget-test", "version": "1.0"}
        }
    });
    write
        .write_all(format!("{init}\n").as_bytes())
        .await
        .unwrap();
    line.clear();
    tokio::time::timeout(Duration::from_secs(10), read.read_line(&mut line))
        .await
        .expect("initialize timeout")
        .unwrap();

    write
        .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n")
        .await
        .unwrap();

    let call = json!({
        "jsonrpc": "2.0", "id": 7, "method": "tools/call",
        "params": {"name": tool, "arguments": arguments}
    });
    write
        .write_all(format!("{call}\n").as_bytes())
        .await
        .unwrap();
    line.clear();
    tokio::time::timeout(Duration::from_secs(20), read.read_line(&mut line))
        .await
        .expect("call response timeout")
        .unwrap();
    let response: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
    proxy.abort();
    response
}

/// Initialize a proxy, issue one launch, then DROP the client end without
/// reading the answer — a caller that goes away mid-launch.
async fn disconnect_during_launch(url: &str, cfg: ReconnectConfig) {
    let (client_side, server_side) = tokio::io::duplex(64 * 1024);
    let (server_read, server_write) = tokio::io::split(server_side);
    let proxy_url = url.to_owned();
    let proxy = tokio::spawn(async move {
        run_stdio_proxy_rw_config(&proxy_url, server_read, server_write, cfg).await
    });
    let (read, mut write) = tokio::io::split(client_side);
    let mut read = BufReader::new(read);
    let mut line = String::new();

    let init = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "disconnect-test", "version": "1.0"}
        }
    });
    write
        .write_all(format!("{init}\n").as_bytes())
        .await
        .unwrap();
    line.clear();
    tokio::time::timeout(Duration::from_secs(10), read.read_line(&mut line))
        .await
        .expect("initialize timeout")
        .unwrap();
    write
        .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n")
        .await
        .unwrap();
    let call = json!({
        "jsonrpc": "2.0", "id": 9, "method": "tools/call",
        "params": {"name": "spawn_td", "arguments": {
            "exePath": "/opt/td/TouchDesigner", "waitTimeoutMs": 100
        }}
    });
    write
        .write_all(format!("{call}\n").as_bytes())
        .await
        .unwrap();
    // No flush-and-read: the caller simply vanishes mid-flight.
    drop(write);
    drop(read);
    drop(proxy);
}

#[tokio::test]
async fn a_launch_that_outlives_the_proxy_deadline_is_reported_as_a_launch_timeout() {
    // 100ms requested + 50ms head-room = a 150ms ceiling, against a 30s stall.
    // Under the old behaviour this was cut at the 105s read-class ceiling and
    // reported as a stalled link.
    let cfg = launch_config(Duration::from_millis(50), Duration::from_millis(200));
    let (url, _admin, _calls) = spawn_stalling_daemon().await;
    let response = call_through_proxy(
        &url,
        cfg,
        "spawn_td",
        json!({"exePath": "/opt/td/TouchDesigner", "waitTimeoutMs": 100}),
    )
    .await;

    let error = response
        .get("error")
        .unwrap_or_else(|| panic!("expected a launch error, got {response}"));
    let data = &error["data"];
    assert_eq!(
        data["code"],
        codes::SPAWN_WAIT_TIMEOUT,
        "a slow launch must not be reported as a link fault: {error}"
    );
    // The two failures have to stay distinguishable, or a caller cannot tell
    // "still starting" from "connection is wedged".
    assert_ne!(data["code"], codes::DAEMON_UNREACHABLE, "{error}");
    // The budget really did follow the caller's own request plus head-room.
    assert_eq!(data["requestedWaitMs"], 100, "{error}");
    assert_eq!(data["budgetMs"], 150, "{error}");
    // Nothing was healed, and nothing is claimed to have been.
    assert_eq!(data["linkHealed"], false, "{error}");
    assert_eq!(data["effect"], "unknown", "{error}");
    let suggestion = data["suggestion"].as_str().unwrap_or_default();
    assert!(
        suggestion.contains("do NOT spawn again"),
        "the double-spawn guard must be in the message a caller acts on: {suggestion}"
    );
}

#[tokio::test]
async fn a_cut_launch_leaves_the_link_healthy() {
    // The other half of the diagnosis fix. `forward_bounded` heals on a budget
    // overrun, and `heal` calls `mark_unhealthy`, so a slow launch used to
    // actively degrade a connection that was working fine.
    let cfg = launch_config(Duration::from_millis(50), Duration::from_millis(200));
    let (url, admin, _calls) = spawn_stalling_daemon().await;
    call_through_proxy(
        &url,
        cfg,
        "spawn_td",
        json!({"exePath": "/opt/td/TouchDesigner", "waitTimeoutMs": 100}),
    )
    .await;

    // The daemon is still there and still answering, which is the whole point:
    // nothing was wrong with the link.
    let client = reqwest::Client::new();
    let health = client
        .get(format!("{admin}/mcp/health"))
        .send()
        .await
        .expect("health reachable after a cut launch");
    assert!(health.status().is_success(), "{:?}", health.status());
}

#[tokio::test]
async fn a_requested_wait_the_proxy_would_have_ignored_now_survives() {
    // The mismatch B3 exists to fix, proven end to end: asking for more wait
    // than the read-class ceiling used to be possible ONLY because the ceiling
    // was derived from the request. Here the request asks for 200ms and the
    // configured launch_max is 200ms, so the call is cut at the max rather than
    // at some unrelated class ceiling.
    let cfg = launch_config(Duration::from_millis(50), Duration::from_millis(200));
    let (url, _admin, _calls) = spawn_stalling_daemon().await;
    let response = call_through_proxy(
        &url,
        cfg,
        "spawn_td",
        json!({"exePath": "/opt/td/TouchDesigner", "waitTimeoutMs": 200}),
    )
    .await;
    let data = &response["error"]["data"];
    assert_eq!(data["requestedWaitMs"], 200, "{response}");
    // 200 requested + 50 head-room, capped at launch_max=200.
    assert_eq!(data["budgetMs"], 200, "{response}");
}

#[tokio::test]
async fn a_late_success_is_still_reported_and_the_launch_is_not_re_sent() {
    // "Late success": the daemon answers AFTER the proxy has given up. The
    // launch was real, so the answer arriving late must not change what the
    // caller was told — and, critically, must not trigger a second launch.
    //
    // The counter makes that measurable: exactly one tools/call, ever.
    let cfg = launch_config(Duration::from_millis(50), Duration::from_millis(200));
    let (url, _admin, calls) = spawn_stalling_daemon().await;
    let response = call_through_proxy(
        &url,
        cfg,
        "spawn_td",
        json!({"exePath": "/opt/td/TouchDesigner", "waitTimeoutMs": 100}),
    )
    .await;
    assert_eq!(
        response["error"]["data"]["code"],
        codes::SPAWN_WAIT_TIMEOUT,
        "a late answer must not rewrite a timeout into a success: {response}"
    );

    // The stub answers at 30s, long after the 150ms proxy deadline. Wait past
    // it and confirm the proxy did not treat the late arrival as a reason to
    // send the launch again.
    tokio::time::sleep(Duration::from_secs(32)).await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "exactly one launch may ever be dispatched; a retry here is the double-spawn"
    );
}

#[tokio::test]
async fn concurrent_launches_are_budgeted_independently() {
    // Concurrency: several launches in flight at once must not share one
    // budget, and must not be collapsed into a single dispatch. Each caller
    // gets its own deadline and its own call.
    let cfg = launch_config(Duration::from_millis(50), Duration::from_millis(200));
    let (url, _admin, calls) = spawn_stalling_daemon().await;
    let cfg = Arc::new(cfg);

    let mut handles = Vec::new();
    for index in 0..3 {
        let url = url.clone();
        let cfg = cfg.clone();
        handles.push(tokio::spawn(async move {
            call_through_proxy(
                &url,
                (*cfg).clone(),
                "spawn_td",
                json!({"exePath": "/opt/td/TouchDesigner", "waitTimeoutMs": 100 + index}),
            )
            .await
        }));
    }
    let responses = futures::future::join_all(handles).await;

    for (index, joined) in responses.iter().enumerate() {
        let response = joined.as_ref().expect("task joined").clone();
        let data = &response["error"]["data"];
        assert_eq!(
            data["code"],
            codes::SPAWN_WAIT_TIMEOUT,
            "concurrent launch {index}: {response}"
        );
        // Each caller sees ITS OWN requested wait echoed back, which is what
        // proves the budgets were not shared.
        assert_eq!(
            data["requestedWaitMs"],
            100 + index as u64,
            "concurrent launch {index} saw another caller's budget: {response}"
        );
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        3,
        "three launches in, three dispatches out — never fewer, never more"
    );
}

#[tokio::test]
async fn a_caller_that_disconnects_mid_launch_leaves_the_launch_alone() {
    // Disconnect is the one case that must NOT produce a repair. The caller is
    // gone, so there is nobody to answer; the launch is still owned by the
    // daemon and may yet handshake. Healing or re-sending here would be acting
    // on a vanished client — and re-sending is the double-spawn.
    let cfg = launch_config(Duration::from_millis(50), Duration::from_millis(200));
    let (url, _admin, calls) = spawn_stalling_daemon().await;
    disconnect_during_launch(&url, cfg).await;

    // Let the proxy's deadline pass and the stub's own answer arrive after it.
    tokio::time::sleep(Duration::from_secs(32)).await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "a disconnected caller must not cause a second launch"
    );
}

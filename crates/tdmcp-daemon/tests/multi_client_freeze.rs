//! Multi-client MCP session freeze repro.
//!
//! Spawns a real `tdmcp-daemon` binary (no GUI) on a test port with fake TD
//! peers over the real named pipe, then drives it with several concurrent
//! rmcp Streamable HTTP client sessions firing bursts of tool calls.
//!
//! A dedicated probe session calls `fleet` on a strict deadline throughout the
//! storm; any probe that exceeds its budget signals a wedged session/daemon.
//! After the storm a **fresh** session probes `fleet` to distinguish a
//! per-session wedge from a daemon-global freeze.
//!
//! Run:
//! ```text
//! cargo test -p tdmcp-daemon --test multi_client_freeze -- --nocapture --test-threads=1
//! ```

#![cfg(windows)] // named-pipe transport is Windows-only
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test harness"
)]

use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::future::join_all;
use rmcp::model::{CallToolRequestParams, ClientCapabilities, ClientInfo, Implementation};
use rmcp::service::{Peer, ServiceExt};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::RoleClient;
use serde_json::json;
use tdmcp_ipc::{encode, HandshakeRequest, Message};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio::time::timeout;

const NUM_CLIENTS: usize = 8;
const BURST_PER_CLIENT: usize = 16;
const STORM_SECS: u64 = 15;
const PROBE_INTERVAL: Duration = Duration::from_millis(200);
const PROBE_BUDGET: Duration = Duration::from_secs(3);
const CALL_BUDGET: Duration = Duration::from_secs(10);
const PEER_DELAY: Duration = Duration::from_millis(250);
const SLOW_CALL_DELAY: Duration = Duration::from_millis(1500);

/// Shared stats: max probe latency + how many probes missed the budget.
#[derive(Default)]
struct Stats {
    probes: Mutex<Vec<Duration>>,
    missed: Mutex<usize>,
    health_missed: Mutex<usize>,
    call_errs: Mutex<usize>,
    /// Raw TCP connect failures to the daemon port (accept loop / process dead).
    connect_failures: Mutex<usize>,
    /// Times the daemon child process was observed exited.
    process_exits: Mutex<usize>,
}

// ---------------------------------------------------------------------------
// Daemon child
// ---------------------------------------------------------------------------

struct TestDaemon {
    child: Child,
    port: u16,
    pipe: String,
    log_path: std::path::PathBuf,
    _data_dir: tempfile::TempDir,
}

impl Drop for TestDaemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl TestDaemon {
    /// Print the daemon log tail (stderr).
    fn print_log_tail(&self, lines: usize) {
        if let Ok(content) = std::fs::read_to_string(&self.log_path) {
            let tail: Vec<&str> = content.lines().rev().take(lines).collect();
            println!("===== daemon log tail (last {lines}) =====");
            for line in tail.iter().rev() {
                println!("{line}");
            }
            println!("===== end log =====");
        }
    }
}

fn pick_free_port() -> u16 {
    if let Ok(p) = std::env::var("TDMCP_FREEZE_PORT") {
        if let Ok(port) = p.trim().parse::<u16>() {
            return port;
        }
    }
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("probe port")
        .local_addr()
        .expect("local addr")
        .port()
}

fn spawn_daemon() -> TestDaemon {
    let data_dir = tempfile::tempdir().expect("temp dir");
    let port = pick_free_port();
    let pipe = format!(r"\\.\pipe\tdmcp-frz-{}-{}", std::process::id(), port);
    let exe = env!("CARGO_BIN_EXE_tdmcp-daemon");

    // Persistent log under target/ so failures are inspectable after the run.
    let log_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("tmp");
    std::fs::create_dir_all(&log_root).expect("create target/tmp");
    let log_path = log_root.join(format!("freeze-daemon-{port}.log"));
    let log_file = std::fs::File::create(&log_path).expect("create log");
    let stderr = log_file.try_clone().expect("clone log");

    let child = Command::new(exe)
        .arg("start")
        .arg("--port")
        .arg(port.to_string())
        .arg("--data-dir")
        .arg(data_dir.path())
        .arg("--no-gui")
        .env("TDMCP_IPC_PIPE", &pipe)
        .env("TDMCP_IDLE_EXIT_SECS", "0")
        .env("TDMCP_TRACE_ACCEPT", "1")
        .env(
            "RUST_LOG",
            "warn,tdmcp_daemon=info,tdmcp_mcp=info,tdmcp_ipc=info",
        )
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr))
        .spawn()
        .expect("spawn daemon");

    TestDaemon {
        child,
        port,
        pipe,
        log_path,
        _data_dir: data_dir,
    }
}

async fn wait_health(daemon: &mut TestDaemon, budget: Duration) {
    let url = format!("http://127.0.0.1:{}/mcp/health", daemon.port);
    let client = reqwest::Client::new();
    let deadline = Instant::now() + budget;
    loop {
        // Fail fast if the daemon process died.
        if let Ok(Some(status)) = daemon.child.try_wait() {
            daemon.print_log_tail(60);
            panic!("daemon exited early with {status}");
        }
        if client.get(&url).send().await.is_ok() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "daemon never became healthy on port {}",
            daemon.port
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

// ---------------------------------------------------------------------------
// Fake TD peer over the real named pipe
// ---------------------------------------------------------------------------

async fn read_frame(pipe: &mut NamedPipeClient) -> Option<Message> {
    let mut len_buf = [0u8; 4];
    if pipe.read_exact(&mut len_buf).await.is_err() {
        return None;
    }
    let len = u32::from_le_bytes(len_buf) as usize;
    let mut body = vec![0u8; len];
    if pipe.read_exact(&mut body).await.is_err() {
        return None;
    }
    serde_json::from_slice(&body).ok()
}

async fn write_frame(pipe: &mut NamedPipeClient, msg: &Message) -> bool {
    match encode(msg) {
        Ok(bytes) => pipe.write_all(&bytes).await.is_ok(),
        Err(_) => false,
    }
}

/// Connect to the daemon's IPC pipe, handshake, then answer every request.
/// `inspect` responses are delayed by `delay`; `execute_python` by
/// `slow_call_delay` (keeps bridge calls in flight, closer to real TD load).
async fn fake_td_peer(
    pipe: String,
    pid: u32,
    delay: Duration,
    slow_call_delay: Duration,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut client = loop {
            match ClientOptions::new().open(&pipe) {
                Ok(c) => break c,
                Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        };
        let req = HandshakeRequest {
            pid,
            protocol_version: "1".into(),
            title: Some(format!("fake-{pid}.toe")),
            toe_path: None,
            image: Some("TouchDesigner.exe".into()),
            start_time: Some("t0".into()),
        };
        let bytes = encode(&req).expect("encode handshake");
        if client.write_all(&bytes).await.is_err() {
            return;
        }
        // Read handshake response.
        let mut len_buf = [0u8; 4];
        if client.read_exact(&mut len_buf).await.is_err() {
            return;
        }
        let len = u32::from_le_bytes(len_buf) as usize;
        let mut body = vec![0u8; len];
        if client.read_exact(&mut body).await.is_err() {
            return;
        }
        while let Some(msg) = read_frame(&mut client).await {
            let Message::Request { id, method, .. } = msg else {
                continue;
            };
            let wait = match method.as_str() {
                "execute_python" => slow_call_delay,
                _ => delay,
            };
            if !wait.is_zero() {
                tokio::time::sleep(wait).await;
            }
            let result = match method.as_str() {
                "ping" => json!({"ok": true, "pong": true}),
                "execute_python" => json!({"ok": true, "result": 1, "logs": []}),
                "capture" => json!({"ok": true, "bytes": 1024, "path": "/project1/out1",
                                    "mimeType": "image/png", "imageBase64": "iVBORw0KGgo="}),
                "inspect" => json!({"ok": true, "nodes": [{"ok": true, "path": "/project1"}]}),
                _ => json!({"ok": true}),
            };
            let resp = Message::Response {
                id,
                result: Some(result),
                error: None,
            };
            if !write_frame(&mut client, &resp).await {
                break;
            }
        }
    })
}

// ---------------------------------------------------------------------------
// MCP client sessions
// ---------------------------------------------------------------------------

type ClientSession = (
    Arc<Peer<RoleClient>>,
    rmcp::service::RunningService<RoleClient, ClientInfo>,
);

/// Connect with the same pooled HTTP client the production stdio proxy uses
/// (`tdmcp_mcp::daemon_link::connect_http`): rmcp's `from_uri` default
/// disables connection reuse (`pool_max_idle_per_host(0)`), which burns one
/// TCP connection per tool call and exhausts the Windows ephemeral port range
/// under load. A bounded idle pool is the fix under test.
async fn connect_client(daemon_url: &str, name: &str) -> Result<ClientSession, String> {
    let http = reqwest::Client::builder()
        .pool_max_idle_per_host(8)
        .pool_idle_timeout(Duration::from_secs(60))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| e.to_string())?;
    let transport = StreamableHttpClientTransport::with_client(
        http,
        StreamableHttpClientTransportConfig::with_uri(daemon_url.to_string()),
    );
    let client_info = ClientInfo::new(
        ClientCapabilities::default(),
        Implementation::new(name.to_owned(), "0.0.1"),
    );
    let service = client_info
        .serve(transport)
        .await
        .map_err(|e| e.to_string())?;
    let peer = Arc::new(service.peer().clone());
    Ok((peer, service))
}

fn args_obj(value: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    value.as_object().cloned().unwrap_or_default()
}

fn fleet_params() -> CallToolRequestParams {
    CallToolRequestParams::new("fleet").with_arguments(args_obj(json!({})))
}

fn inspect_params(pid: u32) -> CallToolRequestParams {
    CallToolRequestParams::new("inspect").with_arguments(args_obj(json!({
        "pid": pid,
        "paths": ["/project1"],
    })))
}

// ---------------------------------------------------------------------------
// Storm driver
// ---------------------------------------------------------------------------

struct StormReport {
    /// All probe latencies observed during the storm (in order).
    probe_latencies: Vec<Duration>,
    /// Number of probes that exceeded the budget.
    missed_probes: usize,
    /// Number of /mcp/health probes that exceeded the budget (daemon-global layer).
    missed_health: usize,
    /// Number of raw TCP connect failures (accept loop or process dead).
    connect_failures: usize,
    /// Number of times the daemon child was observed exited.
    process_exits: usize,
    /// Number of tool calls that failed with a transport-level error.
    call_errors: usize,
    /// Whether a fresh session's fleet completed after the storm.
    fresh_fleet_ok: bool,
    /// Latency of the fresh-session fleet probe.
    fresh_fleet_latency: Duration,
    /// Outcome of the fresh-session connect (None = succeeded).
    fresh_connect_err: Option<String>,
}

fn execute_python_params(pid: u32) -> CallToolRequestParams {
    CallToolRequestParams::new("execute_python").with_arguments(args_obj(json!({
        "pid": pid,
        "script": "print('hi')",
        "includeLogs": false,
    })))
}

async fn run_storm(daemon_url: String, pids: &[u32], port: u16) -> StormReport {
    // Client sessions + per-client concurrency semaphore (cap concurrent calls
    // per session so bursts stay bounded).
    let mut sessions = Vec::new();
    for i in 0..NUM_CLIENTS {
        sessions.push(
            connect_client(&daemon_url, &format!("storm-{i}"))
                .await
                .expect("initial storm session"),
        );
    }

    let stats = Arc::new(Stats::default());

    // Liveness monitor: raw TCP connect every 300ms. A connect failure while
    // the process is alive = accept loop stalled; a dead process also fails.
    let stats_live = Arc::clone(&stats);
    let monitor = tokio::spawn(async move {
        loop {
            if tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .is_err()
            {
                *stats_live.connect_failures.lock().await += 1;
                println!("[monitor] TCP connect FAILED");
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    });

    // Probe session: fleet on a strict deadline, forever.
    let (probe_peer, probe_svc) = connect_client(&daemon_url, "probe")
        .await
        .expect("probe session");
    let stats_probe = Arc::clone(&stats);
    let probe = tokio::spawn(async move {
        // Keep the session alive for the whole probe lifetime.
        let _keep_alive = probe_svc;
        let mut since_last_miss = Instant::now();
        loop {
            let started = Instant::now();
            let res = timeout(PROBE_BUDGET, probe_peer.call_tool_once(fleet_params())).await;
            let latency = started.elapsed();
            stats_probe.probes.lock().await.push(latency);
            if res.is_err() {
                *stats_probe.missed.lock().await += 1;
                println!(
                    "[probe] MISSED fleet after {:?} (since last miss {:?})",
                    latency,
                    since_last_miss.elapsed()
                );
                since_last_miss = Instant::now();
            }
            tokio::time::sleep(PROBE_INTERVAL).await;
        }
    });

    // Health probe: raw HTTP /mcp/health — bypasses the session layer entirely.
    // A miss here means the daemon's HTTP server itself is starved/frozen.
    let health_base = daemon_url.trim_end_matches("/mcp/rpc").to_owned();
    let stats_health = Arc::clone(&stats);
    let health = tokio::spawn(async move {
        let client = reqwest::Client::new();
        loop {
            let started = Instant::now();
            let ok = timeout(
                Duration::from_secs(2),
                client.get(format!("{health_base}/mcp/health")).send(),
            )
            .await
            .map(|r| r.is_ok())
            .unwrap_or(false);
            let latency = started.elapsed();
            if !ok {
                *stats_health.health_missed.lock().await += 1;
                println!("[health] MISSED after {latency:?}");
            }
            tokio::time::sleep(PROBE_INTERVAL).await;
        }
    });

    // Storm workers: each session fires bursts of concurrent calls.
    let mut workers = Vec::new();
    for (idx, (peer, _svc)) in sessions.iter().enumerate() {
        let peer = Arc::clone(peer);
        let pids = pids.to_vec();
        let stats = Arc::clone(&stats);
        let w = tokio::spawn(async move {
            let started = Instant::now();
            while started.elapsed() < Duration::from_secs(STORM_SECS) {
                let mut calls = Vec::new();
                for k in 0..BURST_PER_CLIENT {
                    let peer = Arc::clone(&peer);
                    let pid = pids[(idx + k) % pids.len()];
                    calls.push(tokio::spawn(async move {
                        // Mix cheap fleet, bridged inspect, and slow execute_python.
                        match k % 4 {
                            0 => timeout(CALL_BUDGET, peer.call_tool_once(fleet_params())).await,
                            3 => {
                                timeout(
                                    CALL_BUDGET,
                                    peer.call_tool_once(execute_python_params(pid)),
                                )
                                .await
                            }
                            _ => {
                                timeout(CALL_BUDGET, peer.call_tool_once(inspect_params(pid))).await
                            }
                        }
                    }));
                }
                let results = join_all(calls).await;
                for r in results {
                    match r {
                        Ok(Ok(_)) => {}
                        Ok(Err(e)) => {
                            *stats.call_errs.lock().await += 1;
                            let msg = e.to_string();
                            if msg.contains("transport") || msg.contains("Transport") {
                                println!("[storm-{idx}] transport error: {e}");
                            }
                        }
                        Err(_) => {
                            *stats.call_errs.lock().await += 1;
                            println!("[storm-{idx}] call task panicked");
                        }
                    }
                }
            }
        });
        workers.push(w);
    }

    // Session churn A: periodically open a fresh session, call fleet, drop it.
    let churn_url = daemon_url.clone();
    let stats_churn = Arc::clone(&stats);
    let churn = tokio::spawn(async move {
        let mut tick = 0u64;
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            tick += 1;
            if tick > STORM_SECS {
                break;
            }
            match timeout(Duration::from_secs(3), connect_client(&churn_url, "churn")).await {
                Ok(Ok((peer, _svc))) => {
                    let _ =
                        timeout(Duration::from_secs(3), peer.call_tool_once(fleet_params())).await;
                    drop(peer);
                }
                Ok(Err(e)) => {
                    *stats_churn.connect_failures.lock().await += 1;
                    println!("[churn] connect failed: {e}");
                }
                Err(_) => {
                    *stats_churn.connect_failures.lock().await += 1;
                    println!("[churn] connect timed out");
                }
            }
        }
    });

    // Session churn B: fire a slow bridged call, then tear the session down
    // mid-call (client drops while the daemon session still has work in flight).
    let teardown_url = daemon_url.clone();
    let teardown_pids = pids.to_vec();
    let stats_teardown = Arc::clone(&stats);
    let teardown = tokio::spawn(async move {
        let mut tick = 0u64;
        loop {
            tokio::time::sleep(Duration::from_millis(700)).await;
            tick += 1;
            if tick > STORM_SECS * 10 / 7 {
                break;
            }
            let Ok(Ok((peer, _svc))) = timeout(
                Duration::from_secs(3),
                connect_client(&teardown_url, "teardown"),
            )
            .await
            else {
                *stats_teardown.connect_failures.lock().await += 1;
                continue;
            };
            let pid = teardown_pids[(tick as usize) % teardown_pids.len()];
            let peer_call = peer.clone();
            let call =
                tokio::spawn(
                    async move { peer_call.call_tool_once(execute_python_params(pid)).await },
                );
            // Drop the session (and its HTTP connection) ~150ms into the call.
            tokio::time::sleep(Duration::from_millis(150)).await;
            drop(peer);
            let _ = call.await;
        }
    });

    for w in workers {
        let _ = w.await;
    }
    let _ = churn.await;
    let _ = teardown.await;

    // Stop the probes + monitor and collect.
    probe.abort();
    let _ = probe.await;
    health.abort();
    let _ = health.await;
    monitor.abort();
    let _ = monitor.await;

    let probe_latencies = stats.probes.lock().await.clone();
    let missed_probes = *stats.missed.lock().await;
    let missed_health = *stats.health_missed.lock().await;
    let connect_failures = *stats.connect_failures.lock().await;
    let process_exits = *stats.process_exits.lock().await;
    let call_errors = *stats.call_errs.lock().await;

    // Fresh-session probe: distinguishes per-session wedge from daemon freeze.
    let fresh_started = Instant::now();
    let fresh_res = timeout(PROBE_BUDGET, async {
        let (peer, _svc) = connect_client(&daemon_url, "fresh").await?;
        peer.call_tool_once(fleet_params())
            .await
            .map_err(|e| e.to_string())
    })
    .await;
    let fresh_fleet_latency = fresh_started.elapsed();
    let (fresh_fleet_ok, fresh_connect_err) = match &fresh_res {
        Ok(Ok(_)) => (true, None),
        Ok(Err(e)) => (false, Some(e.clone())),
        Err(_) => (
            false,
            Some(format!("timed out after {fresh_fleet_latency:?}")),
        ),
    };

    for (_, svc) in &mut sessions {
        let _ = svc;
    }
    let _ = probe_svc;

    StormReport {
        probe_latencies,
        missed_probes,
        missed_health,
        connect_failures,
        process_exits,
        call_errors,
        fresh_fleet_ok,
        fresh_fleet_latency,
        fresh_connect_err,
    }
}

// ---------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn multi_client_storm_does_not_freeze() {
    let daemon = spawn_daemon();
    let port = daemon.port;
    let mut daemon = daemon;
    wait_health(&mut daemon, Duration::from_secs(20)).await;

    // Fake TD peers on distinct pids.
    let pids: Vec<u32> = (0..4).map(|i| 4240 + i).collect();
    let mut peers = Vec::new();
    for pid in &pids {
        peers.push(fake_td_peer(daemon.pipe.clone(), *pid, PEER_DELAY, SLOW_CALL_DELAY).await);
    }

    // Wait for bridges to register.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let daemon_url = format!("http://127.0.0.1:{port}/mcp/rpc");
    println!("== storm start (port {port}) ==");
    let report = run_storm(daemon_url, &pids, port).await;
    println!("== storm end ==");

    // Did the daemon process die during the storm?
    let exited = daemon.child.try_wait().ok().flatten();
    if let Some(status) = &exited {
        println!("daemon process exited during storm: {status}");
    }

    // Dump connection counts on the daemon port (netstat, foreign process).
    // With pooled clients the count stays bounded (connections reused); the
    // pre-fix no-pool clients burned ~16k TIME_WAIT sockets in 20s.
    let mut time_wait_count = 0usize;
    if let Ok(out) = std::process::Command::new("netstat").arg("-ano").output() {
        let text = String::from_utf8_lossy(&out.stdout);
        let matching: Vec<&str> = text
            .lines()
            .filter(|l| l.contains(&format!(":{port} ")) || l.contains(&format!(":{port}\t")))
            .collect();
        let mut by_state = std::collections::HashMap::<&str, usize>::new();
        for line in &matching {
            if let Some(state) = line.split_whitespace().nth(3) {
                *by_state.entry(state).or_default() += 1;
            }
        }
        time_wait_count = by_state.get("TIME_WAIT").copied().unwrap_or(0);
        println!(
            "netstat on :{port}: {} conns {:?}",
            matching.len(),
            by_state
        );
    }

    let max_probe = report
        .probe_latencies
        .iter()
        .copied()
        .max()
        .unwrap_or_default();
    let slow_probes = report
        .probe_latencies
        .iter()
        .filter(|l| **l > PROBE_BUDGET)
        .count();

    println!("probe samples: {}", report.probe_latencies.len());
    println!("max probe latency: {max_probe:?}");
    println!(
        "missed probes (>{PROBE_BUDGET:?}): {}",
        report.missed_probes
    );
    println!("slow probes (>{PROBE_BUDGET:?}): {slow_probes}");
    println!("missed health probes: {}", report.missed_health);
    println!("tcp connect failures: {}", report.connect_failures);
    println!("daemon process exits observed: {}", report.process_exits);
    println!("call errors: {}", report.call_errors);
    println!(
        "fresh-session fleet after storm: ok={} latency={:?} err={:?}",
        report.fresh_fleet_ok, report.fresh_fleet_latency, report.fresh_connect_err
    );

    for p in peers {
        p.abort();
    }

    if report.missed_probes > 0 || report.missed_health > 0 || !report.fresh_fleet_ok {
        daemon.print_log_tail(80);
    }

    assert!(
        report.missed_probes == 0,
        "probe fleet exceeded budget {} times — session/daemon wedged",
        report.missed_probes
    );
    assert!(
        report.missed_health == 0,
        "daemon /mcp/health stopped answering {} times — HTTP server frozen",
        report.missed_health
    );
    assert!(
        report.fresh_fleet_ok,
        "fresh session could not call fleet after the storm — daemon-global freeze"
    );
    assert!(
        time_wait_count < 2_000,
        "client connection churn not bounded: {time_wait_count} TIME_WAIT sockets on :{port} \
         — pooled HTTP client not in effect; sustained load exhausts the Windows \
         ephemeral port range and freezes the MCP transport"
    );
}

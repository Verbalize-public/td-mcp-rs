//! Federation tool-proxy integration tests (P3 Wave Fc).
//!
//! Spawns real master + slave daemons, attaches a named-pipe fake TD on the
//! slave, then exercises master `/mcp/tools/call` proxy routing.
//!
//! Run:
//! ```text
//! cargo test -p tdmcp-daemon --test federation_proxy -- --nocapture --test-threads=1
//! ```

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test harness"
)]

use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tdmcp_diagnostics::codes;
use tdmcp_ipc::{encode, HandshakeRequest, Message};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};
use tokio::task::JoinHandle;

const MASTER_PSK: &str = "fc-master-psk-secret";
const MASTER_ID: &str = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
const SLAVE_A_ID: &str = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb";
const SLAVE_B_ID: &str = "cccccccc-cccc-cccc-cccc-cccccccccccc";
const FAKE_PID: u32 = 4242;

struct TestDaemon {
    child: Child,
    port: u16,
    pipe: String,
    _daemon_id: String,
    log_path: std::path::PathBuf,
    _data_dir: tempfile::TempDir,
    _config_dir: tempfile::TempDir,
}

impl Drop for TestDaemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl TestDaemon {
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

    fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

fn pick_free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("probe port")
        .local_addr()
        .expect("local addr")
        .port()
}

#[allow(clippy::too_many_arguments, reason = "test config fixture")]
fn write_config(
    path: &std::path::Path,
    port: u16,
    role: &str,
    auth_mode: &str,
    psk: &str,
    master_url: &str,
    master_psk: &str,
    daemon_id: &str,
) {
    let text = format!(
        r#"
[server]
port = {port}
bind_address = "127.0.0.1"

[auth]
mode = "{auth_mode}"
psk = "{psk}"

[federation]
role = "{role}"
daemon_id = "{daemon_id}"
master_url = "{master_url}"
master_psk = "{master_psk}"

[daemon]
keep_alive = true
always_on = false
show_tray = false

[bridge]
call_timeout_secs = 45
script_timeout_secs = 120
heartbeat_interval_secs = 5
pong_timeout_secs = 8
idle_dead_secs = 20
"#
    );
    std::fs::write(path, text).expect("write config");
}

fn spawn_daemon(
    role: &str,
    auth_mode: &str,
    psk: &str,
    master_url: &str,
    master_psk: &str,
    daemon_id: &str,
) -> TestDaemon {
    let data_dir = tempfile::tempdir().expect("temp data");
    let config_dir = tempfile::tempdir().expect("temp config");
    let config_path = config_dir.path().join("config.toml");
    let port = pick_free_port();
    write_config(
        &config_path,
        port,
        role,
        auth_mode,
        psk,
        master_url,
        master_psk,
        daemon_id,
    );

    let pipe = format!(
        r"\\.\pipe\tdmcp-fc-{}-{}-{}",
        role,
        std::process::id(),
        port
    );
    let exe = env!("CARGO_BIN_EXE_tdmcp-daemon");
    let log_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("tmp");
    std::fs::create_dir_all(&log_root).expect("create target/tmp");
    let log_path = log_root.join(format!("federation-proxy-{role}-{port}.log"));
    let log_file = std::fs::File::create(&log_path).expect("create log");
    let stderr = log_file.try_clone().expect("clone log");

    let child = Command::new(exe)
        .arg("start")
        .arg("--port")
        .arg(port.to_string())
        .arg("--data-dir")
        .arg(data_dir.path())
        .arg("--no-gui")
        .env("TDMCP_CONFIG_PATH", &config_path)
        .env("TDMCP_IPC_PIPE", &pipe)
        .env("TDMCP_IDLE_EXIT_SECS", "0")
        .env("RUST_LOG", "warn,tdmcp_daemon=info,tdmcp_mcp=info")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr))
        .spawn()
        .expect("spawn daemon");

    TestDaemon {
        child,
        port,
        pipe,
        _daemon_id: daemon_id.to_owned(),
        log_path,
        _data_dir: data_dir,
        _config_dir: config_dir,
    }
}

async fn wait_health(daemon: &mut TestDaemon, auth: Option<&str>, budget: Duration) {
    let url = format!("{}/mcp/health", daemon.base_url());
    let client = reqwest::Client::new();
    let deadline = Instant::now() + budget;
    loop {
        if let Ok(Some(status)) = daemon.child.try_wait() {
            daemon.print_log_tail(80);
            panic!("daemon exited early with {status}");
        }
        let mut req = client.get(&url);
        if let Some(psk) = auth {
            req = req.header("Authorization", format!("Bearer {psk}"));
        }
        if let Ok(resp) = req.send().await {
            if auth.is_some() {
                if resp.status().is_success() {
                    return;
                }
            } else if resp.status() == reqwest::StatusCode::UNAUTHORIZED
                || resp.status().is_success()
            {
                return;
            }
        }
        assert!(
            Instant::now() < deadline,
            "daemon never became reachable on port {}",
            daemon.port
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

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

/// Minimal fake TD: handshake + answer execute_python / capture / inspect.
fn spawn_fake_td(pipe: String, pid: u32) -> JoinHandle<()> {
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
            let result = match method.as_str() {
                "ping" => json!({"ok": true, "pong": true}),
                "execute_python" => json!({"ok": true, "result": 1, "logs": []}),
                "capture" => json!({
                    "ok": true,
                    "bytes": 32,
                    "path": "/project1/out1",
                    "mimeType": "image/png",
                    "imageBase64": "iVBORw0KGgo="
                }),
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

async fn wait_slave_registered(master: &TestDaemon, slave_id: &str, budget: Duration) {
    let client = reqwest::Client::new();
    let deadline = Instant::now() + budget;
    loop {
        let slaves = client
            .get(format!("{}/admin/federation/slaves", master.base_url()))
            .header("Authorization", format!("Bearer {MASTER_PSK}"))
            .send()
            .await
            .expect("slaves")
            .json::<Value>()
            .await
            .expect("json");
        let list = slaves["slaves"].as_array().cloned().unwrap_or_default();
        if list.iter().any(|s| s["daemonId"] == slave_id) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "slave {slave_id} never registered on master"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

async fn wait_fleet_has_pid(master: &TestDaemon, pid: u32, slave_id: &str, budget: Duration) {
    let client = reqwest::Client::new();
    let deadline = Instant::now() + budget;
    loop {
        let fleet = client
            .get(format!("{}/admin/fleet", master.base_url()))
            .header("Authorization", format!("Bearer {MASTER_PSK}"))
            .send()
            .await
            .expect("fleet")
            .json::<Value>()
            .await
            .expect("json");
        let procs = fleet["processes"].as_array().cloned().unwrap_or_default();
        if procs
            .iter()
            .any(|p| p["pid"] == pid && p.get("daemonId").and_then(Value::as_str) == Some(slave_id))
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "pid {pid} on {slave_id} never appeared in master fleet"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

async fn master_tool_call(master: &TestDaemon, name: &str, arguments: Value) -> Value {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/mcp/tools/call", master.base_url()))
        .header("Authorization", format!("Bearer {MASTER_PSK}"))
        .json(&json!({ "name": name, "arguments": arguments }))
        .send()
        .await
        .expect("tool call");
    assert!(
        resp.status().is_success(),
        "tool call HTTP {}",
        resp.status()
    );
    resp.json().await.expect("tool json")
}

#[tokio::test]
async fn proxy_execute_python_with_daemon_id() {
    let mut master = spawn_daemon("master", "psk", MASTER_PSK, "", "", MASTER_ID);
    wait_health(&mut master, Some(MASTER_PSK), Duration::from_secs(20)).await;

    let master_url = master.base_url();
    let mut slave = spawn_daemon("slave", "none", "", &master_url, MASTER_PSK, SLAVE_A_ID);
    wait_health(&mut slave, None, Duration::from_secs(20)).await;

    let _peer = spawn_fake_td(slave.pipe.clone(), FAKE_PID);
    wait_slave_registered(&master, SLAVE_A_ID, Duration::from_secs(15)).await;
    wait_fleet_has_pid(&master, FAKE_PID, SLAVE_A_ID, Duration::from_secs(15)).await;

    // Confirm slave sees the TD locally first.
    let client = reqwest::Client::new();
    let local_fleet = client
        .get(format!("{}/admin/fleet", slave.base_url()))
        .send()
        .await
        .expect("slave fleet")
        .json::<Value>()
        .await
        .expect("json");
    assert!(
        local_fleet["processes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["pid"] == FAKE_PID),
        "slave local fleet missing pid: {local_fleet}"
    );

    let body = master_tool_call(
        &master,
        "execute_python",
        json!({
            "pid": FAKE_PID,
            "daemonId": SLAVE_A_ID,
            "script": "result = 1",
            "includeLogs": false,
        }),
    )
    .await;

    assert_eq!(body["ok"], true, "proxy response: {body}");
    let data = &body["data"];
    assert_eq!(data["routed"], true, "expected routed: {data}");
    assert_eq!(data["result"], 1, "expected result=1: {data}");

    // Capture budget via proxy (tiny maxSize; response must stay under wire budget).
    let cap = master_tool_call(
        &master,
        "capture",
        json!({
            "pid": FAKE_PID,
            "daemonId": SLAVE_A_ID,
            "path": "/project1/out1",
            "maxSize": 64,
        }),
    )
    .await;
    assert_eq!(cap["ok"], true, "capture proxy: {cap}");
    let cap_bytes = serde_json::to_vec(&cap).expect("serialize");
    assert!(
        cap_bytes.len() < 2_000_000,
        "capture response too large: {} bytes",
        cap_bytes.len()
    );
    assert_eq!(cap["data"]["routed"], true);

    let _ = slave;
    let _ = master;
}

#[tokio::test]
async fn ambiguous_pid_without_daemon_id() {
    let mut master = spawn_daemon("master", "psk", MASTER_PSK, "", "", MASTER_ID);
    wait_health(&mut master, Some(MASTER_PSK), Duration::from_secs(20)).await;
    let master_url = master.base_url();

    let mut slave_a = spawn_daemon("slave", "none", "", &master_url, MASTER_PSK, SLAVE_A_ID);
    wait_health(&mut slave_a, None, Duration::from_secs(20)).await;
    let _peer_a = spawn_fake_td(slave_a.pipe.clone(), FAKE_PID);

    let mut slave_b = spawn_daemon("slave", "none", "", &master_url, MASTER_PSK, SLAVE_B_ID);
    wait_health(&mut slave_b, None, Duration::from_secs(20)).await;
    let _peer_b = spawn_fake_td(slave_b.pipe.clone(), FAKE_PID);

    wait_slave_registered(&master, SLAVE_A_ID, Duration::from_secs(15)).await;
    wait_slave_registered(&master, SLAVE_B_ID, Duration::from_secs(15)).await;
    wait_fleet_has_pid(&master, FAKE_PID, SLAVE_A_ID, Duration::from_secs(15)).await;
    wait_fleet_has_pid(&master, FAKE_PID, SLAVE_B_ID, Duration::from_secs(15)).await;

    let body = master_tool_call(
        &master,
        "execute_python",
        json!({
            "pid": FAKE_PID,
            "script": "result = 1",
            "includeLogs": false,
        }),
    )
    .await;

    assert_eq!(body["ok"], false, "expected ambiguous: {body}");
    let code = body["items"][0]["code"].as_str().unwrap_or("");
    assert_eq!(code, codes::FEDERATION_AMBIGUOUS_PID, "body={body}");
    let candidates = body["candidates"].as_array().expect("candidates");
    assert!(candidates.len() >= 2, "candidates={candidates:?}");

    let _ = (slave_a, slave_b, master);
}

#[tokio::test]
async fn slave_unreachable_after_kill() {
    let mut master = spawn_daemon("master", "psk", MASTER_PSK, "", "", MASTER_ID);
    wait_health(&mut master, Some(MASTER_PSK), Duration::from_secs(20)).await;
    let master_url = master.base_url();

    let mut slave = spawn_daemon("slave", "none", "", &master_url, MASTER_PSK, SLAVE_A_ID);
    wait_health(&mut slave, None, Duration::from_secs(20)).await;
    let _peer = spawn_fake_td(slave.pipe.clone(), FAKE_PID);
    wait_slave_registered(&master, SLAVE_A_ID, Duration::from_secs(15)).await;

    // Kill slave process (drop will also kill, but we need mid-test death).
    let _ = slave.child.kill();
    let _ = slave.child.wait();

    let body = master_tool_call(
        &master,
        "execute_python",
        json!({
            "pid": FAKE_PID,
            "daemonId": SLAVE_A_ID,
            "script": "result = 1",
            "includeLogs": false,
        }),
    )
    .await;

    assert_eq!(body["ok"], false, "expected unreachable: {body}");
    let code = body["items"][0]["code"].as_str().unwrap_or("");
    assert_eq!(code, codes::FEDERATION_SLAVE_UNREACHABLE, "body={body}");

    let _ = master;
}


//! Auth middleware + loopback guard integration tests.
//!
//! Spawns a real `tdmcp-daemon` binary with a temp config (`TDMCP_CONFIG_PATH`).
//!
//! Run:
//! ```text
//! cargo test -p tdmcp-daemon --test admin_auth -- --nocapture --test-threads=1
//! ```
//!
//! Remote non-loopback peer rejection for `/admin/shutdown` is covered by unit
//! tests of `is_loopback` + `is_loopback_only_admin` in `middleware.rs` (LAN
//! peer simulation is awkward from the same host). Loopback POST to shutdown
//! while bound on `0.0.0.0` is exercised here.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test harness"
)]

use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;
use tdmcp_diagnostics::codes;

const TEST_PSK: &str = "fa-test-psk-secret";

struct TestDaemon {
    child: Child,
    port: u16,
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

/// Bridge port for a daemon: free and distinct from its HTTP port (a
/// conflicting bridge bind fatally exits the daemon, T-3).
fn free_ipc_port(http_port: u16) -> u16 {
    loop {
        let port = pick_free_port();
        if port != http_port {
            return port;
        }
    }
}

fn write_config(path: &std::path::Path, bind_address: &str, auth_mode: &str, psk: &str) {
    let text = format!(
        r#"
[server]
port = 9860
bind_address = "{bind_address}"

[auth]
mode = "{auth_mode}"
psk = "{psk}"

[daemon]
keep_alive = false
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

fn spawn_daemon(bind_address: &str, auth_mode: &str, psk: &str) -> TestDaemon {
    let data_dir = tempfile::tempdir().expect("temp data");
    let config_dir = tempfile::tempdir().expect("temp config");
    let config_path = config_dir.path().join("config.toml");
    write_config(&config_path, bind_address, auth_mode, psk);

    let port = pick_free_port();
    let exe = env!("CARGO_BIN_EXE_tdmcp-daemon");

    let log_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("tmp");
    std::fs::create_dir_all(&log_root).expect("create target/tmp");
    let log_path = log_root.join(format!("admin-auth-{port}.log"));
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
        .env("TDMCP_IDLE_EXIT_SECS", "0")
        .env("TDMCP_IPC_PORT", free_ipc_port(port).to_string())
        .env("RUST_LOG", "warn,tdmcp_daemon=info")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr))
        .spawn()
        .expect("spawn daemon");

    TestDaemon {
        child,
        port,
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
                // Daemon is up (auth may reject unauthenticated probes).
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

async fn get_health(base: &str, auth: Option<&str>) -> (reqwest::StatusCode, Value) {
    let client = reqwest::Client::new();
    let mut req = client.get(format!("{base}/mcp/health"));
    if let Some(psk) = auth {
        req = req.header("Authorization", format!("Bearer {psk}"));
    }
    let resp = req.send().await.expect("health request");
    let status = resp.status();
    let body: Value = resp.json().await.expect("health json");
    (status, body)
}

#[tokio::test]
async fn psk_rejects_missing_and_wrong_bearer() {
    let mut daemon = spawn_daemon("127.0.0.1", "psk", TEST_PSK);
    wait_health(&mut daemon, Some(TEST_PSK), Duration::from_secs(15)).await;
    let base = daemon.base_url();

    let (status, body) = get_health(&base, None).await;
    assert_eq!(status, reqwest::StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], codes::REMOTE_UNAUTHORIZED);

    let (status, body) = get_health(&base, Some("wrong-psk")).await;
    assert_eq!(status, reqwest::StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], codes::REMOTE_UNAUTHORIZED);

    let (status, body) = get_health(&base, Some(TEST_PSK)).await;
    assert_eq!(status, reqwest::StatusCode::OK);
    assert_eq!(body["ok"], true);
}

#[tokio::test]
async fn mode_none_allows_health_without_token() {
    let mut daemon = spawn_daemon("127.0.0.1", "none", "");
    wait_health(&mut daemon, None, Duration::from_secs(15)).await;
    let (status, body) = get_health(&daemon.base_url(), None).await;
    assert_eq!(status, reqwest::StatusCode::OK);
    assert_eq!(body["ok"], true);
}

#[tokio::test]
async fn bind_all_loopback_shutdown_ok() {
    // bind 0.0.0.0 requires psk validation; peer is still 127.0.0.1 so
    // loopback-only /admin/shutdown must succeed.
    let mut daemon = spawn_daemon("0.0.0.0", "psk", TEST_PSK);
    wait_health(&mut daemon, Some(TEST_PSK), Duration::from_secs(15)).await;

    let client = reqwest::Client::new();
    let status_url = format!("{}/admin/status", daemon.base_url());
    let status_resp = client.get(&status_url).send().await.expect("admin status");
    assert_eq!(status_resp.status(), reqwest::StatusCode::OK);
    let status_body: Value = status_resp.json().await.expect("status json");
    assert_eq!(status_body["bindAddress"], "0.0.0.0");

    let url = format!("{}/admin/shutdown", daemon.base_url());
    let resp = client.post(&url).send().await.expect("shutdown");
    assert!(
        resp.status().is_success(),
        "loopback shutdown should succeed: {}",
        resp.status()
    );
    let body: Value = resp.json().await.expect("shutdown json");
    assert_eq!(body["ok"], true);
}

#[tokio::test]
async fn admin_config_requires_psk_when_enabled() {
    let mut daemon = spawn_daemon("127.0.0.1", "psk", TEST_PSK);
    wait_health(&mut daemon, Some(TEST_PSK), Duration::from_secs(15)).await;
    let client = reqwest::Client::new();
    let url = format!("{}/admin/config", daemon.base_url());

    let missing = client.get(&url).send().await.expect("config no auth");
    assert_eq!(missing.status(), reqwest::StatusCode::UNAUTHORIZED);

    let ok = client
        .get(&url)
        .header("Authorization", format!("Bearer {TEST_PSK}"))
        .send()
        .await
        .expect("config with auth");
    assert_eq!(ok.status(), reqwest::StatusCode::OK);
    let body: Value = ok.json().await.expect("config json");
    assert!(body.get("server").is_some() || body.get("federation").is_some());
}

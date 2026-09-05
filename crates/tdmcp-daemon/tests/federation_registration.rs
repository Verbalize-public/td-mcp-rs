//! Federation registration + fleet-push integration tests.
//!
//! Spawns real `tdmcp-daemon` binaries (master + slave) with temp configs.
//!
//! Run:
//! ```text
//! cargo test -p tdmcp-daemon --test federation_registration -- --nocapture --test-threads=1
//! ```

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

const MASTER_PSK: &str = "fb-master-psk-secret";

struct TestDaemon {
    child: Child,
    port: u16,
    log_path: std::path::PathBuf,
    daemon_id: String,
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
    tdmcp_test_support::unique_test_port().expect("unique test port")
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

    let exe = env!("CARGO_BIN_EXE_tdmcp-daemon");
    let log_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("tmp");
    std::fs::create_dir_all(&log_root).expect("create target/tmp");
    let log_path = log_root.join(format!("federation-{role}-{port}.log"));
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
        daemon_id: daemon_id.to_owned(),
        _data_dir: data_dir,
        _config_dir: config_dir,
    }
}

async fn wait_health(daemon: &mut TestDaemon, auth: Option<&str>, budget: Duration) {
    let url = format!("{}/admin/status", daemon.base_url());
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(1))
        .build()
        .expect("readiness client");
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
            if resp.status().is_success() {
                if let Ok(status) = resp.json::<Value>().await {
                    assert_eq!(
                        status["pid"].as_u64(),
                        Some(u64::from(daemon.child.id())),
                        "readiness reached a different process on port {}: {status}",
                        daemon.port
                    );
                    assert_eq!(status["daemonId"], daemon.daemon_id);
                    return;
                }
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

#[tokio::test]
async fn master_accepts_slave_register_and_lists_slaves() {
    let mut master = spawn_daemon(
        "master",
        "psk",
        MASTER_PSK,
        "",
        "",
        "11111111-1111-1111-1111-111111111111",
    );
    wait_health(&mut master, Some(MASTER_PSK), Duration::from_secs(20)).await;

    let client = reqwest::Client::new();
    let status = client
        .get(format!("{}/admin/federation/status", master.base_url()))
        .send()
        .await
        .expect("status")
        .json::<Value>()
        .await
        .expect("status json");
    assert_eq!(status["ok"], true);
    assert_eq!(status["role"], "master");
    assert_eq!(status["daemonId"], "11111111-1111-1111-1111-111111111111");

    let register = client
        .post(format!("{}/admin/federation/register", master.base_url()))
        .header("Authorization", format!("Bearer {MASTER_PSK}"))
        .json(&serde_json::json!({
            "daemonId": "22222222-2222-2222-2222-222222222222",
            "hostname": "slave-host",
            "version": "0.0.0-test",
            "port": 9999,
            "authToken": "",
            "baseUrl": "http://127.0.0.1:9999",
        }))
        .send()
        .await
        .expect("register");
    assert_eq!(register.status(), reqwest::StatusCode::OK);
    let body: Value = register.json().await.expect("register json");
    assert_eq!(body["ok"], true);

    // overwrite same URL
    let again = client
        .post(format!("{}/admin/federation/register", master.base_url()))
        .header("Authorization", format!("Bearer {MASTER_PSK}"))
        .json(&serde_json::json!({
            "daemonId": "22222222-2222-2222-2222-222222222222",
            "hostname": "slave-host-renamed",
            "version": "0.0.0-test",
            "port": 9999,
            "authToken": "",
            "baseUrl": "http://127.0.0.1:9999",
        }))
        .send()
        .await
        .expect("re-register");
    assert_eq!(again.status(), reqwest::StatusCode::OK);

    // conflict different URL
    let conflict = client
        .post(format!("{}/admin/federation/register", master.base_url()))
        .header("Authorization", format!("Bearer {MASTER_PSK}"))
        .json(&serde_json::json!({
            "daemonId": "22222222-2222-2222-2222-222222222222",
            "hostname": "other",
            "version": "0.0.0-test",
            "port": 10000,
            "authToken": "",
            "baseUrl": "http://127.0.0.1:10000",
        }))
        .send()
        .await
        .expect("conflict");
    assert_eq!(conflict.status(), reqwest::StatusCode::CONFLICT);

    let slaves = client
        .get(format!("{}/admin/federation/slaves", master.base_url()))
        .header("Authorization", format!("Bearer {MASTER_PSK}"))
        .send()
        .await
        .expect("slaves")
        .json::<Value>()
        .await
        .expect("slaves json");
    let list = slaves["slaves"].as_array().expect("slaves array");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["hostname"], "slave-host-renamed");
    assert_eq!(list[0]["daemonId"], "22222222-2222-2222-2222-222222222222");
}

#[tokio::test]
async fn wrong_master_psk_rejected() {
    let mut master = spawn_daemon(
        "master",
        "psk",
        MASTER_PSK,
        "",
        "",
        "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
    );
    wait_health(&mut master, Some(MASTER_PSK), Duration::from_secs(20)).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/admin/federation/register", master.base_url()))
        .header("Authorization", "Bearer wrong-psk")
        .json(&serde_json::json!({
            "daemonId": "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb",
            "hostname": "bad",
            "version": "0.0.0",
            "port": 1,
            "authToken": "",
            "baseUrl": "http://127.0.0.1:1",
        }))
        .send()
        .await
        .expect("register");
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);
    let body: Value = resp.json().await.expect("json");
    assert_eq!(body["code"], codes::REMOTE_UNAUTHORIZED);

    let slaves = client
        .get(format!("{}/admin/federation/slaves", master.base_url()))
        .header("Authorization", format!("Bearer {MASTER_PSK}"))
        .send()
        .await
        .expect("slaves")
        .json::<Value>()
        .await
        .expect("slaves json");
    assert_eq!(slaves["slaves"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn slave_daemon_registers_with_master() {
    let mut master = spawn_daemon(
        "master",
        "psk",
        MASTER_PSK,
        "",
        "",
        "cccccccc-cccc-cccc-cccc-cccccccccccc",
    );
    wait_health(&mut master, Some(MASTER_PSK), Duration::from_secs(20)).await;

    let master_url = master.base_url();
    let mut slave = spawn_daemon(
        "slave",
        "none",
        "",
        &master_url,
        MASTER_PSK,
        "dddddddd-dddd-dddd-dddd-dddddddddddd",
    );
    wait_health(&mut slave, None, Duration::from_secs(20)).await;

    let client = reqwest::Client::new();
    let deadline = Instant::now() + Duration::from_secs(15);
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
        if list
            .iter()
            .any(|s| s["daemonId"] == "dddddddd-dddd-dddd-dddd-dddddddddddd")
        {
            break;
        }
        if Instant::now() >= deadline {
            master.print_log_tail(80);
            slave.print_log_tail(80);
            panic!("slave never appeared in master slaves list");
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // admin config auth
    let cfg = client
        .get(format!("{}/admin/config", master.base_url()))
        .header("Authorization", format!("Bearer {MASTER_PSK}"))
        .send()
        .await
        .expect("config");
    assert_eq!(cfg.status(), reqwest::StatusCode::OK);
}

#[tokio::test]
async fn settings_switch_federation_roles_without_restarting() {
    let mut coordinator = spawn_daemon("standalone", "none", "", "", "", "coordinator-live");
    let mut member = spawn_daemon("standalone", "none", "", "", "", "member-live");
    wait_health(&mut coordinator, None, Duration::from_secs(20)).await;
    wait_health(&mut member, None, Duration::from_secs(20)).await;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    let result: Value = client
        .post(format!("{}/admin/config", coordinator.base_url()))
        .json(&serde_json::json!({"federation": {"role": "master"}}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(result["restartRequired"], serde_json::json!([]));
    client.post(format!("{}/admin/config", member.base_url()))
        .json(&serde_json::json!({"federation": {"role": "slave", "masterUrl": coordinator.base_url()}}))
        .send().await.unwrap().error_for_status().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let fleet: Value = client
            .get(format!(
                "{}/admin/federation/slaves",
                coordinator.base_url()
            ))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if fleet["slaves"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["daemonId"] == "member-live")
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "member never registered: {fleet}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let status: Value = client
        .get(format!("{}/admin/status", member.base_url()))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(status["pid"], member.child.id());
    assert_eq!(status["role"], "slave");

    // Bad edits leave disk and runtime untouched.
    let before = std::fs::read(member._config_dir.path().join("config.toml")).unwrap();
    for patch in [
        serde_json::json!({"auth": {"mode": "typo"}}),
        serde_json::json!({"bridge": {"callTimeoutSecs": "wrong"}}),
    ] {
        let reply = client
            .post(format!("{}/admin/config", member.base_url()))
            .json(&patch)
            .send()
            .await
            .unwrap();
        assert_eq!(reply.status(), reqwest::StatusCode::BAD_REQUEST);
    }
    assert_eq!(
        std::fs::read(member._config_dir.path().join("config.toml")).unwrap(),
        before
    );
    client
        .post(format!("{}/admin/config", coordinator.base_url()))
        .json(&serde_json::json!({"federation": {"role": "standalone"}}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let status: Value = client
        .get(format!("{}/admin/status", coordinator.base_url()))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(status["role"], "standalone");
    assert_eq!(status["pid"], coordinator.child.id());
}

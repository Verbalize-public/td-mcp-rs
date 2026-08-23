//! Mid-session daemon death must not be a dead end: `tdmcp-daemon mcp` (the
//! IDE-spawned stdio proxy) escalates from reconnect-only healing to a real
//! `ensure_daemon` respawn once downtime crosses the reconnect config's
//! `stale` threshold — reusing the exact ensure/lock/spawn machinery as cold
//! start (see `ensure.rs`'s `ensure_daemon` and `daemon_link.rs`'s
//! `maybe_trigger_respawn`).
//!
//! This spawns a real daemon, a real `mcp` stdio proxy child pointed at it,
//! kills the daemon out from under the proxy, and asserts a fresh daemon
//! comes back healthy without the test itself calling `ensure`.

#![allow(clippy::unwrap_used, reason = "test setup/assertions may panic")]
#![allow(clippy::expect_used, reason = "test setup/assertions may panic")]
#![allow(clippy::panic, reason = "test assertions may panic")]

use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use tdmcp_daemon::{health_ok, read_daemon_lock_pid};
use tempfile::tempdir;

fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
    listener.local_addr().expect("local_addr").port()
}

fn daemon_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_tdmcp-daemon"))
}

fn kill_pid(pid: u32) {
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(unix)]
    {
        let _ = Command::new("kill").args(["-9", &pid.to_string()]).status();
    }
}

async fn wait_healthy(port: u16, timeout: Duration) {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if health_ok(port).await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("daemon not healthy on port {port} within {timeout:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn mid_session_daemon_death_triggers_automatic_respawn() {
    let dir = tempdir().expect("tempdir");
    let port = free_port();
    let config_path = dir.path().join("test-config.toml");
    tdmcp_config::ensure_default(&config_path, true).expect("seed config");
    let pipe = format!(r"\\.\pipe\tdmcp-rs-respawn-test-{port}");

    // Cold-start a real headless daemon, exactly like `ensure` would.
    let mut daemon = Command::new(daemon_bin())
        .args([
            "start",
            "--port",
            &port.to_string(),
            "--data-dir",
            dir.path().to_str().expect("utf8 data_dir"),
            "--no-gui",
        ])
        .env("TDMCP_IDLE_EXIT_SECS", "0")
        .env("TDMCP_IPC_PIPE", &pipe)
        .env(tdmcp_config::CONFIG_PATH_ENV, &config_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn daemon");
    wait_healthy(port, Duration::from_secs(15)).await;
    let original_pid = read_daemon_lock_pid(dir.path()).expect("daemon.lock after cold start");

    // A real `mcp` stdio proxy child, exactly like an IDE would spawn — piped
    // stdin kept open (never closed/written) so its stdio server keeps
    // waiting and its background respawn watcher keeps running.
    let mut mcp = Command::new(daemon_bin())
        .args([
            "mcp",
            "--port",
            &port.to_string(),
            "--data-dir",
            dir.path().to_str().expect("utf8 data_dir"),
        ])
        .env("TDMCP_IDLE_EXIT_SECS", "0")
        .env("TDMCP_IPC_PIPE", &pipe)
        .env(tdmcp_config::CONFIG_PATH_ENV, &config_path)
        // Fast, deterministic escalation: the daemon dies well past `recent`,
        // so the very first watcher tick after `stale` fires the respawn.
        .env("TDMCP_RECONNECT_STALE_MS", "1500")
        .env("TDMCP_RECONNECT_PROBE_INTERVAL_MS", "300")
        .env("TDMCP_RECONNECT_PROBE_MAX_MS", "500")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn mcp proxy");
    let _mcp_stdin = mcp.stdin.take().expect("mcp stdin");

    // Give the proxy a moment to establish its initial link, then kill the
    // daemon out from under it — simulating a crash / manual kill mid-session.
    tokio::time::sleep(Duration::from_millis(500)).await;
    kill_pid(original_pid);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while health_ok(port).await && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(!health_ok(port).await, "daemon should be down after kill");

    // The `mcp` proxy — not this test — must bring a fresh daemon back up.
    wait_healthy(port, Duration::from_secs(30)).await;
    let new_pid = read_daemon_lock_pid(dir.path()).expect("daemon.lock after respawn");
    assert_ne!(
        new_pid, original_pid,
        "respawned daemon should be a fresh process, not the killed one"
    );

    // Cleanup: stop the fresh daemon, then tear down both children.
    let _ = Command::new(daemon_bin())
        .args(["stop", "--port", &port.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while health_ok(port).await && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    if let Some(pid) = read_daemon_lock_pid(dir.path()) {
        kill_pid(pid);
    }
    let _ = std::fs::remove_file(dir.path().join("daemon.lock"));
    kill_child(&mut mcp);
    kill_child(&mut daemon);
}

fn kill_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

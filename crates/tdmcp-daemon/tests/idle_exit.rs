//! Binary smoke: empty daemon exits after idle timeout.

#![allow(clippy::unwrap_used, reason = "test setup/assertions may panic")]
#![allow(clippy::expect_used, reason = "test setup/assertions may panic")]
#![allow(clippy::panic, reason = "test assertions may panic")]

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
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
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("daemon not healthy on port {port} within {timeout:?}");
}

async fn wait_unhealthy(port: u16, timeout: Duration) {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if !health_ok(port).await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("daemon still healthy on port {port} after {timeout:?}");
}

fn spawn_idle(port: u16, data_dir: &Path, idle_secs: u64) -> std::process::Child {
    // Isolate from a live TD on the production pipe (Windows global name).
    let pipe = format!(r"\\.\pipe\tdmcp-rs-idle-test-{port}");
    Command::new(daemon_bin())
        .args([
            "start",
            "--port",
            &port.to_string(),
            "--data-dir",
            data_dir.to_str().expect("utf8 data_dir"),
            "--no-gui",
        ])
        .env("TDMCP_IDLE_EXIT_SECS", idle_secs.to_string())
        .env("TDMCP_IPC_PIPE", pipe)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn start")
}

#[tokio::test]
async fn empty_daemon_exits_after_idle_timeout() {
    let dir = tempdir().expect("tempdir");
    let data_dir = dir.path();
    let port = free_port();
    let mut child = spawn_idle(port, data_dir, 2);

    wait_healthy(port, Duration::from_secs(10)).await;
    wait_unhealthy(port, Duration::from_secs(8)).await;

    // Lock should be cleared by the idle-exit path.
    let lock = data_dir.join("daemon.lock");
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while lock.exists() && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        !lock.exists(),
        "daemon.lock should be removed after idle exit"
    );

    let _ = child.try_wait();
    if let Some(pid) = read_daemon_lock_pid(data_dir) {
        kill_pid(pid);
    }
    let _ = child.kill();
}

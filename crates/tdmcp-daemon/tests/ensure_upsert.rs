//! Concurrent `ensure_daemon` must yield exactly one healthy listener.

#![allow(clippy::unwrap_used, reason = "test setup/assertions may panic")]
#![allow(clippy::expect_used, reason = "test setup/assertions may panic")]
#![allow(clippy::panic, reason = "test assertions may panic")]

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use tdmcp_daemon::{ensure_daemon, health_ok, EnsureOptions};
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

fn stop_daemon(port: u16, data_dir: &Path) {
    let _ = Command::new(daemon_bin())
        .args(["stop", "--port", &port.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    std::thread::sleep(Duration::from_millis(400));
    if let Ok(text) = std::fs::read_to_string(data_dir.join("daemon.lock")) {
        if let Ok(pid) = text.trim().parse::<u32>() {
            kill_pid(pid);
        }
    }
    std::thread::sleep(Duration::from_millis(200));
}

#[tokio::test]
async fn concurrent_ensure_one_healthy_daemon() {
    let dir = tempdir().expect("tempdir");
    let port = free_port();
    let exe = daemon_bin();
    let opts = EnsureOptions {
        port,
        data_dir: dir.path().to_path_buf(),
        exe: Some(exe.clone()),
        timeout: Duration::from_secs(20),
        poll_only: false,
    };

    let a = tokio::spawn(ensure_daemon(opts.clone()));
    let b = tokio::spawn(ensure_daemon(opts.clone()));

    let ra = a.await.expect("join a").expect("ensure a");
    let rb = b.await.expect("join b").expect("ensure b");

    assert!(
        health_ok(port).await,
        "daemon should be healthy after concurrent ensure"
    );
    assert!(
        ra.spawned || rb.spawned || ra.already_running || rb.already_running,
        "at least one ensure should observe a running daemon: {ra:?} {rb:?}"
    );
    let spawn_count = u8::from(ra.spawned) + u8::from(rb.spawned);
    assert!(
        spawn_count <= 1,
        "expected at most one spawn, got {spawn_count}: {ra:?} {rb:?}"
    );

    let lock = dir.path().join("daemon.lock");
    let pid_text = std::fs::read_to_string(&lock).expect("daemon.lock");
    let pid: u32 = pid_text.trim().parse().expect("pid");
    assert!(pid > 0, "daemon.lock pid");

    stop_daemon(port, dir.path());
    for _ in 0..50 {
        if !health_ok(port).await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("daemon still healthy after stop+kill on port {port}");
}

#[tokio::test]
async fn ensure_noop_when_already_healthy() {
    let dir = tempdir().expect("tempdir");
    let port = free_port();
    let exe = daemon_bin();
    let opts = EnsureOptions {
        port,
        data_dir: dir.path().to_path_buf(),
        exe: Some(exe),
        timeout: Duration::from_secs(20),
        poll_only: false,
    };

    let first = ensure_daemon(opts.clone()).await.expect("first ensure");
    assert!(first.spawned || first.already_running);
    assert!(health_ok(port).await);

    let second = ensure_daemon(opts).await.expect("second ensure");
    assert!(second.already_running);
    assert!(!second.spawned);

    stop_daemon(port, dir.path());
}

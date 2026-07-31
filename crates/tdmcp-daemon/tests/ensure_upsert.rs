//! Concurrent `ensure_daemon` must yield exactly one healthy listener.
//! Also covers exclusive start refuse, restart handoff, and stale lock reclaim.

#![allow(clippy::unwrap_used, reason = "test setup/assertions may panic")]
#![allow(clippy::expect_used, reason = "test setup/assertions may panic")]
#![allow(clippy::panic, reason = "test assertions may panic")]

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use tdmcp_daemon::{
    ensure_daemon, health_ok, pid_alive, read_daemon_lock_pid, reclaim_stale_daemon_lock,
    EnsureOptions,
};
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

fn ensure_opts(port: u16, data_dir: &Path) -> EnsureOptions {
    EnsureOptions {
        port,
        data_dir: data_dir.to_path_buf(),
        exe: Some(daemon_bin()),
        timeout: Duration::from_secs(20),
        poll_only: false,
        // Integration tests are headless — never open the tray UI.
        no_gui: true,
        // Keep daemon alive for the duration of ensure/restart tests.
        idle_exit_secs: Some(0),
    }
}

fn spawn_start(port: u16, data_dir: &Path) -> std::process::Child {
    Command::new(daemon_bin())
        .args([
            "start",
            "--port",
            &port.to_string(),
            "--data-dir",
            data_dir.to_str().expect("utf8 data_dir"),
            "--no-gui",
        ])
        .env("TDMCP_IDLE_EXIT_SECS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn start")
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

#[tokio::test]
async fn concurrent_ensure_one_healthy_daemon() {
    let dir = tempdir().expect("tempdir");
    let port = free_port();
    let opts = ensure_opts(port, dir.path());

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
    let opts = ensure_opts(port, dir.path());

    let first = ensure_daemon(opts.clone()).await.expect("first ensure");
    assert!(first.spawned || first.already_running);
    assert!(health_ok(port).await);

    let second = ensure_daemon(opts).await.expect("second ensure");
    assert!(second.already_running);
    assert!(!second.spawned);

    stop_daemon(port, dir.path());
}

#[tokio::test]
async fn second_start_refuses_while_healthy() {
    let dir = tempdir().expect("tempdir");
    let port = free_port();
    let mut child = spawn_start(port, dir.path());
    wait_healthy(port, Duration::from_secs(15)).await;

    let output = Command::new(daemon_bin())
        .args([
            "start",
            "--port",
            &port.to_string(),
            "--data-dir",
            dir.path().to_str().expect("utf8"),
            "--no-gui",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("second start");

    assert!(
        !output.status.success(),
        "second start should refuse, got success; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("already running") || combined.contains("bind"),
        "expected exclusive refuse message, got: {combined}"
    );

    stop_daemon(port, dir.path());
    let _ = child.wait();
}

#[tokio::test]
async fn admin_restart_yields_one_healthy_listener() {
    let dir = tempdir().expect("tempdir");
    let port = free_port();
    let mut child = spawn_start(port, dir.path());
    wait_healthy(port, Duration::from_secs(15)).await;

    let old_pid = read_daemon_lock_pid(dir.path()).expect("daemon.lock before restart");

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://127.0.0.1:{port}/admin/restart"))
        .send()
        .await
        .expect("restart post");
    assert!(
        resp.status().is_success(),
        "restart status {}",
        resp.status()
    );

    // Wait for a new healthy owner (possibly new pid).
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let mut new_pid = None;
    while std::time::Instant::now() < deadline {
        if health_ok(port).await {
            if let Some(pid) = read_daemon_lock_pid(dir.path()) {
                if pid != old_pid && pid_alive(pid) {
                    new_pid = Some(pid);
                    break;
                }
                // Same pid can briefly remain if restart is slow; keep waiting for turnover
                // or accept healthy after old is gone.
                if !pid_alive(old_pid) && pid_alive(pid) {
                    new_pid = Some(pid);
                    break;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }

    assert!(
        health_ok(port).await,
        "daemon should be healthy after restart"
    );
    let pid = new_pid
        .or_else(|| read_daemon_lock_pid(dir.path()))
        .expect("daemon.lock after restart");
    assert!(pid_alive(pid), "replacement pid {pid} should be alive");

    // Exactly one healthy listener: a second ensure must be noop (no extra spawn needed).
    let second = ensure_daemon(ensure_opts(port, dir.path()))
        .await
        .expect("ensure after restart");
    assert!(second.already_running);
    assert!(!second.spawned);

    stop_daemon(port, dir.path());
    let _ = child.wait();
}

#[tokio::test]
async fn stale_daemon_lock_reclaimed_on_start() {
    let dir = tempdir().expect("tempdir");
    let port = free_port();

    // Fake a dead owner pid in daemon.lock.
    let fake_pid = 1u32; // System Idle / init — treat carefully; reclaim uses pid_alive.
                         // Prefer a definitely-dead pid: use a high unused number after verifying not alive.
    let mut dead_pid = 4_000_000u32;
    while pid_alive(dead_pid) {
        dead_pid += 1;
    }
    let _ = fake_pid;
    std::fs::write(dir.path().join("daemon.lock"), dead_pid.to_string()).expect("write lock");

    reclaim_stale_daemon_lock(dir.path());
    assert!(
        read_daemon_lock_pid(dir.path()).is_none(),
        "stale lock should be reclaimed by helper"
    );

    // Write it again and start for real — start path must reclaim and become healthy.
    std::fs::write(dir.path().join("daemon.lock"), dead_pid.to_string()).expect("rewrite lock");
    let mut child = spawn_start(port, dir.path());
    wait_healthy(port, Duration::from_secs(15)).await;

    let owner = read_daemon_lock_pid(dir.path()).expect("owner after start");
    assert_ne!(owner, dead_pid, "start should replace stale lock pid");
    assert!(pid_alive(owner));

    stop_daemon(port, dir.path());
    let _ = child.wait();
}

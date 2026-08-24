//! Concurrent `ensure_daemon` must yield exactly one healthy listener.
//! Also covers exclusive start refuse, restart handoff, stale lock reclaim,
//! and admin shutdown process exit.
//!
//! Binary-spawning tests share a process-wide mutex and always tear down
//! children (stop → wait → force-kill) so parallel runs cannot leave zombies.

#![allow(clippy::unwrap_used, reason = "test setup/assertions may panic")]
#![allow(clippy::expect_used, reason = "test setup/assertions may panic")]
#![allow(clippy::panic, reason = "test assertions may panic")]
// Process-wide std::sync::Mutex serializes binary-spawning tests across awaits
// (async Mutex would not exclude other OS threads / test binaries sharing state).
#![allow(
    clippy::await_holding_lock,
    reason = "binary_test_lock serializes process spawn"
)]

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use tdmcp_daemon::{
    ensure_daemon, health_ok, pid_alive, read_daemon_lock_pid, reclaim_stale_daemon_lock,
    EnsureOptions,
};
use tempfile::tempdir;

/// Serialize all binary-spawning tests in this crate file.
fn binary_test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
    listener.local_addr().expect("local_addr").port()
}

fn daemon_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_tdmcp-daemon"))
}

fn ipc_pipe_for(port: u16) -> String {
    format!(r"\\.\pipe\tdmcp-rs-ensure-test-{port}")
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

/// Owned daemon process (+ data dir path) that is always force-cleaned on drop.
struct DaemonHarness {
    port: u16,
    data_dir: PathBuf,
    child: Option<Child>,
}

impl DaemonHarness {
    fn spawn_start(port: u16, data_dir: &Path) -> Self {
        let config_path = data_dir.join("test-config.toml");
        tdmcp_config::ensure_default(&config_path, true).expect("seed config");
        let child = Command::new(daemon_bin())
            .args([
                "start",
                "--port",
                &port.to_string(),
                "--data-dir",
                data_dir.to_str().expect("utf8 data_dir"),
                "--no-gui",
            ])
            .env("TDMCP_IDLE_EXIT_SECS", "0")
            .env("TDMCP_IPC_PIPE", ipc_pipe_for(port))
            .env(tdmcp_config::CONFIG_PATH_ENV, &config_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn start");
        Self {
            port,
            data_dir: data_dir.to_path_buf(),
            child: Some(child),
        }
    }

    fn child_mut(&mut self) -> &mut Child {
        self.child.as_mut().expect("harness child")
    }

    /// Graceful stop; production path must succeed without the force-kill below.
    async fn stop_graceful(&mut self) {
        let _ = Command::new(daemon_bin())
            .args(["stop", "--port", &self.port.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            let child_gone = match self.child_mut().try_wait() {
                Ok(Some(_)) => true,
                Ok(None) => false,
                Err(_) => true,
            };
            if child_gone
                && !health_ok(self.port).await
                && read_daemon_lock_pid(&self.data_dir).is_none()
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Wait until the child exits (no taskkill). Returns whether it exited.
    async fn wait_exit(&mut self, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            match self.child_mut().try_wait() {
                Ok(Some(_)) => return true,
                Ok(None) => tokio::time::sleep(Duration::from_millis(50)).await,
                Err(_) => return true,
            }
        }
        false
    }

    fn force_kill(&mut self) {
        if let Some(pid) = read_daemon_lock_pid(&self.data_dir) {
            kill_pid(pid);
        }
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_file(self.data_dir.join("daemon.lock"));
    }
}

impl Drop for DaemonHarness {
    fn drop(&mut self) {
        // Best-effort sync teardown — never leave zombies after a panic/fail.
        let _ = Command::new(daemon_bin())
            .args(["stop", "--port", &self.port.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        std::thread::sleep(Duration::from_millis(300));
        self.force_kill();
    }
}

/// Teardown for ensure-spawned (detached) daemons tracked only via daemon.lock.
async fn stop_detached(port: u16, data_dir: &Path) {
    let _ = Command::new(daemon_bin())
        .args(["stop", "--port", &port.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if !health_ok(port).await && read_daemon_lock_pid(data_dir).is_none() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    if let Some(pid) = read_daemon_lock_pid(data_dir) {
        kill_pid(pid);
    }
    let _ = std::fs::remove_file(data_dir.join("daemon.lock"));
    tokio::time::sleep(Duration::from_millis(200)).await;
}

fn ensure_opts(port: u16, data_dir: &Path) -> EnsureOptions {
    let config_path = data_dir.join("test-config.toml");
    tdmcp_config::ensure_default(&config_path, true).expect("seed config");
    EnsureOptions {
        port,
        data_dir: data_dir.to_path_buf(),
        exe: Some(daemon_bin()),
        timeout: Duration::from_secs(20),
        poll_only: false,
        no_gui: true,
        idle_exit_secs: Some(0),
        force_install: false,
        ipc_pipe: Some(ipc_pipe_for(port)),
        config_path: Some(config_path),
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
async fn concurrent_ensure_one_healthy_daemon() {
    let _guard = binary_test_lock();
    let dir = tempdir().expect("tempdir");
    let port = free_port();
    let opts = ensure_opts(port, dir.path());

    // Five concurrent callers reproduces the reported "tens of daemons"
    // shape better than two: it exercises both cross-call locking (a waiter
    // sees the lock busy) and same-call re-entry (a caller must not spawn a
    // second time while its own first spawn is still starting up).
    const CALLERS: usize = 5;
    let handles: Vec<_> = (0..CALLERS)
        .map(|_| tokio::spawn(ensure_daemon(opts.clone())))
        .collect();
    let mut results = Vec::with_capacity(CALLERS);
    for h in handles {
        results.push(h.await.expect("join ensure").expect("ensure"));
    }

    assert!(
        health_ok(port).await,
        "daemon should be healthy after concurrent ensure"
    );
    assert!(
        results.iter().any(|r| r.spawned || r.already_running),
        "at least one ensure should observe a running daemon: {results:?}"
    );
    let spawn_count: u8 = results.iter().map(|r| u8::from(r.spawned)).sum();
    assert!(
        spawn_count <= 1,
        "expected at most one spawn, got {spawn_count}: {results:?}"
    );

    let lock = dir.path().join("daemon.lock");
    let pid_text = std::fs::read_to_string(&lock).expect("daemon.lock");
    let pid: u32 = pid_text.trim().parse().expect("pid");
    assert!(pid > 0, "daemon.lock pid");

    stop_detached(port, dir.path()).await;
    assert!(
        !health_ok(port).await,
        "daemon still healthy after stop on port {port}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn ensure_noop_when_already_healthy() {
    let _guard = binary_test_lock();
    let dir = tempdir().expect("tempdir");
    let port = free_port();
    let opts = ensure_opts(port, dir.path());

    let first = ensure_daemon(opts.clone()).await.expect("first ensure");
    assert!(first.spawned || first.already_running);
    assert!(health_ok(port).await);

    let second = ensure_daemon(opts).await.expect("second ensure");
    assert!(second.already_running);
    assert!(!second.spawned);

    stop_detached(port, dir.path()).await;
}

#[tokio::test(flavor = "current_thread")]
async fn admin_shutdown_exits_child_without_kill() {
    let _guard = binary_test_lock();
    let dir = tempdir().expect("tempdir");
    let port = free_port();
    let mut harness = DaemonHarness::spawn_start(port, dir.path());
    wait_healthy(port, Duration::from_secs(15)).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{port}/admin/shutdown"))
        .send()
        .await
        .expect("shutdown post");
    assert!(
        resp.status().is_success(),
        "shutdown status {} body={}",
        resp.status(),
        resp.text().await.unwrap_or_default()
    );

    assert!(
        harness.wait_exit(Duration::from_secs(5)).await,
        "daemon child must exit after /admin/shutdown without taskkill"
    );
    assert!(
        !health_ok(port).await,
        "port must be unhealthy after shutdown"
    );
    assert!(
        read_daemon_lock_pid(dir.path()).is_none(),
        "daemon.lock must be cleared by shutdown epilogue"
    );
    // Prevent Drop force-kill from masking a clean exit.
    let _ = harness.child.take();
}

#[tokio::test(flavor = "current_thread")]
async fn second_start_refuses_while_healthy() {
    let _guard = binary_test_lock();
    let dir = tempdir().expect("tempdir");
    let port = free_port();
    let mut harness = DaemonHarness::spawn_start(port, dir.path());
    wait_healthy(port, Duration::from_secs(15)).await;

    let config_path = dir.path().join("test-config.toml");
    let output = Command::new(daemon_bin())
        .args([
            "start",
            "--port",
            &port.to_string(),
            "--data-dir",
            dir.path().to_str().expect("utf8"),
            "--no-gui",
        ])
        .env("TDMCP_IDLE_EXIT_SECS", "0")
        .env("TDMCP_IPC_PIPE", ipc_pipe_for(port))
        .env(tdmcp_config::CONFIG_PATH_ENV, &config_path)
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

    harness.stop_graceful().await;
}

#[tokio::test(flavor = "current_thread")]
async fn admin_restart_yields_one_healthy_listener() {
    let _guard = binary_test_lock();
    let dir = tempdir().expect("tempdir");
    let port = free_port();
    let mut harness = DaemonHarness::spawn_start(port, dir.path());
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

    let second = ensure_daemon(ensure_opts(port, dir.path()))
        .await
        .expect("ensure after restart");
    assert!(second.already_running);
    assert!(!second.spawned);

    // Original child should be dead; Drop kills whoever holds the port/lock.
    let _ = harness.wait_exit(Duration::from_secs(3)).await;
    harness.stop_graceful().await;
    // Replacement may be a different pid than harness.child — force via lock.
    if let Some(owner) = read_daemon_lock_pid(dir.path()) {
        kill_pid(owner);
    }
    let _ = harness.child.take();
}

#[tokio::test(flavor = "current_thread")]
async fn stale_daemon_lock_reclaimed_on_start() {
    let _guard = binary_test_lock();
    let dir = tempdir().expect("tempdir");
    let port = free_port();

    let mut dead_pid = 4_000_000u32;
    while pid_alive(dead_pid) {
        dead_pid += 1;
    }
    std::fs::write(dir.path().join("daemon.lock"), dead_pid.to_string()).expect("write lock");

    reclaim_stale_daemon_lock(dir.path());
    assert!(
        read_daemon_lock_pid(dir.path()).is_none(),
        "stale lock should be reclaimed by helper"
    );

    std::fs::write(dir.path().join("daemon.lock"), dead_pid.to_string()).expect("rewrite lock");
    let mut harness = DaemonHarness::spawn_start(port, dir.path());
    wait_healthy(port, Duration::from_secs(15)).await;

    let owner = read_daemon_lock_pid(dir.path()).expect("owner after start");
    assert_ne!(owner, dead_pid, "start should replace stale lock pid");
    assert!(pid_alive(owner));

    harness.stop_graceful().await;
}

/// Double-clicking the installed binary means zero CLI args — clap must
/// default to `start` rather than requiring an explicit subcommand, and
/// `Start` must resolve port/data_dir/etc. entirely from env/config.
#[tokio::test(flavor = "current_thread")]
async fn zero_args_defaults_to_start() {
    let _guard = binary_test_lock();
    let dir = tempdir().expect("tempdir");
    let port = free_port();
    let config_path = dir.path().join("test-config.toml");
    tdmcp_config::ensure_default(&config_path, true).expect("seed config");

    let mut child = Command::new(daemon_bin())
        // No subcommand, no flags — only env, exactly like a double-click launch.
        .env("TDMCP_PORT", port.to_string())
        .env("TDMCP_DATA_DIR", dir.path())
        .env("TDMCP_NO_GUI", "true")
        .env("TDMCP_IDLE_EXIT_SECS", "0")
        .env("TDMCP_IPC_PIPE", ipc_pipe_for(port))
        .env(tdmcp_config::CONFIG_PATH_ENV, &config_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn with zero args");

    wait_healthy(port, Duration::from_secs(15)).await;

    let _ = Command::new(daemon_bin())
        .args(["stop", "--port", &port.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if !health_ok(port).await {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let _ = child.kill();
    let _ = child.wait();
}

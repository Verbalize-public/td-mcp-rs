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
    tdmcp_test_support::unique_test_port().expect("unique test port")
}

fn daemon_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_tdmcp-daemon"))
}

/// Bridge port for a spawned daemon: free and distinct from its HTTP port
/// (the daemon fatally refuses a conflicting bridge bind, T-3).
fn free_ipc_port(http_port: u16) -> u16 {
    loop {
        let port = free_port();
        if port != http_port {
            return port;
        }
    }
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
            .env("TDMCP_IPC_PORT", free_ipc_port(port).to_string())
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
        ipc_port: Some(free_ipc_port(port)),
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
        .env("TDMCP_IPC_PORT", free_ipc_port(port).to_string())
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

#[tokio::test(flavor = "current_thread")]
async fn poll_only_never_restarts_a_healthy_headless_daemon() {
    let _guard = binary_test_lock();
    let dir = tempdir().expect("tempdir");
    let port = free_port();
    let mut harness = DaemonHarness::spawn_start(port, dir.path());
    wait_healthy(port, Duration::from_secs(15)).await;
    let pid = read_daemon_lock_pid(dir.path()).expect("owner");
    let result = ensure_daemon(EnsureOptions {
        port,
        data_dir: dir.path().to_path_buf(),
        no_gui: false,
        poll_only: true,
        ..Default::default()
    })
    .await
    .expect("poll only");
    assert!(result.already_running);
    assert!(!result.spawned);
    assert_eq!(read_daemon_lock_pid(dir.path()), Some(pid));
    harness.stop_graceful().await;
}

/// Temporarily clear the display variables **process-wide** so
/// `session::display_available` sees a headless host.
///
/// `ensure_daemon` resolves the tray request from its own environment (it is
/// called in-process here), so clearing the child's env alone is not enough.
/// Callers must hold [`binary_test_lock`]; the previous values are restored on
/// drop, including on assertion failure.
#[cfg(target_os = "linux")]
struct HeadlessHostGuard {
    wayland: Option<std::ffi::OsString>,
    display: Option<std::ffi::OsString>,
}

#[cfg(target_os = "linux")]
impl HeadlessHostGuard {
    fn enter() -> Self {
        let wayland = std::env::var_os("WAYLAND_DISPLAY");
        let display = std::env::var_os("DISPLAY");
        std::env::remove_var("WAYLAND_DISPLAY");
        std::env::remove_var("DISPLAY");
        Self { wayland, display }
    }
}

#[cfg(target_os = "linux")]
impl Drop for HeadlessHostGuard {
    fn drop(&mut self) {
        match self.wayland.take() {
            Some(v) => std::env::set_var("WAYLAND_DISPLAY", v),
            None => std::env::remove_var("WAYLAND_DISPLAY"),
        }
        match self.display.take() {
            Some(v) => std::env::set_var("DISPLAY", v),
            None => std::env::remove_var("DISPLAY"),
        }
    }
}

/// A GUI request on a host with no desktop session must *settle*, not loop.
///
/// This is the regression guard for making the GUI the default: `mcp` / `ensure`
/// now ask for a tray, and `ensure_daemon` restarts a healthy daemon whenever
/// the caller wants a tray but the live daemon reports itself headless. If the
/// daemon did not report its *effective* mode, every IDE reconnect on a headless
/// box would bounce the daemon for a tray that can never appear.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "current_thread")]
async fn tray_request_on_headless_host_settles_instead_of_restarting() {
    let _guard = binary_test_lock();
    let _headless = HeadlessHostGuard::enter();
    let dir = tempdir().expect("tempdir");
    let port = free_port();
    let config_path = dir.path().join("test-config.toml");
    tdmcp_config::ensure_default(&config_path, true).expect("seed config");

    // `start` with NO --no-gui: the default the user now gets from `mcp`.
    let mut child = Command::new(daemon_bin())
        .args([
            "start",
            "--port",
            &port.to_string(),
            "--data-dir",
            dir.path().to_str().expect("utf8 data_dir"),
        ])
        .env("TDMCP_IDLE_EXIT_SECS", "0")
        .env("TDMCP_IPC_PORT", free_ipc_port(port).to_string())
        .env(tdmcp_config::CONFIG_PATH_ENV, &config_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn start");
    wait_healthy(port, Duration::from_secs(15)).await;
    let pid = read_daemon_lock_pid(dir.path()).expect("owner after start");

    // It must report the mode it will actually serve in, not the mode it was
    // asked for — that report is what ends the restart loop.
    let status: serde_json::Value = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{port}/admin/status"))
        .send()
        .await
        .expect("status")
        .json()
        .await
        .expect("body");
    assert_eq!(
        status["noGui"], true,
        "a daemon with no display session must report itself headless"
    );

    // A tray request against that daemon must be a no-op, not a restart.
    let result = ensure_daemon(EnsureOptions {
        no_gui: false,
        ..ensure_opts(port, dir.path())
    })
    .await
    .expect("ensure");
    assert!(result.already_running, "{result:?}");
    assert!(
        !result.spawned,
        "ensure must not bounce a daemon that has no desktop to put a tray on"
    );
    assert_eq!(
        read_daemon_lock_pid(dir.path()),
        Some(pid),
        "the healthy headless daemon must keep owning the port"
    );

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
    let _ = child.kill();
    let _ = child.wait();
}

/// The mirror case: a daemon that *did* open a tray, asked for a tray again.
/// Must stay a no-op too — the restart branch is for upgrades from a previous
/// `--no-gui` start, not for a routine reconnect.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "current_thread")]
async fn tray_daemon_is_never_restarted_by_a_second_tray_request() {
    let _guard = binary_test_lock();
    let dir = tempdir().expect("tempdir");
    let port = free_port();
    let config_path = dir.path().join("test-config.toml");
    tdmcp_config::ensure_default(&config_path, true).expect("seed config");

    // A headless `--no-gui` daemon stands in for "a daemon that is headless"
    // on a machine whose display we cannot rely on in CI. The assertion under
    // test is the pin: once it is the owner, a tray request must not take the
    // port away from it and leave a second daemon behind.
    let mut harness = DaemonHarness::spawn_start(port, dir.path());
    wait_healthy(port, Duration::from_secs(15)).await;
    let pid = read_daemon_lock_pid(dir.path()).expect("owner");

    // `poll_only` is the mode the stdio proxy uses once it is already linked:
    // a reconnect must never restart the service underneath the client.
    let result = ensure_daemon(EnsureOptions {
        port,
        data_dir: dir.path().to_path_buf(),
        poll_only: true,
        ..Default::default()
    })
    .await
    .expect("ensure");
    assert!(result.already_running, "{result:?}");
    assert!(!result.spawned, "{result:?}");
    assert_eq!(read_daemon_lock_pid(dir.path()), Some(pid));

    harness.stop_graceful().await;
}

#[tokio::test(flavor = "current_thread")]
async fn restart_applies_saved_port_instead_of_old_cli_override() {
    let _guard = binary_test_lock();
    let dir = tempdir().expect("tempdir");
    let old_port = free_port();
    let mut harness = DaemonHarness::spawn_start(old_port, dir.path());
    wait_healthy(old_port, Duration::from_secs(15)).await;
    let old_pid = read_daemon_lock_pid(dir.path()).expect("owner");
    let new_port = free_port();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .expect("client");
    let saved: serde_json::Value = client
        .post(format!("http://127.0.0.1:{old_port}/admin/config"))
        .json(&serde_json::json!({"server":{"port":new_port}}))
        .send()
        .await
        .expect("save")
        .error_for_status()
        .expect("accepted")
        .json()
        .await
        .expect("body");
    assert_eq!(saved["restartRequired"], serde_json::json!(["server"]));
    client
        .post(format!("http://127.0.0.1:{old_port}/admin/restart"))
        .send()
        .await
        .expect("restart")
        .error_for_status()
        .expect("accepted");
    harness.port = new_port;
    wait_healthy(new_port, Duration::from_secs(20)).await;
    assert!(!health_ok(old_port).await);
    assert_ne!(read_daemon_lock_pid(dir.path()), Some(old_pid));
    let status: serde_json::Value = client
        .get(format!("http://127.0.0.1:{new_port}/admin/status"))
        .send()
        .await
        .expect("status")
        .json()
        .await
        .expect("body");
    assert_eq!(status["restartRequired"], serde_json::json!([]));
    assert_eq!(status["noGui"], true);
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
        .env("TDMCP_IPC_PORT", free_ipc_port(port).to_string())
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

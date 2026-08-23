//! Upsert the long-lived daemon: health → lock → detached spawn → poll.
//!
//! Port of td-mcp `ensureHub` for the Rust control plane.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use tracing::{debug, info, warn};

use crate::install;

/// Options for [`ensure_daemon`].
#[derive(Debug, Clone)]
pub struct EnsureOptions {
    /// Listen port (default 9860).
    pub port: u16,
    /// Data directory.
    pub data_dir: PathBuf,
    /// Binary to spawn as `start` (defaults to current exe).
    pub exe: Option<PathBuf>,
    /// Max wait for health after spawn.
    pub timeout: Duration,
    /// When true, never spawn — only poll health.
    pub poll_only: bool,
    /// When true, spawn with `--no-gui` (headless; used by tests / CI).
    pub no_gui: bool,
    /// Optional child `TDMCP_IDLE_EXIT_SECS`. `None` → child uses TOML config /
    /// idle defaults; tests pass `Some(0)` to disable idle exit.
    pub idle_exit_secs: Option<u64>,
    /// When true, re-extract embedded bridge/catalog/tox even if already current.
    pub force_install: bool,
    /// Child `TDMCP_IPC_PIPE` override (tests isolate from the live TD pipe).
    pub ipc_pipe: Option<String>,
    /// Child `TDMCP_CONFIG_PATH` override (tests isolate from the user config).
    pub config_path: Option<PathBuf>,
}

impl Default for EnsureOptions {
    fn default() -> Self {
        Self {
            port: tdmcp_config::DEFAULT_PORT,
            data_dir: install::default_data_dir(),
            exe: None,
            timeout: Duration::from_secs(15),
            poll_only: false,
            no_gui: false,
            idle_exit_secs: None,
            force_install: false,
            ipc_pipe: None,
            config_path: None,
        }
    }
}

/// Result of an ensure pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnsureResult {
    /// Base URL of the healthy daemon.
    pub base_url: String,
    /// Whether the daemon was already healthy before this call.
    pub already_running: bool,
    /// Whether this call spawned a new process.
    pub spawned: bool,
}

/// Health-check `:port`; if down, lock + spawn detached `tdmcp-daemon start`.
///
/// When the caller wants a tray (`no_gui = false`) but a healthy **headless**
/// daemon is already listening, shut it down and respawn with the tray so a
/// leftover `--no-gui` debug start cannot pin the port forever.
pub async fn ensure_daemon(opts: EnsureOptions) -> Result<EnsureResult> {
    let base_url = format!("http://127.0.0.1:{}", opts.port);
    install::ensure_installed(&opts.data_dir, opts.force_install)?;

    if health_ok(opts.port).await {
        if !opts.no_gui && daemon_is_headless(opts.port).await {
            info!(
                port = opts.port,
                "ensure: healthy headless daemon but tray requested — restarting with tray"
            );
            let _ = request_shutdown(opts.port).await;
            wait_until_unhealthy(opts.port, Duration::from_secs(5)).await;
        } else {
            return Ok(EnsureResult {
                base_url,
                already_running: true,
                spawned: false,
            });
        }
    }
    if opts.poll_only {
        bail!("ensure: daemon not healthy at {base_url} (pollOnly)");
    }

    reclaim_stale_daemon_lock(&opts.data_dir);

    let deadline = Instant::now() + opts.timeout;
    let mut spawned = false;
    let exe = resolve_exe(opts.exe.as_ref())?;
    // Held from the moment we spawn until health succeeds (or we time out) so
    // no other waiter — including a later iteration of *this* loop — can win
    // the lock and spawn a second daemon while the first is still starting.
    let mut held_lock: Option<LockGuard> = None;

    while Instant::now() < deadline {
        if health_ok(opts.port).await {
            return Ok(EnsureResult {
                base_url,
                already_running: !spawned,
                spawned,
            });
        }

        if spawned {
            // Already spawned this call; keep holding the lock and just wait
            // for the child to come up instead of re-acquiring and spawning again.
            tokio::time::sleep(Duration::from_millis(200)).await;
            continue;
        }

        match try_acquire_lock(&opts.data_dir) {
            Ok(Some(guard)) => {
                // Re-check under lock — another waiter may have won the race.
                if health_ok(opts.port).await {
                    drop(guard);
                    return Ok(EnsureResult {
                        base_url,
                        already_running: true,
                        spawned: false,
                    });
                }
                spawn_detached(
                    &exe,
                    opts.port,
                    &opts.data_dir,
                    opts.no_gui,
                    opts.idle_exit_secs,
                    opts.ipc_pipe.as_deref(),
                    opts.config_path.as_deref(),
                )?;
                spawned = true;
                held_lock = Some(guard);
            }
            Ok(None) => {
                // Another ensure holds the lock — wait.
                debug!("ensure lock busy; waiting");
            }
            Err(e) => {
                warn!(error = %e, "ensure lock acquire failed");
            }
        }

        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    drop(held_lock);

    if health_ok(opts.port).await {
        return Ok(EnsureResult {
            base_url,
            already_running: !spawned,
            spawned,
        });
    }
    bail!("ensure: timed out waiting for {base_url} (spawned={spawned})")
}

/// GET `/admin/status` and return whether the live daemon is headless.
async fn daemon_is_headless(port: u16) -> bool {
    let url = format!("http://127.0.0.1:{port}/admin/status");
    match tokio::time::timeout(Duration::from_millis(800), crate::http_util::get_json(&url)).await {
        Ok(Ok(v)) => v.get("noGui").and_then(|x| x.as_bool()).unwrap_or(false),
        _ => false,
    }
}

/// POST `/admin/shutdown` on `port` (best-effort; a missing daemon is fine).
pub async fn request_shutdown(port: u16) -> Result<()> {
    let url = format!("http://127.0.0.1:{port}/admin/shutdown");
    crate::http_util::post_empty(&url).await
}

/// Wait until no daemon answers health on `port`, up to `budget`.
pub async fn wait_until_unhealthy(port: u16, budget: Duration) {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        if !health_ok(port).await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Version reported by `/admin/status` on `port`, if a daemon answers.
pub async fn running_version(port: u16) -> Option<String> {
    let url = format!("http://127.0.0.1:{port}/admin/status");
    match tokio::time::timeout(Duration::from_millis(800), crate::http_util::get_json(&url)).await {
        Ok(Ok(v)) => v.get("version").and_then(|x| x.as_str()).map(String::from),
        _ => None,
    }
}

/// GET `/mcp/health` and require JSON `ok: true`.
pub async fn health_ok(port: u16) -> bool {
    let url = format!("http://127.0.0.1:{port}/mcp/health");
    match tokio::time::timeout(Duration::from_millis(800), crate::http_util::get_json(&url)).await {
        Ok(Ok(v)) => v.get("ok").and_then(|x| x.as_bool()) == Some(true),
        Ok(Err(_)) | Err(_) => false,
    }
}

fn resolve_exe(override_exe: Option<&PathBuf>) -> Result<PathBuf> {
    if let Some(p) = override_exe {
        return Ok(p.clone());
    }
    std::env::current_exe().context("resolve current_exe for ensure spawn")
}

/// Detach the child so the console does not flash (Windows) and stdin is closed.
///
/// `CREATE_NO_WINDOW` only when headless — it suppresses the notification-area
/// tray icon. GUI restarts must use `DETACHED_PROCESS` alone.
///
/// On Unix, append stdout/stderr to `{data_dir}/daemon.log` when `data_dir` is
/// provided so Cursor-spawned daemons leave a trail; otherwise null both.
pub fn configure_detached_spawn(cmd: &mut Command, no_gui: bool) {
    configure_detached_spawn_with_log(cmd, no_gui, None);
}

/// Like [`configure_detached_spawn`], optionally appending logs under `data_dir`.
pub fn configure_detached_spawn_with_log(cmd: &mut Command, no_gui: bool, data_dir: Option<&Path>) {
    cmd.stdin(Stdio::null());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let flags = if no_gui {
            DETACHED_PROCESS | CREATE_NO_WINDOW
        } else {
            DETACHED_PROCESS
        };
        cmd.creation_flags(flags);
        cmd.stdout(Stdio::null()).stderr(Stdio::null());
        let _ = data_dir;
    }
    #[cfg(not(windows))]
    {
        let _ = no_gui;
        // New process group so the child is not tied to the Cursor/`mcp`
        // session and survives parent exit without SIGHUP surprises.
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
        if let Some(dir) = data_dir {
            if let Err(e) = attach_unix_daemon_log(cmd, dir) {
                warn!(error = %e, "ensure: could not attach daemon.log");
                cmd.stdout(Stdio::null()).stderr(Stdio::null());
            }
        } else {
            cmd.stdout(Stdio::null()).stderr(Stdio::null());
        }
    }
}

#[cfg(unix)]
fn attach_unix_daemon_log(cmd: &mut Command, data_dir: &Path) -> std::io::Result<()> {
    fs::create_dir_all(data_dir)?;
    let log_path = data_dir.join("daemon.log");
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    match file.try_clone() {
        Ok(err_file) => {
            cmd.stdout(Stdio::from(file)).stderr(Stdio::from(err_file));
        }
        Err(_) => {
            cmd.stdout(Stdio::from(file)).stderr(Stdio::null());
        }
    }
    Ok(())
}

fn spawn_detached(
    exe: &Path,
    port: u16,
    data_dir: &Path,
    no_gui: bool,
    idle_exit_secs: Option<u64>,
    ipc_pipe: Option<&str>,
    config_path: Option<&Path>,
) -> Result<()> {
    info!(
        exe = %exe.display(),
        port,
        data_dir = %data_dir.display(),
        no_gui,
        idle_exit_secs,
        ipc_pipe,
        config_path = ?config_path.map(|p| p.display().to_string()),
        "ensure: spawning detached daemon"
    );
    let mut cmd = Command::new(exe);
    cmd.arg("start")
        .arg("--port")
        .arg(port.to_string())
        .arg("--data-dir")
        .arg(data_dir);
    if no_gui {
        cmd.arg("--no-gui");
    }
    // Production keep_alive / idle timeout come from the TOML config the child
    // loads. Optional env override remains for integration tests.
    if let Some(secs) = idle_exit_secs {
        cmd.env("TDMCP_IDLE_EXIT_SECS", secs.to_string());
    }
    if let Some(pipe) = ipc_pipe {
        cmd.env("TDMCP_IPC_PIPE", pipe);
    }
    if let Some(cfg) = config_path {
        cmd.env(tdmcp_config::CONFIG_PATH_ENV, cfg);
    }
    configure_detached_spawn_with_log(&mut cmd, no_gui, Some(data_dir));
    let mut child = cmd
        .spawn()
        .with_context(|| format!("spawn detached {} start --port {port}", exe.display()))?;
    let child_pid = child.id();
    info!(child_pid, "ensure: detached daemon spawned");
    // Reap on a side thread. Dropping `Child` without `wait` leaves a zombie
    // under the long-lived `mcp` parent; on macOS/BSD `kill -0` still sees
    // zombies, so a crashed GUI child would pin a stale `daemon.lock`.
    // The child itself must outlive this ensure call — only wait for exit.
    std::thread::Builder::new()
        .name("tdmcp-ensure-reap".into())
        .spawn(move || match child.wait() {
            Ok(status) if !status.success() => {
                warn!(?status, child_pid, "detached daemon exited non-zero");
            }
            Ok(_) => {}
            Err(e) => warn!(error = %e, child_pid, "failed to reap detached daemon"),
        })
        .context("spawn ensure reap thread")?;
    Ok(())
}

struct LockGuard {
    path: PathBuf,
    _file: File,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn lock_path(data_dir: &Path) -> PathBuf {
    data_dir.join("ensure.lock")
}

fn try_acquire_lock(data_dir: &Path) -> Result<Option<LockGuard>> {
    fs::create_dir_all(data_dir)?;
    let path = lock_path(data_dir);
    // Stale ensure.lock from a dead process: if older than 30s and no health, reclaim.
    if path.exists() {
        if let Ok(meta) = fs::metadata(&path) {
            if let Ok(modified) = meta.modified() {
                if modified.elapsed().unwrap_or_default() > Duration::from_secs(30) {
                    warn!("reclaiming stale ensure.lock");
                    let _ = fs::remove_file(&path);
                }
            }
        }
    }
    match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(mut f) => {
            let _ = writeln!(f, "{}", std::process::id());
            Ok(Some(LockGuard { path, _file: f }))
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(None),
        Err(e) => Err(e).context("create ensure.lock"),
    }
}

/// Path to the long-lived owner pid file.
pub fn daemon_lock_path(data_dir: &Path) -> PathBuf {
    data_dir.join("daemon.lock")
}

/// Read the pid stored in `daemon.lock`, if any.
pub fn read_daemon_lock_pid(data_dir: &Path) -> Option<u32> {
    let text = fs::read_to_string(daemon_lock_path(data_dir)).ok()?;
    text.trim().parse().ok()
}

/// If `daemon.lock` names a dead pid, remove it.
pub fn reclaim_stale_daemon_lock(data_dir: &Path) {
    let path = daemon_lock_path(data_dir);
    let Ok(text) = fs::read_to_string(&path) else {
        return;
    };
    let Ok(pid) = text.trim().parse::<u32>() else {
        let _ = fs::remove_file(&path);
        return;
    };
    if !pid_alive(pid) {
        warn!(pid, "reclaiming stale daemon.lock");
        let _ = fs::remove_file(&path);
    }
}

/// Refuse to start when another live, healthy owner already holds the port.
///
/// Stale locks are reclaimed first. A live pid with a healthy listener is an
/// exclusive conflict; a live pid without health (e.g. restart handoff after
/// the lock was cleared) is left for the bind-retry loop.
pub async fn refuse_if_daemon_owned(data_dir: &Path, port: u16) -> Result<()> {
    reclaim_stale_daemon_lock(data_dir);
    let Some(pid) = read_daemon_lock_pid(data_dir) else {
        return Ok(());
    };
    if pid == std::process::id() {
        return Ok(());
    }
    if pid_alive(pid) && health_ok(port).await {
        bail!(
            "daemon already running at pid {pid} on port {port}; use `tdmcp-daemon stop` or `/admin/restart`"
        );
    }
    Ok(())
}

/// Whether `pid` appears to be alive on this OS.
pub fn pid_alive(pid: u32) -> bool {
    #[cfg(windows)]
    {
        // Avoid unsafe OpenProcess; tasklist is good enough for lock reclaim.
        let output = Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output();
        match output {
            Ok(o) => {
                let s = String::from_utf8_lossy(&o.stdout);
                s.contains(&pid.to_string())
            }
            Err(_) => true, // unknown → don't reclaim
        }
    }
    #[cfg(target_os = "linux")]
    {
        Path::new(&format!("/proc/{pid}")).exists()
    }
    #[cfg(all(unix, not(target_os = "linux")))]
    {
        // macOS/BSD have no /proc by default. `kill -0` is the portable
        // existence probe without `unsafe` (workspace forbids unsafe_code).
        // Exit 0 → alive (or zombie still in the table); non-zero → gone.
        // Unknown command failure → assume alive so we do not reclaim a live lock.
        match Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
        {
            Ok(status) => status.success(),
            Err(_) => true,
        }
    }
}

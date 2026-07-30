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
}

impl Default for EnsureOptions {
    fn default() -> Self {
        Self {
            port: 9860,
            data_dir: install::default_data_dir(),
            exe: None,
            timeout: Duration::from_secs(15),
            poll_only: false,
            no_gui: false,
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
pub async fn ensure_daemon(opts: EnsureOptions) -> Result<EnsureResult> {
    let base_url = format!("http://127.0.0.1:{}", opts.port);
    install::ensure_installed(&opts.data_dir)?;

    if health_ok(opts.port).await {
        return Ok(EnsureResult {
            base_url,
            already_running: true,
            spawned: false,
        });
    }
    if opts.poll_only {
        bail!("ensure: daemon not healthy at {base_url} (pollOnly)");
    }

    reclaim_stale_daemon_lock(&opts.data_dir);

    let deadline = Instant::now() + opts.timeout;
    let mut spawned = false;
    let exe = resolve_exe(opts.exe.as_ref())?;

    while Instant::now() < deadline {
        if health_ok(opts.port).await {
            return Ok(EnsureResult {
                base_url,
                already_running: !spawned,
                spawned,
            });
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
                spawn_detached(&exe, opts.port, &opts.data_dir, opts.no_gui)?;
                spawned = true;
                drop(guard);
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

    if health_ok(opts.port).await {
        return Ok(EnsureResult {
            base_url,
            already_running: !spawned,
            spawned,
        });
    }
    bail!("ensure: timed out waiting for {base_url} (spawned={spawned})")
}

/// GET `/mcp/health` and require JSON `ok: true`.
pub async fn health_ok(port: u16) -> bool {
    match http_get_health(port).await {
        Ok(body) => {
            // Body may include HTTP headers; scan for the JSON payload.
            let json = body.rfind('{').map(|i| &body[i..]).unwrap_or(body.as_str());
            match serde_json::from_str::<serde_json::Value>(json) {
                Ok(v) => v.get("ok").and_then(|x| x.as_bool()) == Some(true),
                Err(_) => false,
            }
        }
        Err(_) => false,
    }
}

async fn http_get_health(port: u16) -> Result<String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
    let req = "GET /mcp/health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n";
    stream.write_all(req.as_bytes()).await?;
    let mut buf = Vec::new();
    tokio::time::timeout(Duration::from_millis(800), stream.read_to_end(&mut buf))
        .await
        .context("health read timeout")??;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

fn resolve_exe(override_exe: Option<&PathBuf>) -> Result<PathBuf> {
    if let Some(p) = override_exe {
        return Ok(p.clone());
    }
    std::env::current_exe().context("resolve current_exe for ensure spawn")
}

fn spawn_detached(exe: &Path, port: u16, data_dir: &Path, no_gui: bool) -> Result<()> {
    info!(
        exe = %exe.display(),
        port,
        data_dir = %data_dir.display(),
        no_gui,
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
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS always. CREATE_NO_WINDOW only when headless — tray needs a
        // normal process (CREATE_NO_WINDOW suppresses the notification-area icon).
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let flags = if no_gui {
            DETACHED_PROCESS | CREATE_NO_WINDOW
        } else {
            DETACHED_PROCESS
        };
        cmd.creation_flags(flags);
    }
    let child = cmd
        .spawn()
        .with_context(|| format!("spawn detached {} start --port {port}", exe.display()))?;
    info!(child_pid = child.id(), "ensure: detached daemon spawned");
    // Intentionally leak/drop without wait — child must outlive this process.
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
    #[cfg(unix)]
    {
        Path::new(&format!("/proc/{pid}")).exists()
    }
}

//! Lifecycle: spawn TouchDesigner deterministically, kill by known pid.
//!
//! Spawn law (proposal §3.6): the pid is registered pre-handshake
//! (`Starting`) and a DETACHED waiter task owns its lifecycle — client
//! disconnects cannot orphan rows; handshake heals Starting→Connected.
//! Surface-only dialogs: popups ride along in every payload, never
//! auto-dismissed.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

/// Args for `spawn_td`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpawnTdParams {
    /// Explicit TouchDesigner.exe (overrides installId/config).
    #[serde(default)]
    pub exe_path: Option<String>,
    /// Install id from `td_installs` (default = newest usable).
    #[serde(default)]
    pub install_id: Option<String>,
    /// Project file to open at start.
    #[serde(default)]
    pub project_path: Option<String>,
    /// Extra CLI args passed through.
    #[serde(default)]
    pub args: Vec<String>,
    /// Handshake wait budget (ms).
    #[serde(default = "default_wait")]
    pub wait_timeout_ms: u64,
}

fn default_wait() -> u64 {
    60_000
}

/// Args for `kill_td`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KillTdParams {
    /// Target pid.
    pub pid: u32,
    /// `graceful` (WM_CLOSE + grace window) or `force` (TerminateProcess).
    #[serde(default)]
    pub mode: KillMode,
    /// Grace window for graceful mode (ms).
    #[serde(default = "default_grace")]
    pub grace_ms: u64,
}

fn default_grace() -> u64 {
    5_000
}

/// Kill mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum KillMode {
    /// Post WM_CLOSE and wait; surfaces popups if it lingers.
    #[default]
    Graceful,
    /// Terminate unconditionally.
    Force,
}
use tokio::sync::Mutex as AsyncMutex;

use tdmcp_config::ConfigFile;
use tdmcp_core::{PidRegistry, SpawnRecord};
use tdmcp_projectio::resolve;

/// Resolved install for a spawn/pack operation.
pub struct ResolvedInstall {
    /// TouchDesigner.exe to launch.
    pub exe: PathBuf,
    /// Versioned install dir name (TouchDesigner.<build>).
    pub root_name: Option<String>,
}

/// Tool-layer failure carrying its diagnostic code (mirrors project_unpack).
#[derive(Debug)]
pub struct CodedError {
    /// Human-readable failure.
    pub message: String,
    /// Stable diagnostic code.
    pub code: &'static str,
}

fn resolve_install(
    cfg: &ConfigFile,
    exe_path: Option<&str>,
    install_id: Option<&str>,
) -> Result<ResolvedInstall, CodedError> {
    // Explicit exe wins; then config td_exe pin; then scan newest-complete.
    if let Some(p) = exe_path {
        let p = PathBuf::from(p);
        return Ok(ResolvedInstall {
            exe: p.clone(),
            root_name: None,
        });
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(exe) = &cfg.official_tools.td_exe {
        candidates.push(exe.clone());
    }
    for root in resolve::default_scan_roots(&resolve::std_env) {
        for exe in resolve::scan_install_exes(&root) {
            if let Some(want) = install_id {
                let id_ok = exe
                    .parent()
                    .and_then(Path::parent)
                    .and_then(|r| r.file_name())
                    .and_then(std::ffi::OsStr::to_str)
                    .is_some_and(|n| n == want);
                if !id_ok {
                    continue;
                }
            }
            let info = resolve::inspect_install(&exe);
            if info.toeexpand.is_some() && info.toecollapse.is_some() {
                candidates.push(exe);
            }
        }
    }
    let exe = candidates.into_iter().next().ok_or_else(|| CodedError {
        message: "no complete TouchDesigner installation found".into(),
        code: "spawn.exe_incomplete",
    })?;
    let root_name = exe
        .parent()
        .and_then(Path::parent)
        .and_then(|r| r.file_name())
        .and_then(std::ffi::OsStr::to_str)
        .map(str::to_string);
    Ok(ResolvedInstall { exe, root_name })
}

/// Spawn + register + wait for THAT pid's handshake. Detached waiter owns the
/// registry row until terminal state; caller awaits the final payload.
pub async fn spawn_td(
    registry: &Arc<AsyncMutex<PidRegistry>>,
    cfg: &ConfigFile,
    exe_path: Option<&str>,
    install_id: Option<&str>,
    project_path: Option<&str>,
    extra_args: &[String],
    wait_timeout_ms: u64,
) -> Result<Value, CodedError> {
    let install = resolve_install(cfg, exe_path, install_id)?;
    // Refuse stub installs (probe lesson): require toeexpand beside exe.
    let info = resolve::inspect_install(&install.exe);
    if info.toeexpand.is_none() || info.toecollapse.is_none() {
        return Err(CodedError {
            message: format!(
                "install at {} lacks official tools — refusing to start",
                install.exe.display()
            ),
            code: "spawn.exe_incomplete",
        });
    }

    let mut cmd = Command::new(&install.exe);
    if let Some(pp) = project_path {
        cmd.arg(pp);
    }
    for a in extra_args {
        cmd.arg(a);
    }
    // No shell anywhere; inherit no handles we care about.
    let child = cmd.spawn().map_err(|e| CodedError {
        message: format!("spawn failed: {e}"),
        code: "spawn.spawn_failed",
    })?;
    let pid = child.id();
    let started_at = chrono::Utc::now();
    {
        let mut reg = registry.lock().await;
        let ok = reg.register_starting(
            pid,
            SpawnRecord {
                started_at,
                exe_path: install.exe.to_string_lossy().into_owned(),
                expected_project: project_path.map(str::to_string),
            },
        );
        if !ok {
            return Err(CodedError {
                message: format!("pid {pid} already registered connected"),
                code: "spawn.spawn_failed",
            });
        }
    }
    tracing::info!(pid, exe = %install.exe.display(), "TouchDesigner spawned");

    // Detached waiter: survives client disconnect; always drives the row to a
    // terminal state (Connected via handshake heal / removed on death).
    let (tx, rx) = tokio::sync::oneshot::channel::<Value>();
    let reg2 = registry.clone();
    let shared_dialogs = crate::dialogs::get().cloned();
    tokio::spawn(async move {
        let mut child = child;
        let deadline = Instant::now() + Duration::from_millis(wait_timeout_ms.max(1000));
        loop {
            if let Ok(Some(code)) = child.try_wait() {
                reg2.lock().await.remove_starting(pid);
                let _ = tx.send(json!({
                    "ok": false,
                    "outcome": "exited_early",
                    "exitCode": code.code(),
                    "pid": pid,
                }));
                return;
            }
            {
                let reg = reg2.lock().await;
                if let Some(e) = reg.get(pid) {
                    if e.bridge == tdmcp_core::BridgeStatus::Connected {
                        let payload = json!({
                            "ok": true,
                            "pid": pid,
                            "handshake": {
                                "title": e.process.title,
                                "toePath": e.process.toe_path,
                            },
                            "startupDialogs": shared_dialogs.as_ref().and_then(|d| {
                                d.snapshots.lock().unwrap_or_else(|p| p.into_inner()).get(&pid).cloned()
                            }),
                        });
                        let _ = tx.send(payload);
                        return; // Connected row stays; normal ownership from here.
                    }
                } else {
                    // Row vanished (external cleanup) — nothing to own.
                    let _ = tx.send(json!({"ok": false, "outcome": "wait_timeout", "pid": pid}));
                    return;
                }
            }
            if Instant::now() >= deadline {
                let _ = tx.send(json!({
                    "ok": false,
                    "outcome": "wait_timeout",
                    "pid": pid,
                    "stillAlive": child.try_wait().map(|r| r.is_none()).unwrap_or(true),
                    "startupDialogs": shared_dialogs.as_ref().and_then(|d| {
                        d.snapshots.lock().unwrap_or_else(|p| p.into_inner()).get(&pid).cloned()
                    }),
                }));
                return;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    });

    match rx.await {
        Ok(v) => Ok(v),
        Err(_) => Err(CodedError {
            message: "spawn waiter dropped unexpectedly".into(),
            code: "spawn.wait_timeout",
        }),
    }
}

/// Graceful-then-force kill of a known TD pid. Returns the wire payload.
pub async fn kill_td(
    registry: &Arc<AsyncMutex<PidRegistry>>,
    source: Option<&dyn tdmcp_core::DialogSource>,
    pid: u32,
    mode: KillMode,
    grace_ms: u64,
) -> Result<Value, CodedError> {
    // Known-pid check: registry membership OR image basename is TD.
    let in_registry = registry.lock().await.get(pid).is_some();
    if !in_registry {
        let is_td = source
            .and_then(|s| s.process_image_name(pid))
            .is_some_and(|n| n.eq_ignore_ascii_case("TouchDesigner.exe"));
        if !is_td {
            return Err(CodedError {
                message: format!("pid {pid} is not a known TD process"),
                code: "kill.not_td_pid",
            });
        }
    }
    if mode == KillMode::Graceful {
        #[cfg(windows)]
        let posted = tdmcp_dialogs::sys::windows::close_pid_windows(pid);
        #[cfg(not(windows))]
        let posted = 0;
        let deadline = Instant::now() + Duration::from_millis(grace_ms.max(500));
        loop {
            let alive = source.is_some_and(|s| s.process_alive(pid));
            if !alive {
                return Ok(json!({ "ok": true, "pid": pid, "how": "graceful" }));
            }
            if Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        let open_popups = source.map(|s| s.snapshot(pid).popups.len()).unwrap_or(0);
        return Err(CodedError {
            message: format!(
                "still alive after graceful close ({posted} window(s) posted); open popups: {open_popups}"
            ),
            code: "kill.graceful_timeout",
        });
    }
    #[cfg(windows)]
    {
        if tdmcp_dialogs::sys::windows::terminate_process(pid) {
            return Ok(json!({ "ok": true, "pid": pid, "how": "force" }));
        }
        Err(CodedError {
            message: format!("TerminateProcess failed for {pid}"),
            code: "kill.access_denied",
        })
    }
    #[cfg(not(windows))]
    {
        Err(CodedError {
            message: "force kill unsupported here",
            code: "kill.access_denied",
        })
    }
}

impl KillTdParams {
    /// Wire-friendly mode string for the lifecycle fn.
    pub fn mode_str(&self) -> &'static str {
        match self.mode {
            KillMode::Graceful => "graceful",
            KillMode::Force => "force",
        }
    }
}

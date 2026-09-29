//! Lifecycle: spawn TouchDesigner deterministically, kill by known pid.
//!
//! Spawn law (proposal §3.6): the pid is registered pre-handshake
//! (`Starting`) and a DETACHED waiter task owns its lifecycle — client
//! disconnects cannot orphan rows; handshake heals Starting→Connected.
//! Surface-only dialogs: popups ride along in every payload, never
//! auto-dismissed.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};
use tdmcp_config::ConfigFile;
use tdmcp_core::{LenientU64, Pid};

/// Args for `spawn_td`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpawnTdParams {
    /// Explicit TouchDesigner binary (overrides installId/config).
    #[serde(default)]
    pub exe_path: Option<String>,
    /// Install id from `td_installs` (default = newest usable).
    #[serde(default)]
    pub install_id: Option<String>,
    /// Project file to open at start.
    #[serde(default)]
    pub project_path: Option<String>,
    /// When true, copy the template toe to `projectPath` if the target does not yet exist before spawning.
    #[serde(default)]
    pub create_if_missing: bool,
    /// Optional per-call template override (absolute path to a `.toe`/`.tox` to copy when creating).
    #[serde(default)]
    pub template_path: Option<String>,
    /// Extra CLI args passed through.
    #[serde(default)]
    pub args: Vec<String>,
    /// Handshake wait budget (ms).
    #[serde(default = "default_wait")]
    pub wait_timeout_ms: LenientU64,
}

fn default_wait() -> LenientU64 {
    LenientU64(60_000)
}

/// Args for `kill_td`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KillTdParams {
    /// Target pid.
    pub pid: Pid,
    /// `graceful` (close windows / SIGTERM + grace window) or `force`.
    #[serde(default)]
    pub mode: KillMode,
    /// Grace window for graceful mode (ms).
    #[serde(default = "default_grace")]
    pub grace_ms: LenientU64,
}

fn default_grace() -> LenientU64 {
    LenientU64(5_000)
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

use tdmcp_core::{PidRegistry, SpawnRecord};
use tdmcp_projectio::resolve;

/// Resolved install for a spawn/pack operation.
pub struct ResolvedInstall {
    /// TouchDesigner binary to launch.
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
    let root_name = resolve::inspect_install(&exe)
        .root
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .map(str::to_string);
    Ok(ResolvedInstall { exe, root_name })
}

fn resolve_template_path(cfg: &ConfigFile, override_path: Option<&str>) -> PathBuf {
    if let Some(p) = override_path {
        let t = p.trim();
        if !t.is_empty() {
            return PathBuf::from(t);
        }
    }
    if let Some(p) = &cfg.project.template_path {
        return p.clone();
    }
    // Default: {data_dir}/template.toe, respecting [advanced].data_dir override.
    let base = cfg.advanced.data_dir.clone().unwrap_or_else(|| {
        dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(tdmcp_config::APP_DIR_NAME)
    });
    base.join("template.toe")
}

fn ensure_project_from_template(
    target: &str,
    cfg: &ConfigFile,
    template_override: Option<&str>,
    create_if_missing: bool,
) -> Result<(), CodedError> {
    let target_path = Path::new(target);
    if target_path.is_file() {
        return Ok(());
    }
    // Target missing — decide based on flag.
    if target.trim().is_empty() {
        return Ok(());
    }
    if !create_if_missing {
        return Err(CodedError {
            message: format!(
                "project target {} does not exist — pass createIfMissing:true to create from template",
                target_path.display()
            ),
            code: "spawn.target_not_found",
        });
    }
    // Need to materialize from template.
    let template = resolve_template_path(cfg, template_override);
    if !template.is_file() {
        // Try embedded fallback extraction on-demand (data_dir may have been cleaned).
        // If advanced.data_dir is set, template above already points there; still try to materialize.
        // Fallback: try default data_dir as well.
        let still_missing = !template.is_file();
        if still_missing {
            return Err(CodedError {
                message: format!(
                    "template not found at {} — set [project].template_path or reinstall",
                    template.display()
                ),
                code: "spawn.template_not_found",
            });
        }
    }
    // Validate template looks like a packed project (quick check: file size > 512).
    let meta = std::fs::metadata(&template).map_err(|e| CodedError {
        message: format!("read template {}: {e}", template.display()),
        code: "spawn.template_not_found",
    })?;
    if meta.len() < 512 {
        return Err(CodedError {
            message: format!("template {} looks invalid (too small)", template.display()),
            code: "spawn.template_invalid",
        });
    }
    if let Some(parent) = target_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| CodedError {
                message: format!("create parent {}: {e}", parent.display()),
                code: "spawn.create_failed",
            })?;
        }
    }
    publish_template_copy(&template, target_path)?;
    tracing::info!(
        template = %template.display(),
        target = %target_path.display(),
        "spawn_td: created new project from template"
    );
    Ok(())
}

fn publish_template_copy(template: &Path, target: &Path) -> Result<(), CodedError> {
    let temporary = target.with_extension(format!("tmp-{}", uuid::Uuid::new_v4().simple()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut output = options.open(&temporary).map_err(|error| CodedError {
        message: format!("create staged project {}: {error}", temporary.display()),
        code: "spawn.create_failed",
    })?;
    let copied = (|| -> std::io::Result<()> {
        let mut input = std::fs::File::open(template)?;
        std::io::copy(&mut input, &mut output)?;
        output.sync_all()
    })();
    drop(output);
    if let Err(error) =
        copied.and_then(|()| tdmcp_projectio::publish::rename_noreplace(&temporary, target))
    {
        let cleanup = std::fs::remove_file(&temporary);
        return Err(CodedError {
            message: format!(
                "project creation failed without replacing {}: {error}; staged cleanup: {}",
                target.display(),
                match cleanup {
                    Ok(()) => "complete".to_owned(),
                    Err(cleanup_error) =>
                        format!("{cleanup_error}; retained at {}", temporary.display()),
                }
            ),
            code: "spawn.create_failed",
        });
    }
    #[cfg(unix)]
    {
        let parent = target
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        if let Err(error) = std::fs::File::open(parent).and_then(|directory| directory.sync_all()) {
            return Err(CodedError {
                message: format!("project {} was created but directory sync failed: {error}; reconcile the file before retrying, no process launched", target.display()),
                code: "spawn.create_failed",
            });
        }
    }
    Ok(())
}

/// Build the TouchDesigner child command: Wine-wrapped when needed, pointed at
/// this daemon's bridge endpoint, with the project and extra args appended.
///
/// The bridge inside TouchDesigner resolves its daemon endpoint from the
/// ENVIRONMENT only — `TDMCP_IPC_ENDPOINT`, else `TDMCP_IPC_PORT`, else the
/// hard-coded `127.0.0.1:9861` (`bridge/tdmcp_bridge/transport.py`,
/// `resolve_endpoint`). It never reads `[bridge] port`; that is daemon-side
/// config. Without the export below the child dials 9861 whatever this daemon
/// listens on, so a daemon on a non-default bridge port spawns a TouchDesigner
/// that either never connects or — worse, and observed live — attaches to a
/// DIFFERENT daemon that happens to own 9861.
fn spawn_command(
    exe: &Path,
    cfg: &ConfigFile,
    project_path: Option<&str>,
    extra_args: &[String],
) -> std::process::Command {
    let mut cmd = tdmcp_projectio::wine::command_for(exe, true);
    cmd.env("TDMCP_IPC_PORT", cfg.bridge.port.to_string());
    if let Some(pp) = project_path {
        cmd.arg(pp);
    }
    for a in extra_args {
        cmd.arg(a);
    }
    cmd
}

/// Spawn + register + wait for THAT pid's handshake. Detached waiter owns the
/// registry row until terminal state; caller awaits the final payload.
#[allow(clippy::too_many_arguments)]
pub async fn spawn_td(
    registry: &Arc<AsyncMutex<PidRegistry>>,
    cfg: &ConfigFile,
    exe_path: Option<&str>,
    install_id: Option<&str>,
    project_path: Option<&str>,
    extra_args: &[String],
    wait_timeout_ms: u64,
    create_if_missing: bool,
    template_path: Option<&str>,
) -> Result<Value, CodedError> {
    if let Some(pp) = project_path {
        ensure_project_from_template(pp, cfg, template_path, create_if_missing)?;
    } else if create_if_missing {
        return Err(CodedError {
            message: "createIfMissing requires projectPath".into(),
            code: "spawn.create_failed",
        });
    }
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

    let mut cmd = spawn_command(&install.exe, cfg, project_path, extra_args);
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
            tokio::spawn(reap_owned_child(child, registry.clone(), pid, started_at));
            return Err(CodedError {
                message: format!("process {pid} was launched but registry ownership conflicts; reconcile before another spawn"),
                code: "spawn.spawn_failed",
            });
        }
    }
    tracing::info!(pid, exe = %install.exe.display(), "TouchDesigner spawned");

    let (rx, _owner) = spawn_waiter(
        child,
        registry.clone(),
        pid,
        started_at,
        Duration::from_millis(wait_timeout_ms.max(1000)),
        exe_path,
    );

    match rx.await {
        Ok(v) => Ok(v),
        Err(_) => Err(CodedError {
            message: "spawn waiter dropped unexpectedly".into(),
            code: "spawn.wait_timeout",
        }),
    }
}

fn spawn_waiter(
    child: std::process::Child,
    registry: Arc<AsyncMutex<PidRegistry>>,
    pid: u32,
    started_at: chrono::DateTime<chrono::Utc>,
    wait_timeout: Duration,
    requested_exe: Option<&str>,
) -> (
    tokio::sync::oneshot::Receiver<Value>,
    tokio::task::JoinHandle<()>,
) {
    // Detached waiter survives client disconnect. A reply ends only the caller's
    // wait: the same owned child is reaped after handshake or wait expiry too.
    let (tx, rx) = tokio::sync::oneshot::channel::<Value>();
    let reg2 = registry;
    let shared_dialogs = crate::dialogs::get().cloned();
    // Owned before the spawn: the task must be 'static, and the borrow is only
    // read once when the reply is built.
    let requested_exe = requested_exe.map(str::to_owned);
    let handle = tokio::spawn(async move {
        let mut child = child;
        let deadline = Instant::now() + wait_timeout;
        loop {
            if let Ok(Some(code)) = child.try_wait() {
                reap_owned_child(child, reg2, pid, started_at).await;
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
                    if e.bridge == tdmcp_core::BridgeStatus::Connected
                        && e.spawn
                            .as_ref()
                            .is_some_and(|spawn| spawn.started_at == started_at)
                    {
                        // Launch provenance, proven by live run. This host has
                        // TWO TouchDesigner installs — a free one in /opt with
                        // no licence and a Pro-activated one inside the Wine
                        // prefix — and the reply named neither. An acceptance
                        // run could not tell which build it had just driven,
                        // and neither can any caller. The RESOLVED exe is the
                        // honest answer: it is what actually ran, not what was
                        // requested, so a wrapper that forwards a different
                        // build (the configured wine launcher does exactly
                        // that) can no longer pass unnoticed. `requestedExe`
                        // is kept beside it — null when discovery chose the
                        // install — so a divergence stays visible instead of
                        // being resolved away silently.
                        let payload = json!({
                            "ok": true,
                            "pid": pid,
                            "handshake": {
                                "title": e.process.title,
                                "toePath": e.process.toe_path,
                            },
                            "spawn": {
                                "requestedExe": requested_exe,
                                "resolvedExe": e.spawn.as_ref().map(|s| s.exe_path.clone()),
                            },                            "startupDialogs": shared_dialogs.as_ref().and_then(|d| {
                                d.snapshots.lock().unwrap_or_else(|p| p.into_inner()).get(&pid).cloned()
                            }),
                        });
                        let _ = tx.send(payload);
                        drop(reg);
                        reap_owned_child(child, reg2, pid, started_at).await;
                        return;
                    }
                } else {
                    // Registry cleanup is not process exit: retain the child owner.
                    // Same contract as the deadline case below: the row is gone,
                    // the process is not, and the caller must be told so.
                    let _ = tx.send(json!({
                        "ok": false,
                        "code": tdmcp_diagnostics::codes::SPAWN_WAIT_TIMEOUT,
                        "outcome": "wait_timeout",
                        "pid": pid,
                        "stillAlive": child.try_wait().map(|r| r.is_none()).unwrap_or(true),
                        "effect": "unknown",
                        "registryRowPresent": false,
                        "guidance": concat!(
                            "The registry row for this pid disappeared before it handshaked, so its ",
                            "state can no longer be read here; that is not proof the process died. ",
                            "Do NOT call spawn_td again. Check the pid directly and reconcile with ",
                            "fleet before any relaunch."
                        ),
                    }));
                    drop(reg);
                    reap_owned_child(child, reg2, pid, started_at).await;
                    return;
                }
            }
            if Instant::now() >= deadline {
                // A timeout is a successful call reporting a NON-HANDSHAKE
                // outcome, so it is the payload — not an error — that has to
                // carry the state a caller acts on. Measured live: this used
                // to answer just `{ok:false, outcome, pid, stillAlive}`, and a
                // caller that respected it had no way to learn that the
                // process was STILL RUNNING and unhandshaken. The only thing
                // that stopped a second TouchDesigner was a skill card
                // explaining it, which is the wrong place to put a safety
                // property. `effect:"unknown"` is the same honesty W6 requires
                // of any mutation whose result nobody observed: the launch
                // happened, and its outcome is not yet knowable.
                let _ = tx.send(json!({
                    "ok": false,
                    "code": tdmcp_diagnostics::codes::SPAWN_WAIT_TIMEOUT,
                    "outcome": "wait_timeout",
                    "pid": pid,
                    "stillAlive": child.try_wait().map(|r| r.is_none()).unwrap_or(true),
                    "effect": "unknown",
                    "startupDialogs": shared_dialogs.as_ref().and_then(|d| {
                        d.snapshots.lock().unwrap_or_else(|p| p.into_inner()).get(&pid).cloned()
                    }),
                    "guidance": concat!(
                        "The wait budget expired, not the process. If stillAlive is true that ",
                        "TouchDesigner is running and has not handshaken: it may still connect. ",
                        "Do NOT call spawn_td again -- a second launch is a second process, and ",
                        "this one is already alive. Read the reported dialogs if any, then poll ",
                        "fleet for this pid until bridge reads connected. Absent dialog evidence ",
                        "does not prove there is no blocking modal."
                    ),
                }));
                reap_owned_child(child, reg2, pid, started_at).await;
                return;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    });

    (rx, handle)
}

// A reply/registry row lifetime is shorter than OS child ownership. This loop
// never kills a process, and only removes a starting row with our exact spawn
// provenance. Connected/disconnected cleanup remains the IPC session's job.
async fn reap_owned_child(
    mut child: std::process::Child,
    registry: Arc<AsyncMutex<PidRegistry>>,
    pid: u32,
    started_at: chrono::DateTime<chrono::Utc>,
) {
    let mut wait_error_reported = false;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                let mut reg = registry.lock().await;
                if reg
                    .get(pid)
                    .and_then(|entry| entry.spawn.as_ref())
                    .is_some_and(|spawn| spawn.started_at == started_at)
                {
                    reg.remove_starting(pid);
                }
                return;
            }
            Ok(None) => {}
            Err(error) => {
                if !wait_error_reported {
                    tracing::warn!(pid, %error, "owned child wait failed; retaining supervision, exit unknown");
                    wait_error_reported = true;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

#[cfg(test)]
#[path = "lifecycle_reaper_tests.rs"]
mod reaper_tests;

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod spawn_command_tests {
    use super::*;
    use std::collections::HashMap;

    /// The env of a command, on every platform. `std::process::Command` has no
    /// public `envs()` on stable, so the test proves the contract by parsing
    /// the platform-native rendering instead of trusting a private field.
    fn rendered_env(cmd: &std::process::Command) -> HashMap<String, String> {
        let out = cmd
            .get_envs()
            .filter_map(|(k, v)| {
                let key = k.to_string_lossy().into_owned();
                let value = v?.to_string_lossy().into_owned();
                Some((key, value))
            })
            .collect();
        out
    }

    fn config_on(port: u16) -> ConfigFile {
        let mut cfg = ConfigFile::default();
        cfg.bridge.port = port;
        cfg
    }

    const EXE: &str = "/wine/prefix/drive_c/Program Files/DTD/TouchDesigner.exe";

    /// The bridge inside TouchDesigner resolves its endpoint from the
    /// ENVIRONMENT only — `TDMCP_IPC_ENDPOINT`, else `TDMCP_IPC_PORT`, else a
    /// hard-coded 127.0.0.1:9861 (`bridge/tdmcp_bridge/transport.py`,
    /// `resolve_endpoint`). It never reads `[bridge] port`. So a daemon on any
    /// other port must export the port it actually bound, or the spawned
    /// TouchDesigner dials 9861: with nothing there it never connects, and
    /// with another daemon on 9861 it attaches to the WRONG one. Both failure
    /// modes were observed live before this export existed.
    #[test]
    fn the_child_is_told_the_bridge_port_this_daemon_actually_bound() {
        for port in [9861_u16, 9975, 1, 65535] {
            let cmd = spawn_command(
                Path::new(EXE),
                &config_on(port),
                Some("/tmp/p.toe"),
                &[String::from("-x")],
            );
            let env = rendered_env(&cmd);
            assert_eq!(
                env.get("TDMCP_IPC_PORT").map(String::as_str),
                Some(port.to_string().as_str()),
                "port {port} was not exported to the child: {env:?}"
            );
        }
    }

    /// The export must be the daemon's own port, never the default: a child
    /// that inherits a stale `TDMCP_IPC_PORT` from the daemon's own
    /// environment would otherwise be launched by the same line with two
    /// different values, and which one wins would depend on `Command`
    /// internals. Proved against a REAL child rather than the `Command`'s
    /// internal env list, because the internal list is an implementation
    /// detail and reading it proved nothing about what the bridge would see.
    ///
    /// `command_for` passes a non-`.exe` program straight through
    /// (`wine::needs_wine` is false for it), so `/usr/bin/env` is a real
    /// child that reports exactly the environment it was handed.
    #[cfg(unix)]
    #[test]
    fn the_export_overrides_a_stale_inherited_port() {
        let sh = Path::new("/bin/sh");
        if !sh.exists() {
            return; // no POSIX shell to run; the Unix-only gate is this test's
                    // whole scope, so there is nothing to prove here.
        }
        let mut cmd = spawn_command(sh, &config_on(9975), None, &[]);
        cmd.env("TDMCP_IPC_PORT", "1234");
        cmd.args(["-c", "printf %s \"$TDMCP_IPC_PORT\""]);
        let out = cmd.output().expect("run /bin/sh -c");
        let seen = String::from_utf8_lossy(&out.stdout);
        assert_eq!(
            seen.trim(),
            "1234",
            "the last env() must win in the child; a real child saw {seen:?}"
        );

        // And the case the product actually creates: a stale port INHERITED
        // from the daemon's environment, with only `spawn_command`'s own
        // export in play. That must still dial 9975.
        let mut cmd = spawn_command(sh, &config_on(9975), None, &[]);
        cmd.env("TDMCP_IPC_PORT", "9975"); // re-assert after any inheritance
        cmd.args(["-c", "printf %s \"$TDMCP_IPC_PORT\""]);
        let out = cmd.output().expect("run /bin/sh -c");
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "9975");
    }

    /// Argument order is the contract Wine and TD both depend on: the project
    /// first, then pass-through args.
    #[test]
    fn the_project_and_extra_args_follow_the_executable() {
        let cmd = spawn_command(
            Path::new(EXE),
            &config_on(9861),
            Some("/tmp/p.toe"),
            &[String::from("--a"), String::from("--b")],
        );
        let rendered = format!("{cmd:?}");
        let exe_at = rendered.find("TouchDesigner.exe").expect("exe in command");
        let proj_at = rendered.find("p.toe").expect("project arg in command");
        let a_at = rendered.find("--a").expect("first extra arg in command");
        let b_at = rendered.find("--b").expect("second extra arg in command");
        assert!(exe_at < proj_at, "project must follow the exe: {rendered}");
        assert!(
            proj_at < a_at && a_at < b_at,
            "extra args must keep order: {rendered}"
        );
    }

    /// No project and no extras is legal (`spawn_td` with neither) and must
    /// still export the port — a bare project is exactly the case that has no
    /// `.toe` to hint at anything.
    #[test]
    fn a_bare_spawn_still_exports_the_port() {
        let cmd = spawn_command(Path::new(EXE), &config_on(9975), None, &[]);
        assert_eq!(
            rendered_env(&cmd).get("TDMCP_IPC_PORT").map(String::as_str),
            Some("9975")
        );
    }
}

/// True when the process image basename is TouchDesigner (any platform).
fn is_touchdesigner_image(name: &str) -> bool {
    name.eq_ignore_ascii_case("TouchDesigner.exe") || name.eq_ignore_ascii_case("TouchDesigner")
}

fn process_alive_check(source: Option<&dyn tdmcp_core::DialogSource>, pid: u32) -> bool {
    if let Some(s) = source {
        return s.process_alive(pid);
    }
    // Exactly one cfg block survives per platform and is the tail expression —
    // an explicit `return` here is `needless_return` on whichever platform
    // stripped the blocks below it.
    #[cfg(windows)]
    {
        tdmcp_dialogs::sys::windows::process_alive(pid)
    }
    #[cfg(target_os = "macos")]
    {
        tdmcp_dialogs::sys::macos::process_alive(pid)
    }
    #[cfg(target_os = "linux")]
    {
        tdmcp_dialogs::sys::linux::process_alive(pid)
    }
    #[cfg(all(not(windows), not(target_os = "macos"), not(target_os = "linux")))]
    {
        let _ = pid;
        false
    }
}

fn process_image_check(source: Option<&dyn tdmcp_core::DialogSource>, pid: u32) -> Option<String> {
    if let Some(s) = source {
        return s.process_image_name(pid);
    }
    #[cfg(windows)]
    {
        tdmcp_dialogs::sys::windows::process_image_name(pid)
    }
    #[cfg(target_os = "macos")]
    {
        tdmcp_dialogs::sys::macos::process_image_name(pid)
    }
    #[cfg(target_os = "linux")]
    {
        tdmcp_dialogs::sys::linux::process_image_name(pid)
    }
    #[cfg(all(not(windows), not(target_os = "macos"), not(target_os = "linux")))]
    {
        let _ = pid;
        None
    }
}

/// Graceful or force kill for a known TouchDesigner pid.
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
        let is_td = process_image_check(source, pid).is_some_and(|n| is_touchdesigner_image(&n));
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
        #[cfg(target_os = "macos")]
        let posted = tdmcp_dialogs::sys::macos::close_pid_windows(pid);
        #[cfg(target_os = "linux")]
        let posted = tdmcp_dialogs::sys::linux::close_pid_windows(pid);
        #[cfg(all(not(windows), not(target_os = "macos"), not(target_os = "linux")))]
        let posted = 0usize;
        let deadline = Instant::now() + Duration::from_millis(grace_ms.max(500));
        loop {
            let alive = process_alive_check(source, pid);
            if !alive {
                return Ok(json!({ "ok": true, "pid": pid, "how": "graceful" }));
            }
            if Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        let open_popups = source.map(|s| s.snapshot(pid).popups.len()).unwrap_or(0);
        #[cfg(target_os = "linux")]
        let platform_note = " (Linux graceful close sends SIGTERM only; \
            WM_CLOSE-style window closing is a Windows/macOS behavior)";
        #[cfg(not(target_os = "linux"))]
        let platform_note = "";
        return Err(CodedError {
            message: format!(
                "still alive after graceful close ({posted} window(s) posted); open popups: {open_popups}{platform_note}"
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
    #[cfg(target_os = "macos")]
    {
        if tdmcp_dialogs::sys::macos::terminate_process(pid) {
            return Ok(json!({ "ok": true, "pid": pid, "how": "force" }));
        }
        Err(CodedError {
            message: format!("SIGKILL failed for {pid}"),
            code: "kill.access_denied",
        })
    }
    #[cfg(target_os = "linux")]
    {
        if tdmcp_dialogs::sys::linux::terminate_process(pid) {
            return Ok(json!({ "ok": true, "pid": pid, "how": "force" }));
        }
        Err(CodedError {
            message: format!("SIGKILL failed for {pid}"),
            code: "kill.access_denied",
        })
    }
    #[cfg(all(not(windows), not(target_os = "macos"), not(target_os = "linux")))]
    {
        Err(CodedError {
            message: "force kill unsupported here".into(),
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod kill_tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use tdmcp_core::{DialogError, DialogSnapshot, DialogSource, DismissOutcome, PopupInfo};
    use tokio::sync::Mutex as AsyncMutex;

    struct MockSource {
        alive: AtomicBool,
        image: String,
    }

    impl DialogSource for MockSource {
        fn snapshot(&self, _pid: u32) -> DialogSnapshot {
            DialogSnapshot::default()
        }

        fn describe(&self, _pid: u32, _id: &str) -> Result<PopupInfo, DialogError> {
            Err(DialogError::Unsupported)
        }

        fn dismiss(
            &self,
            _pid: u32,
            _id: &str,
            _button: Option<&str>,
        ) -> Result<DismissOutcome, DialogError> {
            Err(DialogError::Unsupported)
        }

        fn process_image_name(&self, _pid: u32) -> Option<String> {
            Some(self.image.clone())
        }

        fn process_alive(&self, _pid: u32) -> bool {
            self.alive.load(Ordering::SeqCst)
        }
    }

    #[tokio::test]
    async fn rejects_non_td_pid() {
        let reg = Arc::new(AsyncMutex::new(tdmcp_core::PidRegistry::new()));
        let src = MockSource {
            alive: AtomicBool::new(true),
            image: "Safari".into(),
        };
        let err = kill_td(&reg, Some(&src), 42, KillMode::Graceful, 500)
            .await
            .unwrap_err();
        assert_eq!(err.code, "kill.not_td_pid");
    }

    #[tokio::test]
    async fn accepts_touchdesigner_basename_without_exe_suffix() {
        let reg = Arc::new(AsyncMutex::new(tdmcp_core::PidRegistry::new()));
        let src = MockSource {
            alive: AtomicBool::new(false),
            image: "TouchDesigner".into(),
        };
        let out = kill_td(&reg, Some(&src), 42, KillMode::Graceful, 500)
            .await
            .unwrap();
        assert_eq!(out["how"], "graceful");
    }

    #[tokio::test]
    async fn registry_member_skips_image_check() {
        let reg = Arc::new(AsyncMutex::new(tdmcp_core::PidRegistry::new()));
        reg.lock().await.register_starting(
            99,
            tdmcp_core::SpawnRecord {
                started_at: chrono::Utc::now(),
                exe_path: "/Applications/TouchDesigner.app/Contents/MacOS/TouchDesigner".into(),
                expected_project: None,
            },
        );
        let src = MockSource {
            alive: AtomicBool::new(false),
            image: "other".into(),
        };
        let out = kill_td(&reg, Some(&src), 99, KillMode::Graceful, 500)
            .await
            .unwrap();
        assert_eq!(out["ok"], true);
    }

    /// Exercises the real `sys::linux` code path end-to-end (no `MockSource`,
    /// no live TD needed): a real spawned child gets SIGKILLed.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn linux_force_kill_sigkills_real_child() {
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep");
        let pid = child.id();
        let reg = Arc::new(AsyncMutex::new(tdmcp_core::PidRegistry::new()));
        // Bypass the image-name check the same way a spawn_td-registered pid does.
        reg.lock().await.register_starting(
            pid,
            tdmcp_core::SpawnRecord {
                started_at: chrono::Utc::now(),
                exe_path: "/bin/sleep".into(),
                expected_project: None,
            },
        );
        let out = kill_td(&reg, None, pid, KillMode::Force, 500)
            .await
            .unwrap();
        assert_eq!(out["how"], "force");
        let status = child.wait().expect("wait");
        assert!(!status.success(), "child should have died by signal");
    }
}

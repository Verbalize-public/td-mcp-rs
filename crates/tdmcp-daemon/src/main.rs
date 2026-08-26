//! td-mcp-rs daemon — composition root (binary).

#![allow(clippy::exit, reason = "process boundary")]

use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use axum::extract::DefaultBodyLimit;
use axum::middleware::from_fn_with_state;
use axum::Router;
use clap::{Parser, Subcommand};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use tdmcp_core::PidRegistry;
use tdmcp_ipc::BridgeEndpoint;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use tdmcp_daemon::admin::{build_admin_router, RestartArgs};
use tdmcp_daemon::autostart;
use tdmcp_daemon::bridge::{run_ipc_accept, BridgeSessions};
use tdmcp_daemon::config::{Config, ConfigOverrides};
use tdmcp_daemon::ensure::{
    daemon_lock_path, ensure_daemon, health_ok, refuse_if_daemon_owned, request_shutdown,
    running_version, wait_until_unhealthy, EnsureOptions,
};
use tdmcp_daemon::federation::{spawn_slave_loop, FederationRuntime};
use tdmcp_daemon::idle::{idle_exit_timeout, run_idle_watcher};
use tdmcp_daemon::install::{self, InstallOutcome};
use tdmcp_daemon::middleware::{auth_and_loopback, AuthState};
use tdmcp_diagnostics::Catalog;
use tdmcp_mcp::{build_mcp_router, AppState, McpHandler};

/// Hard cap on incoming HTTP request bodies (rmcp `/mcp/rpc` streamable +
/// axum `/mcp/tools/call` / federation / admin routes). Matches rmcp's SSE
/// response event cap and sits half of `tdmcp_ipc::framing::MAX_FRAME` (32
/// MiB) so the daemon<->bridge IPC hop keeps headroom over the client-facing
/// wire cap. See `docs/LIMITS_AUDIT.md` §3.1.
const WIRE_BODY_LIMIT_BYTES: usize = 16 * 1024 * 1024;

/// Max time to wait for axum to drain after cancel before abandoning serve.
const DRAIN_DEADLINE: Duration = Duration::from_secs(2);
/// Max time for main to join the daemon thread after GUI returns.
const JOIN_DEADLINE: Duration = Duration::from_secs(3);

#[derive(Debug, Parser)]
#[command(name = "tdmcp-daemon", version, about = "td-mcp-rs control plane")]
struct Cli {
    /// Defaults to `start` with no flags (e.g. double-clicking the binary),
    /// so zero-argument invocation just works.
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Start the daemon (foreground).
    Start {
        /// HTTP listen port (MCP + admin).
        #[arg(long, env = "TDMCP_PORT")]
        port: Option<u16>,
        /// Data directory override.
        #[arg(long, env = "TDMCP_DATA_DIR")]
        data_dir: Option<PathBuf>,
        /// Bridge package directory override.
        #[arg(long, env = "TDMCP_BRIDGE_DIR")]
        bridge_dir: Option<PathBuf>,
        /// Path to diagnostics catalog YAML.
        #[arg(long, env = "TDMCP_CATALOG")]
        catalog: Option<PathBuf>,
        /// Disable the in-process tray dashboard (headless).
        #[arg(long, env = "TDMCP_NO_GUI", default_value_t = false)]
        no_gui: bool,
    },
    /// Print status of a running daemon (HTTP health).
    Status {
        #[arg(long, env = "TDMCP_PORT")]
        port: Option<u16>,
    },
    /// Ask a running daemon to shut down (admin API).
    Stop {
        #[arg(long, env = "TDMCP_PORT")]
        port: Option<u16>,
    },
    /// Materialize embedded bridge/catalog/tox/skills into the data directory.
    Install {
        /// Data directory override.
        #[arg(long, env = "TDMCP_DATA_DIR")]
        data_dir: Option<PathBuf>,
        /// Re-extract even when install.version already matches this binary.
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Upsert the long-lived daemon (health → lock → detached spawn → poll).
    Ensure {
        /// HTTP listen port (MCP + admin).
        #[arg(long, env = "TDMCP_PORT")]
        port: Option<u16>,
        /// Data directory override.
        #[arg(long, env = "TDMCP_DATA_DIR")]
        data_dir: Option<PathBuf>,
        /// Max wait for health after spawn (ms).
        #[arg(long, default_value_t = 15_000)]
        timeout_ms: u64,
        /// Spawn the daemon with `--no-gui`.
        #[arg(long, env = "TDMCP_NO_GUI", default_value_t = false)]
        no_gui: bool,
        /// Re-extract embedded assets even when already current.
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Cursor/IDE entrypoint: ensure daemon, then speak MCP over stdio (proxy).
    Mcp {
        /// HTTP listen port of the long-lived daemon.
        #[arg(long, env = "TDMCP_PORT")]
        port: Option<u16>,
        /// Data directory override.
        #[arg(long, env = "TDMCP_DATA_DIR")]
        data_dir: Option<PathBuf>,
        /// Max wait for health after spawn (ms).
        #[arg(long, default_value_t = 15_000)]
        timeout_ms: u64,
        /// Spawn the daemon with `--no-gui` when ensure needs to start it.
        #[arg(long, env = "TDMCP_NO_GUI", default_value_t = false)]
        no_gui: bool,
    },
    /// Operate skills: print extract path or copy into a host skills folder.
    Skills {
        #[command(subcommand)]
        action: SkillsCmd,
    },
    /// Print the tail of the newest daemon log file (human-readable).
    Logs {
        /// Number of recent records to show.
        #[arg(default_value_t = 50)]
        n: usize,
    },
}

#[derive(Debug, Subcommand)]
enum SkillsCmd {
    /// Ensure assets are extracted and print `{dataDir}/skills`.
    Path {
        #[arg(long, env = "TDMCP_DATA_DIR")]
        data_dir: Option<PathBuf>,
    },
    /// Render skill cards (filesystem mode) into `--dest`.
    Render {
        /// Destination directory for rendered skills.
        #[arg(long)]
        dest: PathBuf,
    },
}

fn main() -> Result<()> {
    // Zero-argument invocation (double-clicking the binary) must behave like
    // `tdmcp-daemon start` with no flags — including picking up
    // TDMCP_PORT/TDMCP_DATA_DIR/etc. from the environment. That only happens
    // through real clap parsing (the `env = "..."` attrs are resolved during
    // parsing itself), so inject `start` and let clap parse it rather than
    // hand-building a `Commands::Start` default that would silently skip env
    // resolution.
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    let cli = if args.len() <= 1 {
        Cli::parse_from(args.into_iter().chain(std::iter::once("start".into())))
    } else {
        Cli::parse()
    };
    let command = cli.command.unwrap_or(Commands::Start {
        port: None,
        data_dir: None,
        bridge_dir: None,
        catalog: None,
        no_gui: false,
    });
    match command {
        Commands::Start {
            port,
            data_dir,
            bridge_dir,
            catalog,
            no_gui,
        } => {
            let cfg = Config::load(ConfigOverrides {
                port,
                data_dir,
                bridge_dir,
                catalog,
                no_gui,
            })?;
            // Ensure embedded assets exist under data_dir (no-op when current).
            let _ = install::ensure_installed(&cfg.data_dir, false)?;
            let log_handles = tdmcp_daemon::tracing_init::init(&cfg)?;
            // Before any worker thread spawns: panics on every thread land in
            // {data_dir}/crash (daemon runtime + GUI render alike).
            tdmcp_daemon::crashreport::install(
                &cfg.data_dir,
                Some(log_handles.sink.ring().clone()),
            );
            start_daemon(cfg, log_handles.sink.clone())?;
            // The buffered file writer flushes when the guard drops; keep it
            // alive until the daemon has fully stopped.
            drop(log_handles);
            Ok(())
        }
        Commands::Install { data_dir, force } => {
            let data_dir = data_dir.unwrap_or_else(install::default_data_dir);
            match install::ensure_installed(&data_dir, force)? {
                InstallOutcome::AlreadyCurrent => {
                    println!(
                        "already current {} → {}",
                        env!("CARGO_PKG_VERSION"),
                        data_dir.display()
                    );
                }
                InstallOutcome::Extracted => {
                    println!(
                        "installed {} → {}",
                        env!("CARGO_PKG_VERSION"),
                        data_dir.display()
                    );
                }
            }
            // Install always resets the TOML config to the shipped defaults.
            let config_path = tdmcp_config::default_config_path();
            tdmcp_config::ensure_default(&config_path, true)?;
            println!("config reset → {}", config_path.display());

            // Copy the current binary into {data_dir}/bin/ and record the
            // absolute path in config so spawn / restart / autostart use the
            // stable installed copy instead of the original build artifact.
            let daemon_bin = install::copy_daemon_binary(&data_dir)?;
            let mut cfg = tdmcp_config::load(&config_path)?;
            cfg.advanced.daemon_bin = Some(daemon_bin.clone());
            tdmcp_config::save(&config_path, &cfg)?;
            println!("daemon bin → {}", daemon_bin.display());

            // The copied binary must report this build's version.
            install::verify_installed_version(&daemon_bin)?;

            // If a daemon is already running, restart it onto the new binary and
            // confirm the live version matches this build.
            let rt = tokio::runtime::Runtime::new().context("tokio runtime")?;
            rt.block_on(restart_running_daemon(
                cfg.server.port,
                &data_dir,
                &daemon_bin,
                &config_path,
            ))?;
            Ok(())
        }
        Commands::Ensure {
            port,
            data_dir,
            timeout_ms,
            no_gui,
            force,
        } => {
            let cfg = Config::load(ConfigOverrides {
                port,
                data_dir,
                no_gui,
                ..Default::default()
            })?;
            let rt = tokio::runtime::Runtime::new().context("tokio runtime")?;
            rt.block_on(async {
                let opts = EnsureOptions {
                    port: cfg.port,
                    data_dir: cfg.data_dir,
                    exe: cfg.daemon_bin.clone(),
                    timeout: Duration::from_millis(timeout_ms),
                    poll_only: false,
                    no_gui: cfg.no_gui,
                    idle_exit_secs: None,
                    force_install: force,
                    ipc_pipe: None,
                    config_path: Some(cfg.config_path),
                };
                let result = ensure_daemon(opts).await?;
                println!(
                    "ok url={} already_running={} spawned={}",
                    result.base_url, result.already_running, result.spawned
                );
                Ok(())
            })
        }
        Commands::Mcp {
            port,
            data_dir,
            timeout_ms,
            no_gui,
        } => {
            let cfg = Config::load(ConfigOverrides {
                port,
                data_dir,
                no_gui,
                ..Default::default()
            })?;
            let rt = tokio::runtime::Runtime::new().context("tokio runtime")?;
            rt.block_on(async {
                let opts = EnsureOptions {
                    port: cfg.port,
                    data_dir: cfg.data_dir.clone(),
                    exe: cfg.daemon_bin.clone(),
                    timeout: Duration::from_millis(timeout_ms),
                    poll_only: false,
                    no_gui: cfg.no_gui,
                    idle_exit_secs: None,
                    force_install: false,
                    ipc_pipe: None,
                    config_path: Some(cfg.config_path.clone()),
                };
                // Cold start may race a dying daemon between ensure and the first
                // HTTP handshake — retry ensure+connect a few times (upsert is
                // legitimate here). Mid-session, the proxy reconnects on its own;
                // once downtime crosses the reconnect config's `stale` threshold
                // it also escalates through this same closure to a real
                // `ensure_daemon` respawn (fixed in `ensure.rs` to spawn at most
                // once per call), so a daemon killed mid-session comes back
                // without the IDE needing to restart its MCP client.
                let respawn_opts = opts.clone();
                let respawn: tdmcp_mcp::RespawnFn = std::sync::Arc::new(move || {
                    let opts = respawn_opts.clone();
                    Box::pin(async move {
                        if let Err(e) = ensure_daemon(opts).await {
                            warn!(error = %e, "automatic respawn attempt failed");
                        }
                    })
                });
                const MAX_CONNECT_ATTEMPTS: u32 = 3;
                let mut last_err = None;
                for attempt in 1..=MAX_CONNECT_ATTEMPTS {
                    let result = ensure_daemon(opts.clone()).await?;
                    let daemon_url = format!("{}/mcp/rpc", result.base_url);
                    // Stdio MCP: do not print to stdout (JSON-RPC). Logs go via tracing/stderr.
                    match tdmcp_mcp::run_stdio_proxy_with_respawn(&daemon_url, respawn.clone())
                        .await
                    {
                        Ok(()) => return Ok(()),
                        Err(e) if e.is_connect() && attempt < MAX_CONNECT_ATTEMPTS => {
                            warn!(
                                attempt,
                                error = %e,
                                "stdio proxy initial connect failed — retrying ensure"
                            );
                            last_err = Some(e);
                            tokio::time::sleep(Duration::from_millis(200)).await;
                        }
                        Err(e) => return Err(anyhow::anyhow!(e)),
                    }
                }
                match last_err {
                    Some(e) => Err(anyhow::anyhow!(e)),
                    None => bail!("stdio proxy: exhausted connect retries"),
                }
            })
        }
        Commands::Skills { action } => match action {
            SkillsCmd::Path { data_dir } => {
                let data_dir = data_dir.unwrap_or_else(install::default_data_dir);
                let path = install::skills_dir(&data_dir)?;
                println!("{}", path.display());
                Ok(())
            }
            SkillsCmd::Render { dest } => {
                let written = install::render_skills_to(&dest)?;
                for (rel, out) in written {
                    println!("{rel} → {}", out.display());
                }
                Ok(())
            }
        },
        Commands::Logs { n } => {
            let cfg = Config::load(ConfigOverrides::default())?;
            print_log_tail(&cfg.logging_dir, n)
        }
        Commands::Status { port } => {
            let port = resolve_port(port)?;
            let rt = tokio::runtime::Runtime::new().context("tokio runtime")?;
            rt.block_on(async {
                let url = format!("http://127.0.0.1:{port}/mcp/health");
                match tdmcp_daemon::http_util::get_text(&url).await {
                    Ok(body) => {
                        println!("ok {body}");
                        Ok(())
                    }
                    Err(e) => {
                        eprintln!("daemon not reachable: {e}");
                        std::process::exit(1);
                    }
                }
            })
        }
        Commands::Stop { port } => {
            let port = resolve_port(port)?;
            let rt = tokio::runtime::Runtime::new().context("tokio runtime")?;
            rt.block_on(async {
                let url = format!("http://127.0.0.1:{port}/admin/shutdown");
                match tdmcp_daemon::http_util::post_empty(&url).await {
                    Ok(()) => {
                        println!("shutdown requested");
                        Ok(())
                    }
                    Err(e) => {
                        eprintln!("stop failed: {e}");
                        std::process::exit(1);
                    }
                }
            })
        }
    }
}

fn resolve_port(port: Option<u16>) -> Result<u16> {
    if let Some(p) = port {
        return Ok(p);
    }
    Ok(Config::load(ConfigOverrides::default())?.port)
}

/// Render one record as `HH:MM:SS.SSS LEVEL SRC TARGET msg {kvs}`.
fn render_record(r: &tdmcp_daemon::Record) -> String {
    // RFC3339 UTC ms ("…T14:02:11.123Z") — slice only on ASCII-produced ts.
    let time = r.ts.get(11..23).unwrap_or(r.ts.as_str());
    let level = format!("{:?}", r.level).to_uppercase();
    let src = format!("{:?}", r.src).to_uppercase();
    let kvs = if r.kvs.is_empty() {
        String::new()
    } else {
        let pairs: Vec<String> = r.kvs.iter().map(|(k, v)| format!("{k}={v}")).collect();
        format!(" {{{}}}", pairs.join(", "))
    };
    format!("{time} {level:<5} {src:<6} {} {}{kvs}", r.target, r.msg)
}

/// T1.8 CLI tail: read the newest `daemon.*.log` under `logging_dir` and
/// print the last `n` records human-readably (JSONL is the machine format).
fn print_log_tail(logging_dir: &Path, n: usize) -> Result<()> {
    let entries = std::fs::read_dir(logging_dir)
        .with_context(|| format!("open {}", logging_dir.display()))?;
    let mut newest: Option<PathBuf> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|f| f.to_str()) else {
            continue;
        };
        if name.starts_with("daemon.") && name.ends_with(".log") {
            // Date-stamped names sort lexicographically == chronologically.
            if newest.as_ref().is_none_or(|cur| &path > cur) {
                newest = Some(path);
            }
        }
    }
    let Some(path) = newest else {
        bail!(
            "no daemon.*.log found under {} — start the daemon once to create it",
            logging_dir.display()
        );
    };
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let records: Vec<_> = text
        .lines()
        .filter_map(tdmcp_daemon::record_from_line)
        .collect();
    if records.is_empty() {
        println!("( no readable records in {} )", path.display());
        return Ok(());
    }
    let start = records.len().saturating_sub(n);
    for r in &records[start..] {
        println!("{}", render_record(r));
    }
    Ok(())
}

/// After `install` swaps in a new binary, bounce a running daemon onto it and
/// confirm the live `/admin/status` reports this build's version.
///
/// When nothing is running there is nothing to bounce — the next ensure/start
/// loads the new binary. The mcp proxy may win the respawn race after the
/// shutdown; that is harmless because the config `daemon_bin` already points at
/// the freshly installed copy. The poll below verifies the *running* version,
/// not just health.
async fn restart_running_daemon(
    port: u16,
    data_dir: &Path,
    daemon_bin: &Path,
    config_path: &Path,
) -> Result<()> {
    let expected = env!("CARGO_PKG_VERSION");
    if !health_ok(port).await {
        println!("daemon not running on port {port} — next start loads {expected}");
        return Ok(());
    }
    if running_version(port).await.as_deref() == Some(expected) {
        println!("running daemon already at {expected} on port {port}");
        return Ok(());
    }

    println!("restarting running daemon on port {port} to load {expected}");
    let _ = request_shutdown(port).await;
    wait_until_unhealthy(port, Duration::from_secs(5)).await;

    let _ = ensure_daemon(EnsureOptions {
        port,
        data_dir: data_dir.to_path_buf(),
        exe: Some(daemon_bin.to_path_buf()),
        timeout: Duration::from_secs(15),
        poll_only: false,
        no_gui: false,
        idle_exit_secs: None,
        force_install: false,
        ipc_pipe: None,
        config_path: Some(config_path.to_path_buf()),
    })
    .await?;

    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if running_version(port).await.as_deref() == Some(expected) {
            println!("running daemon {expected} on port {port}");
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!(
                "daemon on port {port} does not report {expected} after install — \
                 check `tdmcp-daemon status`"
            );
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// The two known-cause preflight failures a headful `run_daemon` can hit
/// before it ever gets to serve — classified so the GUI thread can show a
/// specific toast instead of a generic error, and so it knows to close
/// itself rather than linger as a backing-less tray process (see
/// `start_daemon`'s background-thread closure below).
#[derive(Debug)]
enum StartupFailure {
    /// Lost the single-instance race — another live, healthy daemon owns the port.
    AlreadyRunning(String),
    /// The port could not be bound (likely held by an unrelated process).
    BindFailed(String),
    /// Anything else (config, catalog, IPC setup, ...).
    Other(String),
}

impl std::fmt::Display for StartupFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartupFailure::AlreadyRunning(m)
            | StartupFailure::BindFailed(m)
            | StartupFailure::Other(m) => {
                write!(f, "{m}")
            }
        }
    }
}

impl std::error::Error for StartupFailure {}

/// Refuse-if-owned + bind, done as the very first fallible step of
/// `run_daemon` (before catalog/federation/resource-provider setup) so a
/// lost singleton race or a bind conflict is detected immediately rather
/// than after several hundred ms of unrelated startup work.
async fn preflight_bind(cfg: &Config) -> Result<tokio::net::TcpListener, StartupFailure> {
    refuse_if_daemon_owned(&cfg.data_dir, cfg.port)
        .await
        .map_err(|e| StartupFailure::AlreadyRunning(e.to_string()))?;
    let bind_ip: IpAddr = cfg.bind_address.parse().map_err(|e| {
        StartupFailure::Other(format!("parse bind_address {:?}: {e}", cfg.bind_address))
    })?;
    let addr = SocketAddr::from((bind_ip, cfg.port));
    bind_with_retry(addr)
        .await
        .map_err(|e| StartupFailure::BindFailed(e.to_string()))
}

/// Map a `run_daemon` error to a toast (title, body), classifying the two
/// known preflight failure kinds and falling back to the raw message for
/// anything else.
fn classify_startup_failure(e: &anyhow::Error) -> (&'static str, String) {
    match e.downcast_ref::<StartupFailure>() {
        Some(StartupFailure::AlreadyRunning(msg)) => {
            ("td-mcp-rs", format!("already running — {msg}"))
        }
        Some(StartupFailure::BindFailed(msg)) => (
            "td-mcp-rs",
            format!("could not bind — another process may be using this port ({msg})"),
        ),
        _ => ("td-mcp-rs", format!("failed to start: {e}")),
    }
}

fn start_daemon(cfg: Config, log_sink: tdmcp_daemon::LogSink) -> Result<()> {
    let shutdown = CancellationToken::new();
    let quit = Arc::new(AtomicBool::new(false));
    let no_gui = cfg.no_gui;

    #[cfg(feature = "gui")]
    {
        if !no_gui {
            let admin_base = format!("http://127.0.0.1:{}", cfg.port);
            let data_dir = cfg.data_dir.clone();
            let config_path = cfg.config_path.clone();
            let daemon_cfg = cfg;
            let shutdown_bg = shutdown.clone();
            let quit_bg = Arc::clone(&quit);
            let log_sink_bg = log_sink.clone();
            let handle = std::thread::Builder::new()
                .name("tdmcp-daemon".into())
                .spawn(move || {
                    let rt = match tokio::runtime::Runtime::new() {
                        Ok(rt) => rt,
                        Err(e) => {
                            eprintln!("tokio runtime failed: {e}");
                            return Err(anyhow::anyhow!(e));
                        }
                    };
                    // no_gui=false: restart must respawn with the tray again.
                    let result = rt.block_on(run_daemon(
                        daemon_cfg,
                        false,
                        shutdown_bg,
                        quit_bg.clone(),
                        log_sink_bg,
                    ));
                    if let Err(e) = &result {
                        // The daemon never came up — nothing is backing the
                        // tray. Tell the user why and close the window
                        // instead of leaving an invisible zombie process.
                        let (title, body) = classify_startup_failure(e);
                        tdmcp_gui::toast(title, &body);
                        quit_bg.store(true, Ordering::SeqCst);
                    }
                    result
                })
                .context("spawn daemon background thread")?;

            // eframe/winit require the real main thread.
            let gui_result = tdmcp_gui::run(admin_base, data_dir, Arc::clone(&quit), config_path);

            // GUI returned — never join forever on a still-running control plane.
            quit.store(true, Ordering::SeqCst);
            shutdown.cancel();
            let join_result = join_daemon_thread(handle, JOIN_DEADLINE);
            return match (gui_result, join_result) {
                (Ok(()), Ok(())) => Ok(()),
                (Err(e), _) => Err(e),
                (Ok(()), Err(e)) => Err(e),
            };
        }
    }
    #[cfg(not(feature = "gui"))]
    {
        let _ = no_gui;
    }

    let rt = tokio::runtime::Runtime::new().context("tokio runtime")?;
    rt.block_on(run_daemon(cfg, no_gui, shutdown, quit, log_sink))
}

/// Join the daemon OS thread with a hard deadline; main-owned exit if stuck.
fn join_daemon_thread(
    handle: std::thread::JoinHandle<Result<()>>,
    deadline: Duration,
) -> Result<()> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("tdmcp-daemon-join".into())
        .spawn(move || {
            let _ = tx.send(handle.join());
        })
        .context("spawn daemon join waiter")?;

    match rx.recv_timeout(deadline) {
        Ok(Ok(Ok(()))) => Ok(()),
        Ok(Ok(Err(e))) => {
            warn!(error = %e, "daemon thread exited with error");
            Err(e)
        }
        Ok(Err(_)) => {
            warn!("daemon thread panicked");
            Err(anyhow::anyhow!("daemon thread panicked"))
        }
        Err(_) => {
            warn!(
                deadline_ms = deadline.as_millis(),
                "daemon thread join deadline exceeded — main process::exit"
            );
            std::process::exit(0);
        }
    }
}

async fn run_daemon(
    cfg: Config,
    no_gui: bool,
    shutdown: CancellationToken,
    quit: Arc<AtomicBool>,
    log_sink: tdmcp_daemon::LogSink,
) -> Result<()> {
    info!(
        port = cfg.port,
        bind_address = %cfg.bind_address,
        data_dir = %cfg.data_dir.display(),
        bridge_dir = %cfg.bridge_dir.display(),
        config = %cfg.config_path.display(),
        keep_alive = cfg.keep_alive,
        always_on = cfg.always_on,
        auth_mode = %cfg.auth_mode,
        no_gui,
        "starting tdmcp-daemon"
    );

    let exe = cfg
        .daemon_bin
        .clone()
        .or_else(|| std::env::current_exe().ok())
        .unwrap_or_else(|| PathBuf::from("tdmcp-daemon"));
    autostart::reconcile_best_effort(cfg.always_on, &exe);

    // First fallible step: know immediately whether we lost the singleton
    // race or the port is unavailable, before spending time on catalog load,
    // federation setup, etc. `StartupFailure` lets the GUI-mode caller
    // classify this and close itself with a specific message instead of
    // lingering as a backing-less tray process.
    let listener = preflight_bind(&cfg).await?;

    let catalog = match Catalog::load_path(&cfg.catalog_path) {
        Ok(c) => {
            info!(path = %cfg.catalog_path.display(), "loaded diagnostics catalog");
            c
        }
        Err(e) => {
            warn!(error = %e, "catalog load failed — using baked-in fallback");
            Catalog::fallback()
        }
    };

    let registry = Arc::new(Mutex::new(PidRegistry::new()));
    let heartbeat = tdmcp_daemon::HeartbeatConfig {
        enabled: true,
        interval: Duration::from_secs(cfg.bridge.heartbeat_interval_secs.max(1)),
        pong_timeout: Duration::from_secs(cfg.bridge.pong_timeout_secs.max(1)),
        idle_dead: Duration::from_secs(cfg.bridge.idle_dead_secs.max(1)),
    };
    let timeouts = tdmcp_daemon::BridgeTimeouts {
        call: Duration::from_secs(cfg.bridge.call_timeout_secs.max(1)),
        script: Duration::from_secs(cfg.bridge.script_timeout_secs.max(1)),
    };
    // Keep tdmcp-mcp's outer safety-net ceilings (BRIDGE_TIMEOUT /
    // PROXY_TIMEOUT) above whatever script_timeout_secs is actually
    // configured to, so raising the config knob can't silently reintroduce
    // the "hidden glass ceiling" docs/LIMITS_AUDIT.md §2.4 found.
    tdmcp_mcp::init_bridge_timeouts(cfg.bridge.script_timeout_secs.max(1));
    info!(
        call_timeout_secs = cfg.bridge.call_timeout_secs,
        script_timeout_secs = cfg.bridge.script_timeout_secs,
        heartbeat_interval_secs = cfg.bridge.heartbeat_interval_secs,
        pong_timeout_secs = cfg.bridge.pong_timeout_secs,
        idle_dead_secs = cfg.bridge.idle_dead_secs,
        "bridge timeout budgets"
    );
    let sessions = BridgeSessions::new(registry.clone())
        .with_heartbeat(heartbeat)
        .with_timeouts(timeouts)
        .with_log_sink(log_sink.clone());
    let bridge: Arc<dyn tdmcp_mcp::BridgeRpc> = Arc::new(sessions.clone());

    // Dialogs watcher (v2 D1): daemon-side popup sampling + window_status fill.
    if cfg.dialogs.enabled && cfg!(windows) {
        let shared = Arc::new(tdmcp_daemon::dialogs::Shared {
            source: tdmcp_daemon::dialogs::build_source(),
            snapshots: std::sync::Mutex::new(std::collections::HashMap::new()),
            intercept: cfg.dialogs.intercept,
        });
        if tdmcp_mcp::dialogs::install(shared.clone()) {
            tokio::spawn(tdmcp_daemon::dialogs::run_dialogs_watcher(
                registry.clone(),
                shared,
                cfg.dialogs.poll_ms,
                shutdown.clone(),
            ));
            info!(poll_ms = cfg.dialogs.poll_ms, "dialogs watcher installed");
        }
    } else {
        info!("dialogs watcher disabled");
    }
    let resource_provider = Arc::new(
        tdmcp_mcp::ResourceProvider::from_embedded()
            .map_err(|e| anyhow::anyhow!("initialize embedded skills resource provider: {e}"))?,
    );

    let federation = FederationRuntime::from_config(&cfg);
    let federation_for_slave = federation.clone();
    let fed_ctx = tdmcp_mcp::FederationCtx {
        local_daemon_id: federation.daemon_id.clone(),
        local_hostname: federation.hostname.clone(),
        slaves: federation.slaves.clone(),
        http: tdmcp_mcp::FederationCtx::build_http_client(),
    };
    let state = AppState::new_shared(registry.clone(), catalog, bridge, resource_provider)
        .with_federation(fed_ctx);
    let admin_state = state.clone();
    let mcp_handler_state = state.clone();
    let mcp_sessions = state.mcp_sessions.clone();

    // Real MCP transport: rmcp Streamable HTTP over the same AppState the
    // JSON fallback (`/mcp/tools/*`) uses. One `McpHandler` per session
    // (legacy mode) — cheap, since `AppState` is Arc-backed.
    //
    // Pin config to match integration tests / idle-exit assumptions: no SSE
    // keepalive (session held by client activity + lease registry), and wire
    // the daemon shutdown token so graceful drain cancels the transport.
    //
    // rmcp's own session-worker keep_alive defaults to 300s — an IDE killed
    // without a clean disconnect would otherwise leave a lease alive for up
    // to 5 minutes, blocking the daemon's own ~30s idle-exit watcher
    // (idle.rs's busy check counts live MCP sessions) the whole time. Shrink
    // it to 60s so a genuinely-dropped connection is reaped promptly; this is
    // independent of `keep_alive` (daemon.toml), which gates whether the
    // daemon exits when idle at all, not whether a dead transport-level
    // session gets swept.
    let mut session_manager = LocalSessionManager::default();
    session_manager.session_config.keep_alive = Some(Duration::from_secs(60));
    let streamable_http: StreamableHttpService<McpHandler, LocalSessionManager> =
        StreamableHttpService::new(
            move || Ok(McpHandler::new(mcp_handler_state.clone())),
            session_manager.into(),
            StreamableHttpServerConfig::default()
                .with_sse_keep_alive(None)
                .with_max_request_body_bytes(WIRE_BODY_LIMIT_BYTES)
                .with_cancellation_token(shutdown.child_token()),
        );

    let restart = RestartArgs {
        exe: exe.clone(),
        port: cfg.port,
        bind_address: cfg.bind_address.clone(),
        data_dir: cfg.data_dir.clone(),
        bridge_dir: cfg.bridge_dir.clone(),
        catalog_path: cfg.catalog_path.clone(),
        no_gui,
    };

    let slave_app = admin_state.clone();
    let auth_state = AuthState {
        mode: cfg.auth_mode.clone(),
        psk: cfg.auth_psk.clone(),
    };

    let app = Router::new()
        .merge(build_mcp_router(state))
        .merge(build_admin_router(
            admin_state,
            restart,
            shutdown.clone(),
            Arc::clone(&quit),
            federation.clone(),
            log_sink.clone(),
            cfg.logging_dir.clone(),
        ))
        .nest_service("/mcp/rpc", streamable_http)
        // Axum's Json/Bytes extractors default to 2 MiB regardless of the
        // rmcp streamable body cap set above, so the same payload could
        // succeed on /mcp/rpc and get rejected on /mcp/tools/call and the
        // federation/admin routes. See docs/LIMITS_AUDIT.md §4.5.
        .layer(DefaultBodyLimit::max(WIRE_BODY_LIMIT_BYTES))
        .layer(from_fn_with_state(auth_state, auth_and_loopback));

    // Already bound in `preflight_bind` above; only recompute `addr` here for
    // the log line below.
    let bind_ip: IpAddr = cfg
        .bind_address
        .parse()
        .with_context(|| format!("parse bind_address {:?}", cfg.bind_address))?;
    let addr = SocketAddr::from((bind_ip, cfg.port));
    // Diagnostic wrapper: log every accepted TCP connection when
    // TDMCP_TRACE_ACCEPT=1 (used to locate multi-client accept stalls).
    let listener = axum::serve::ListenerExt::tap_io(listener, |io| {
        if std::env::var("TDMCP_TRACE_ACCEPT").is_ok() {
            let peer = io
                .peer_addr()
                .map(|a| a.to_string())
                .unwrap_or_else(|_| "?".into());
            tracing::info!(peer, "accepted tcp connection");
        }
    });
    info!(%addr, "listening (MCP rmcp /mcp/rpc + JSON fallback /mcp/* + admin /admin/*)");

    let lock_path = daemon_lock_path(&cfg.data_dir);
    std::fs::create_dir_all(&cfg.data_dir)?;
    std::fs::write(&lock_path, std::process::id().to_string())?;

    // Retention sweep: immediate first pass, then every 24 h until shutdown.
    tokio::spawn(tdmcp_daemon::logring::run_sweep_loop(
        cfg.logging_dir.clone(),
        cfg.data_dir.clone(),
        cfg.logging_retention_days,
        shutdown.clone(),
    ));

    // IPC accept loop: bind the local bridge endpoint and spawn a per-pid
    // session actor for each handshaken TD peer.
    let endpoint = BridgeEndpoint::default_endpoint(&cfg.data_dir);
    let ipc_registry = registry.clone();
    let ipc_sessions = sessions.clone();
    let bridge_dir = cfg.bridge_dir.clone();
    let ipc_handle = tokio::spawn(async move {
        run_ipc_accept(endpoint, bridge_dir, ipc_registry, ipc_sessions).await;
    });

    let _slave_handle = if cfg.federation_role == "slave" {
        Some(spawn_slave_loop(
            federation_for_slave,
            slave_app,
            shutdown.clone(),
        ))
    } else {
        let _ = federation_for_slave;
        None
    };

    let idle_handle = if cfg.keep_alive || cfg.federation_role == "slave" {
        info!(role = %cfg.federation_role, keep_alive = cfg.keep_alive, "idle exit disabled");
        None
    } else if let Some(timeout) = idle_exit_timeout() {
        info!(idle_secs = timeout.as_secs(), "idle exit armed");
        let idle_bridges = sessions.clone();
        let idle_shutdown = shutdown.clone();
        let idle_quit = Arc::clone(&quit);
        Some(tokio::spawn(async move {
            run_idle_watcher(
                idle_bridges,
                mcp_sessions,
                timeout,
                idle_shutdown,
                idle_quit,
            )
            .await;
        }))
    } else {
        info!("idle exit disabled (TDMCP_IDLE_EXIT_SECS=0)");
        None
    };

    let shutdown_for_signal = shutdown.clone();
    let quit_for_signal = Arc::clone(&quit);
    let shutdown_for_deadline = shutdown.clone();
    let serve = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        wait_shutdown(shutdown_for_signal, quit_for_signal).await;
    });

    // Diagnostic heartbeat: proves the runtime is still schedulable while
    // TDMCP_TRACE_ACCEPT is set (accept stalls vs runtime starvation).
    if std::env::var("TDMCP_TRACE_ACCEPT").is_ok() {
        let hb_shutdown = shutdown.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(2));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = ticker.tick() => {
                        info!("heartbeat: runtime schedulable");
                    }
                    _ = hb_shutdown.cancelled() => break,
                }
            }
        });
    }

    tokio::select! {
        result = serve => {
            result?;
        }
        () = async {
            shutdown_for_deadline.cancelled().await;
            tokio::time::sleep(DRAIN_DEADLINE).await;
        } => {
            warn!(
                deadline_ms = DRAIN_DEADLINE.as_millis(),
                "graceful drain deadline exceeded — abandoning serve"
            );
        }
    }

    ipc_handle.abort();
    if let Some(h) = idle_handle {
        h.abort();
    }
    let _ = std::fs::remove_file(&lock_path);
    info!("daemon serve stopped");
    Ok(())
}

/// ctrl_c or external cancel — never `process::exit` here (bg/tokio path).
async fn wait_shutdown(shutdown: CancellationToken, quit: Arc<AtomicBool>) {
    tokio::select! {
        result = tokio::signal::ctrl_c() => {
            if let Err(e) = result {
                warn!(error = %e, "ctrl_c listener failed");
            }
            info!("shutdown signal (ctrl_c)");
            quit.store(true, Ordering::SeqCst);
            shutdown.cancel();
        }
        () = shutdown.cancelled() => {
            info!("shutdown signal (cancel)");
        }
    }
}

/// Retry bind briefly to absorb `/admin/restart` spawn-then-exit port overlap.
async fn bind_with_retry(addr: SocketAddr) -> Result<tokio::net::TcpListener> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut last_err = None;
    while Instant::now() < deadline {
        match tokio::net::TcpListener::bind(addr).await {
            Ok(listener) => return Ok(listener),
            Err(e) => {
                last_err = Some(e);
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
    match last_err {
        Some(e) => Err(e).with_context(|| format!("bind {addr} (retried ~5s)")),
        None => bail!("bind {addr} failed with no error"),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "test assertions may panic")]
mod cli_tests {
    use super::*;

    /// Double-clicking the binary (zero args) must not error — it should
    /// parse to no subcommand, which `main()` defaults to `Commands::Start`.
    #[test]
    fn zero_args_parses_to_no_subcommand() {
        let cli = Cli::try_parse_from(["tdmcp-daemon"]).expect("zero-arg parse");
        assert!(cli.command.is_none());
    }

    #[test]
    fn explicit_start_still_parses() {
        let cli = Cli::try_parse_from(["tdmcp-daemon", "start", "--no-gui"]).expect("start parse");
        assert!(matches!(
            cli.command,
            Some(Commands::Start { no_gui: true, .. })
        ));
    }
}

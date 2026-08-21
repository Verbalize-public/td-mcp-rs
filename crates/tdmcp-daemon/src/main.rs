//! td-mcp-rs daemon — composition root (binary).

#![allow(clippy::exit, reason = "process boundary")]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
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
    daemon_lock_path, ensure_daemon, refuse_if_daemon_owned, EnsureOptions,
};
use tdmcp_daemon::idle::{idle_exit_timeout, run_idle_watcher};
use tdmcp_daemon::install::{self, InstallOutcome};
use tdmcp_diagnostics::Catalog;
use tdmcp_mcp::{build_mcp_router, AppState, McpHandler};

/// Max time to wait for axum to drain after cancel before abandoning serve.
const DRAIN_DEADLINE: Duration = Duration::from_secs(2);
/// Max time for main to join the daemon thread after GUI returns.
const JOIN_DEADLINE: Duration = Duration::from_secs(3);

#[derive(Debug, Parser)]
#[command(name = "tdmcp-daemon", version, about = "td-mcp-rs control plane")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
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
    /// Materialize embedded bridge/catalog/tox into the data directory.
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
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
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
            tdmcp_daemon::tracing_init::init(&cfg)?;
            start_daemon(cfg)
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
                    exe: None,
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
                    exe: None,
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
                // legitimate here). Mid-session reconnect is reconnect-only.
                const MAX_CONNECT_ATTEMPTS: u32 = 3;
                let mut last_err = None;
                for attempt in 1..=MAX_CONNECT_ATTEMPTS {
                    let result = ensure_daemon(opts.clone()).await?;
                    let daemon_url = format!("{}/mcp/rpc", result.base_url);
                    // Stdio MCP: do not print to stdout (JSON-RPC). Logs go via tracing/stderr.
                    match tdmcp_mcp::run_stdio_proxy(&daemon_url).await {
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

fn start_daemon(cfg: Config) -> Result<()> {
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
                    rt.block_on(run_daemon(daemon_cfg, false, shutdown_bg, quit_bg))
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
    rt.block_on(run_daemon(cfg, no_gui, shutdown, quit))
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
) -> Result<()> {
    info!(
        port = cfg.port,
        data_dir = %cfg.data_dir.display(),
        bridge_dir = %cfg.bridge_dir.display(),
        config = %cfg.config_path.display(),
        keep_alive = cfg.keep_alive,
        always_on = cfg.always_on,
        no_gui,
        "starting tdmcp-daemon"
    );

    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("tdmcp-daemon"));
    autostart::reconcile_best_effort(cfg.always_on, &exe);

    refuse_if_daemon_owned(&cfg.data_dir, cfg.port).await?;

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
        .with_timeouts(timeouts);
    let bridge: Arc<dyn tdmcp_mcp::BridgeRpc> = Arc::new(sessions.clone());
    let state = AppState::new_shared(registry.clone(), catalog, bridge);
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
    let streamable_http: StreamableHttpService<McpHandler, LocalSessionManager> =
        StreamableHttpService::new(
            move || Ok(McpHandler::new(mcp_handler_state.clone())),
            Default::default(),
            StreamableHttpServerConfig::default()
                .with_sse_keep_alive(None)
                .with_cancellation_token(shutdown.child_token()),
        );

    let restart = RestartArgs {
        exe: exe.clone(),
        port: cfg.port,
        data_dir: cfg.data_dir.clone(),
        bridge_dir: cfg.bridge_dir.clone(),
        catalog_path: cfg.catalog_path.clone(),
        no_gui,
    };

    let app = Router::new()
        .merge(build_mcp_router(state))
        .merge(build_admin_router(
            admin_state,
            restart,
            shutdown.clone(),
            Arc::clone(&quit),
        ))
        .nest_service("/mcp/rpc", streamable_http);

    let addr = SocketAddr::from(([127, 0, 0, 1], cfg.port));
    let listener = bind_with_retry(addr).await?;
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

    // IPC accept loop: bind the local bridge endpoint and spawn a per-pid
    // session actor for each handshaken TD peer.
    let endpoint = BridgeEndpoint::default_endpoint(&cfg.data_dir);
    let ipc_registry = registry.clone();
    let ipc_sessions = sessions.clone();
    let bridge_dir = cfg.bridge_dir.clone();
    let ipc_handle = tokio::spawn(async move {
        run_ipc_accept(endpoint, bridge_dir, ipc_registry, ipc_sessions).await;
    });

    let idle_handle = if cfg.keep_alive {
        info!("idle exit disabled (keep_alive=true)");
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
    let serve = axum::serve(listener, app).with_graceful_shutdown(async move {
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

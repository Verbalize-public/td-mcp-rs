//! td-mcp-rs daemon — composition root (binary).

#![allow(clippy::exit, reason = "process boundary")]

use std::net::SocketAddr;
use std::path::PathBuf;
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
use tracing::{info, warn};

use tdmcp_daemon::admin::{build_admin_router, RestartArgs};
use tdmcp_daemon::bridge::{run_ipc_accept, BridgeSessions};
use tdmcp_daemon::config::Config;
use tdmcp_daemon::ensure::{
    daemon_lock_path, ensure_daemon, refuse_if_daemon_owned, EnsureOptions,
};
use tdmcp_daemon::idle::{idle_exit_timeout, run_idle_watcher};
use tdmcp_daemon::install::{self, InstallOutcome};
use tdmcp_diagnostics::Catalog;
use tdmcp_mcp::{build_mcp_router, AppState, McpHandler};

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
            let cfg = Config::load(port, data_dir, bridge_dir, catalog)?;
            // Ensure embedded assets exist under data_dir (no-op when current).
            let _ = install::ensure_installed(&cfg.data_dir)?;
            tdmcp_daemon::tracing_init::init(&cfg)?;
            start_daemon(cfg, no_gui)
        }
        Commands::Install { data_dir } => {
            let data_dir = data_dir.unwrap_or_else(install::default_data_dir);
            match install::ensure_installed(&data_dir)? {
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
            Ok(())
        }
        Commands::Ensure {
            port,
            data_dir,
            timeout_ms,
            no_gui,
        } => {
            let rt = tokio::runtime::Runtime::new().context("tokio runtime")?;
            rt.block_on(async {
                let opts = EnsureOptions {
                    port: port.unwrap_or(9860),
                    data_dir: data_dir.unwrap_or_else(install::default_data_dir),
                    exe: None,
                    timeout: Duration::from_millis(timeout_ms),
                    poll_only: false,
                    no_gui,
                    idle_exit_secs: None,
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
            let rt = tokio::runtime::Runtime::new().context("tokio runtime")?;
            rt.block_on(async {
                let port = port.unwrap_or(9860);
                let opts = EnsureOptions {
                    port,
                    data_dir: data_dir.unwrap_or_else(install::default_data_dir),
                    exe: None,
                    timeout: Duration::from_millis(timeout_ms),
                    poll_only: false,
                    no_gui,
                    idle_exit_secs: None,
                };
                let result = ensure_daemon(opts).await?;
                let daemon_url = format!("{}/mcp/rpc", result.base_url);
                // Stdio MCP: do not print to stdout (JSON-RPC). Logs go via tracing/stderr.
                tdmcp_mcp::run_stdio_proxy(&daemon_url)
                    .await
                    .map_err(|e| anyhow::anyhow!(e))?;
                Ok(())
            })
        }
        Commands::Status { port } => {
            let rt = tokio::runtime::Runtime::new().context("tokio runtime")?;
            rt.block_on(async {
                let port = port.unwrap_or(9860);
                let url = format!("http://127.0.0.1:{port}/mcp/health");
                match http_get(&url).await {
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
            let rt = tokio::runtime::Runtime::new().context("tokio runtime")?;
            rt.block_on(async {
                let port = port.unwrap_or(9860);
                let url = format!("http://127.0.0.1:{port}/admin/shutdown");
                match http_post_empty(&url).await {
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

fn start_daemon(cfg: Config, no_gui: bool) -> Result<()> {
    #[cfg(feature = "gui")]
    {
        if !no_gui {
            let admin_base = format!("http://127.0.0.1:{}", cfg.port);
            let daemon_cfg = cfg;
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
                    rt.block_on(run_daemon(daemon_cfg, false))
                })
                .context("spawn daemon background thread")?;

            // eframe/winit require the real main thread.
            let gui_result = tdmcp_gui::run(admin_base);
            match handle.join() {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    warn!(error = %e, "daemon thread exited with error");
                    if gui_result.is_ok() {
                        return Err(e);
                    }
                }
                Err(_) => warn!("daemon thread panicked"),
            }
            return gui_result;
        }
    }
    #[cfg(not(feature = "gui"))]
    {
        let _ = no_gui;
    }

    let rt = tokio::runtime::Runtime::new().context("tokio runtime")?;
    rt.block_on(run_daemon(cfg, no_gui))
}

async fn run_daemon(cfg: Config, no_gui: bool) -> Result<()> {
    info!(
        port = cfg.port,
        data_dir = %cfg.data_dir.display(),
        bridge_dir = %cfg.bridge_dir.display(),
        no_gui,
        "starting tdmcp-daemon"
    );

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
    let sessions = BridgeSessions::new(registry.clone());
    let bridge: Arc<dyn tdmcp_mcp::BridgeRpc> = Arc::new(sessions.clone());
    let state = AppState::new_shared(registry.clone(), catalog, bridge);
    let admin_state = state.clone();
    let mcp_handler_state = state.clone();
    let mcp_sessions = state.mcp_sessions.clone();

    // Real MCP transport: rmcp Streamable HTTP over the same AppState the
    // JSON fallback (`/mcp/tools/*`) uses. One `McpHandler` per session
    // (legacy mode) — cheap, since `AppState` is Arc-backed.
    let streamable_http: StreamableHttpService<McpHandler, LocalSessionManager> =
        StreamableHttpService::new(
            move || Ok(McpHandler::new(mcp_handler_state.clone())),
            Default::default(),
            StreamableHttpServerConfig::default(),
        );

    let restart = RestartArgs {
        exe: std::env::current_exe().unwrap_or_else(|_| PathBuf::from("tdmcp-daemon")),
        port: cfg.port,
        data_dir: cfg.data_dir.clone(),
        bridge_dir: cfg.bridge_dir.clone(),
        catalog_path: cfg.catalog_path.clone(),
        no_gui,
    };

    let app = Router::new()
        .merge(build_mcp_router(state))
        .merge(build_admin_router(admin_state, restart))
        .nest_service("/mcp/rpc", streamable_http);

    let addr = SocketAddr::from(([127, 0, 0, 1], cfg.port));
    let listener = bind_with_retry(addr).await?;
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

    let idle_handle = if let Some(timeout) = idle_exit_timeout() {
        info!(idle_secs = timeout.as_secs(), "idle exit armed");
        let idle_bridges = sessions.clone();
        let idle_data_dir = cfg.data_dir.clone();
        Some(tokio::spawn(async move {
            run_idle_watcher(idle_bridges, mcp_sessions, idle_data_dir, timeout).await;
        }))
    } else {
        info!("idle exit disabled (TDMCP_IDLE_EXIT_SECS=0)");
        None
    };

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    ipc_handle.abort();
    if let Some(h) = idle_handle {
        h.abort();
    }
    let _ = std::fs::remove_file(&lock_path);
    Ok(())
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

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    info!("shutdown signal");
}

fn port_from_url(url: &str) -> u16 {
    url.rsplit('/')
        .nth(1)
        .and_then(|hostport| hostport.rsplit(':').next())
        .and_then(|p| p.parse().ok())
        .unwrap_or(9860)
}

async fn http_get(url: &str) -> Result<String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let port = port_from_url(url);
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
    let req = "GET /mcp/health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n";
    stream.write_all(req.as_bytes()).await?;
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

async fn http_post_empty(url: &str) -> Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let port = port_from_url(url);
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
    let req = "POST /admin/shutdown HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    stream.write_all(req.as_bytes()).await?;
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await?;
    Ok(())
}

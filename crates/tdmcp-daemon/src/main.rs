//! td-mcp-rs daemon — composition root (binary).

#![allow(clippy::exit, reason = "process boundary")]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
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
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Start {
            port,
            data_dir,
            bridge_dir,
            catalog,
        } => {
            let cfg = Config::load(port, data_dir, bridge_dir, catalog)?;
            tdmcp_daemon::tracing_init::init(&cfg)?;
            run_daemon(cfg).await
        }
        Commands::Status { port } => {
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
        }
        Commands::Stop { port } => {
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
        }
    }
}

async fn run_daemon(cfg: Config) -> Result<()> {
    info!(
        port = cfg.port,
        data_dir = %cfg.data_dir.display(),
        bridge_dir = %cfg.bridge_dir.display(),
        "starting tdmcp-daemon"
    );

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
    };

    let app = Router::new()
        .merge(build_mcp_router(state))
        .merge(build_admin_router(admin_state, restart))
        .nest_service("/mcp/rpc", streamable_http);

    let addr = SocketAddr::from(([127, 0, 0, 1], cfg.port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("bind {addr}"))?;
    info!(%addr, "listening (MCP rmcp /mcp/rpc + JSON fallback /mcp/* + admin /admin/*)");

    let lock_path = cfg.data_dir.join("daemon.lock");
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

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    ipc_handle.abort();
    let _ = std::fs::remove_file(&lock_path);
    Ok(())
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

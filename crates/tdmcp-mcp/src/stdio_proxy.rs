//! Stdio MCP server that proxies tool calls to a remote Streamable HTTP daemon.
//!
//! v1 implements request/response forwarding only (`list_tools` / `get_tool` /
//! `call_tool`). Server-initiated notifications are not forwarded.

use std::sync::Arc;

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, Implementation, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::{RequestContext, RunningService};
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::{ErrorData, Peer, RoleClient, RoleServer, ServerHandler, ServiceError, ServiceExt};
use tokio::io::{AsyncRead, AsyncWrite};
use tracing::{debug, info, warn};

/// Errors from the stdio↔HTTP MCP proxy.
#[derive(Debug, thiserror::Error)]
pub enum StdioProxyError {
    /// Failed to connect or initialize against the daemon HTTP MCP endpoint.
    #[error("stdio_proxy: connect to daemon failed: {0}")]
    Connect(String),
    /// Failed to serve the stdio (or test) MCP server transport.
    #[error("stdio_proxy: serve failed: {0}")]
    Serve(String),
    /// Underlying join / wait failure after the session ended.
    #[error("stdio_proxy: session ended with error: {0}")]
    Session(String),
}

/// Run a stdio MCP server that proxies to `daemon_url`
/// (e.g. `http://127.0.0.1:9860/mcp/rpc`).
///
/// Blocks until the stdio client disconnects.
pub async fn run(daemon_url: &str) -> Result<(), StdioProxyError> {
    run_with_transport(daemon_url, rmcp::transport::stdio()).await
}

/// Like [`run`], but with an arbitrary AsyncRead+AsyncWrite pair (tests).
pub async fn run_with_rw<R, W>(daemon_url: &str, read: R, write: W) -> Result<(), StdioProxyError>
where
    R: AsyncRead + Send + Unpin + 'static,
    W: AsyncWrite + Send + Unpin + 'static,
{
    run_with_transport(daemon_url, (read, write)).await
}

async fn run_with_transport<T, E, A>(daemon_url: &str, transport: T) -> Result<(), StdioProxyError>
where
    T: rmcp::transport::IntoTransport<RoleServer, E, A> + Send + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    info!(%daemon_url, "stdio_proxy: connecting to daemon");
    let http = StreamableHttpClientTransport::from_uri(daemon_url.to_string());
    let client: RunningService<RoleClient, ()> = ()
        .serve(http)
        .await
        .map_err(|e| StdioProxyError::Connect(e.to_string()))?;

    let proxy = StdioProxy {
        peer: Arc::new(client.peer().clone()),
    };

    let server = proxy
        .serve(transport)
        .await
        .map_err(|e| StdioProxyError::Serve(e.to_string()))?;

    info!("stdio_proxy: serving (request/response only; notifications not forwarded)");
    let quit = server
        .waiting()
        .await
        .map_err(|e| StdioProxyError::Session(e.to_string()))?;
    debug!(?quit, "stdio_proxy: stdio session ended");
    let _ = client.cancel().await;
    Ok(())
}

/// Stdio-facing handler that forwards tool ops to a daemon [`Peer`].
#[derive(Clone)]
struct StdioProxy {
    peer: Arc<Peer<RoleClient>>,
}

impl ServerHandler for StdioProxy {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "tdmcp-daemon",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(
                "td-mcp-rs control plane (stdio proxy). Call `fleet` to discover connected \
                 TouchDesigner processes by pid, then `execute_python` / `inspect` / `capture` \
                 against a pid. v1 proxy forwards tools only — server notifications are not \
                 forwarded.",
            )
    }

    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        self.peer
            .list_tools(request)
            .await
            .map_err(service_err_to_error_data)
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        // Sync API — cannot await the peer. Clients use list_tools for the catalog.
        let _ = name;
        None
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        match self.peer.call_tool_once(request).await {
            Ok(response) => Ok(response),
            Err(e) => {
                warn!(error = %e, "stdio_proxy: call_tool forward failed");
                Err(service_err_to_error_data(e))
            }
        }
    }
}

fn service_err_to_error_data(err: ServiceError) -> ErrorData {
    ErrorData::internal_error(err.to_string(), None)
}

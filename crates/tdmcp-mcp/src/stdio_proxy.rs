//! Stdio MCP server that proxies tool calls to a remote Streamable HTTP daemon.
//!
//! v1 implements request/response forwarding only (`list_tools` / `get_tool` /
//! `call_tool`). Server-initiated notifications are not forwarded.

use std::sync::Arc;

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, ClientCapabilities, ClientInfo, Implementation,
    InitializeRequestParams, InitializeResult, ListToolsResult, PaginatedRequestParams,
    ProtocolVersion, ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::{RequestContext, RunningService};
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::{ErrorData, Peer, RoleClient, RoleServer, ServerHandler, ServiceError, ServiceExt};
use tokio::io::{AsyncRead, AsyncWrite};
use tracing::{debug, info, warn};

/// Client name advertised on the HTTP side so the daemon GUI can list the lease.
pub const STDIO_PROXY_CLIENT_NAME: &str = "tdmcp-stdio-proxy";

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
    let client_info = ClientInfo::new(
        ClientCapabilities::default(),
        Implementation::new(STDIO_PROXY_CLIENT_NAME, env!("CARGO_PKG_VERSION")),
    );
    let client: RunningService<RoleClient, ClientInfo> = client_info
        .serve(http)
        .await
        .map_err(|e| StdioProxyError::Connect(e.to_string()))?;

    let admin_base = admin_base_from_daemon_url(daemon_url);
    let proxy = StdioProxy {
        peer: Arc::new(client.peer().clone()),
        admin_base,
        annotated: Arc::new(std::sync::atomic::AtomicBool::new(false)),
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

/// Derive `http://127.0.0.1:9860` from `http://127.0.0.1:9860/mcp/rpc`.
fn admin_base_from_daemon_url(daemon_url: &str) -> String {
    let trimmed = daemon_url.trim_end_matches('/');
    trimmed
        .strip_suffix("/mcp/rpc")
        .or_else(|| trimmed.strip_suffix("/mcp"))
        .unwrap_or(trimmed)
        .to_owned()
}

/// Stdio-facing handler that forwards tool ops to a daemon [`Peer`].
#[derive(Clone)]
struct StdioProxy {
    peer: Arc<Peer<RoleClient>>,
    admin_base: String,
    annotated: Arc<std::sync::atomic::AtomicBool>,
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

    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, ErrorData> {
        context.peer.set_peer_info(request.clone());
        // Best-effort: annotate the daemon-side HTTP lease with the IDE clientInfo.
        if !self
            .annotated
            .swap(true, std::sync::atomic::Ordering::Relaxed)
        {
            let admin_base = self.admin_base.clone();
            let name = request.client_info.name.clone();
            let version = request.client_info.version.clone();
            tokio::spawn(async move {
                if let Err(e) = annotate_daemon_session(&admin_base, &name, &version).await {
                    warn!(error = %e, "stdio_proxy: annotate MCP session failed");
                }
            });
        }
        let mut info = self.get_info();
        if ProtocolVersion::KNOWN_VERSIONS.contains(&request.protocol_version) {
            info.protocol_version = request.protocol_version.clone();
        }
        Ok(info)
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

async fn annotate_daemon_session(
    admin_base: &str,
    client_name: &str,
    client_version: &str,
) -> Result<(), String> {
    let url = format!("{admin_base}/admin/mcp-sessions/annotate");
    let body = serde_json::json!({
        "matchClientName": STDIO_PROXY_CLIENT_NAME,
        "clientName": client_name,
        "clientVersion": client_version,
    });
    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    Ok(())
}

fn service_err_to_error_data(err: ServiceError) -> ErrorData {
    match err {
        // Preserve upstream protocol codes (e.g. -32602 invalid_params for bad include).
        ServiceError::McpError(data) => data,
        other => ErrorData::internal_error(other.to_string(), None),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;

    #[test]
    fn admin_base_strips_mcp_rpc() {
        assert_eq!(
            admin_base_from_daemon_url("http://127.0.0.1:9860/mcp/rpc"),
            "http://127.0.0.1:9860"
        );
        assert_eq!(
            admin_base_from_daemon_url("http://127.0.0.1:9860/mcp/rpc/"),
            "http://127.0.0.1:9860"
        );
    }
}

//! Stdio MCP server that proxies tool *calls* to a remote Streamable HTTP daemon.
//!
//! `list_tools` / `get_tool` / operate **resources** (`tdmcp://docs/*`) are served
//! locally from the embedded catalog (no HTTP round-trip) so Cursor and other
//! clients always see a stable tool/resource list even if the daemon is mid-restart.
//! `call_tool` still forwards to the HTTP daemon.
//!
//! The HTTP daemon link is established **after** stdio `initialize` can proceed
//! (background prefetch + lazy ensure on first tool call). Blocking on the HTTP
//! handshake before reading stdio caused Cursor `Client closed` when the daemon
//! was slow or mid-restart.
//!
//! Server-initiated notifications are not forwarded.
//!
//! When the HTTP daemon connection is lost, the proxy attempts a reconnect-only
//! heal (never spawns a daemon) and returns an informative
//! `tdmcp.daemon.unreachable` error for the failed call. The next call benefits
//! from a healed link.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, Implementation, InitializeRequestParams,
    InitializeResult, ListResourcesResult, ListToolsResult, PaginatedRequestParams, ProtocolVersion,
    ReadResourceRequestParams, ReadResourceResponse, ServerInfo, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceError, ServiceExt};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::OnceCell;
use tracing::{debug, info, warn};

use crate::daemon_link::{
    call_timeout_error, is_transport_error, unreachable_error, DaemonLink, HealOutcome,
    ReconnectConfig,
};
use crate::resources::{self, STDIO_SERVER_INSTRUCTIONS};
use crate::schema::input_schema_for;
use crate::tools::{tool_descriptors, ToolName};

/// Client name advertised on the HTTP side so the daemon GUI can list the lease.
pub use crate::daemon_link::STDIO_PROXY_CLIENT_NAME;

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

impl StdioProxyError {
    /// Whether this is an initial HTTP connect/handshake failure (cold-start retryable).
    #[must_use]
    pub fn is_connect(&self) -> bool {
        matches!(self, Self::Connect(_))
    }
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

/// Like [`run_with_rw`], with an explicit reconnect config (tests).
pub async fn run_with_rw_config<R, W>(
    daemon_url: &str,
    read: R,
    write: W,
    config: ReconnectConfig,
) -> Result<(), StdioProxyError>
where
    R: AsyncRead + Send + Unpin + 'static,
    W: AsyncWrite + Send + Unpin + 'static,
{
    run_with_transport_config(daemon_url, (read, write), config).await
}

async fn run_with_transport<T, E, A>(daemon_url: &str, transport: T) -> Result<(), StdioProxyError>
where
    T: rmcp::transport::IntoTransport<RoleServer, E, A> + Send + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    run_with_transport_config(daemon_url, transport, ReconnectConfig::from_env()).await
}

async fn run_with_transport_config<T, E, A>(
    daemon_url: &str,
    transport: T,
    config: ReconnectConfig,
) -> Result<(), StdioProxyError>
where
    T: rmcp::transport::IntoTransport<RoleServer, E, A> + Send + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    let admin_base = admin_base_from_daemon_url(daemon_url);
    info!(%daemon_url, "stdio_proxy: serving stdio (daemon link lazy)");

    let proxy = StdioProxy {
        daemon_url: daemon_url.to_owned(),
        admin_base: admin_base.clone(),
        config: config.clone(),
        link: Arc::new(OnceCell::new()),
        pending_ide: Arc::new(Mutex::new(None)),
    };

    // Prefetch HTTP link without blocking Cursor initialize / tools/list.
    {
        let prefetch = Arc::clone(&proxy.link);
        let url = daemon_url.to_owned();
        let admin = admin_base.clone();
        let cfg = config.clone();
        let pending = Arc::clone(&proxy.pending_ide);
        tokio::spawn(async move {
            match DaemonLink::connect(&url, admin, cfg).await {
                Ok(link) => {
                    apply_pending_ide(&link, &pending);
                    let _ = prefetch.set(link);
                }
                Err(e) => {
                    warn!(error = %e, "stdio_proxy: background daemon connect failed");
                }
            }
        });
    }

    let link_cell = Arc::clone(&proxy.link);
    let server = proxy
        .serve(transport)
        .await
        .map_err(|e| StdioProxyError::Serve(e.to_string()))?;

    info!("stdio_proxy: initialized (request/response only; notifications not forwarded)");
    let quit = server
        .waiting()
        .await
        .map_err(|e| StdioProxyError::Session(e.to_string()))?;
    debug!(?quit, "stdio_proxy: stdio session ended");
    if let Some(link) = link_cell.get() {
        link.shutdown().await;
    }
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

/// Stdio-facing handler that forwards tool ops through a lazily connected [`DaemonLink`].
#[derive(Clone)]
struct StdioProxy {
    daemon_url: String,
    admin_base: String,
    config: ReconnectConfig,
    link: Arc<OnceCell<Arc<DaemonLink>>>,
    pending_ide: Arc<Mutex<Option<(String, String)>>>,
}

impl ServerHandler for StdioProxy {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(resources::server_capabilities())
            .with_server_info(Implementation::new(
                "tdmcp-daemon",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(STDIO_SERVER_INSTRUCTIONS)
    }

    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, ErrorData> {
        context.peer.set_peer_info(request.clone());
        let name = request.client_info.name.clone();
        let version = request.client_info.version.clone();
        if let Some(link) = self.link.get() {
            link.set_ide_client(name.clone(), version.clone());
        } else if let Ok(mut guard) = self.pending_ide.lock() {
            *guard = Some((name.clone(), version.clone()));
        }
        // Best-effort: annotate the daemon-side HTTP lease with the IDE clientInfo.
        let admin_base = self
            .link
            .get()
            .map(|l| l.admin_base().to_owned())
            .unwrap_or_else(|| self.admin_base.clone());
        tokio::spawn(async move {
            if let Err(e) = annotate_daemon_session(&admin_base, &name, &version).await {
                warn!(error = %e, "stdio_proxy: annotate MCP session failed");
            }
        });
        let mut info = self.get_info();
        if ProtocolVersion::KNOWN_VERSIONS.contains(&request.protocol_version) {
            info.protocol_version = request.protocol_version.clone();
        }
        Ok(info)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        // Local catalog — do not depend on HTTP forward for discovery (Cursor
        // tool count / enable UI reads this on connect).
        let tools = tool_descriptors()
            .into_iter()
            .map(tool_from_descriptor)
            .collect();
        Ok(ListToolsResult::with_all_items(tools))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        tool_descriptors()
            .into_iter()
            .find(|d| d.name == name)
            .map(tool_from_descriptor)
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        Ok(resources::list_resources())
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        match resources::read_resource(&request.uri) {
            Ok(result) => Ok(ReadResourceResponse::Complete(result)),
            Err(msg) => Err(ErrorData::resource_not_found(msg, None)),
        }
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let link = self.ensure_link().await?;
        let budget = link.config().tool_call_budget(&request.name);
        self.forward_bounded(&link, budget, |peer| async move {
            peer.call_tool_once(request).await
        })
        .await
    }
}

fn tool_from_descriptor(d: crate::tools::ToolDescriptor) -> Tool {
    let schema = if d.input_schema.is_empty() {
        let tool = ToolName::from_wire(&d.name).unwrap_or(ToolName::DescribeTools);
        Arc::new(input_schema_for(tool))
    } else {
        Arc::new(d.input_schema)
    };
    Tool::new(d.name.clone(), d.description, schema)
}

impl StdioProxy {
    /// Resolve (or create) the HTTP daemon link. Discovery never needs this.
    async fn ensure_link(&self) -> Result<Arc<DaemonLink>, ErrorData> {
        if let Some(link) = self.link.get() {
            return Ok(Arc::clone(link));
        }
        match DaemonLink::connect(&self.daemon_url, self.admin_base.clone(), self.config.clone())
            .await
        {
            Ok(link) => {
                apply_pending_ide(&link, &self.pending_ide);
                match self.link.set(Arc::clone(&link)) {
                    Ok(()) => Ok(link),
                    Err(_existing) => Ok(Arc::clone(self.link.get().expect("OnceCell set raced"))),
                }
            }
            Err(e) => {
                warn!(error = %e, "stdio_proxy: daemon connect failed on tool call");
                Err(unreachable_error(
                    HealOutcome {
                        healed: false,
                        downtime: None,
                    },
                    &self.config,
                ))
            }
        }
    }

    /// Forward one operation with a hard wall-clock budget.
    ///
    /// A wedged daemon session would otherwise hang the MCP client forever:
    /// rmcp's per-session worker blocks on a full SSE stream channel when the
    /// client stops reading, and every new request from that session queues up
    /// behind it with no server-side timeout. On budget expiry we treat the
    /// call like a transport failure: heal the link (fresh session), which
    /// also closes the old HTTP connections and lets the daemon-side session
    /// unwedge and drain.
    async fn forward_bounded<F, Fut, T>(
        &self,
        link: &DaemonLink,
        budget: Duration,
        op: F,
    ) -> Result<T, ErrorData>
    where
        F: FnOnce(Arc<rmcp::Peer<rmcp::RoleClient>>) -> Fut,
        Fut: std::future::Future<Output = Result<T, ServiceError>>,
    {
        let (peer, gen) = link.current_peer().await;
        match tokio::time::timeout(budget, op(peer)).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(e)) if is_transport_error(&e) => {
                warn!(error = %e, "stdio_proxy: transport error — attempting heal");
                let outcome = link.heal(gen).await;
                Err(unreachable_error(outcome, link.config()))
            }
            Ok(Err(e)) => {
                warn!(error = %e, "stdio_proxy: call forward failed");
                Err(service_err_to_error_data(e))
            }
            Err(_) => {
                warn!(
                    budget_ms = budget.as_millis(),
                    "stdio_proxy: call exceeded budget — healing link"
                );
                let outcome = link.heal(gen).await;
                Err(call_timeout_error(budget, outcome))
            }
        }
    }
}

fn apply_pending_ide(link: &DaemonLink, pending: &Mutex<Option<(String, String)>>) {
    let Some((name, version)) = pending.lock().ok().and_then(|mut g| g.take()) else {
        return;
    };
    link.set_ide_client(name, version);
}

/// Whether the daemon-side wait budget for this method is the long script
/// timeout (mirrors `tdmcp_daemon::bridge::BridgeTimeouts::for_method`).
#[cfg(test)]
fn is_script_class(name: &str) -> bool {
    matches!(name, "execute_python" | "mutate_nodes")
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

    #[test]
    fn script_class_detection() {
        assert!(is_script_class("execute_python"));
        assert!(is_script_class("mutate_nodes"));
        assert!(!is_script_class("fleet"));
        assert!(!is_script_class("inspect"));
        assert!(!is_script_class("capture"));
        assert!(!is_script_class("api_help"));
        assert!(!is_script_class("editor_context"));
        assert!(!is_script_class("list_tools"));
    }
}

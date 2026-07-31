//! Stdio MCP server that proxies tool calls to a remote Streamable HTTP daemon.
//!
//! v1 implements request/response forwarding only (`list_tools` / `get_tool` /
//! `call_tool`). Server-initiated notifications are not forwarded.
//!
//! When the HTTP daemon connection is lost, the proxy attempts a reconnect-only
//! heal (never spawns a daemon) and returns an informative
//! `tdmcp.daemon.unreachable` error for the failed call. The next call benefits
//! from a healed link.

use std::sync::Arc;

use rmcp::model::{
	CallToolRequestParams, CallToolResponse, InitializeRequestParams, InitializeResult,
	ListToolsResult, PaginatedRequestParams, ProtocolVersion, ServerCapabilities, ServerInfo,
	Tool, Implementation,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceError, ServiceExt};
use tokio::io::{AsyncRead, AsyncWrite};
use tracing::{debug, info, warn};

use crate::daemon_link::{
	is_transport_error, unreachable_error, DaemonLink, ReconnectConfig,
};

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
	info!(%daemon_url, "stdio_proxy: connecting to daemon");
	let admin_base = admin_base_from_daemon_url(daemon_url);
	let link = DaemonLink::connect(daemon_url, admin_base, config)
		.await
		.map_err(StdioProxyError::Connect)?;

	let proxy = StdioProxy {
		link: Arc::clone(&link),
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
	link.shutdown().await;
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

/// Stdio-facing handler that forwards tool ops through a [`DaemonLink`].
#[derive(Clone)]
struct StdioProxy {
	link: Arc<DaemonLink>,
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
				 forwarded. If the daemon restarts, the proxy reconnects (never auto-spawns) and \
				 returns `tdmcp.daemon.unreachable` for the failed call.",
			)
	}

	async fn initialize(
		&self,
		request: InitializeRequestParams,
		context: RequestContext<RoleServer>,
	) -> Result<InitializeResult, ErrorData> {
		context.peer.set_peer_info(request.clone());
		self.link.set_ide_client(
			request.client_info.name.clone(),
			request.client_info.version.clone(),
		);
		// Best-effort: annotate the daemon-side HTTP lease with the IDE clientInfo.
		let admin_base = self.link.admin_base().to_owned();
		let name = request.client_info.name.clone();
		let version = request.client_info.version.clone();
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
		request: Option<PaginatedRequestParams>,
		_context: RequestContext<RoleServer>,
	) -> Result<ListToolsResult, ErrorData> {
		self.forward(|peer| async move { peer.list_tools(request).await })
			.await
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
		self.forward(|peer| async move { peer.call_tool_once(request).await })
			.await
	}
}

impl StdioProxy {
	async fn forward<F, Fut, T>(&self, op: F) -> Result<T, ErrorData>
	where
		F: FnOnce(Arc<rmcp::Peer<rmcp::RoleClient>>) -> Fut,
		Fut: std::future::Future<Output = Result<T, ServiceError>>,
	{
		let (peer, gen) = self.link.current_peer().await;
		match op(peer).await {
			Ok(response) => Ok(response),
			Err(e) if is_transport_error(&e) => {
				warn!(error = %e, "stdio_proxy: transport error — attempting heal");
				let outcome = self.link.heal(gen).await;
				Err(unreachable_error(outcome, self.link.config()))
			}
			Err(e) => {
				warn!(error = %e, "stdio_proxy: call forward failed");
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

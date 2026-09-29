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
//! When the HTTP daemon connection is lost, the proxy attempts to reconnect.
//! With the CLI-supplied respawn callback, sustained downtime can also ensure a
//! daemon; callers without that callback remain reconnect-only. The failed call
//! returns `tdmcp.daemon.unreachable` and is never silently replayed. A later
//! call can use the healed link; possible effects still require reconciliation.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rmcp::model::{
    CacheScope, CallToolRequestParams, CallToolResponse, Implementation, InitializeRequestParams,
    InitializeResult, ListResourceTemplatesResult, ListResourcesResult, ListToolsResult,
    PaginatedRequestParams, ProtocolVersion, ReadResourceRequestParams, ReadResourceResponse,
    ResultType, ServerInfo, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceError, ServiceExt};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, OnceCell};
use tracing::field::{Field, Visit};
use tracing::{debug, info, warn, Event};
use tracing_subscriber::layer::{Context as LayerContext, Layer};
use tracing_subscriber::{prelude::*, EnvFilter};

use crate::daemon_link::{
    call_timeout_error, is_transport_error, launch_timeout_error, unreachable_error, DaemonLink,
    HealOutcome, ReconnectConfig, RespawnFn,
};
use crate::resources::{server_capabilities, ResourceProvider, STDIO_SERVER_INSTRUCTIONS};
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
    run_with_transport(daemon_url, rmcp::transport::stdio(), None).await
}

/// Like [`run`], but escalates to `respawn` (typically `ensure_daemon`,
/// injected from `main.rs`) when the daemon stays unreachable past the
/// reconnect config's `stale` threshold, instead of only reconnecting.
pub async fn run_with_respawn(daemon_url: &str, respawn: RespawnFn) -> Result<(), StdioProxyError> {
    run_with_transport(daemon_url, rmcp::transport::stdio(), Some(respawn)).await
}

/// Like [`run`], but with an arbitrary AsyncRead+AsyncWrite pair (tests).
pub async fn run_with_rw<R, W>(daemon_url: &str, read: R, write: W) -> Result<(), StdioProxyError>
where
    R: AsyncRead + Send + Unpin + 'static,
    W: AsyncWrite + Send + Unpin + 'static,
{
    run_with_transport(daemon_url, (read, write), None).await
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
    run_with_transport_config(daemon_url, (read, write), config, None).await
}

async fn run_with_transport<T, E, A>(
    daemon_url: &str,
    transport: T,
    respawn: Option<RespawnFn>,
) -> Result<(), StdioProxyError>
where
    T: rmcp::transport::IntoTransport<RoleServer, E, A> + Send + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    run_with_transport_config(daemon_url, transport, ReconnectConfig::from_env(), respawn).await
}

async fn run_with_transport_config<T, E, A>(
    daemon_url: &str,
    transport: T,
    config: ReconnectConfig,
    respawn: Option<RespawnFn>,
) -> Result<(), StdioProxyError>
where
    T: rmcp::transport::IntoTransport<RoleServer, E, A> + Send + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    let admin_base = admin_base_from_daemon_url(daemon_url);
    // Only the real production entrypoint (`run_with_respawn`, the sole
    // caller that passes a respawn fn) installs the global subscriber — the
    // plain `run`/`run_with_rw*` variants exist for tests/manual use and
    // must not fight over (or silently no-op on) a process-wide singleton.
    if respawn.is_some() {
        install_log_uplink(admin_base.clone());
    }
    info!(%daemon_url, "serving stdio (daemon link lazy)");

    // Same gate as the daemon: a card teaching a tool this build does not
    // register is withheld rather than offered as a name that would then be
    // refused. Derived from live tool registration, so it cannot drift.
    let gate =
        crate::CapabilityGate::from_tools(crate::tool_descriptors().iter().map(|d| d.name.clone()));
    let resource_provider = Arc::new(
        ResourceProvider::from_embedded_with_gate(gate)
            .map_err(|e| StdioProxyError::Serve(format!("resource provider: {e}")))?,
    );

    let proxy = StdioProxy {
        daemon_url: daemon_url.to_owned(),
        admin_base: admin_base.clone(),
        config: config.clone(),
        link: Arc::new(OnceCell::new()),
        pending_ide: Arc::new(Mutex::new(None)),
        resource_provider,
        respawn: respawn.clone(),
    };

    // Prefetch HTTP link without blocking Cursor initialize / tools/list.
    {
        let prefetch = Arc::clone(&proxy.link);
        let url = daemon_url.to_owned();
        let admin = admin_base.clone();
        let cfg = config.clone();
        let pending = Arc::clone(&proxy.pending_ide);
        let respawn = respawn.clone();
        tokio::spawn(async move {
            match DaemonLink::connect_with_respawn(&url, admin, cfg, respawn).await {
                Ok(link) => {
                    apply_pending_ide(&link, &pending);
                    let _ = prefetch.set(link);
                }
                Err(e) => {
                    warn!(error = %e, "background daemon connect failed");
                }
            }
        });
    }

    let link_cell = Arc::clone(&proxy.link);
    let server = proxy
        .serve(transport)
        .await
        .map_err(|e| StdioProxyError::Serve(e.to_string()))?;

    info!("initialized (request/response only; notifications not forwarded)");
    let quit = server
        .waiting()
        .await
        .map_err(|e| StdioProxyError::Session(e.to_string()))?;
    debug!(?quit, "stdio session ended");
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
    resource_provider: Arc<ResourceProvider>,
    respawn: Option<RespawnFn>,
}

impl ServerHandler for StdioProxy {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(server_capabilities())
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
                warn!(error = %e, "annotate MCP session failed");
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
        Ok(ListToolsResult::with_all_items(tools)
            .with_ttl_ms(0)
            .with_cache_scope(CacheScope::Private))
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
        Ok(self.resource_provider.list_resources())
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        Ok(self.resource_provider.list_resource_templates())
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        match self.resource_provider.read_resource(&request.uri) {
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
        let budget = budget_for(link.config(), &request);
        // A launch is forwarded on its own path so a timeout is not misreported
        // as a stalled link. The requested wait is read here, before `request`
        // moves into the closure, because the error needs to say what the caller
        // actually asked for.
        let requested_wait = requested_launch_wait(&request);
        let is_launch = request.name.as_ref() == "spawn_td";
        let forward = if is_launch {
            self.forward_launch(&link, budget, requested_wait, |peer| async move {
                peer.call_tool_once(request).await
            })
            .await
        } else {
            self.forward_bounded(&link, budget, |peer| async move {
                peer.call_tool_once(request).await
            })
            .await
        };
        let mut response = forward?;
        if let CallToolResponse::Complete(result) = &mut response {
            // The HTTP peer may use a legacy protocol and omit this field.
            // Restore it for modern stdio clients; rmcp strips it for legacy peers.
            result.result_type = Some(ResultType::COMPLETE);
        }
        Ok(response)
    }
}

/// The forwarding ceiling for one call.
///
/// A launch is deliberately NOT classified as a tool class. `spawn_td` carries
/// its own `waitTimeoutMs`, which is a request about a HANDSHAKE, and under the
/// 105s read-class ceiling a caller asking for 200s was cut at 105s with no way
/// to ask for more — which makes the catalog's own mitigation for
/// `tdmcp.spawn.wait_timeout` ("retry with higher waitTimeoutMs")
/// impossible to follow. The value is read from the request arguments here so
/// the ceiling is derived from what the caller actually asked for.
fn budget_for(cfg: &ReconnectConfig, request: &CallToolRequestParams) -> Duration {
    if request.name.as_ref() != "spawn_td" {
        return cfg.tool_call_budget(request.name.as_ref());
    }
    cfg.launch_budget(requested_launch_wait(request))
}

/// The launch wait a request asked for, in `Duration`.
///
/// One reader for this value, so the budget and the error can never disagree
/// about what the caller asked for — a mismatch there would produce a message
/// citing a wait the caller never sent.
///
/// A STRING is accepted because this is a JSON-RPC boundary and clients do send
/// numbers that way; refusing it would reintroduce an unfixable short cut. An
/// unparseable value is `None` rather than a large default, so a typo cannot
/// become an unbounded wait.
fn requested_launch_wait(request: &CallToolRequestParams) -> Option<Duration> {
    request
        .arguments
        .as_ref()
        .and_then(|args| args.get("waitTimeoutMs"))
        .and_then(|raw| {
            raw.as_u64().or_else(|| {
                raw.as_str()
                    .and_then(|text| text.trim().parse::<u64>().ok())
            })
        })
        .map(Duration::from_millis)
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
        match DaemonLink::connect_with_respawn(
            &self.daemon_url,
            self.admin_base.clone(),
            self.config.clone(),
            self.respawn.clone(),
        )
        .await
        {
            Ok(link) => {
                apply_pending_ide(&link, &self.pending_ide);
                // Another caller may have won the race — either way,
                // the freshly-constructed link is functionally equivalent.
                let _ = self.link.set(Arc::clone(&link));
                Ok(link)
            }
            Err(e) => {
                warn!(error = %e, "daemon connect failed on tool call");
                Err(unreachable_error(
                    HealOutcome {
                        healed: false,
                        downtime: None,
                    },
                    &self.config,
                    self.respawn.is_some(),
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
    ///
    /// Single-flight via `link.call_gate()`: a second call sent while a first
    /// is still outstanding on the same session can trip a session-reinit
    /// edge case in rmcp's client that fails the *first* call's pending
    /// response too. Queueing here means a
    /// slow call now delays unrelated calls up to its own budget instead of
    /// silently losing one of them.
    /// Forward a call, treating a budget overrun as a LAUNCH still starting
    /// rather than a stalled link.
    ///
    /// `forward_bounded` answers a timeout with "the MCP session may have
    /// stalled, I reconnected, the effect is unknown". That is the right answer
    /// for a stalled read, and the WRONG one for a spawn, for two reasons:
    ///
    /// * The diagnosis is false. A launch that is still coming up is not a
    ///   stalled session, and a caller reading "reconnected to a fresh session"
    ///   would reasonably conclude the launch failed and retry — a retried
    ///   `spawn_td` is exactly the double-spawn B3 forbids.
    /// * The remedy is harmful. `heal` calls `mark_unhealthy` on entry, so
    ///   diagnosing a slow launch as a link fault actively degrades a working
    ///   connection.
    ///
    /// So a launch timeout does not heal. It reports the caller's own wait
    /// budget, states that the child is still owned and may yet handshake, and
    /// says the outcome is unknown — which is true, and is the same conclusion
    /// W6 reaches for any operation whose result nobody observed.
    async fn forward_launch<F, Fut, T>(
        &self,
        link: &DaemonLink,
        budget: Duration,
        requested_wait: Option<Duration>,
        op: F,
    ) -> Result<T, ErrorData>
    where
        F: FnOnce(Arc<rmcp::Peer<rmcp::RoleClient>>) -> Fut,
        Fut: std::future::Future<Output = Result<T, ServiceError>>,
    {
        let _gate = link.call_gate().await;
        let (peer, _gen) = link.current_peer().await;
        match tokio::time::timeout(budget, op(peer)).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(e)) if is_transport_error(&e) => {
                // A genuine transport failure IS a link fault, so this arm
                // keeps the heal and the link diagnosis.
                warn!(error = %e, "launch transport error — attempting heal");
                let outcome = link.heal(_gen).await;
                Err(unreachable_error(
                    outcome,
                    link.config(),
                    link.can_respawn(),
                ))
            }
            Ok(Err(e)) => {
                warn!(error = %e, "launch forward failed");
                Err(service_err_to_error_data(e))
            }
            Err(_) => {
                let waited = requested_wait.map(|wait| wait.as_millis() as u64);
                warn!(
                    budget_ms = budget.as_millis(),
                    "launch exceeded its budget — the child is still owned and was NOT treated as a link fault"
                );
                Err(launch_timeout_error(budget, waited))
            }
        }
    }

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
        let _gate = link.call_gate().await;
        let (peer, gen) = link.current_peer().await;
        match tokio::time::timeout(budget, op(peer)).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(e)) if is_transport_error(&e) => {
                warn!(error = %e, "transport error — attempting heal");
                let outcome = link.heal(gen).await;
                Err(unreachable_error(
                    outcome,
                    link.config(),
                    link.can_respawn(),
                ))
            }
            Ok(Err(e)) => {
                warn!(error = %e, "call forward failed");
                Err(service_err_to_error_data(e))
            }
            Err(_) => {
                warn!(
                    budget_ms = budget.as_millis(),
                    "call exceeded budget — healing link"
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
        // Preserve upstream protocol codes (e.g. -32602 invalid_params for unknown tool).
        ServiceError::McpError(data) => data,
        other => ErrorData::internal_error(other.to_string(), None),
    }
}

// --- M5: proxy-side log uplink -----------------------------------------
//
// The stdio proxy runs as its own OS process (spawned per Cursor session),
// with no file sink of its own — today its `tracing::info!`/`warn!` calls
// go nowhere (no subscriber is installed for this code path). This wires a
// stderr fmt layer (the "logs go via tracing/stderr" the call site comment
// already promises) plus a tiny capture layer that batches events and POSTs
// them to the daemon's central `/admin/logs/ingest` (M5), landing them
// alongside everything else in the same JSONL file.

/// Built-in filter when `RUST_LOG` is unset or invalid: quiet by default,
/// `info` from this crate specifically (mirrors the daemon's own
/// `tdmcp_daemon=debug` carve-out in `tracing_init::DEFAULT_FILE_FILTER`).
const DEFAULT_PROXY_FILTER: &str = "warn,tdmcp_mcp=info";
/// Records batched per `/admin/logs/ingest` POST.
const PROXY_LOG_BATCH_LINES: usize = 128;
/// Flush cadence, matching the bridge uplink's batch interval (M2).
const PROXY_LOG_FLUSH_INTERVAL: Duration = Duration::from_millis(500);
/// Bounded channel between the tracing layer (any thread) and the flusher
/// task — full means drop, never block the caller mid-`tracing::event!`.
const PROXY_LOG_CHANNEL_CAPACITY: usize = 1024;
/// Rate limit for the "uplink failing" stderr note (Cursor owns stderr —
/// a wedged daemon must not spam it once per POST).
const PROXY_LOG_WARN_INTERVAL: Duration = Duration::from_secs(60);

/// One captured tracing event, pre-serialization.
struct ProxyLogLine {
    level: &'static str,
    target: String,
    msg: String,
    kvs: BTreeMap<String, String>,
}

/// Install the stderr fmt layer + log-uplink layer as the process-wide
/// tracing subscriber. Best-effort: if a subscriber is already installed
/// (impossible today outside tests, since this only runs from the one real
/// production entrypoint) this silently no-ops rather than panicking.
fn install_log_uplink(admin_base: String) {
    let (tx, rx) = mpsc::channel(PROXY_LOG_CHANNEL_CAPACITY);
    tokio::spawn(run_proxy_log_flusher(admin_base, rx));

    let filter = std::env::var("RUST_LOG")
        .ok()
        .and_then(|s| EnvFilter::try_new(s).ok())
        .unwrap_or_else(|| EnvFilter::new(DEFAULT_PROXY_FILTER));
    let fmt_filter = std::env::var("RUST_LOG")
        .ok()
        .and_then(|s| EnvFilter::try_new(s).ok())
        .unwrap_or_else(|| EnvFilter::new(DEFAULT_PROXY_FILTER));

    let uplink_layer = ProxyLogLayer { tx }.with_filter(filter);
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_writer(std::io::stderr)
        .with_filter(fmt_filter);

    let _ = tracing_subscriber::registry()
        .with(uplink_layer)
        .with(fmt_layer)
        .try_init();
}

struct ProxyLogLayer {
    tx: mpsc::Sender<ProxyLogLine>,
}

impl<S: tracing::Subscriber> Layer<S> for ProxyLogLayer {
    fn on_event(&self, event: &Event<'_>, _cx: LayerContext<'_, S>) {
        let mut visitor = ProxyLogVisitor::default();
        event.record(&mut visitor);
        let line = ProxyLogLine {
            level: level_str(event.metadata().level()),
            target: event.metadata().target().to_owned(),
            msg: visitor.msg,
            kvs: visitor.kvs,
        };
        // Fire-and-forget: a full channel means drop this line, never block
        // (or panic) the caller mid-event.
        let _ = self.tx.try_send(line);
    }
}

fn level_str(level: &tracing::Level) -> &'static str {
    match *level {
        tracing::Level::TRACE => "trace",
        tracing::Level::DEBUG => "debug",
        tracing::Level::INFO => "info",
        tracing::Level::WARN => "warn",
        tracing::Level::ERROR => "error",
    }
}

#[derive(Default)]
struct ProxyLogVisitor {
    msg: String,
    kvs: BTreeMap<String, String>,
}

impl Visit for ProxyLogVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.store(field.name(), value.to_owned());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.store(field.name(), format!("{value:?}"));
    }
}

impl ProxyLogVisitor {
    fn store(&mut self, name: &str, value: String) {
        if name == "message" {
            self.msg = value;
        } else {
            self.kvs.insert(name.to_owned(), value);
        }
    }
}

/// Drains `rx` in batches of up to [`PROXY_LOG_BATCH_LINES`] every
/// [`PROXY_LOG_FLUSH_INTERVAL`] and POSTs them to the daemon. Never
/// terminates except when the layer (and therefore `tx`) is dropped, which
/// only happens at process exit (the subscriber is a `'static` singleton).
async fn run_proxy_log_flusher(admin_base: String, mut rx: mpsc::Receiver<ProxyLogLine>) {
    let client = reqwest::Client::new();
    let pid = std::process::id();
    let url = format!("{}/admin/logs/ingest", admin_base.trim_end_matches('/'));
    let mut last_warn: Option<Instant> = None;

    loop {
        let mut batch = Vec::new();
        let deadline = tokio::time::sleep(PROXY_LOG_FLUSH_INTERVAL);
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                line = rx.recv() => {
                    match line {
                        Some(l) => {
                            batch.push(l);
                            if batch.len() >= PROXY_LOG_BATCH_LINES {
                                break;
                            }
                        }
                        None => return, // subscriber dropped — process is exiting
                    }
                }
                () = &mut deadline => break,
            }
        }
        if batch.is_empty() {
            continue;
        }
        let lines: Vec<serde_json::Value> = batch
            .into_iter()
            .map(|l| {
                serde_json::json!({
                    "level": l.level,
                    "target": l.target,
                    "msg": l.msg,
                    "kvs": l.kvs,
                })
            })
            .collect();
        let body = serde_json::json!({ "pid": pid, "lines": lines });
        if let Err(e) = client.post(&url).json(&body).send().await {
            let due = last_warn.is_none_or(|t| t.elapsed() > PROXY_LOG_WARN_INTERVAL);
            if due {
                eprintln!("tdmcp-mcp: log uplink to {url} failed: {e}");
                last_warn = Some(Instant::now());
            }
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "unit tests"
)]
mod tests {
    use super::*;
    use rmcp::model::JsonObject;

    /// A `spawn_td` request, optionally carrying `waitTimeoutMs`.
    fn spawn_request(wait: Option<serde_json::Value>) -> CallToolRequestParams {
        let mut arguments: JsonObject = JsonObject::new();
        arguments.insert("pid".to_owned(), serde_json::json!(1));
        arguments.insert(
            "exePath".to_owned(),
            serde_json::json!("/opt/td/TouchDesigner"),
        );
        if let Some(wait) = wait {
            arguments.insert("waitTimeoutMs".to_owned(), wait);
        }
        CallToolRequestParams::new("spawn_td").with_arguments(arguments)
    }

    #[test]
    fn a_spawn_is_budgeted_by_its_own_wait_not_the_read_ceiling() {
        // B3's defect, at the place it bites: a caller asking for 200s used to
        // be cut at the 105s read-class ceiling, so the catalog's mitigation for
        // tdmcp.spawn.wait_timeout ("retry with higher waitTimeoutMs") could not
        // be followed.
        let cfg = ReconnectConfig::default();
        let budget = budget_for(&cfg, &spawn_request(Some(serde_json::json!(200_000))));
        assert!(
            budget > cfg.call_timeout,
            "200s asked, {:?} allowed",
            budget
        );
        assert_eq!(budget, Duration::from_millis(200_000) + cfg.launch_margin);
    }

    #[test]
    fn a_spawn_with_no_wait_uses_the_spawn_default() {
        let cfg = ReconnectConfig::default();
        let budget = budget_for(&cfg, &spawn_request(None));
        assert_eq!(
            budget,
            Duration::from_millis(crate::daemon_link::DEFAULT_SPAWN_WAIT_MS) + cfg.launch_margin
        );
    }

    #[test]
    fn a_wait_sent_as_a_string_is_honoured() {
        // A JSON-RPC boundary: clients do send numbers as strings, and
        // refusing that would reintroduce an unfixable 105s cut.
        let cfg = ReconnectConfig::default();
        let budget = budget_for(&cfg, &spawn_request(Some(serde_json::json!("200000"))));
        assert_eq!(budget, Duration::from_millis(200_000) + cfg.launch_margin);
    }

    #[test]
    fn an_unparseable_wait_falls_back_rather_than_becoming_unbounded() {
        let cfg = ReconnectConfig::default();
        for bad in [
            serde_json::json!("soon"),
            serde_json::json!({}),
            serde_json::json!(-1),
        ] {
            let budget = budget_for(&cfg, &spawn_request(Some(bad.clone())));
            assert_eq!(
                budget,
                Duration::from_millis(crate::daemon_link::DEFAULT_SPAWN_WAIT_MS)
                    + cfg.launch_margin,
                "{bad} must not become an unbounded wait"
            );
        }
    }

    #[test]
    fn every_other_tool_keeps_its_class_ceiling() {
        // The change must be confined to the launch: a read must not have
        // quietly become a long wait.
        let cfg = ReconnectConfig::default();
        for name in [
            "inspect",
            "execute_python",
            "mutate_nodes",
            "palette_probe",
            "fleet",
        ] {
            let request =
                CallToolRequestParams::new(name).with_arguments(JsonObject::from_iter([(
                    "pid".to_owned(),
                    serde_json::json!(1),
                )]));
            assert_eq!(
                budget_for(&cfg, &request),
                cfg.tool_call_budget(name),
                "{name} must keep its class ceiling"
            );
        }
    }

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

    #[test]
    fn level_str_covers_all_tracing_levels() {
        assert_eq!(level_str(&tracing::Level::TRACE), "trace");
        assert_eq!(level_str(&tracing::Level::DEBUG), "debug");
        assert_eq!(level_str(&tracing::Level::INFO), "info");
        assert_eq!(level_str(&tracing::Level::WARN), "warn");
        assert_eq!(level_str(&tracing::Level::ERROR), "error");
    }

    #[test]
    fn proxy_log_visitor_routes_message_vs_kvs() {
        let mut v = ProxyLogVisitor::default();
        v.store("message", "hello".to_owned());
        v.store("ms", "42".to_owned());
        assert_eq!(v.msg, "hello");
        assert_eq!(v.kvs.get("ms").map(String::as_str), Some("42"));
    }

    /// M5 acceptance shape (pure flusher, no global-subscriber install):
    /// batched lines POST to `/admin/logs/ingest` as `{pid, lines}` with the
    /// exact per-line schema the daemon's `ingest_proxy_logs` expects.
    #[tokio::test]
    async fn flusher_posts_batched_lines_to_ingest_endpoint() {
        use std::sync::Mutex as StdMutex;

        let received: Arc<StdMutex<Vec<serde_json::Value>>> = Arc::new(StdMutex::new(Vec::new()));
        let received_srv = Arc::clone(&received);
        let app = axum::Router::new().route(
            "/admin/logs/ingest",
            axum::routing::post(move |axum::Json(body): axum::Json<serde_json::Value>| {
                let received = Arc::clone(&received_srv);
                async move {
                    received.lock().expect("lock").push(body);
                    axum::Json(serde_json::json!({"ok": true}))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("local_addr");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });

        let (tx, rx) = mpsc::channel(16);
        let admin_base = format!("http://{addr}");
        let flusher = tokio::spawn(run_proxy_log_flusher(admin_base, rx));

        tx.send(ProxyLogLine {
            level: "warn",
            target: "tdmcp_mcp::stdio_proxy".to_owned(),
            msg: "heal attempted".to_owned(),
            kvs: BTreeMap::from([("attempt".to_owned(), "1".to_owned())]),
        })
        .await
        .expect("send");

        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            if !received.lock().expect("lock").is_empty() {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                panic!("flusher did not POST within budget");
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        flusher.abort();
        server.abort();

        let bodies = received.lock().expect("lock");
        assert_eq!(bodies.len(), 1);
        let body = &bodies[0];
        assert!(body["pid"].as_u64().is_some());
        let lines = body["lines"].as_array().expect("lines array");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["level"], "warn");
        assert_eq!(lines[0]["target"], "tdmcp_mcp::stdio_proxy");
        assert_eq!(lines[0]["msg"], "heal attempted");
        assert_eq!(lines[0]["kvs"]["attempt"], "1");
    }
}

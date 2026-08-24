//! Reconnect-only link from the stdio MCP proxy to the HTTP daemon.
//!
//! Reconnect (re-handshake against whatever is already listening) is always
//! attempted first and never spawns anything. When the caller supplies a
//! [`RespawnFn`] (`tdmcp-daemon mcp`'s cold-start `ensure_daemon` closure,
//! injected from `main.rs` since this crate cannot depend on
//! `tdmcp-daemon`), sustained downtime past `config.stale` additionally
//! triggers a real respawn attempt through that same hook — reusing the
//! daemon's own lock/spawn machinery instead of inventing a second one here.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rmcp::model::{ClientCapabilities, ClientInfo, Implementation};
use rmcp::service::RunningService;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::{ErrorData, Peer, RoleClient, ServiceError, ServiceExt};
use serde_json::json;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use tdmcp_diagnostics::codes;

/// Client name advertised on the HTTP side so the daemon GUI can list the lease.
pub const STDIO_PROXY_CLIENT_NAME: &str = "tdmcp-stdio-proxy";

/// Default: downtime below this is "likely mid-restart".
pub const DEFAULT_RECENT_MS: u64 = 3_000;
/// Default: downtime above this means "probably dead — run ensure".
pub const DEFAULT_STALE_MS: u64 = 15_000;
/// Default: skip overlapping probes within this window.
pub const DEFAULT_DEBOUNCE_MS: u64 = 250;
/// Default: watcher backoff start while unhealthy.
pub const DEFAULT_PROBE_INTERVAL_MS: u64 = 500;
/// Default: watcher backoff cap while unhealthy.
pub const DEFAULT_PROBE_MAX_MS: u64 = 5_000;
/// Default ceiling for short tool calls: the daemon's `[bridge]`
/// `call_timeout_secs` default (45s) plus margin, so a live call is never cut
/// early while a wedged session still fails fast.
pub const DEFAULT_PROXY_CALL_TIMEOUT_MS: u64 = 105_000;
/// Default ceiling for script-class calls: the daemon's `[bridge]`
/// `script_timeout_secs` default (120s) plus margin (mirrors the daemon's own
/// 180s `BRIDGE_TIMEOUT`).
pub const DEFAULT_PROXY_SCRIPT_TIMEOUT_MS: u64 = 180_000;
/// Default ceiling for `tools/list` (cheap; fail fast on a wedged session).
pub const DEFAULT_PROXY_LIST_TIMEOUT_MS: u64 = 30_000;
/// Health GET budget (mirrors `ensure::health_ok`).
const HEALTH_TIMEOUT: Duration = Duration::from_millis(800);
/// Fresh `serve()` handshake budget.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// Bound for waiters blocked on an in-flight heal (single-flight gate).
const HEAL_GATE_WAIT: Duration = Duration::from_secs(5);
/// Minimum gap between automatic respawn attempts — `ensure_daemon` already
/// has its own internal lock/timeout, this just keeps the watcher from
/// hammering it every backoff tick while still down.
const RESPAWN_COOLDOWN: Duration = Duration::from_secs(10);

/// Fire-and-forget closure that attempts to spawn/ensure a fresh daemon.
/// Injected from `main.rs` (wraps `ensure_daemon`) since `tdmcp-mcp` cannot
/// depend on `tdmcp-daemon` without a crate cycle.
pub type RespawnFn = Arc<dyn Fn() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// Env-configurable proxy timing (reconnect + per-call ceilings).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconnectConfig {
    /// Downtime below this → "likely mid-restart" message.
    pub recent: Duration,
    /// Downtime above this → "run ensure" message.
    pub stale: Duration,
    /// Minimum gap between health/reconnect probes.
    pub debounce: Duration,
    /// Watcher sleep while unhealthy (starts here, grows to [`Self::probe_max`]).
    pub probe_interval: Duration,
    /// Watcher sleep cap while unhealthy.
    pub probe_max: Duration,
    /// Wall-clock ceiling for short tool calls.
    pub call_timeout: Duration,
    /// Wall-clock ceiling for script-class calls (`execute_python` / `mutate_nodes`).
    pub script_timeout: Duration,
    /// Wall-clock ceiling for `tools/list`.
    pub list_timeout: Duration,
}

impl Default for ReconnectConfig {
    fn default() -> Self {
        Self {
            recent: Duration::from_millis(DEFAULT_RECENT_MS),
            stale: Duration::from_millis(DEFAULT_STALE_MS),
            debounce: Duration::from_millis(DEFAULT_DEBOUNCE_MS),
            probe_interval: Duration::from_millis(DEFAULT_PROBE_INTERVAL_MS),
            probe_max: Duration::from_millis(DEFAULT_PROBE_MAX_MS),
            call_timeout: Duration::from_millis(DEFAULT_PROXY_CALL_TIMEOUT_MS),
            script_timeout: Duration::from_millis(DEFAULT_PROXY_SCRIPT_TIMEOUT_MS),
            list_timeout: Duration::from_millis(DEFAULT_PROXY_LIST_TIMEOUT_MS),
        }
    }
}

impl ReconnectConfig {
    /// Load from `TDMCP_RECONNECT_*` / `TDMCP_PROXY_*` env vars; invalid values
    /// fall back to defaults.
    #[must_use]
    pub fn from_env() -> Self {
        Self::from_env_vars(
            std::env::var("TDMCP_RECONNECT_RECENT_MS").ok().as_deref(),
            std::env::var("TDMCP_RECONNECT_STALE_MS").ok().as_deref(),
            std::env::var("TDMCP_RECONNECT_DEBOUNCE_MS").ok().as_deref(),
            std::env::var("TDMCP_RECONNECT_PROBE_INTERVAL_MS")
                .ok()
                .as_deref(),
            std::env::var("TDMCP_RECONNECT_PROBE_MAX_MS")
                .ok()
                .as_deref(),
            std::env::var("TDMCP_PROXY_CALL_TIMEOUT_MS").ok().as_deref(),
            std::env::var("TDMCP_PROXY_SCRIPT_TIMEOUT_MS")
                .ok()
                .as_deref(),
            std::env::var("TDMCP_PROXY_LIST_TIMEOUT_MS").ok().as_deref(),
        )
    }

    /// Parse optional raw env strings (tests).
    #[must_use]
    #[allow(
        clippy::too_many_arguments,
        reason = "one param per env knob; mirrors config surface"
    )]
    pub fn from_env_vars(
        recent: Option<&str>,
        stale: Option<&str>,
        debounce: Option<&str>,
        probe_interval: Option<&str>,
        probe_max: Option<&str>,
        call_timeout: Option<&str>,
        script_timeout: Option<&str>,
        list_timeout: Option<&str>,
    ) -> Self {
        let defaults = Self::default();
        Self {
            recent: parse_ms(recent, defaults.recent, "TDMCP_RECONNECT_RECENT_MS"),
            stale: parse_ms(stale, defaults.stale, "TDMCP_RECONNECT_STALE_MS"),
            debounce: parse_ms(debounce, defaults.debounce, "TDMCP_RECONNECT_DEBOUNCE_MS"),
            probe_interval: parse_ms(
                probe_interval,
                defaults.probe_interval,
                "TDMCP_RECONNECT_PROBE_INTERVAL_MS",
            ),
            probe_max: parse_ms(
                probe_max,
                defaults.probe_max,
                "TDMCP_RECONNECT_PROBE_MAX_MS",
            ),
            call_timeout: parse_ms(
                call_timeout,
                defaults.call_timeout,
                "TDMCP_PROXY_CALL_TIMEOUT_MS",
            ),
            script_timeout: parse_ms(
                script_timeout,
                defaults.script_timeout,
                "TDMCP_PROXY_SCRIPT_TIMEOUT_MS",
            ),
            list_timeout: parse_ms(
                list_timeout,
                defaults.list_timeout,
                "TDMCP_PROXY_LIST_TIMEOUT_MS",
            ),
        }
    }

    /// Wall-clock budget for a forwarded `tools/call`, by method class.
    #[must_use]
    pub fn tool_call_budget(&self, name: &str) -> Duration {
        if matches!(name, "execute_python" | "mutate_nodes") {
            self.script_timeout
        } else {
            self.call_timeout
        }
    }
}

fn parse_ms(raw: Option<&str>, default: Duration, name: &str) -> Duration {
    match raw {
        None => default,
        Some(s) => match s.trim().parse::<u64>() {
            Ok(ms) => Duration::from_millis(ms),
            Err(_) => {
                warn!(%name, value = %s, "invalid env — using default {:?}", default);
                default
            }
        },
    }
}

/// Outcome of a synchronous heal attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HealOutcome {
    /// Whether the link is healthy after this call (reconnected or already healed).
    pub healed: bool,
    /// How long the link was (or has been) down, if known.
    pub downtime: Option<Duration>,
}

struct LinkState {
    peer: Arc<Peer<RoleClient>>,
    service: RunningService<RoleClient, ClientInfo>,
    generation: u64,
}

/// Owns the live HTTP MCP client session and reconnect policy.
pub struct DaemonLink {
    daemon_url: String,
    admin_base: String,
    config: ReconnectConfig,
    state: tokio::sync::RwLock<LinkState>,
    reconnect_gate: tokio::sync::Mutex<()>,
    unhealthy_since: Mutex<Option<Instant>>,
    last_downtime: Mutex<Option<Duration>>,
    last_probe: Mutex<Option<Instant>>,
    /// Successful reconnect count (tests / diagnostics).
    reconnects: AtomicU64,
    ide_client: Mutex<Option<(String, String)>>,
    shutdown: CancellationToken,
    http: reqwest::Client,
    respawn: Option<RespawnFn>,
    last_respawn_attempt: Mutex<Option<Instant>>,
}

impl DaemonLink {
    /// Connect once, start the keep-warm watcher, return the link.
    /// Reconnect-only — no respawn escalation on sustained downtime.
    #[allow(dead_code, reason = "kept as the reconnect-only entry point for callers/tests without a respawn hook")]
    pub async fn connect(
        daemon_url: &str,
        admin_base: String,
        config: ReconnectConfig,
    ) -> Result<Arc<Self>, String> {
        Self::connect_with_respawn(daemon_url, admin_base, config, None).await
    }

    /// Like [`Self::connect`], but escalates to `respawn` (fire-and-forget)
    /// once downtime exceeds `config.stale`, in addition to the normal
    /// reconnect-only healing.
    pub async fn connect_with_respawn(
        daemon_url: &str,
        admin_base: String,
        config: ReconnectConfig,
        respawn: Option<RespawnFn>,
    ) -> Result<Arc<Self>, String> {
        let service = connect_http(daemon_url).await?;
        let peer = Arc::new(service.peer().clone());
        let http = reqwest::Client::builder()
            .timeout(HEALTH_TIMEOUT)
            .build()
            .map_err(|e| e.to_string())?;
        let link = Arc::new(Self {
            daemon_url: daemon_url.to_owned(),
            admin_base,
            config,
            state: tokio::sync::RwLock::new(LinkState {
                peer,
                service,
                generation: 0,
            }),
            reconnect_gate: tokio::sync::Mutex::new(()),
            unhealthy_since: Mutex::new(None),
            last_downtime: Mutex::new(None),
            last_probe: Mutex::new(None),
            reconnects: AtomicU64::new(0),
            ide_client: Mutex::new(None),
            shutdown: CancellationToken::new(),
            http,
            respawn,
            last_respawn_attempt: Mutex::new(None),
        });
        link.clone().spawn_watcher();
        Ok(link)
    }

    /// Whether this link was given a respawn hook (shapes user-facing messages).
    #[must_use]
    pub fn can_respawn(&self) -> bool {
        self.respawn.is_some()
    }

    /// Remember the IDE clientInfo so we can re-annotate after reconnect.
    pub fn set_ide_client(&self, name: String, version: String) {
        if let Ok(mut guard) = self.ide_client.lock() {
            *guard = Some((name, version));
        }
    }

    /// Base URL used for `/mcp/health` and admin annotate.
    #[must_use]
    pub fn admin_base(&self) -> &str {
        &self.admin_base
    }

    /// Active reconnect timing config.
    #[must_use]
    pub fn config(&self) -> &ReconnectConfig {
        &self.config
    }

    /// Cheap snapshot of the current peer + generation.
    pub async fn current_peer(&self) -> (Arc<Peer<RoleClient>>, u64) {
        let state = self.state.read().await;
        (Arc::clone(&state.peer), state.generation)
    }

    /// Current generation (tests).
    #[must_use]
    pub fn generation(&self) -> u64 {
        // Best-effort without async: use a try_read if available, else 0.
        match self.state.try_read() {
            Ok(state) => state.generation,
            Err(_) => 0,
        }
    }

    /// Number of successful reconnect handshakes.
    #[must_use]
    #[allow(dead_code, reason = "integration / future diagnostics")]
    pub fn reconnect_count(&self) -> u64 {
        self.reconnects.load(Ordering::Relaxed)
    }

    /// Whether the link is currently marked unhealthy.
    #[must_use]
    pub fn is_unhealthy(&self) -> bool {
        self.unhealthy_since
            .lock()
            .map(|g| g.is_some())
            .unwrap_or(false)
    }

    /// Mark the link unhealthy (first failure wins the timestamp).
    pub fn mark_unhealthy(&self) {
        if let Ok(mut guard) = self.unhealthy_since.lock() {
            if guard.is_none() {
                *guard = Some(Instant::now());
            }
        }
    }

    /// Current downtime if unhealthy (or last completed downtime).
    #[must_use]
    pub fn downtime(&self) -> Option<Duration> {
        if let Ok(guard) = self.unhealthy_since.lock() {
            if let Some(since) = *guard {
                return Some(since.elapsed());
            }
        }
        self.last_downtime.lock().ok().and_then(|g| *g)
    }

    /// Attempt a reconnect-only heal for the generation that just failed.
    ///
    /// Never spawns a daemon. Single-flight with waiters: concurrent callers
    /// block on the gate (bounded) and share the in-flight outcome instead of
    /// failing open with `healed: false`. Debounced only for the gate holder.
    pub async fn heal(&self, generation_used: u64) -> HealOutcome {
        self.mark_unhealthy();
        let downtime = self.downtime();

        {
            let state = self.state.read().await;
            if state.generation != generation_used {
                return HealOutcome {
                    healed: true,
                    downtime: self.downtime().or(downtime),
                };
            }
        }

        let gate = match tokio::time::timeout(HEAL_GATE_WAIT, self.reconnect_gate.lock()).await {
            Ok(guard) => guard,
            Err(_) => {
                debug!("heal gate wait timed out");
                return HealOutcome {
                    healed: false,
                    downtime: self.downtime().or(downtime),
                };
            }
        };
        let _gate = gate;

        // Another waiter may have healed (or the watcher) while we waited.
        {
            let state = self.state.read().await;
            if state.generation != generation_used {
                return HealOutcome {
                    healed: true,
                    downtime: self.downtime().or(downtime),
                };
            }
        }

        if self.debounced() {
            debug!("heal debounced");
            return HealOutcome {
                healed: false,
                downtime: self.downtime().or(downtime),
            };
        }

        self.note_probe();

        if !self.health_ok().await {
            debug!("health probe failed");
            return HealOutcome {
                healed: false,
                downtime: self.downtime(),
            };
        }

        match tokio::time::timeout(CONNECT_TIMEOUT, connect_http(&self.daemon_url)).await {
            Ok(Ok(new_service)) => {
                let new_peer = Arc::new(new_service.peer().clone());
                let old = {
                    let mut state = self.state.write().await;
                    let old_service = std::mem::replace(&mut state.service, new_service);
                    state.peer = new_peer;
                    state.generation = state.generation.wrapping_add(1);
                    old_service
                };
                let downtime = self.clear_unhealthy();
                self.reconnects.fetch_add(1, Ordering::Relaxed);
                info!(
                    generation = self.generation(),
                    downtime_ms = downtime.map(|d| d.as_millis()).unwrap_or(0),
                    "reconnected to daemon"
                );
                let _ = old.cancel().await;
                self.reannotate_best_effort().await;
                HealOutcome {
                    healed: true,
                    downtime,
                }
            }
            Ok(Err(e)) => {
                warn!(error = %e, "reconnect handshake failed");
                HealOutcome {
                    healed: false,
                    downtime: self.downtime(),
                }
            }
            Err(_) => {
                warn!("reconnect handshake timed out");
                HealOutcome {
                    healed: false,
                    downtime: self.downtime(),
                }
            }
        }
    }

    /// Stop the watcher and cancel the live HTTP client session.
    pub async fn shutdown(&self) {
        self.shutdown.cancel();
        let mut state = self.state.write().await;
        let _ = state.service.close().await;
    }

    fn spawn_watcher(self: Arc<Self>) {
        let token = self.shutdown.clone();
        tokio::spawn(async move {
            let mut backoff = self.config.probe_interval;
            loop {
                tokio::select! {
                    () = token.cancelled() => break,
                    () = tokio::time::sleep(if self.is_unhealthy() {
                        backoff
                    } else {
                        self.config.probe_interval
                    }) => {}
                }
                if token.is_cancelled() {
                    break;
                }
                if !self.is_unhealthy() {
                    backoff = self.config.probe_interval;
                    continue;
                }
                let gen = {
                    let state = self.state.read().await;
                    state.generation
                };
                let outcome = self.heal(gen).await;
                if outcome.healed {
                    backoff = self.config.probe_interval;
                } else {
                    backoff = (backoff.saturating_mul(2)).min(self.config.probe_max);
                    self.maybe_trigger_respawn();
                }
            }
            debug!("watcher stopped");
        });
    }

    /// Fire the respawn hook (if configured) once downtime crosses
    /// `config.stale`, at most once per [`RESPAWN_COOLDOWN`]. Fire-and-forget:
    /// `ensure_daemon` owns its own lock/timeout, and the next watcher
    /// `heal()` picks up the freshly spawned daemon exactly like any other
    /// restart — this only supplements reconnect, never replaces it.
    fn maybe_trigger_respawn(&self) {
        let Some(respawn) = self.respawn.clone() else {
            return;
        };
        let Some(downtime) = self.downtime() else {
            return;
        };
        if downtime < self.config.stale {
            return;
        }
        {
            let Ok(mut guard) = self.last_respawn_attempt.lock() else {
                return;
            };
            if let Some(last) = *guard {
                if last.elapsed() < RESPAWN_COOLDOWN {
                    return;
                }
            }
            *guard = Some(Instant::now());
        }
        info!(
            downtime_ms = downtime.as_millis(),
            "sustained downtime — triggering automatic daemon respawn"
        );
        tokio::spawn(async move { (respawn)().await });
    }

    fn debounced(&self) -> bool {
        let Ok(guard) = self.last_probe.lock() else {
            return false;
        };
        guard
            .map(|t| t.elapsed() < self.config.debounce)
            .unwrap_or(false)
    }

    fn note_probe(&self) {
        if let Ok(mut guard) = self.last_probe.lock() {
            *guard = Some(Instant::now());
        }
    }

    fn clear_unhealthy(&self) -> Option<Duration> {
        let downtime = if let Ok(mut guard) = self.unhealthy_since.lock() {
            guard.take().map(|t| t.elapsed())
        } else {
            None
        };
        if let Ok(mut last) = self.last_downtime.lock() {
            *last = downtime;
        }
        downtime
    }

    async fn health_ok(&self) -> bool {
        let url = format!("{}/mcp/health", self.admin_base);
        match self.http.get(&url).send().await {
            Ok(resp) if resp.status().is_success() => {
                match resp.json::<serde_json::Value>().await {
                    Ok(v) => v.get("ok").and_then(|x| x.as_bool()) == Some(true),
                    Err(_) => false,
                }
            }
            _ => false,
        }
    }

    async fn reannotate_best_effort(&self) {
        let Some((name, version)) = self.ide_client.lock().ok().and_then(|g| g.clone()) else {
            return;
        };
        let url = format!("{}/admin/mcp-sessions/annotate", self.admin_base);
        let body = json!({
            "matchClientName": STDIO_PROXY_CLIENT_NAME,
            "clientName": name,
            "clientVersion": version,
        });
        if let Err(e) = self.http.post(&url).json(&body).send().await {
            warn!(error = %e, "re-annotate after reconnect failed");
        }
    }
}

/// Idle connections the stdio proxy keeps per daemon host for reuse.
///
/// rmcp's default Streamable HTTP client sets `pool_max_idle_per_host(0)`
/// (no reuse, to dodge a Linux delayed-ACK stall), which makes **every** tool
/// call open a fresh TCP connection. On Windows each closed connection parks
/// in TIME_WAIT for minutes, so sustained multi-client load exhausts the
/// machine-wide dynamic port range (49152–65535) and the MCP transport
/// freezes: new connects fail with WSAEADDRINUSE and the client can neither
/// reconnect nor answer `fleet`. A small idle pool fixes the churn while
/// keeping reuse bounded.
const HTTP_POOL_MAX_IDLE: usize = 8;

async fn connect_http(daemon_url: &str) -> Result<RunningService<RoleClient, ClientInfo>, String> {
    // Own pooled client instead of `StreamableHttpClientTransport::from_uri`,
    // whose default disables connection reuse entirely (see above).
    let http = reqwest::Client::builder()
        .pool_max_idle_per_host(HTTP_POOL_MAX_IDLE)
        .pool_idle_timeout(Duration::from_secs(60))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| e.to_string())?;
    let transport = StreamableHttpClientTransport::with_client(
        http,
        StreamableHttpClientTransportConfig::with_uri(daemon_url.to_string()),
    );
    let client_info = ClientInfo::new(
        ClientCapabilities::default(),
        Implementation::new(STDIO_PROXY_CLIENT_NAME, env!("CARGO_PKG_VERSION")),
    );
    client_info
        .serve(transport)
        .await
        .map_err(|e| e.to_string())
}

/// Whether this error warrants a reconnect attempt.
#[must_use]
pub fn is_transport_error(err: &ServiceError) -> bool {
    matches!(
        err,
        ServiceError::TransportSend(_)
            | ServiceError::TransportClosed
            | ServiceError::Timeout { .. }
    )
}

/// Message tier for agent-facing errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnreachableTier {
    /// Link was healed during this call — effect unknown.
    Healed,
    /// Just lost; likely mid-restart.
    Recent,
    /// Still starting / not answering health.
    Starting,
    /// Long enough that ensure is needed.
    Stale,
}

/// Pick a message tier from heal outcome + config.
#[must_use]
pub fn unreachable_tier(outcome: &HealOutcome, config: &ReconnectConfig) -> UnreachableTier {
    if outcome.healed {
        return UnreachableTier::Healed;
    }
    let downtime = outcome.downtime.unwrap_or(Duration::ZERO);
    if downtime < config.recent {
        UnreachableTier::Recent
    } else if downtime < config.stale {
        UnreachableTier::Starting
    } else {
        UnreachableTier::Stale
    }
}

/// Build `ErrorData` for a daemon-unreachable failure.
///
/// `respawn_available` reflects whether this link was given a [`RespawnFn`]
/// (see [`DaemonLink::can_respawn`]) — when true, the `Stale` tier message
/// tells the agent a restart was already requested automatically instead of
/// instructing them to run one by hand.
#[must_use]
pub fn unreachable_error(
    outcome: HealOutcome,
    config: &ReconnectConfig,
    respawn_available: bool,
) -> ErrorData {
    let downtime = outcome.downtime.unwrap_or(Duration::ZERO);
    let downtime_ms = downtime.as_millis() as u64;
    let tier = unreachable_tier(&outcome, config);
    let (message, suggestion) = match tier {
		UnreachableTier::Healed => (
			format!(
				"daemon connection was lost mid-call and has been reconnected (down for ~{downtime_ms}ms). \
				 This call's effect on the target is unknown — verify with `inspect` before retrying a mutation."
			),
			"verify with inspect before retrying a mutation",
		),
		UnreachableTier::Recent => (
			format!(
				"daemon connection lost {downtime_ms}ms ago (likely mid-restart) — retry in a moment."
			),
			"retry in a moment",
		),
		UnreachableTier::Starting => {
			let secs = downtime.as_secs().max(1);
			(
				format!(
					"daemon has been unreachable for {secs}s and not yet answering health checks — \
					 it may still be starting; wait and retry."
				),
				"wait and retry",
			)
		}
		UnreachableTier::Stale if respawn_available => {
			let secs = downtime.as_secs().max(1);
			(
				format!(
					"daemon has been unreachable for {secs}s — the proxy has automatically requested a \
					 daemon restart; retry in a few seconds."
				),
				"retry in a few seconds — a restart was requested automatically",
			)
		}
		UnreachableTier::Stale => {
			let secs = downtime.as_secs().max(1);
			(
				format!(
					"daemon has been unreachable for {secs}s. The stdio bridge does not auto-spawn a daemon \
					 (by design) — run `tdmcp-daemon ensure` or restart your MCP client to relaunch it."
				),
				"run `tdmcp-daemon ensure` or restart the MCP client",
			)
		}
	};
    let data = json!({
        "code": codes::DAEMON_UNREACHABLE,
        "downtimeMs": downtime_ms,
        "healed": outcome.healed,
        "suggestion": suggestion,
    });
    ErrorData::internal_error(message, Some(data))
}

/// Build `ErrorData` for a forwarded call that exceeded its budget.
///
/// Distinct from [`unreachable_error`]: the link may be healthy, but the
/// daemon-side MCP session stalled (rmcp's per-session worker blocks on a full
/// SSE stream when the client stops reading, and new requests pile up behind
/// it with no server-side timeout). The proxy already healed the link before
/// constructing this error; the effect of the timed-out call is unknown.
#[must_use]
pub fn call_timeout_error(budget: Duration, outcome: HealOutcome) -> ErrorData {
    let budget_ms = budget.as_millis() as u64;
    let healed = outcome.healed;
    let data = json!({
        "code": codes::DAEMON_UNREACHABLE,
        "budgetMs": budget_ms,
        "healed": healed,
        "suggestion": "verify with inspect before retrying a mutation",
    });
    ErrorData::internal_error(
        format!(
            "daemon call exceeded its {budget_ms}ms budget (the MCP session may have stalled) \
             and the proxy has reconnected to a fresh session. \
             This call's effect on the target is unknown — verify with `inspect` before retrying a mutation."
        ),
        Some(data),
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;
    use rmcp::model::ErrorCode;
    use std::any::TypeId;

    #[test]
    fn classify_transport_errors() {
        assert!(is_transport_error(&ServiceError::TransportClosed));
        assert!(is_transport_error(&ServiceError::Timeout {
            timeout: Duration::from_secs(1)
        }));
        assert!(is_transport_error(&ServiceError::TransportSend(
            rmcp::transport::DynamicTransportError::from_parts(
                "test",
                TypeId::of::<()>(),
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::ConnectionReset,
                    "x"
                )),
            )
        )));
    }

    #[test]
    fn classify_non_transport_errors() {
        assert!(!is_transport_error(&ServiceError::McpError(
            ErrorData::new(ErrorCode::INVALID_PARAMS, "bad", None)
        )));
        assert!(!is_transport_error(&ServiceError::UnexpectedResponse));
        assert!(!is_transport_error(&ServiceError::Cancelled {
            reason: None
        }));
        assert!(!is_transport_error(&ServiceError::SubscriptionLagged {
            capacity: 8
        }));
        assert!(!is_transport_error(
            &ServiceError::InputRequiredRoundsExceeded { max_rounds: 3 }
        ));
    }

    #[test]
    fn tier_selection() {
        let cfg = ReconnectConfig::default();
        assert_eq!(
            unreachable_tier(
                &HealOutcome {
                    healed: true,
                    downtime: Some(Duration::from_millis(500))
                },
                &cfg
            ),
            UnreachableTier::Healed
        );
        assert_eq!(
            unreachable_tier(
                &HealOutcome {
                    healed: false,
                    downtime: Some(Duration::from_millis(500))
                },
                &cfg
            ),
            UnreachableTier::Recent
        );
        assert_eq!(
            unreachable_tier(
                &HealOutcome {
                    healed: false,
                    downtime: Some(Duration::from_secs(5))
                },
                &cfg
            ),
            UnreachableTier::Starting
        );
        assert_eq!(
            unreachable_tier(
                &HealOutcome {
                    healed: false,
                    downtime: Some(Duration::from_secs(20))
                },
                &cfg
            ),
            UnreachableTier::Stale
        );
    }

    #[test]
    fn env_parse_defaults_and_overrides() {
        let cfg = ReconnectConfig::from_env_vars(None, None, None, None, None, None, None, None);
        assert_eq!(cfg, ReconnectConfig::default());

        let cfg = ReconnectConfig::from_env_vars(
            Some("1000"),
            Some("20000"),
            Some("100"),
            Some("250"),
            Some("2000"),
            Some("7000"),
            Some("99999"),
            Some("1234"),
        );
        assert_eq!(cfg.recent, Duration::from_millis(1000));
        assert_eq!(cfg.stale, Duration::from_millis(20000));
        assert_eq!(cfg.debounce, Duration::from_millis(100));
        assert_eq!(cfg.probe_interval, Duration::from_millis(250));
        assert_eq!(cfg.probe_max, Duration::from_millis(2000));
        assert_eq!(cfg.call_timeout, Duration::from_millis(7000));
        assert_eq!(cfg.script_timeout, Duration::from_millis(99_999));
        assert_eq!(cfg.list_timeout, Duration::from_millis(1234));

        let cfg =
            ReconnectConfig::from_env_vars(Some("nope"), None, None, None, None, None, None, None);
        assert_eq!(cfg.recent, Duration::from_millis(DEFAULT_RECENT_MS));
    }

    #[test]
    fn tool_call_budget_by_class_and_defaults() {
        let cfg = ReconnectConfig::default();
        // Defaults must stay above the daemon's [bridge] call/script budgets
        // (45s / 120s) so a live call is never cut early.
        assert!(cfg.call_timeout > Duration::from_secs(45));
        assert!(cfg.script_timeout > Duration::from_secs(120));
        assert_eq!(
            cfg.call_timeout,
            Duration::from_millis(DEFAULT_PROXY_CALL_TIMEOUT_MS)
        );
        assert_eq!(
            cfg.script_timeout,
            Duration::from_millis(DEFAULT_PROXY_SCRIPT_TIMEOUT_MS)
        );
        assert_eq!(cfg.tool_call_budget("fleet"), cfg.call_timeout);
        assert_eq!(cfg.tool_call_budget("inspect"), cfg.call_timeout);
        assert_eq!(cfg.tool_call_budget("execute_python"), cfg.script_timeout);
        assert_eq!(cfg.tool_call_budget("mutate_nodes"), cfg.script_timeout);
    }

    #[test]
    fn unreachable_error_payload() {
        let cfg = ReconnectConfig::default();
        let err = unreachable_error(
            HealOutcome {
                healed: false,
                downtime: Some(Duration::from_millis(400)),
            },
            &cfg,
            false,
        );
        assert_eq!(err.code, ErrorCode::INTERNAL_ERROR);
        let data = err.data.expect("data");
        assert_eq!(
            data.get("code").and_then(|c| c.as_str()),
            Some(codes::DAEMON_UNREACHABLE)
        );
        assert_eq!(data.get("healed").and_then(|h| h.as_bool()), Some(false));
        assert!(err.message.contains("mid-restart") || err.message.contains("retry"));
    }

    #[test]
    fn stale_tier_message_reflects_respawn_availability() {
        let cfg = ReconnectConfig::default();
        let outcome = HealOutcome {
            healed: false,
            downtime: Some(cfg.stale + Duration::from_secs(1)),
        };
        let without = unreachable_error(outcome, &cfg, false);
        let with = unreachable_error(outcome, &cfg, true);
        assert!(without.message.contains("run `tdmcp-daemon ensure`"));
        assert!(!with.message.contains("run `tdmcp-daemon ensure`"));
        assert!(with.message.contains("automatically requested"));
    }

    #[test]
    fn call_timeout_error_payload() {
        let err = call_timeout_error(
            Duration::from_secs(105),
            HealOutcome {
                healed: true,
                downtime: None,
            },
        );
        assert_eq!(err.code, ErrorCode::INTERNAL_ERROR);
        let data = err.data.expect("data");
        assert_eq!(
            data.get("code").and_then(|c| c.as_str()),
            Some(codes::DAEMON_UNREACHABLE)
        );
        assert_eq!(data.get("budgetMs").and_then(|b| b.as_u64()), Some(105_000));
        assert_eq!(data.get("healed").and_then(|h| h.as_bool()), Some(true));
        assert!(err.message.contains("budget"));
        assert!(err.message.contains("verify"));
    }
}

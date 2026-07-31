//! Reconnect-only link from the stdio MCP proxy to the HTTP daemon.
//!
//! Never spawns / upserts a daemon — that remains the job of
//! `tdmcp-daemon ensure` / `mcp` cold start. When the HTTP session dies
//! (restart, crash, idle-exit), this module probes health and re-handshakes
//! against whatever is already listening.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rmcp::model::{ClientCapabilities, ClientInfo, Implementation};
use rmcp::service::RunningService;
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
/// Health GET budget (mirrors `ensure::health_ok`).
const HEALTH_TIMEOUT: Duration = Duration::from_millis(800);
/// Fresh `serve()` handshake budget.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// Env-configurable reconnect timing.
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
}

impl Default for ReconnectConfig {
    fn default() -> Self {
        Self {
            recent: Duration::from_millis(DEFAULT_RECENT_MS),
            stale: Duration::from_millis(DEFAULT_STALE_MS),
            debounce: Duration::from_millis(DEFAULT_DEBOUNCE_MS),
            probe_interval: Duration::from_millis(DEFAULT_PROBE_INTERVAL_MS),
            probe_max: Duration::from_millis(DEFAULT_PROBE_MAX_MS),
        }
    }
}

impl ReconnectConfig {
    /// Load from `TDMCP_RECONNECT_*` env vars; invalid values fall back to defaults.
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
        )
    }

    /// Parse optional raw env strings (tests).
    #[must_use]
    pub fn from_env_vars(
        recent: Option<&str>,
        stale: Option<&str>,
        debounce: Option<&str>,
        probe_interval: Option<&str>,
        probe_max: Option<&str>,
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
}

impl DaemonLink {
    /// Connect once, start the keep-warm watcher, return the link.
    pub async fn connect(
        daemon_url: &str,
        admin_base: String,
        config: ReconnectConfig,
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
        });
        link.clone().spawn_watcher();
        Ok(link)
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
    /// Never spawns a daemon. Single-flight; debounced.
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

        if self.debounced() {
            debug!("daemon_link: heal debounced");
            return HealOutcome {
                healed: false,
                downtime,
            };
        }

        let Ok(_gate) = self.reconnect_gate.try_lock() else {
            debug!("daemon_link: heal single-flight busy");
            return HealOutcome {
                healed: false,
                downtime,
            };
        };

        {
            let state = self.state.read().await;
            if state.generation != generation_used {
                return HealOutcome {
                    healed: true,
                    downtime: self.downtime().or(downtime),
                };
            }
        }

        self.note_probe();

        if !self.health_ok().await {
            debug!("daemon_link: health probe failed");
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
                    "daemon_link: reconnected to daemon"
                );
                let _ = old.cancel().await;
                self.reannotate_best_effort().await;
                HealOutcome {
                    healed: true,
                    downtime,
                }
            }
            Ok(Err(e)) => {
                warn!(error = %e, "daemon_link: reconnect handshake failed");
                HealOutcome {
                    healed: false,
                    downtime: self.downtime(),
                }
            }
            Err(_) => {
                warn!("daemon_link: reconnect handshake timed out");
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
                }
            }
            debug!("daemon_link: watcher stopped");
        });
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
            warn!(error = %e, "daemon_link: re-annotate after reconnect failed");
        }
    }
}

async fn connect_http(daemon_url: &str) -> Result<RunningService<RoleClient, ClientInfo>, String> {
    let http = StreamableHttpClientTransport::from_uri(daemon_url.to_string());
    let client_info = ClientInfo::new(
        ClientCapabilities::default(),
        Implementation::new(STDIO_PROXY_CLIENT_NAME, env!("CARGO_PKG_VERSION")),
    );
    client_info.serve(http).await.map_err(|e| e.to_string())
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
#[must_use]
pub fn unreachable_error(outcome: HealOutcome, config: &ReconnectConfig) -> ErrorData {
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
        let cfg = ReconnectConfig::from_env_vars(None, None, None, None, None);
        assert_eq!(cfg, ReconnectConfig::default());

        let cfg = ReconnectConfig::from_env_vars(
            Some("1000"),
            Some("20000"),
            Some("100"),
            Some("250"),
            Some("2000"),
        );
        assert_eq!(cfg.recent, Duration::from_millis(1000));
        assert_eq!(cfg.stale, Duration::from_millis(20000));
        assert_eq!(cfg.debounce, Duration::from_millis(100));
        assert_eq!(cfg.probe_interval, Duration::from_millis(250));
        assert_eq!(cfg.probe_max, Duration::from_millis(2000));

        let cfg = ReconnectConfig::from_env_vars(Some("nope"), None, None, None, None);
        assert_eq!(cfg.recent, Duration::from_millis(DEFAULT_RECENT_MS));
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
}

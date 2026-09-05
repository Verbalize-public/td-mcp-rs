//! Federation HTTP routes and slave background register / fleet-push.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{ConnectInfo, Extension, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tdmcp_config as cfgfile;
use tdmcp_core::{
    AggregatedFleetProcess, BridgeStatus, DaemonId, DaemonIdConflict, RemoteFleetProcess,
    SlaveEntry, SlaveReachability, SlaveRegistry,
};
use tdmcp_diagnostics::codes;
use tdmcp_mcp::{fleet_summary, AppState, FleetParams, FleetProcess};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::config::Config;

/// Shared federation state for admin routes + slave background task.
#[derive(Clone)]
pub struct FederationRuntime {
    /// Human-readable connection state, updated by the federation supervisor.
    pub link_status: tokio::sync::watch::Sender<String>,
    /// Shared settings and live role changes.
    pub settings: crate::settings::Settings,
    /// `standalone` | `master` | `slave`.
    pub role: String,
    /// Persistent daemon id.
    pub daemon_id: DaemonId,
    /// Advertised hostname.
    pub hostname: String,
    /// Listen port.
    pub port: u16,
    /// Bind address from config.
    pub bind_address: String,
    /// Absolute config path (for `/admin/config`).
    pub config_path: PathBuf,
    /// Master-side slave map.
    pub slaves: Arc<Mutex<SlaveRegistry>>,
    /// Local auth token advertised on register (`auth.psk` or empty).
    pub local_auth_token: String,
    /// Master base URL when role is slave.
    pub master_url: String,
    /// Master PSK when role is slave.
    pub master_psk: String,
    /// Daemon package version.
    pub version: String,
}

impl FederationRuntime {
    /// Build from resolved runtime config.
    #[must_use]
    pub fn from_config(cfg: &Config) -> Self {
        Self {
            link_status: tokio::sync::watch::channel("Starting".to_owned()).0,
            settings: crate::settings::Settings::new(cfg.config_path.clone(), cfg.file.clone()),
            role: cfg.federation_role.clone(),
            daemon_id: DaemonId::new(cfg.daemon_id.clone()),
            hostname: local_hostname(),
            port: cfg.port,
            bind_address: cfg.bind_address.clone(),
            config_path: cfg.config_path.clone(),
            slaves: Arc::new(Mutex::new(SlaveRegistry::new())),
            local_auth_token: if cfg.auth_mode == "psk" {
                cfg.auth_psk.clone()
            } else {
                String::new()
            },
            master_url: cfg.master_url.clone(),
            master_psk: cfg.master_psk.clone(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }
}

/// Best-effort hostname without extra crates.
#[must_use]
pub fn local_hostname() -> String {
    for key in ["COMPUTERNAME", "HOSTNAME"] {
        if let Ok(h) = std::env::var(key) {
            let trimmed = h.trim();
            if !trimmed.is_empty() {
                return trimmed.to_owned();
            }
        }
    }
    "localhost".to_owned()
}

/// Advertised base URL for register (maps wildcard binds to 127.0.0.1).
#[must_use]
pub fn advertised_base_url(bind_address: &str, port: u16) -> String {
    let host = if bind_address == "0.0.0.0" || bind_address == "::" {
        "127.0.0.1"
    } else {
        bind_address
    };
    let host = host
        .parse::<std::net::IpAddr>()
        .map(|ip| std::net::SocketAddr::new(ip, port).to_string());
    match host {
        Ok(authority) => format!("http://{authority}"),
        Err(_) => format!("http://{bind_address}:{port}"),
    }
}

/// Resolve a joining computer's callback origin.
fn registration_base_url(
    advertised: Option<&str>,
    port: u16,
    peer: Option<std::net::SocketAddr>,
) -> Result<String, &'static str> {
    let base = advertised
        .map(str::to_owned)
        .unwrap_or_else(|| advertised_base_url("127.0.0.1", port));
    let url = reqwest::Url::parse(&base).map_err(|_| "baseUrl must be an HTTP(S) origin")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err("baseUrl must be an HTTP(S) origin");
    }
    let local_only = url.host_str().is_some_and(|host| {
        host == "localhost"
            || host
                .trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback() || ip.is_unspecified())
    });
    // Repair only unusable local advertisements; preserve explicitly configured
    // LAN/TLS origins. Validate first so rewriting cannot hide an invalid URL.
    if let Some(peer) = peer.filter(|p| !p.ip().is_loopback() && local_only) {
        return Ok(advertised_base_url(&peer.ip().to_string(), port));
    }
    Ok(base.trim_end_matches('/').to_owned())
}

/// Routes: `/admin/federation/*` + `/admin/config`.
pub fn federation_router(rt: FederationRuntime) -> Router {
    Router::new()
        .route("/admin/federation/status", get(federation_status))
        .route("/admin/federation/register", post(federation_register))
        .route("/admin/federation/fleet-push", post(federation_fleet_push))
        .route("/admin/federation/slaves", get(federation_slaves))
        .route(
            "/admin/config",
            get(get_admin_config).post(post_admin_config),
        )
        .with_state(rt)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FederationStatusBody {
    ok: bool,
    version: String,
    role: String,
    hostname: String,
    daemon_id: String,
    port: u16,
    bind_address: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    slave_count: Option<usize>,
}

async fn federation_status(State(rt): State<FederationRuntime>) -> Json<FederationStatusBody> {
    let role = rt.settings.current().federation.role;
    let slave_count = if role == "master" {
        Some(rt.slaves.lock().await.len())
    } else {
        None
    };
    Json(FederationStatusBody {
        ok: true,
        version: rt.version.clone(),
        role,
        hostname: rt.hostname.clone(),
        daemon_id: rt.daemon_id.as_str().to_owned(),
        port: rt.port,
        bind_address: rt.bind_address.clone(),
        slave_count,
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegisterBody {
    daemon_id: String,
    hostname: String,
    version: String,
    port: u16,
    auth_token: String,
    #[serde(default)]
    base_url: Option<String>,
}

async fn federation_register(
    State(rt): State<FederationRuntime>,
    peer: Option<Extension<ConnectInfo<std::net::SocketAddr>>>,
    Json(body): Json<RegisterBody>,
) -> (StatusCode, Json<Value>) {
    if rt.settings.current().federation.role != "master" {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": "register requires role=master" })),
        );
    }
    if body.daemon_id.trim().is_empty() || body.daemon_id == rt.daemon_id.as_str() || body.port == 0
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(
                json!({"ok": false, "error": "registration requires a distinct daemonId and a nonzero port"}),
            ),
        );
    }
    let peer = peer.map(|Extension(ConnectInfo(peer))| peer);
    let base_url = match registration_base_url(body.base_url.as_deref(), body.port, peer) {
        Ok(url) => url,
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"ok":false,"error":error})),
            )
        }
    };
    let entry = SlaveEntry {
        daemon_id: DaemonId::new(body.daemon_id),
        hostname: body.hostname,
        version: body.version,
        base_url,
        port: body.port,
        auth_token: body.auth_token,
        last_fleet_push: Some(Utc::now()),
        reachability: SlaveReachability::Reachable,
        fleet_processes: vec![],
    };
    let mut slaves = rt.slaves.lock().await;
    match slaves.register(entry) {
        Ok(()) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "masterDaemonId": rt.daemon_id.as_str(),
            })),
        ),
        Err(DaemonIdConflict {
            daemon_id,
            existing_base_url,
            attempted_base_url,
        }) => (
            StatusCode::CONFLICT,
            Json(json!({
                "ok": false,
                "error": "daemon_id conflict",
                "daemonId": daemon_id.as_str(),
                "existingBaseUrl": existing_base_url,
                "attemptedBaseUrl": attempted_base_url,
            })),
        ),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FleetPushBody {
    daemon_id: String,
    #[serde(default)]
    processes: Vec<FleetPushProcess>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FleetPushProcess {
    pid: u32,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    toe_path: Option<String>,
    #[serde(default)]
    bridge: Option<String>,
}

async fn federation_fleet_push(
    State(rt): State<FederationRuntime>,
    Json(body): Json<FleetPushBody>,
) -> (StatusCode, Json<Value>) {
    if rt.settings.current().federation.role != "master" {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": "fleet-push requires role=master" })),
        );
    }
    let processes: Vec<RemoteFleetProcess> = body
        .processes
        .into_iter()
        .map(|p| RemoteFleetProcess {
            pid: p.pid,
            title: p.title,
            toe_path: p.toe_path,
            bridge: match p.bridge.as_deref() {
                Some("disconnected") => BridgeStatus::Disconnected,
                Some("starting") => BridgeStatus::Starting,
                _ => BridgeStatus::Connected,
            },
        })
        .collect();
    let mut slaves = rt.slaves.lock().await;
    if slaves.update_fleet(&DaemonId::new(body.daemon_id), processes, Utc::now()) {
        (StatusCode::OK, Json(json!({ "ok": true })))
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(json!({ "ok": false, "error": "unknown daemonId — register first" })),
        )
    }
}

async fn federation_slaves(State(rt): State<FederationRuntime>) -> Json<Value> {
    let mut slaves = rt.slaves.lock().await;
    slaves.tick_stale(Utc::now());
    let list: Vec<Value> = slaves
        .slaves()
        .into_iter()
        .map(|s| {
            json!({
                "daemonId": s.daemon_id.as_str(),
                "hostname": s.hostname,
                "version": s.version,
                "baseUrl": s.base_url,
                "port": s.port,
                "authToken": s.auth_token,
                "reachability": match s.reachability {
                    SlaveReachability::Reachable => "reachable",
                    SlaveReachability::Disconnected => "disconnected",
                    SlaveReachability::Unreachable => "unreachable",
                },
                "lastFleetPush": s.last_fleet_push,
                "processCount": s.fleet_processes.len(),
            })
        })
        .collect();
    Json(json!({ "slaves": list }))
}

async fn get_admin_config(State(rt): State<FederationRuntime>) -> (StatusCode, Json<Value>) {
    match cfgfile::load(&rt.config_path) {
        Ok(cfg) => match serde_json::to_value(cfg) {
            Ok(v) => (StatusCode::OK, Json(v)),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "ok": false, "error": e.to_string() })),
            ),
        },
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "ok": false, "error": e.to_string() })),
        ),
    }
}

async fn post_admin_config(
    State(rt): State<FederationRuntime>,
    Json(patch): Json<Value>,
) -> (StatusCode, Json<Value>) {
    match rt.settings.patch(patch).await {
        Ok(cfg) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "config": cfg,
                "restartRequired": rt.settings.restart_required(),
            })),
        ),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false, "error": format!("{error:#}")
            })),
        ),
    }
}

/// Tag local fleet rows with this daemon id/hostname.
#[must_use]
pub fn tag_local_processes(
    daemon_id: &DaemonId,
    hostname: &str,
    processes: &[FleetProcess],
) -> Vec<AggregatedFleetProcess> {
    processes
        .iter()
        .map(|p| AggregatedFleetProcess {
            pid: p.pid.get(),
            title: p.title.clone(),
            toe_path: p.toe_path.clone(),
            bridge: p.bridge,
            daemon_id: Some(daemon_id.clone()),
            hostname: Some(hostname.to_owned()),
        })
        .collect()
}

/// Slave background: register with backoff, then fleet-push every 2s.
pub fn spawn_slave_loop(
    rt: FederationRuntime,
    app: AppState,
    shutdown: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut settings = rt.settings.subscribe();
        loop {
            let cfg = settings.borrow_and_update().clone();
            let link_config = cfg.federation.clone();
            let mut active = rt.clone();
            active.role = cfg.federation.role;
            active.master_url = cfg.federation.master_url;
            active.master_psk = cfg.federation.master_psk;
            rt.link_status.send_replace(
                match active.role.as_str() {
                    "slave" => "Connecting to coordinator",
                    "master" => "Coordinating",
                    _ => "Local only",
                }
                .to_owned(),
            );
            let child_shutdown = shutdown.child_token();
            let child = if active.role == "slave" {
                Some(spawn_slave_connection(
                    active,
                    app.clone(),
                    child_shutdown.clone(),
                ))
            } else {
                None
            };
            loop {
                tokio::select! {
                    () = shutdown.cancelled() => break,
                    result = settings.changed() => {
                        if result.is_err() || settings.borrow_and_update().federation != link_config { break; }
                    },
                }
            }
            child_shutdown.cancel();
            if let Some(child) = child {
                child.abort();
                let _ = child.await;
            }
            // A former coordinator must not continue forwarding stale routes.
            if rt.settings.current().federation.role != "master" {
                *rt.slaves.lock().await = SlaveRegistry::new();
            }
            if shutdown.is_cancelled() {
                break;
            }
        }
    })
}

fn spawn_slave_connection(
    rt: FederationRuntime,
    app: AppState,
    shutdown: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let client = match reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                warn!(error = %e, "federation slave: HTTP client build failed");
                return;
            }
        };
        let master = rt.master_url.trim_end_matches('/').to_owned();
        if master.is_empty() {
            warn!("federation role=slave but master_url empty — slave loop idle");
            return;
        }
        let register_url = format!("{master}/admin/federation/register");
        let push_url = format!("{master}/admin/federation/fleet-push");
        let base_url = advertised_base_url(&rt.bind_address, rt.port);
        let mut backoff = Duration::from_secs(1);
        let max_backoff = Duration::from_secs(30);
        let mut registered = false;

        while !shutdown.is_cancelled() {
            if !registered {
                let body = json!({
                    "daemonId": rt.daemon_id.as_str(),
                    "hostname": rt.hostname,
                    "version": rt.version,
                    "port": rt.port,
                    "authToken": rt.local_auth_token,
                    "baseUrl": base_url,
                });
                let mut req = client.post(&register_url).json(&body);
                if !rt.master_psk.is_empty() {
                    req = req.bearer_auth(&rt.master_psk);
                }
                match req.send().await {
                    Ok(resp) if resp.status().is_success() => {
                        info!(master = %master, "federation slave registered");
                        registered = true;
                        backoff = Duration::from_secs(1);
                    }
                    Ok(resp) => {
                        let status = resp.status();
                        rt.link_status
                            .send_replace(if status == StatusCode::UNAUTHORIZED {
                                "Access key rejected — check the coordinator key".to_owned()
                            } else {
                                format!("Registration rejected (HTTP {status}) — retrying")
                            });
                        if status == StatusCode::UNAUTHORIZED {
                            warn!(
                                code = codes::FEDERATION_AUTH_REJECTED,
                                "federation register unauthorized — retrying"
                            );
                        } else {
                            warn!(%status, "federation register rejected — retrying");
                        }
                        tokio::select! {
                            () = shutdown.cancelled() => break,
                            () = tokio::time::sleep(backoff) => {}
                        }
                        backoff = (backoff * 2).min(max_backoff);
                        continue;
                    }
                    Err(e) => {
                        rt.link_status.send_replace(
                            "Coordinator unreachable — check URL and network; retrying".to_owned(),
                        );
                        warn!(error = %e, "federation register transport error");
                        tokio::select! {
                            () = shutdown.cancelled() => break,
                            () = tokio::time::sleep(backoff) => {}
                        }
                        backoff = (backoff * 2).min(max_backoff);
                        continue;
                    }
                }
            }

            let processes = {
                let registry = app.registry.lock().await;
                let fleet = fleet_summary(
                    &registry,
                    &FleetParams {
                        pids: None,
                        include: vec![],
                    },
                    &[],
                    None,
                );
                fleet
                    .processes
                    .into_iter()
                    .map(|p| {
                        json!({
                            "pid": p.pid.get(),
                            "title": p.title,
                            "toePath": p.toe_path,
                            "bridge": match p.bridge {
                                BridgeStatus::Starting => "starting",
                                BridgeStatus::Connected => "connected",
                                BridgeStatus::Disconnected => "disconnected",
                            },
                        })
                    })
                    .collect::<Vec<_>>()
            };
            let body = json!({
                "daemonId": rt.daemon_id.as_str(),
                "processes": processes,
            });
            let mut req = client.post(&push_url).json(&body);
            if !rt.master_psk.is_empty() {
                req = req.bearer_auth(&rt.master_psk);
            }
            match req.send().await {
                Ok(resp) if resp.status().is_success() => {
                    rt.link_status
                        .send_replace("Connected to coordinator".to_owned());
                }
                Ok(resp) if resp.status() == StatusCode::NOT_FOUND => {
                    rt.link_status
                        .send_replace("Reconnecting to coordinator".to_owned());
                    warn!("fleet-push unknown to master — will re-register");
                    registered = false;
                }
                Ok(resp) if resp.status() == StatusCode::UNAUTHORIZED => {
                    rt.link_status
                        .send_replace("Access key rejected — check the coordinator key".to_owned());
                    warn!(
                        code = codes::FEDERATION_AUTH_REJECTED,
                        "fleet-push unauthorized — will re-register"
                    );
                    registered = false;
                }
                Ok(resp) => {
                    rt.link_status.send_replace(format!(
                        "Fleet update rejected (HTTP {}) — retrying",
                        resp.status()
                    ));
                    warn!(status = %resp.status(), "fleet-push failed");
                }
                Err(e) => {
                    rt.link_status
                        .send_replace("Coordinator unreachable — retrying".to_owned());
                    warn!(error = %e, "fleet-push transport error");
                    registered = false;
                }
            }

            tokio::select! {
                () = shutdown.cancelled() => break,
                () = tokio::time::sleep(Duration::from_secs(2)) => {}
            }
        }
        info!("federation slave loop stopped");
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test fixtures")]
mod tests {
    use super::*;

    #[test]
    fn callback_repair_preserves_remote_origins_and_rejects_invalid_inputs() {
        let peer = Some("192.168.2.10:4567".parse().unwrap());
        assert_eq!(
            registration_base_url(Some("http://127.0.0.1:9860"), 9860, peer).unwrap(),
            "http://192.168.2.10:9860"
        );
        assert_eq!(
            registration_base_url(Some("https://render.example:443/"), 9860, peer).unwrap(),
            "https://render.example:443"
        );
        assert_eq!(
            registration_base_url(
                Some("http://[::]:9860"),
                9860,
                Some("[fd00::5]:1234".parse().unwrap())
            )
            .unwrap(),
            "http://[fd00::5]:9860"
        );
        for bad in [
            "http://127.0.0.1/path",
            "ftp://127.0.0.1",
            "http://user:key@127.0.0.1",
            "http://localhost/?key=secret",
        ] {
            assert!(
                registration_base_url(Some(bad), 9860, peer).is_err(),
                "{bad}"
            );
        }
    }
}

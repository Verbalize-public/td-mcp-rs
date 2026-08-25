//! Federation HTTP routes and slave background register / fleet-push.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tdmcp_config::{self as cfgfile, ConfigFile};
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
    format!("http://{host}:{port}")
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
    let slave_count = if rt.role == "master" {
        Some(rt.slaves.lock().await.len())
    } else {
        None
    };
    Json(FederationStatusBody {
        ok: true,
        version: rt.version.clone(),
        role: rt.role.clone(),
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
    Json(body): Json<RegisterBody>,
) -> (StatusCode, Json<Value>) {
    if rt.role != "master" {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": "register requires role=master" })),
        );
    }
    let base_url = body
        .base_url
        .unwrap_or_else(|| advertised_base_url("127.0.0.1", body.port));
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
    if rt.role != "master" {
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
    let mut cfg = match cfgfile::load(&rt.config_path) {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "ok": false, "error": e.to_string() })),
            );
        }
    };
    if let Err(e) = merge_config_patch(&mut cfg, &patch) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": e })),
        );
    }
    if let Err(e) = cfgfile::validate_remote_auth(&cfg) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": e.to_string() })),
        );
    }
    if let Err(e) = cfgfile::save(&rt.config_path, &cfg) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "ok": false, "error": e.to_string() })),
        );
    }
    match serde_json::to_value(&cfg) {
        Ok(v) => (StatusCode::OK, Json(json!({ "ok": true, "config": v }))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "ok": false, "error": e.to_string() })),
        ),
    }
}

fn merge_config_patch(cfg: &mut ConfigFile, patch: &Value) -> Result<(), String> {
    let obj = patch
        .as_object()
        .ok_or_else(|| "body must be a JSON object".to_owned())?;
    if let Some(server) = obj.get("server").and_then(Value::as_object) {
        if let Some(v) = server.get("port").and_then(Value::as_u64) {
            cfg.server.port =
                u16::try_from(v).map_err(|_| "server.port out of range".to_owned())?;
        }
        if let Some(s) = server
            .get("bindAddress")
            .or_else(|| server.get("bind_address"))
            .and_then(Value::as_str)
        {
            cfg.server.bind_address = s.to_owned();
        }
    }
    if let Some(auth) = obj.get("auth").and_then(Value::as_object) {
        if let Some(s) = auth.get("mode").and_then(Value::as_str) {
            cfg.auth.mode = s.to_owned();
        }
        if let Some(s) = auth.get("psk").and_then(Value::as_str) {
            cfg.auth.psk = s.to_owned();
        }
    }
    if let Some(fed) = obj.get("federation").and_then(Value::as_object) {
        if let Some(s) = fed.get("role").and_then(Value::as_str) {
            cfg.federation.role = s.to_owned();
        }
        if let Some(s) = fed
            .get("masterUrl")
            .or_else(|| fed.get("master_url"))
            .and_then(Value::as_str)
        {
            cfg.federation.master_url = s.to_owned();
        }
        if let Some(s) = fed
            .get("masterPsk")
            .or_else(|| fed.get("master_psk"))
            .and_then(Value::as_str)
        {
            cfg.federation.master_psk = s.to_owned();
        }
    }
    if let Some(daemon) = obj.get("daemon").and_then(Value::as_object) {
        if let Some(v) = daemon
            .get("keepAlive")
            .or_else(|| daemon.get("keep_alive"))
            .and_then(Value::as_bool)
        {
            cfg.daemon.keep_alive = v;
        }
        if let Some(v) = daemon
            .get("alwaysOn")
            .or_else(|| daemon.get("always_on"))
            .and_then(Value::as_bool)
        {
            cfg.daemon.always_on = v;
        }
        if let Some(v) = daemon
            .get("showTray")
            .or_else(|| daemon.get("show_tray"))
            .and_then(Value::as_bool)
        {
            cfg.daemon.show_tray = v;
        }
    }
    if let Some(bridge) = obj.get("bridge").and_then(Value::as_object) {
        if let Some(v) = bridge
            .get("callTimeoutSecs")
            .or_else(|| bridge.get("call_timeout_secs"))
            .and_then(Value::as_u64)
        {
            cfg.bridge.call_timeout_secs = v;
        }
        if let Some(v) = bridge
            .get("scriptTimeoutSecs")
            .or_else(|| bridge.get("script_timeout_secs"))
            .and_then(Value::as_u64)
        {
            cfg.bridge.script_timeout_secs = v;
        }
        if let Some(v) = bridge
            .get("heartbeatIntervalSecs")
            .or_else(|| bridge.get("heartbeat_interval_secs"))
            .and_then(Value::as_u64)
        {
            cfg.bridge.heartbeat_interval_secs = v;
        }
        if let Some(v) = bridge
            .get("pongTimeoutSecs")
            .or_else(|| bridge.get("pong_timeout_secs"))
            .and_then(Value::as_u64)
        {
            cfg.bridge.pong_timeout_secs = v;
        }
        if let Some(v) = bridge
            .get("idleDeadSecs")
            .or_else(|| bridge.get("idle_dead_secs"))
            .and_then(Value::as_u64)
        {
            cfg.bridge.idle_dead_secs = v;
        }
    }
    Ok(())
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
                Ok(resp) if resp.status().is_success() => {}
                Ok(resp) if resp.status() == StatusCode::NOT_FOUND => {
                    warn!("fleet-push unknown to master — will re-register");
                    registered = false;
                }
                Ok(resp) if resp.status() == StatusCode::UNAUTHORIZED => {
                    warn!(
                        code = codes::FEDERATION_AUTH_REJECTED,
                        "fleet-push unauthorized — will re-register"
                    );
                    registered = false;
                }
                Ok(resp) => warn!(status = %resp.status(), "fleet-push failed"),
                Err(e) => {
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

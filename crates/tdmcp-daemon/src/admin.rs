//! Loopback admin API for the GUI (`/admin/*`).

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;
use tracing::info;

use tdmcp_mcp::{fleet_summary, AppState, FleetInclude, FleetParams};

use crate::ensure::configure_detached_spawn;
use crate::federation::{tag_local_processes, FederationRuntime};
use crate::logrecord::{Level, Src};
use crate::logring::{ingest_proxy_logs, LogSink};

/// Arguments needed to respawn the daemon after `/admin/restart`.
#[derive(Debug, Clone)]
pub struct RestartArgs {
    /// Absolute path to this daemon binary.
    pub exe: PathBuf,
    /// Listen port.
    pub port: u16,
    /// Configured bind address (reported by `/admin/status`).
    pub bind_address: String,
    /// Data directory.
    pub data_dir: PathBuf,
    /// Bridge package directory.
    pub bridge_dir: PathBuf,
    /// Catalog path.
    pub catalog_path: PathBuf,
    /// When true, respawn with `--no-gui`.
    pub no_gui: bool,
}

/// Admin router state: shared MCP state + restart args + lifecycle controls.
#[derive(Clone)]
struct AdminState {
    app: AppState,
    restart: RestartArgs,
    shutdown: CancellationToken,
    quit: Arc<AtomicBool>,
    federation: FederationRuntime,
    logs: LogSink,
    logs_dir: PathBuf,
}

/// Process start instant for the `/admin/status` uptime field.
/// The GUI runs in-process, so router build time ≈ process start.
static START: OnceLock<Instant> = OnceLock::new();

/// Admin router.
#[allow(clippy::too_many_arguments, reason = "router wiring")]
pub fn build_admin_router(
    state: AppState,
    restart: RestartArgs,
    shutdown: CancellationToken,
    quit: Arc<AtomicBool>,
    federation: FederationRuntime,
    logs: LogSink,
    logs_dir: PathBuf,
) -> Router {
    let _ = START.set(Instant::now());
    let state = AdminState {
        app: state,
        restart,
        shutdown,
        quit,
        federation: federation.clone(),
        logs,
        logs_dir,
    };
    Router::new()
        .route("/admin/status", get(status))
        .route("/admin/fleet", get(admin_fleet))
        .route("/admin/mcp-sessions", get(mcp_sessions))
        .route("/admin/mcp-sessions/annotate", post(annotate_session))
        .route("/admin/shutdown", post(shutdown_handler))
        .route("/admin/restart", post(restart_daemon))
        .route("/admin/logs", get(admin_logs))
        .route("/admin/logs/path", get(admin_logs_path))
        .route("/admin/logs/ingest", post(admin_logs_ingest))
        .with_state(state)
        .merge(crate::federation::federation_router(federation))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StatusBody {
    ok: bool,
    restart_required: Vec<&'static str>,
    federation_connection: String,
    version: &'static str,
    pid: u32,
    mcp_session_count: usize,
    bridge_count: usize,
    /// True when this process was started with `--no-gui` / headless.
    no_gui: bool,
    /// Configured listen IP (`server.bind_address`).
    bind_address: String,
    /// Federation role.
    role: String,
    /// Persistent daemon id.
    daemon_id: String,
    /// Local hostname.
    hostname: String,
    /// Registered slave count when `role = master`.
    #[serde(skip_serializing_if = "Option::is_none")]
    slave_count: Option<usize>,
    /// Seconds since this daemon process started.
    uptime_secs: u64,
    /// Dialogs backend state: `"ok"` (real backend + watcher running),
    /// `"unsupported_platform"` (backend installed, no window introspection —
    /// Linux), or `"disabled"` (`[dialogs].enabled = false`).
    dialogs_status: &'static str,
}

async fn status(State(state): State<AdminState>) -> Json<StatusBody> {
    let bridge_count = {
        let registry = state.app.registry.lock().await;
        let params = FleetParams {
            pids: None,
            include: vec![],
        };
        let fleet = fleet_summary(&registry, &params, &[], None);
        fleet
            .processes
            .iter()
            .filter(|p| p.bridge == tdmcp_core::BridgeStatus::Connected)
            .count()
    };
    let slave_count = if state.federation.settings.current().federation.role == "master" {
        Some(state.federation.slaves.lock().await.len())
    } else {
        None
    };
    let dialogs_status = match tdmcp_mcp::dialogs::get() {
        Some(shared) if shared.source.supports_dialogs() => "ok",
        Some(_) => "unsupported_platform",
        None => "disabled",
    };
    Json(StatusBody {
        ok: true,
        restart_required: state.federation.settings.restart_required(),
        federation_connection: state.federation.link_status.borrow().clone(),
        version: env!("CARGO_PKG_VERSION"),
        pid: std::process::id(),
        mcp_session_count: state.app.mcp_session_count(),
        bridge_count,
        no_gui: state.restart.no_gui,
        bind_address: state.restart.bind_address.clone(),
        role: state.federation.settings.current().federation.role,
        daemon_id: state.federation.daemon_id.as_str().to_owned(),
        hostname: state.federation.hostname.clone(),
        slave_count,
        uptime_secs: START
            .get()
            .map(|t| t.elapsed().as_secs())
            .unwrap_or_default(),
        dialogs_status,
    })
}

async fn admin_fleet(State(state): State<AdminState>) -> Json<Value> {
    let params = FleetParams {
        pids: None,
        include: vec![FleetInclude::Tasks, FleetInclude::Cancelled],
    };
    let pids: Vec<u32> = {
        let registry = state.app.registry.lock().await;
        registry.pids()
    };
    let mut ipc_depths = Vec::new();
    for pid in pids {
        if let Some(depth) = state.app.bridge.job_queue_depth(pid).await {
            if depth > 0 {
                ipc_depths.push((pid, depth));
            }
        }
    }
    let local = {
        let registry = state.app.registry.lock().await;
        fleet_summary(&registry, &params, &ipc_depths, None)
    };
    let mut slaves = state.federation.slaves.lock().await;
    slaves.tick_stale(Utc::now());
    let tagged = tag_local_processes(
        &state.federation.daemon_id,
        &state.federation.hostname,
        &local.processes,
    );
    let aggregated = slaves.aggregate_fleet(
        &state.federation.daemon_id,
        &state.federation.hostname,
        tagged,
    );
    let processes: Vec<Value> = aggregated
        .into_iter()
        .map(|p| serde_json::to_value(&p).unwrap_or(Value::Null))
        .collect();
    Json(json!({ "processes": processes }))
}

async fn mcp_sessions(State(state): State<AdminState>) -> Json<Value> {
    let sessions = state.app.mcp_sessions.list();
    Json(serde_json::json!({ "sessions": sessions }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AnnotateBody {
    /// Exact session id (preferred).
    #[serde(default)]
    id: Option<String>,
    /// Match newest session with this clientName (stdio proxy path).
    #[serde(default)]
    match_client_name: Option<String>,
    client_name: String,
    #[serde(default)]
    client_version: String,
}

async fn annotate_session(
    State(state): State<AdminState>,
    Json(body): Json<AnnotateBody>,
) -> Json<Value> {
    if let Some(id) = body.id.as_deref().filter(|s| !s.is_empty()) {
        let ok = state
            .app
            .mcp_sessions
            .annotate(id, body.client_name, body.client_version);
        return Json(serde_json::json!({ "ok": ok, "id": id }));
    }
    if let Some(match_name) = body.match_client_name.as_deref().filter(|s| !s.is_empty()) {
        match state.app.mcp_sessions.annotate_latest_matching(
            match_name,
            body.client_name,
            body.client_version,
        ) {
            Some(id) => Json(serde_json::json!({ "ok": true, "id": id })),
            None => Json(serde_json::json!({ "ok": false, "error": "no matching session" })),
        }
    } else {
        Json(serde_json::json!({ "ok": false, "error": "id or matchClientName required" }))
    }
}

/// Request process shutdown: set quit flag and cancel the serve loop.
///
/// Brief delay lets this HTTP response flush before axum begins draining.
async fn shutdown_handler(State(state): State<AdminState>) -> Json<Value> {
    let token = state.shutdown.clone();
    let quit = Arc::clone(&state.quit);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        quit.store(true, Ordering::SeqCst);
        token.cancel();
    });
    Json(serde_json::json!({ "ok": true }))
}

async fn restart_daemon(State(state): State<AdminState>) -> (StatusCode, Json<Value>) {
    let args = state.restart.clone();
    let saved = match tdmcp_config::load(&state.federation.config_path) {
        Ok(saved) => saved,
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"ok": false, "error": error.to_string()})),
            )
        }
    };
    let startup = state.federation.settings.startup();
    let overrides = crate::config::ConfigOverrides {
        port: (saved.server.port == startup.server.port).then_some(args.port),
        data_dir: (saved.advanced.data_dir == startup.advanced.data_dir)
            .then_some(args.data_dir.clone()),
        bridge_dir: (saved.advanced.bridge_dir == startup.advanced.bridge_dir)
            .then_some(args.bridge_dir),
        catalog: (saved.advanced.catalog_path == startup.advanced.catalog_path)
            .then_some(args.catalog_path),
        no_gui: args.no_gui && saved.daemon.show_tray == startup.daemon.show_tray,
    };
    let config = match crate::config::Config::from_file(
        state.federation.config_path.clone(),
        saved,
        overrides,
    ) {
        Ok(config) => config,
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"ok": false, "error": error.to_string()})),
            )
        }
    };
    let mut cmd = Command::new(&args.exe);
    cmd.arg("start")
        .arg("--wait-for-pid")
        .arg(std::process::id().to_string())
        .arg("--port")
        .arg(config.port.to_string())
        .arg("--data-dir")
        .arg(&config.data_dir)
        .arg("--bridge-dir")
        .arg(&config.bridge_dir)
        .arg("--catalog")
        .arg(&config.catalog_path)
        .env(tdmcp_config::CONFIG_PATH_ENV, &config.config_path);
    if config.no_gui {
        cmd.arg("--no-gui");
    }
    configure_detached_spawn(&mut cmd, config.no_gui);
    match cmd.spawn() {
        Ok(child) => info!(
            child_pid = child.id(),
            "replacement daemon waiting for shutdown"
        ),
        Err(error) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(
                    json!({"ok": false, "error": format!("could not start replacement: {error}")}),
                ),
            )
        }
    }
    // Retain ownership until shutdown. The child waits for our process to exit,
    // so it cannot bind early or have its fresh lock removed by our cleanup.
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        state.quit.store(true, Ordering::SeqCst);
        state.shutdown.cancel();
    });
    (
        StatusCode::OK,
        Json(json!({"ok": true, "restarting": true, "port": config.port})),
    )
}

/// Records returned per page (also the default when `limit` is omitted).
const LOGS_DEFAULT_LIMIT: usize = 256;
/// Server-side hard cap regardless of the requested `limit` (spec T4.1).
const LOGS_MAX_LIMIT: usize = 512;

#[derive(Debug, Deserialize)]
struct LogsQuery {
    #[serde(default)]
    after: u64,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    level: Option<String>,
    #[serde(default)]
    src: Option<String>,
}

fn bad_request(message: impl Into<String>) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "ok": false, "message": message.into() })),
    )
        .into_response()
}

fn parse_level(s: &str) -> Option<Level> {
    serde_json::from_value(Value::String(s.to_ascii_lowercase())).ok()
}

fn parse_src(s: &str) -> Option<Src> {
    serde_json::from_value(Value::String(s.to_ascii_lowercase())).ok()
}

/// `GET /admin/logs?after&limit&level&src` — tail of the central ring,
/// newest cursor first via `next` (spec T4.1). `src` is a comma-separated
/// list (`src=bridge,daemon`); omitted/empty means no source filter.
async fn admin_logs(State(state): State<AdminState>, Query(q): Query<LogsQuery>) -> Response {
    let limit = q
        .limit
        .unwrap_or(LOGS_DEFAULT_LIMIT)
        .clamp(1, LOGS_MAX_LIMIT);
    let min_level = match q.level.as_deref() {
        None => None,
        Some(s) => match parse_level(s) {
            Some(l) => Some(l),
            None => return bad_request(format!("invalid level: {s}")),
        },
    };
    let srcs: Vec<Src> = match q.src.as_deref().filter(|s| !s.is_empty()) {
        None => Vec::new(),
        Some(s) => {
            let mut out = Vec::new();
            for part in s.split(',') {
                match parse_src(part) {
                    Some(v) => out.push(v),
                    None => return bad_request(format!("invalid src: {part}")),
                }
            }
            out
        }
    };
    let (records, next) = state
        .logs
        .ring()
        .snapshot_after(q.after, limit, min_level, &srcs);
    Json(json!({ "records": records, "next": next })).into_response()
}

/// `GET /admin/logs/path` — the resolved logging directory (tray "Open
/// folder" action).
async fn admin_logs_path(State(state): State<AdminState>) -> Json<Value> {
    Json(json!({ "dir": state.logs_dir.display().to_string() }))
}

/// `POST /admin/logs/ingest` — M5 stdio-proxy uplink.
/// Body: `{"pid": <proxy pid>, "lines": [{level,target,msg,kvs?,code?,ts?}, ...]}`.
/// The proxy pid is a display hint (loopback peer, not an identity) — every
/// ingested record's own `pid` field is 0; the proxy pid lands in
/// `kvs.proxyPid` (see `ingest_proxy_logs`).
async fn admin_logs_ingest(State(state): State<AdminState>, Json(body): Json<Value>) -> Response {
    let proxy_pid = body.get("pid").and_then(Value::as_u64).unwrap_or(0) as u32;
    let ingested = ingest_proxy_logs(proxy_pid, &body, &state.logs);
    Json(json!({ "ok": true, "ingested": ingested })).into_response()
}

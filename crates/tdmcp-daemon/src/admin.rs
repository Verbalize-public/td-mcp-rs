//! Loopback admin API for the GUI (`/admin/*`).

use std::path::PathBuf;
use std::process::Command;

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;
use serde_json::Value;
use tracing::{info, warn};

use tdmcp_mcp::{fleet_summary, AppState, FleetInclude, FleetParams};

use crate::ensure::daemon_lock_path;

/// Arguments needed to respawn the daemon after `/admin/restart`.
#[derive(Debug, Clone)]
pub struct RestartArgs {
    /// Absolute path to this daemon binary.
    pub exe: PathBuf,
    /// Listen port.
    pub port: u16,
    /// Data directory.
    pub data_dir: PathBuf,
    /// Bridge package directory.
    pub bridge_dir: PathBuf,
    /// Catalog path.
    pub catalog_path: PathBuf,
    /// When true, respawn with `--no-gui`.
    pub no_gui: bool,
}

/// Admin router state: shared MCP state + restart args.
#[derive(Clone)]
struct AdminState {
    app: AppState,
    restart: RestartArgs,
}

/// Admin router.
pub fn build_admin_router(state: AppState, restart: RestartArgs) -> Router {
    let state = AdminState {
        app: state,
        restart,
    };
    Router::new()
        .route("/admin/status", get(status))
        .route("/admin/fleet", get(admin_fleet))
        .route("/admin/shutdown", post(shutdown))
        .route("/admin/restart", post(restart_daemon))
        .route("/admin/history", get(history))
        .with_state(state)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StatusBody {
    ok: bool,
    version: &'static str,
    pid: u32,
}

async fn status() -> Json<StatusBody> {
    Json(StatusBody {
        ok: true,
        version: env!("CARGO_PKG_VERSION"),
        pid: std::process::id(),
    })
}

async fn admin_fleet(State(state): State<AdminState>) -> Json<Value> {
    let registry = state.app.registry.lock().await;
    let params = FleetParams {
        pids: None,
        include: vec![FleetInclude::Tasks, FleetInclude::Cancelled],
    };
    let fleet = fleet_summary(&registry, &params);
    Json(serde_json::to_value(fleet).unwrap_or(Value::Null))
}

async fn history(State(state): State<AdminState>) -> Json<Value> {
    let registry = state.app.registry.lock().await;
    let params = FleetParams {
        pids: None,
        include: vec![FleetInclude::Tasks, FleetInclude::Cancelled],
    };
    let fleet = fleet_summary(&registry, &params);
    Json(serde_json::json!({ "history": fleet.processes }))
}

async fn shutdown() -> Json<Value> {
    tokio::spawn(async {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        #[allow(clippy::exit, reason = "admin shutdown endpoint")]
        std::process::exit(0);
    });
    Json(serde_json::json!({ "ok": true }))
}

async fn restart_daemon(State(state): State<AdminState>) -> Json<Value> {
    let args = state.restart.clone();
    info!(exe = %args.exe.display(), port = args.port, "admin restart requested");
    // Drop the owner lock before spawning so the replacement does not refuse
    // as "already running" while we are still alive (spawn-then-exit handoff).
    let _ = std::fs::remove_file(daemon_lock_path(&args.data_dir));
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let mut cmd = Command::new(&args.exe);
        cmd.arg("start")
            .arg("--port")
            .arg(args.port.to_string())
            .arg("--data-dir")
            .arg(&args.data_dir)
            .arg("--bridge-dir")
            .arg(&args.bridge_dir)
            .arg("--catalog")
            .arg(&args.catalog_path);
        if args.no_gui {
            cmd.arg("--no-gui");
        }
        match cmd.spawn() {
            Ok(child) => {
                info!(child_pid = child.id(), "spawned replacement daemon");
            }
            Err(e) => {
                warn!(error = %e, "failed to spawn replacement daemon — exiting anyway");
            }
        }
        #[allow(clippy::exit, reason = "admin restart endpoint")]
        std::process::exit(0);
    });
    Json(serde_json::json!({ "ok": true, "restarting": true }))
}

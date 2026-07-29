//! Loopback admin API for the GUI (`/admin/*`).

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;
use serde_json::Value;

use tdmcp_mcp::{fleet_summary, AppState, FleetInclude, FleetParams};

/// Admin router.
pub fn build_admin_router(state: AppState) -> Router {
    Router::new()
        .route("/admin/status", get(status))
        .route("/admin/fleet", get(admin_fleet))
        .route("/admin/shutdown", post(shutdown))
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

async fn admin_fleet(State(state): State<AppState>) -> Json<Value> {
    let registry = state.registry.lock().await;
    let params = FleetParams {
        pids: None,
        include: vec![FleetInclude::Tasks, FleetInclude::Cancelled],
    };
    let fleet = fleet_summary(&registry, &params);
    Json(serde_json::to_value(fleet).unwrap_or(Value::Null))
}

async fn history(State(state): State<AppState>) -> Json<Value> {
    // Task history is currently the live queue + cancelled stack per pid.
    let registry = state.registry.lock().await;
    let params = FleetParams {
        pids: None,
        include: vec![FleetInclude::Tasks, FleetInclude::Cancelled],
    };
    let fleet = fleet_summary(&registry, &params);
    Json(serde_json::json!({ "history": fleet.processes }))
}

async fn shutdown() -> Json<Value> {
    // Best-effort: schedule exit. Full graceful stop via signal in a later pass.
    tokio::spawn(async {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        #[allow(clippy::exit, reason = "admin shutdown endpoint")]
        std::process::exit(0);
    });
    Json(serde_json::json!({ "ok": true }))
}

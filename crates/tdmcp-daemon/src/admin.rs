//! Loopback admin API for the GUI (`/admin/*`).

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use tdmcp_mcp::{fleet_summary, AppState, FleetInclude, FleetParams};

use crate::ensure::{configure_detached_spawn, daemon_lock_path};

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

/// Admin router state: shared MCP state + restart args + lifecycle controls.
#[derive(Clone)]
struct AdminState {
	app: AppState,
	restart: RestartArgs,
	shutdown: CancellationToken,
	quit: Arc<AtomicBool>,
}

/// Admin router.
pub fn build_admin_router(
	state: AppState,
	restart: RestartArgs,
	shutdown: CancellationToken,
	quit: Arc<AtomicBool>,
) -> Router {
	let state = AdminState {
		app: state,
		restart,
		shutdown,
		quit,
	};
	Router::new()
		.route("/admin/status", get(status))
		.route("/admin/fleet", get(admin_fleet))
		.route("/admin/mcp-sessions", get(mcp_sessions))
		.route("/admin/mcp-sessions/annotate", post(annotate_session))
		.route("/admin/shutdown", post(shutdown_handler))
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
	mcp_session_count: usize,
	bridge_count: usize,
}

async fn status(State(state): State<AdminState>) -> Json<StatusBody> {
	let registry = state.app.registry.lock().await;
	let params = FleetParams {
		pids: None,
		include: vec![],
	};
	let fleet = fleet_summary(&registry, &params);
	let bridge_count = fleet
		.processes
		.iter()
		.filter(|p| p.bridge == tdmcp_core::BridgeStatus::Connected)
		.count();
	Json(StatusBody {
		ok: true,
		version: env!("CARGO_PKG_VERSION"),
		pid: std::process::id(),
		mcp_session_count: state.app.mcp_session_count(),
		bridge_count,
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

async fn history(State(state): State<AdminState>) -> Json<Value> {
	let registry = state.app.registry.lock().await;
	let params = FleetParams {
		pids: None,
		include: vec![FleetInclude::Tasks, FleetInclude::Cancelled],
	};
	let fleet = fleet_summary(&registry, &params);
	Json(serde_json::json!({ "history": fleet.processes }))
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

async fn restart_daemon(State(state): State<AdminState>) -> Json<Value> {
	let args = state.restart.clone();
	let token = state.shutdown.clone();
	let quit = Arc::clone(&state.quit);
	info!(exe = %args.exe.display(), port = args.port, "admin restart requested");
	// Drop the owner lock before spawning so the replacement does not refuse
	// as "already running" while we are still alive (spawn-then-die handoff).
	let _ = std::fs::remove_file(daemon_lock_path(&args.data_dir));
	tokio::spawn(async move {
		tokio::time::sleep(Duration::from_millis(200)).await;
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
		// Same detach as ensure — no inherited console flash on Windows.
		configure_detached_spawn(&mut cmd, args.no_gui);
		match cmd.spawn() {
			Ok(child) => {
				info!(child_pid = child.id(), "spawned replacement daemon");
			}
			Err(e) => {
				warn!(error = %e, "failed to spawn replacement daemon — exiting anyway");
			}
		}
		quit.store(true, Ordering::SeqCst);
		token.cancel();
	});
	Json(serde_json::json!({ "ok": true, "restarting": true }))
}

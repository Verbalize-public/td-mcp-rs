//! Axum router for MCP-style JSON tool calls + rmcp-ready state.
//!
//! Streamable HTTP via `rmcp` is nested when the `rmcp` feature set is available;
//! this module always exposes a simple `/mcp/tools/*` JSON surface for tests
//! and as a fallback.

use std::sync::Arc;

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::Value;
use tdmcp_core::PidRegistry;
use tdmcp_diagnostics::Catalog;
use tokio::sync::Mutex;

use crate::tools::{dispatch_tool, tool_descriptors, ToolCallError};

/// Shared daemon state for MCP + admin handlers.
#[derive(Clone)]
pub struct AppState {
    /// Pid registry.
    pub registry: Arc<Mutex<PidRegistry>>,
    /// Diagnostic catalog.
    pub catalog: Arc<Catalog>,
}

impl AppState {
    /// Construct shared state.
    #[must_use]
    pub fn new(registry: PidRegistry, catalog: Catalog) -> Self {
        Self {
            registry: Arc::new(Mutex::new(registry)),
            catalog: Arc::new(catalog),
        }
    }
}

#[derive(Debug, Deserialize)]
struct CallBody {
    name: String,
    #[serde(default)]
    arguments: Value,
}

/// Build the MCP JSON router (`/mcp/...`).
pub fn build_mcp_router(state: AppState) -> Router {
    Router::new()
        .route(
            "/mcp/health",
            get(|| async { Json(serde_json::json!({"ok": true})) }),
        )
        .route("/mcp/tools/list", get(list_tools))
        .route("/mcp/tools/call", post(call_tool))
        .with_state(state)
}

async fn list_tools() -> Json<Value> {
    Json(serde_json::json!({ "tools": tool_descriptors() }))
}

async fn call_tool(
    State(state): State<AppState>,
    Json(body): Json<CallBody>,
) -> Result<Json<Value>, axum::http::StatusCode> {
    let mut registry = state.registry.lock().await;
    match dispatch_tool(&mut registry, &state.catalog, &body.name, body.arguments) {
        Ok(v) => Ok(Json(serde_json::json!({ "ok": true, "data": v }))),
        Err(ToolCallError::Failed {
            summary,
            diagnostics,
        }) => Ok(Json(serde_json::json!({
            "ok": false,
            "summary": summary,
            "diagnostics": diagnostics,
        }))),
        Err(ToolCallError::UnknownTool(_)) | Err(ToolCallError::InvalidArgs(_)) => {
            Err(axum::http::StatusCode::BAD_REQUEST)
        }
    }
}

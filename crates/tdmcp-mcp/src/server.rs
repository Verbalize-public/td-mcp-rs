//! Axum router for MCP-style JSON tool calls.
//!
//! A simple `/mcp/*` JSON surface for tests and as a fallback. The rmcp
//! Streamable HTTP layer nests on top of the same [`AppState`].

use std::sync::Arc;

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::Value;
use tdmcp_core::PidRegistry;
use tdmcp_diagnostics::Catalog;
use tokio::sync::Mutex;

use crate::bridge_rpc::BridgeRpc;
use crate::tools::{dispatch_tool, tool_descriptors, ToolCallError};

/// Shared daemon state for MCP + admin handlers.
#[derive(Clone)]
pub struct AppState {
    /// Pid registry.
    pub registry: Arc<Mutex<PidRegistry>>,
    /// Diagnostic catalog.
    pub catalog: Arc<Catalog>,
    /// Live bridge transport (daemon-supplied).
    pub bridge: Arc<dyn BridgeRpc>,
}

impl AppState {
    /// Construct shared state from an owned registry.
    #[must_use]
    pub fn new(registry: PidRegistry, catalog: Catalog, bridge: Arc<dyn BridgeRpc>) -> Self {
        Self {
            registry: Arc::new(Mutex::new(registry)),
            catalog: Arc::new(catalog),
            bridge,
        }
    }

    /// Construct shared state from an already-shared registry (daemon composition
    /// root shares one registry Arc across MCP + bridge sessions + admin).
    #[must_use]
    pub fn new_shared(
        registry: Arc<Mutex<PidRegistry>>,
        catalog: Catalog,
        bridge: Arc<dyn BridgeRpc>,
    ) -> Self {
        Self {
            registry,
            catalog: Arc::new(catalog),
            bridge,
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
    // Same descriptors (including derived inputSchema) as the rmcp surface.
    Json(serde_json::json!({ "tools": tool_descriptors() }))
}

async fn call_tool(
    State(state): State<AppState>,
    Json(body): Json<CallBody>,
) -> Result<Json<Value>, axum::http::StatusCode> {
    match dispatch_tool(
        &state.registry,
        &state.catalog,
        state.bridge.as_ref(),
        &body.name,
        body.arguments,
    )
    .await
    {
        Ok(v) => Ok(Json(serde_json::json!({ "ok": true, "data": v }))),
        Err(ToolCallError::Failed {
            summary,
            diagnostics,
        }) => Ok(Json(serde_json::json!({
            "ok": false,
            "summary": summary,
            "diagnostics": diagnostics,
        }))),
        Err(ToolCallError::UnknownTool(_) | ToolCallError::InvalidArgs(_)) => {
            Err(axum::http::StatusCode::BAD_REQUEST)
        }
    }
}

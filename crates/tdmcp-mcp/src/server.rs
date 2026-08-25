//! Axum router for MCP-style JSON tool calls.
//!
//! A simple `/mcp/*` JSON surface for tests and as a fallback. The rmcp
//! Streamable HTTP layer nests on top of the same [`AppState`].

use std::sync::Arc;
use std::time::Duration;

use axum::extract::rejection::JsonRejection;
use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::Value;
use tdmcp_core::{DaemonId, PidRegistry, SlaveRegistry};
use tdmcp_diagnostics::Catalog;
use tokio::sync::Mutex;

use crate::bridge_rpc::BridgeRpc;
use crate::resources::ResourceProvider;
use crate::session_registry::McpSessionRegistry;
use crate::tools::{dispatch_tool, tool_descriptors, ToolCallError};

/// Master-side federation context shared with tool proxy dispatch.
#[derive(Clone)]
pub struct FederationCtx {
    /// This daemon's persistent id.
    pub local_daemon_id: DaemonId,
    /// Local hostname for aggregated fleet rows.
    pub local_hostname: String,
    /// Shared slave map (same `Arc` as the daemon federation runtime).
    pub slaves: Arc<Mutex<SlaveRegistry>>,
    /// Pooled HTTP client for master→slave tool proxy.
    pub http: reqwest::Client,
}

impl FederationCtx {
    /// Build a pooled client for proxy calls (`pool_max_idle_per_host=8`, idle 60s).
    #[must_use]
    pub fn build_http_client() -> reqwest::Client {
        reqwest::Client::builder()
            .pool_max_idle_per_host(8)
            .pool_idle_timeout(Duration::from_secs(60))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    }
}

/// Shared daemon state for MCP + admin handlers.
#[derive(Clone)]
pub struct AppState {
    /// Pid registry.
    pub registry: Arc<Mutex<PidRegistry>>,
    /// Diagnostic catalog.
    pub catalog: Arc<Catalog>,
    /// Live bridge transport (daemon-supplied).
    pub bridge: Arc<dyn BridgeRpc>,
    /// Live Streamable HTTP MCP session registry (GUI + idle exit).
    pub mcp_sessions: Arc<McpSessionRegistry>,
    /// MCP resource provider (skills rendered in MCP mode).
    pub resource_provider: Arc<ResourceProvider>,
    /// Federation (master) context when role is master/standalone with slaves.
    pub federation: Option<FederationCtx>,
}

impl AppState {
    /// Construct shared state from an owned registry.
    #[must_use]
    pub fn new(
        registry: PidRegistry,
        catalog: Catalog,
        bridge: Arc<dyn BridgeRpc>,
        resource_provider: Arc<ResourceProvider>,
    ) -> Self {
        Self {
            registry: Arc::new(Mutex::new(registry)),
            catalog: Arc::new(catalog),
            bridge,
            mcp_sessions: Arc::new(McpSessionRegistry::new()),
            resource_provider,
            federation: None,
        }
    }

    /// Construct shared state from an already-shared registry (daemon composition
    /// root shares one registry Arc across MCP + bridge sessions + admin).
    #[must_use]
    pub fn new_shared(
        registry: Arc<Mutex<PidRegistry>>,
        catalog: Catalog,
        bridge: Arc<dyn BridgeRpc>,
        resource_provider: Arc<ResourceProvider>,
    ) -> Self {
        Self {
            registry,
            catalog: Arc::new(catalog),
            bridge,
            mcp_sessions: Arc::new(McpSessionRegistry::new()),
            resource_provider,
            federation: None,
        }
    }

    /// Attach federation context (shared slave registry with the admin runtime).
    #[must_use]
    pub fn with_federation(mut self, federation: FederationCtx) -> Self {
        self.federation = Some(federation);
        self
    }

    /// Number of live Streamable HTTP MCP session leases.
    #[must_use]
    pub fn mcp_session_count(&self) -> usize {
        self.mcp_sessions.len()
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
    body: Result<Json<CallBody>, JsonRejection>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    // A bare `Json<CallBody>` param would let axum answer oversized/malformed
    // bodies itself — a raw text body, breaking the curated `{ok:false,
    // items[]}` envelope every other failure on this route carries (see
    // docs/LIMITS_AUDIT.md §4.5). Catching the rejection here keeps the
    // envelope uniform regardless of which layer rejected the request.
    let Json(body) = match body {
        Ok(json) => json,
        Err(rejection) => {
            return Err((
                rejection.status(),
                Json(serde_json::json!({
                    "ok": false,
                    "summary": rejection.body_text(),
                })),
            ));
        }
    };
    match dispatch_tool(
        &state.registry,
        &state.catalog,
        state.bridge.as_ref(),
        &body.name,
        body.arguments,
        None, // JSON fallback has no MCP session lease; pid exclusive still applies
        state.federation.as_ref(),
    )
    .await
    {
        Ok(v) => Ok(Json(serde_json::json!({ "ok": true, "data": v }))),
        Err(ToolCallError::Failed(fail)) => {
            let mut payload = fail.structured_content();
            if let Some(b64) = fail.image_base64.as_ref().filter(|s| !s.is_empty()) {
                if let Some(obj) = payload.as_object_mut() {
                    obj.insert("imageBase64".into(), Value::String(b64.clone()));
                    if let Some(mime) = fail.image_mime_type.as_ref().filter(|s| !s.is_empty()) {
                        obj.insert("mimeType".into(), Value::String(mime.clone()));
                    }
                }
            }
            Ok(Json(payload))
        }
        Err(ToolCallError::UnknownTool(name)) => {
            let hint = crate::args_diag::suggest_tool(&name)
                .map(|s| format!(" — did you mean `{s}`?"))
                .unwrap_or_default();
            Err((
                axum::http::StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "ok": false,
                    "summary":
                        format!("unknown tool: {name}{hint} — call /mcp/tools/list or describe_tools"),
                })),
            ))
        }
    }
}

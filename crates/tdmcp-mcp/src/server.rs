//! Axum router for MCP-style JSON tool calls.
//!
//! A simple `/mcp/*` JSON surface for tests and as a fallback. The rmcp
//! Streamable HTTP layer nests on top of the same [`AppState`].

use std::sync::Arc;

use crate::receipts::ReceiptFactory;

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
use crate::evidence::EvidenceStore;
use crate::evidence_capture::EvidenceBinding;
use crate::resources::ResourceProvider;
use crate::session_registry::McpSessionRegistry;
use crate::tools::{dispatch_tool_recorded, tool_descriptors, ToolCallError};

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
    /// Builds a per-request receipt port from the caller's identity claim.
    ///
    /// `None` on a daemon with no journal wired: tool calls still work, and
    /// they carry no receipt. That is honest — the row is absent rather than
    /// attributed to a principal nobody authenticated.
    pub receipts: Option<Arc<dyn ReceiptFactory>>,
    /// The store `output.retain: true` writes into.
    ///
    /// `None` on a daemon that could not open one — the same shape as a failed
    /// journal open: calls still work, retention is refused with a reason
    /// naming the cause, and nothing is silently dropped. The owner is
    /// resolved per request from the same identity claim as the receipts, so a
    /// store without an owner is never usable.
    pub evidence: Option<Arc<dyn EvidenceStore>>,
    /// Bytes of encoded image admitted into an MCP result's `content[]`.
    ///
    /// Defaults to [`tdmcp_config::DEFAULT_ADMISSION_MAX_IMAGE_BYTES`] in both
    /// constructors rather than being `Option`: a missing policy is not a
    /// licence to admit everything, and it is not a licence to admit nothing
    /// either. The daemon sets the operator's configured value.
    pub admission_max_image_bytes: usize,
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
            receipts: None,
            evidence: None,
            admission_max_image_bytes: tdmcp_config::DEFAULT_ADMISSION_MAX_IMAGE_BYTES,
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
            receipts: None,
            evidence: None,
            admission_max_image_bytes: tdmcp_config::DEFAULT_ADMISSION_MAX_IMAGE_BYTES,
        }
    }

    /// Attach the receipt factory that records dispatches per client.
    #[must_use]
    pub fn with_receipts(mut self, receipts: Arc<dyn ReceiptFactory>) -> Self {
        self.receipts = Some(receipts);
        self
    }

    /// Attach federation context (shared slave registry with the admin runtime).
    #[must_use]
    pub fn with_federation(mut self, federation: FederationCtx) -> Self {
        self.federation = Some(federation);
        self
    }

    /// Attach the evidence store that backs `output.retain: true`.
    #[must_use]
    pub fn with_evidence(mut self, evidence: Arc<dyn EvidenceStore>) -> Self {
        self.evidence = Some(evidence);
        self
    }

    /// Set the encoded-image admission bound from configuration.
    ///
    /// A zero bound is refused rather than honoured: with no budget every
    /// capture would be withheld, which reads as "captures are broken" rather
    /// than "the configuration is wrong". The derived default is kept instead,
    /// so the mismatch surfaces in the operator's log rather than silently in
    /// every response.
    #[must_use]
    pub fn with_admission(mut self, max_image_bytes: usize) -> Self {
        if max_image_bytes == 0 {
            tracing::warn!(
                requested = max_image_bytes,
                applied = self.admission_max_image_bytes,
                "admission_max_image_bytes of 0 would withhold every image; keeping the default"
            );
            return self;
        }
        self.admission_max_image_bytes = max_image_bytes;
        self
    }

    /// Number of live Streamable HTTP MCP session leases.
    #[must_use]
    pub fn mcp_session_count(&self) -> usize {
        self.mcp_sessions.len()
    }
}

/// Resolve one request's evidence seam.
///
/// Returns `None` unless BOTH halves are present: a store to write into and a
/// principal resolved from this request's own identity claim. A store with no
/// owner is not usable (there is nothing to scope the bytes to) and an owner
/// with no store has nowhere to put them, so neither partial case produces a
/// context. `None` therefore means "this call retains nothing", which the
/// producer reports as a refusal when the caller asked for retention — never as
/// a value that will quietly be dropped.
#[must_use]
pub fn evidence_binding(
    store: Option<&Arc<dyn EvidenceStore>>,
    receipts: Option<&Arc<dyn ReceiptFactory>>,
    claim: Option<&str>,
) -> Option<EvidenceBinding> {
    let store = store?;
    let owner = receipts?.owner_for_claim(claim)?;
    Some(EvidenceBinding::new(Arc::clone(store), owner))
}

/// The client's install-scoped identity claim, if it sent one.
pub fn claim_header(headers: &axum::http::HeaderMap) -> Option<&str> {
    headers
        .get(crate::client_id::CLIENT_ID_HEADER)
        .and_then(|value| value.to_str().ok())
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
    headers: axum::http::HeaderMap,
    body: Result<Json<CallBody>, JsonRejection>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    // A bare `Json<CallBody>` param would let axum answer oversized/malformed
    // bodies itself — a raw text body, breaking the curated `{ok:false,
    // items[]}` envelope every other failure on this route carries. Catching
    // the rejection here keeps the envelope uniform regardless of which layer
    // rejected the request.
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
    // The identity claim is the only thing the caller contributes; the
    // principal is resolved by the daemon's factory against the transport
    // credential, never chosen here.
    let claim = state
        .receipts
        .as_ref()
        .and_then(|factory| factory.for_claim(claim_header(&headers)));
    if state.receipts.is_some() && claim.is_none() {
        // A factory that could not produce a port means the receipt is
        // missing, not that the call was recorded. Say so, rather than
        // returning a success the client would read as fully accounted for.
        tracing::warn!(
            tool = %body.name,
            "the client's identity claim was refused; this call has no receipt"
        );
    }
    // Retention needs BOTH the store and a principal. The principal comes from
    // the same claim the receipt port was built from, so one authenticated
    // call has exactly one owner; `None` here (refused claim, or no store) is
    // the honest "this call retains nothing" answer rather than a default
    // owner.
    let evidence = evidence_binding(
        state.evidence.as_ref(),
        state.receipts.as_ref(),
        claim_header(&headers),
    );
    match dispatch_tool_recorded(
        &state.registry,
        &state.catalog,
        state.bridge.as_ref(),
        &body.name,
        body.arguments,
        None, // JSON fallback has no MCP session lease; pid exclusive still applies
        state.federation.as_ref(),
        claim.as_deref(),
        evidence,
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

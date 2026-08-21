//! MCP tool handlers for td-mcp-rs.
//!
//! Tool logic lives here; the daemon wires transport ([`rmcp`] Streamable HTTP
//! and/or axum admin routes).

#![warn(missing_docs)]

use include_dir::{include_dir, Dir};

pub mod bridge_rpc;
mod daemon_link;
mod editor_context;
mod fleet;
mod outcomes;
pub mod resources;
mod rmcp_handler;
mod schema;
mod server;
mod session_registry;
pub mod stdio_proxy;
pub mod template;
pub mod testing;
mod tools;

pub use bridge_rpc::{BridgeRpc, BridgeRpcError};
pub use daemon_link::ReconnectConfig;
pub use editor_context::EditorContextParams;
pub use fleet::{fleet_summary, FleetInclude, FleetParams, FleetProcess, FleetResponse};
pub use resources::{
    server_capabilities, ResourceProvider, SERVER_INSTRUCTIONS, STDIO_SERVER_INSTRUCTIONS,
};
pub use rmcp_handler::McpHandler;
pub use schema::input_schema_for;
pub use server::{build_mcp_router, AppState};
pub use session_registry::{BridgeCallSlot, McpSessionInfo, McpSessionRegistry};
pub use stdio_proxy::{
    run as run_stdio_proxy, run_with_rw as run_stdio_proxy_rw,
    run_with_rw_config as run_stdio_proxy_rw_config, StdioProxyError,
};
pub use template::{Catalog, CatalogEntry, RenderMode, TemplateEngine};
pub use tools::{
    dispatch_tool, tool_descriptors, ApiHelpFamily, ApiHelpParams, ApiHelpQuery, BridgeOutcome,
    CaptureMode, CaptureParams, DetailLevel, ExecutePythonParams, FormatMode, InspectInclude,
    InspectParams, MutateNodesParams, MutateStep, SessionGate, ToolCallError, ToolDescriptor,
    ToolFailPayload, ToolName, API_HELP_QUERIES_LIMIT, BRIDGE_TIMEOUT, CHILDREN_ROSTER_LIMIT,
    EDITOR_PANES_LIMIT, EDITOR_SELECTION_LIMIT, INSPECT_PATHS_LIMIT,
};

// ---------------------------------------------------------------------------
// Embedded assets (shared by daemon + MCP handlers)
// ---------------------------------------------------------------------------

/// Embedded `skills/MANIFEST.yaml` — catalog of all skill ids.
pub static MANIFEST_YAML: &str = include_str!("../../../skills/MANIFEST.yaml");

/// Embedded template tree (`skills/templates/`).
pub static TEMPLATES: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../skills/templates");

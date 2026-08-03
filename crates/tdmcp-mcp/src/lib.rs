//! MCP tool handlers for td-mcp-rs.
//!
//! Tool logic lives here; the daemon wires transport ([`rmcp`] Streamable HTTP
//! and/or axum admin routes).

#![warn(missing_docs)]

pub mod bridge_rpc;
mod daemon_link;
mod fleet;
mod outcomes;
mod rmcp_handler;
mod schema;
mod server;
mod session_registry;
pub mod stdio_proxy;
pub mod testing;
mod tools;

pub use bridge_rpc::{BridgeRpc, BridgeRpcError};
pub use daemon_link::ReconnectConfig;
pub use fleet::{fleet_summary, FleetInclude, FleetParams, FleetProcess, FleetResponse};
pub use rmcp_handler::McpHandler;
pub use schema::input_schema_for;
pub use server::{build_mcp_router, AppState};
pub use session_registry::{McpSessionInfo, McpSessionRegistry};
pub use stdio_proxy::{
    run as run_stdio_proxy, run_with_rw as run_stdio_proxy_rw,
    run_with_rw_config as run_stdio_proxy_rw_config, StdioProxyError,
};
pub use tools::{
    dispatch_tool, tool_descriptors, ApiHelpFamily, ApiHelpParams, ApiHelpQuery, BridgeOutcome,
    CaptureMode, CaptureParams, DetailLevel, ExecutePythonParams, FormatMode, InspectInclude,
    InspectParams, MutateNodesParams, MutateStep, ToolCallError, ToolDescriptor, ToolFailPayload,
    ToolName, API_HELP_QUERIES_LIMIT, BRIDGE_TIMEOUT, CHILDREN_ROSTER_LIMIT, INSPECT_PATHS_LIMIT,
};

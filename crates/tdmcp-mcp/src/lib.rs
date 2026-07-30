//! MCP tool handlers for td-mcp-rs.
//!
//! Tool logic lives here; the daemon wires transport ([`rmcp`] Streamable HTTP
//! and/or axum admin routes).

#![warn(missing_docs)]

pub mod bridge_rpc;
mod fleet;
mod outcomes;
mod rmcp_handler;
mod schema;
mod server;
pub mod stdio_proxy;
pub mod testing;
mod tools;

pub use bridge_rpc::{BridgeRpc, BridgeRpcError};
pub use fleet::{fleet_summary, FleetInclude, FleetParams, FleetProcess, FleetResponse};
pub use rmcp_handler::McpHandler;
pub use schema::input_schema_for;
pub use server::{build_mcp_router, AppState};
pub use stdio_proxy::{run as run_stdio_proxy, run_with_rw as run_stdio_proxy_rw, StdioProxyError};
pub use tools::{
    dispatch_tool, tool_descriptors, BridgeOutcome, CaptureMode, CaptureParams, DetailLevel,
    ExecutePythonParams, InspectInclude, InspectParams, ToolCallError, ToolDescriptor,
    BRIDGE_TIMEOUT,
};

//! MCP tool handlers for td-mcp-rs.
//!
//! Tool logic lives here; the daemon wires transport ([`rmcp`] Streamable HTTP
//! and/or axum admin routes).

#![warn(missing_docs)]

pub mod bridge_rpc;
mod fleet;
mod outcomes;
mod server;
pub mod testing;
mod tools;

pub use bridge_rpc::{BridgeRpc, BridgeRpcError};
pub use fleet::{fleet_summary, FleetInclude, FleetParams, FleetProcess, FleetResponse};
pub use server::{build_mcp_router, AppState};
pub use tools::{
    dispatch_tool, tool_descriptors, BridgeOutcome, CaptureParams, ExecutePythonParams,
    InspectParams, ToolCallError, ToolDescriptor, BRIDGE_TIMEOUT,
};

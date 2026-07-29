//! MCP tool handlers for td-mcp-rs.
//!
//! Tool logic lives here; the daemon wires transport ([`rmcp`] Streamable HTTP
//! and/or axum admin routes).

#![warn(missing_docs)]

mod fleet;
mod server;
mod tools;

pub use fleet::{fleet_summary, FleetInclude, FleetParams, FleetProcess, FleetResponse};
pub use server::{build_mcp_router, AppState};
pub use tools::{execute_python_stub, tool_descriptors, ToolCallError, ToolDescriptor};

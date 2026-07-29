//! [`rmcp::ServerHandler`] adapter — bridges the real MCP Streamable HTTP
//! transport to [`dispatch_tool`], the same dispatch path used by the JSON
//! fallback router in [`crate::server`].
//!
//! One [`McpHandler`] is constructed per rmcp session (legacy mode) or per
//! request (stateless mode) via the `service_factory` closure passed to
//! `StreamableHttpService::new`. Construction is cheap: [`AppState`] wraps
//! `Arc`s, so cloning it does not duplicate daemon state.

use std::sync::Arc;

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, Implementation, JsonObject,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler};
use serde_json::Value;

use crate::tools::{dispatch_tool, tool_descriptors, ToolCallError};
use crate::AppState;

/// `rmcp` server handler over the shared daemon [`AppState`].
#[derive(Clone)]
pub struct McpHandler {
    state: AppState,
}

impl McpHandler {
    /// Wrap shared daemon state for the rmcp transport.
    #[must_use]
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

impl ServerHandler for McpHandler {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "tdmcp-daemon",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(
                "td-mcp-rs control plane. Call `fleet` to discover connected TouchDesigner \
                 processes by pid, then `execute_python` / `inspect` / `capture` against a pid.",
            )
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let tools = tool_descriptors()
            .into_iter()
            .map(tool_from_descriptor)
            .collect();
        Ok(ListToolsResult::with_all_items(tools))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        tool_descriptors()
            .into_iter()
            .find(|d| d.name == name)
            .map(tool_from_descriptor)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let name = request.name.into_owned();
        let args = request.arguments.map(Value::Object).unwrap_or(Value::Null);

        match dispatch_tool(
            &self.state.registry,
            &self.state.catalog,
            self.state.bridge.as_ref(),
            &name,
            args,
        )
        .await
        {
            Ok(value) => Ok(CallToolResult::structured(value).into()),
            Err(ToolCallError::UnknownTool(name)) => Err(ErrorData::invalid_params(
                format!("unknown tool: {name}"),
                None,
            )),
            Err(ToolCallError::InvalidArgs(msg)) => Err(ErrorData::invalid_params(msg, None)),
            Err(ToolCallError::Failed {
                summary,
                diagnostics,
            }) => {
                let payload = serde_json::to_value(&diagnostics)
                    .unwrap_or_else(|_| serde_json::json!({ "summary": summary }));
                Ok(CallToolResult::structured_error(payload).into())
            }
        }
    }
}

fn tool_from_descriptor(d: crate::tools::ToolDescriptor) -> Tool {
    Tool::new(
        d.name.clone(),
        d.description,
        Arc::new(input_schema_for(&d.name)),
    )
}

/// Minimal-but-accurate JSON Schema per tool (matches the `serde` param
/// structs in [`crate::tools`] / [`crate::fleet`]). Provisional: hand-authored
/// rather than derived, since v1 tool params are still settling.
fn input_schema_for(tool_name: &str) -> JsonObject {
    let schema = match tool_name {
        "fleet" => serde_json::json!({
            "type": "object",
            "properties": {
                "pids": { "type": "array", "items": { "type": "integer" } },
                "include": {
                    "type": "array",
                    "items": {
                        "type": "string",
                        "enum": ["process", "bridge", "tasks", "cancelled", "popups"]
                    }
                }
            }
        }),
        "execute_python" => serde_json::json!({
            "type": "object",
            "required": ["pid", "script"],
            "properties": {
                "pid": { "type": "integer" },
                "script": { "type": "string" },
                "exclusive": { "type": "boolean" },
                "contextPath": { "type": "string" }
            }
        }),
        "capture" => serde_json::json!({
            "type": "object",
            "required": ["pid", "path"],
            "properties": {
                "pid": { "type": "integer" },
                "path": { "type": "string" },
                "mode": { "type": "string", "enum": ["top", "preview", "auto"] },
                "contextPath": { "type": "string" }
            }
        }),
        "inspect" => serde_json::json!({
            "type": "object",
            "required": ["pid", "path"],
            "properties": {
                "pid": { "type": "integer" },
                "path": { "type": "string" },
                "contextPath": { "type": "string" },
                "include": { "type": "array", "items": { "type": "string" } },
                "detailLevel": { "type": "string", "enum": ["summary", "detailed"] }
            }
        }),
        _ => serde_json::json!({ "type": "object" }),
    };
    schema.as_object().cloned().unwrap_or_default()
}

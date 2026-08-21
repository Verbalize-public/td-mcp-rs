//! [`rmcp::ServerHandler`] adapter — bridges the real MCP Streamable HTTP
//! transport to [`dispatch_tool`], the same dispatch path used by the JSON
//! fallback router in [`crate::server`].
//!
//! One [`McpHandler`] is constructed per rmcp session (legacy mode) or per
//! request (stateless mode) via the `service_factory` closure passed to
//! `StreamableHttpService::new`. Construction is cheap: [`AppState`] wraps
//! `Arc`s, so cloning it does not duplicate daemon state.
//!
//! Each handler acquires one [`McpSessionLease`] on [`AppState::mcp_sessions`].
//! Clones share the lease `Arc` (no double-count); the row is removed when
//! the last clone of that session's handler is dropped.

use std::sync::Arc;

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    InitializeRequestParams, InitializeResult, ListResourcesResult, ListToolsResult,
    PaginatedRequestParams, ProtocolVersion, ReadResourceRequestParams, ReadResourceResponse,
    ServerInfo, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler};
use serde_json::Value;

use crate::resources::{self, SERVER_INSTRUCTIONS};
use crate::schema::input_schema_for;
use crate::session_registry::McpSessionRegistry;
use crate::tools::ToolName;
use crate::tools::{dispatch_tool, tool_descriptors, SessionGate, ToolCallError};
use crate::AppState;

/// Removes the session row when the last `Arc` clone is dropped.
struct McpSessionLease {
    registry: Arc<McpSessionRegistry>,
    id: String,
}

impl McpSessionLease {
    fn acquire(registry: Arc<McpSessionRegistry>) -> Arc<Self> {
        let id = registry.acquire();
        Arc::new(Self { registry, id })
    }
}

impl Drop for McpSessionLease {
    fn drop(&mut self) {
        self.registry.release(&self.id);
    }
}

/// `rmcp` server handler over the shared daemon [`AppState`].
#[derive(Clone)]
pub struct McpHandler {
    state: AppState,
    /// Shared across clones of this session's handler; not incremented on clone.
    lease: Arc<McpSessionLease>,
}

impl McpHandler {
    /// Wrap shared daemon state for the rmcp transport and acquire a session lease.
    #[must_use]
    pub fn new(state: AppState) -> Self {
        let lease = McpSessionLease::acquire(state.mcp_sessions.clone());
        Self { state, lease }
    }

    /// Registry session id for this handler lease.
    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.lease.id
    }
}

impl ServerHandler for McpHandler {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(resources::server_capabilities()).with_server_info(
            Implementation::new("tdmcp-daemon", env!("CARGO_PKG_VERSION")),
        ).with_instructions(SERVER_INSTRUCTIONS)
    }

    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, ErrorData> {
        context.peer.set_peer_info(request.clone());
        self.state.mcp_sessions.set_client_info(
            &self.lease.id,
            request.client_info.name.clone(),
            request.client_info.version.clone(),
        );
        let mut info = self.get_info();
        info.protocol_version =
            negotiate_protocol_version(&request.protocol_version, info.protocol_version);
        Ok(info)
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

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        Ok(resources::list_resources())
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        match resources::read_resource(&request.uri) {
            Ok(result) => Ok(ReadResourceResponse::Complete(result)),
            Err(msg) => Err(ErrorData::resource_not_found(msg, None)),
        }
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
            Some(SessionGate {
                session_id: self.session_id(),
                sessions: self.state.mcp_sessions.as_ref(),
            }),
        )
        .await
        {
            Ok(value) => Ok(call_tool_result_from_value(&name, value).into()),
            Err(ToolCallError::UnknownTool(name)) => Err(ErrorData::invalid_params(
                format!("unknown tool: {name}"),
                None,
            )),
            Err(ToolCallError::InvalidArgs(msg)) => Err(ErrorData::invalid_params(msg, None)),
            Err(ToolCallError::Failed(fail)) => {
                let payload = fail.structured_content();
                Ok(call_tool_error_result(payload, fail.image_base64, fail.image_mime_type).into())
            }
        }
    }
}

/// Echoes the client-requested version if known; otherwise returns `server_fallback`.
/// Mirrors rmcp's private `negotiate_protocol_version`.
fn negotiate_protocol_version(
    client_requested: &ProtocolVersion,
    server_fallback: ProtocolVersion,
) -> ProtocolVersion {
    if ProtocolVersion::KNOWN_VERSIONS.contains(client_requested) {
        client_requested.clone()
    } else {
        server_fallback
    }
}

fn tool_from_descriptor(d: crate::tools::ToolDescriptor) -> Tool {
    // Prefer the schema already attached to the descriptor (same SSOT as JSON fallback).
    let schema = if d.input_schema.is_empty() {
        let tool = ToolName::from_wire(&d.name).unwrap_or(ToolName::DescribeTools);
        Arc::new(input_schema_for(tool))
    } else {
        Arc::new(d.input_schema)
    };
    Tool::new(d.name.clone(), d.description, schema)
}

/// Build an MCP tool result, promoting top-level `imageBase64` to an image block.
fn call_tool_result_from_value(tool: &str, value: Value) -> CallToolResult {
    if tool == "capture" {
        return match try_perception_image_result(value) {
            Ok(result) => result,
            Err(value) => CallToolResult::structured(value),
        };
    }
    CallToolResult::structured(value)
}

fn call_tool_error_result(
    payload: Value,
    image_base64: Option<String>,
    image_mime_type: Option<String>,
) -> CallToolResult {
    if let Some(b64) = image_base64.filter(|s| !s.is_empty()) {
        let mime = image_mime_type
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "image/png".into());
        let mut result = CallToolResult::error(vec![
            ContentBlock::image(b64, mime),
            ContentBlock::text(payload.to_string()),
        ]);
        result.structured_content = Some(payload);
        return result;
    }
    CallToolResult::structured_error(payload)
}

/// Promote top-level PNG (or other) payload to an image content block.
fn try_perception_image_result(mut value: Value) -> Result<CallToolResult, Value> {
    let Some(obj) = value.as_object_mut() else {
        return Err(value);
    };
    let b64 = match obj.remove("imageBase64") {
        Some(Value::String(s)) if !s.is_empty() => s,
        other => {
            if let Some(v) = other {
                obj.insert("imageBase64".into(), v);
            }
            return Err(value);
        }
    };
    let mime = obj
        .get("mimeType")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("image/png")
        .to_owned();
    let path = obj
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or("capture")
        .to_owned();
    let bytes = obj.get("bytes").and_then(|v| v.as_u64()).unwrap_or(0);
    let note = format!("Captured TOP image from {path} ({bytes} bytes, {mime}).");
    let mut result = CallToolResult::success(vec![
        ContentBlock::image(b64, mime),
        ContentBlock::text(note),
    ]);
    result.structured_content = Some(value);
    Ok(result)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;
    use serde_json::json;
    use tdmcp_core::PidRegistry;
    use tdmcp_diagnostics::Catalog;

    use crate::testing::FakeBridgeRpc;

    #[test]
    fn session_lease_counts_acquire_and_shared_drop() {
        let state = AppState::new(
            PidRegistry::new(),
            Catalog::fallback(),
            Arc::new(FakeBridgeRpc::responding(json!({}))),
        );
        assert_eq!(state.mcp_session_count(), 0);
        let a = McpHandler::new(state.clone());
        assert_eq!(state.mcp_session_count(), 1);
        let b = a.clone();
        assert_eq!(state.mcp_session_count(), 1);
        drop(a);
        assert_eq!(state.mcp_session_count(), 1);
        drop(b);
        assert_eq!(state.mcp_session_count(), 0);
    }

    #[test]
    fn capture_promotes_png_to_image_content() {
        let value = json!({
            "ok": true,
            "path": "/project1/out1",
            "bytes": 12,
            "mimeType": "image/png",
            "imageBase64": "AAAA",
        });
        let result = call_tool_result_from_value("capture", value);
        assert_eq!(result.is_error, Some(false));
        assert_eq!(result.content.len(), 2);
        let image = result.content[0].as_image().expect("image block");
        assert_eq!(image.mime_type, "image/png");
        let structured = result.structured_content.expect("structured");
        assert!(structured.pointer("/imageBase64").is_none());
        assert_eq!(
            structured.pointer("/path").and_then(|v| v.as_str()),
            Some("/project1/out1")
        );
    }

    #[test]
    fn non_capture_stays_structured_only() {
        let value = json!({"ok": true, "node": {"path": "/project1"}});
        let result = call_tool_result_from_value("inspect", value);
        assert!(result.content.iter().all(|c| c.as_image().is_none()));
    }
}

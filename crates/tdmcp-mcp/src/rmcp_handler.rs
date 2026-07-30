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
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler};
use serde_json::{json, Value};

use crate::schema::input_schema_for;
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
            Ok(value) => Ok(call_tool_result_from_value(&name, value).into()),
            Err(ToolCallError::UnknownTool(name)) => Err(ErrorData::invalid_params(
                format!("unknown tool: {name}"),
                None,
            )),
            Err(ToolCallError::InvalidArgs(msg)) => Err(ErrorData::invalid_params(msg, None)),
            Err(ToolCallError::Failed {
                summary,
                diagnostics,
                image_jpeg_base64,
            }) => {
                let payload = serde_json::to_value(&diagnostics)
                    .unwrap_or_else(|_| json!({ "summary": summary }));
                Ok(call_tool_error_result(payload, image_jpeg_base64).into())
            }
        }
    }
}

fn tool_from_descriptor(d: crate::tools::ToolDescriptor) -> Tool {
    // Prefer the schema already attached to the descriptor (same SSOT as JSON fallback).
    let schema = if d.input_schema.is_empty() {
        Arc::new(input_schema_for(&d.name))
    } else {
        Arc::new(d.input_schema)
    };
    Tool::new(d.name.clone(), d.description, schema)
}

/// Build an MCP tool result, promoting `capture.jpegBase64` to an image block.
fn call_tool_result_from_value(tool: &str, value: Value) -> CallToolResult {
    if tool == "capture" {
        return match try_perception_image_result(value) {
            Ok(result) => result,
            Err(value) => CallToolResult::structured(value),
        };
    }
    CallToolResult::structured(value)
}

fn call_tool_error_result(payload: Value, image_jpeg_base64: Option<String>) -> CallToolResult {
    if let Some(b64) = image_jpeg_base64.filter(|s| !s.is_empty()) {
        let mut result = CallToolResult::error(vec![
            ContentBlock::image(b64, "image/jpeg"),
            ContentBlock::text(payload.to_string()),
        ]);
        result.structured_content = Some(payload);
        return result;
    }
    CallToolResult::structured_error(payload)
}

/// Promote JPEG payload to an image content block, or return the value unchanged.
fn try_perception_image_result(mut value: Value) -> Result<CallToolResult, Value> {
    let Some(capture) = value.get_mut("capture").and_then(|c| c.as_object_mut()) else {
        return Err(value);
    };
    let b64 = match capture.remove("jpegBase64") {
        Some(Value::String(s)) if !s.is_empty() => s,
        other => {
            if let Some(v) = other {
                capture.insert("jpegBase64".into(), v);
            }
            return Err(value);
        }
    };
    let path = capture
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or("capture")
        .to_owned();
    let bytes = capture.get("bytes").and_then(|v| v.as_u64()).unwrap_or(0);
    let note = format!("Captured TOP JPEG from {path} ({bytes} bytes).");
    let mut result = CallToolResult::success(vec![
        ContentBlock::image(b64, "image/jpeg"),
        ContentBlock::text(note),
    ]);
    result.structured_content = Some(value);
    Ok(result)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;

    #[test]
    fn capture_promotes_jpeg_to_image_content() {
        let value = json!({
            "ok": true,
            "capture": {
                "ok": true,
                "path": "/project1/out1",
                "bytes": 12,
                "mimeType": "image/jpeg",
                "jpegBase64": "AAAA",
            }
        });
        let result = call_tool_result_from_value("capture", value);
        assert_eq!(result.is_error, Some(false));
        assert_eq!(result.content.len(), 2);
        assert!(result.content[0].as_image().is_some());
        let structured = result.structured_content.expect("structured");
        assert!(structured.pointer("/capture/jpegBase64").is_none());
        assert_eq!(
            structured.pointer("/capture/path").and_then(|v| v.as_str()),
            Some("/project1/out1")
        );
    }

    #[test]
    fn non_capture_stays_structured_only() {
        let value = json!({"ok": true, "inspect": {"node": {"path": "/project1"}}});
        let result = call_tool_result_from_value("inspect", value);
        assert!(result.content.iter().all(|c| c.as_image().is_none()));
    }
}

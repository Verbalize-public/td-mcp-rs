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
    CacheScope, CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock,
    Implementation, InitializeRequestParams, InitializeResult, ListResourceTemplatesResult,
    ListResourcesResult, ListToolsResult, PaginatedRequestParams, ProtocolVersion,
    ReadResourceRequestParams, ReadResourceResponse, ServerInfo, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler};
use serde_json::Value;

use crate::resources::{server_capabilities, SERVER_INSTRUCTIONS};
use crate::schema::input_schema_for;
use crate::session_registry::McpSessionRegistry;
use crate::tools::ToolName;
use crate::tools::{dispatch_tool_recorded, tool_descriptors, SessionGate, ToolCallError};
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
        ServerInfo::new(server_capabilities())
            .with_server_info(Implementation::new(
                "tdmcp-daemon",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(SERVER_INSTRUCTIONS)
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
        Ok(ListToolsResult::with_all_items(tools)
            .with_ttl_ms(0)
            .with_cache_scope(CacheScope::Private))
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
        Ok(self.state.resource_provider.list_resources())
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        Ok(self.state.resource_provider.list_resource_templates())
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        match self.state.resource_provider.read_resource(&request.uri) {
            Ok(result) => Ok(ReadResourceResponse::Complete(result)),
            Err(msg) => Err(ErrorData::resource_not_found(msg, None)),
        }
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let name = request.name.into_owned();
        let args = request.arguments.map(Value::Object).unwrap_or(Value::Null);

        // The Streamable HTTP transport injects the remaining `http` request
        // parts into the request context, so the same identity claim the JSON
        // route reads is available here. `axum::http` is the same `http` crate,
        // which is why this needs no new dependency.
        let headers = context
            .extensions
            .get::<axum::http::request::Parts>()
            .map(|parts| &parts.headers);
        let claim =
            self.state.receipts.as_ref().and_then(|factory| {
                factory.for_claim(headers.and_then(crate::server::claim_header))
            });
        if self.state.receipts.is_some() && claim.is_none() {
            tracing::warn!(
                tool = %name,
                "the client's identity claim was refused; this call has no receipt"
            );
        }

        // Retention needs BOTH the store and a principal, and the principal
        // comes from the same claim the receipt port was built from: one
        // authenticated call has exactly one owner.
        let evidence = crate::server::evidence_binding(
            self.state.evidence.as_ref(),
            self.state.receipts.as_ref(),
            headers.and_then(crate::server::claim_header),
        );
        match dispatch_tool_recorded(
            &self.state.registry,
            &self.state.catalog,
            self.state.bridge.as_ref(),
            &name,
            args,
            Some(SessionGate {
                session_id: self.session_id(),
                sessions: self.state.mcp_sessions.as_ref(),
            }),
            self.state.federation.as_ref(),
            claim.as_deref(),
            evidence,
        )
        .await
        {
            Ok(value) => Ok(call_tool_result_from_value(&name, value).into()),
            Err(ToolCallError::UnknownTool(name)) => {
                let hint = crate::args_diag::suggest_tool(&name)
                    .map(|s| format!(" — did you mean `{s}`?"))
                    .unwrap_or_default();
                Err(ErrorData::invalid_params(
                    format!("unknown tool: {name}{hint} — call describe_tools for available tools"),
                    None,
                ))
            }
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

/// Above this serialized size, the redundant `content[].text` copy of
/// `structuredContent` that `CallToolResult::structured` / `structured_error`
/// always attach (`content.to_string()` duplicated verbatim into a text block)
/// is replaced by a pointer instead. Free for small results — some MCP clients
/// only render `content[]`, not `structuredContent` — but for large ones the
/// copy doubles wire size and eats directly into rmcp's fixed 16 MiB SSE event
/// cap.
const DUPLICATE_TEXT_SKIP_BYTES: usize = 256 * 1024;

/// Make sure a client that renders only `content[]` is never handed less than
/// the data, and never handed it twice.
///
/// Above [`DUPLICATE_TEXT_SKIP_BYTES`] a verbatim text copy of
/// `structured_content` is a liability: it doubles wire size and eats directly
/// into rmcp's fixed 16 MiB SSE event cap. So the copy is dropped — but it is
/// REPLACED by a short pointer, never by an empty content array, because an
/// empty array is a silent substitution. A client that only renders `content[]`
/// is the very reason the copy exists; leaving it with nothing lets it read
/// "this call returned nothing".
///
/// The tricky part is that "has a text block" is the wrong test. The image
/// paths attach a one-line note beside the attachment, which is a text block
/// carrying no data at all. What matters is whether any text block is a full
/// rendering of `structured_content`, so that is what is measured.
fn text_copy_or_pointer(mut result: CallToolResult) -> CallToolResult {
    let Some(structured) = result.structured_content.as_ref() else {
        return result;
    };
    let rendered = structured.to_string();
    if rendered.len() <= DUPLICATE_TEXT_SKIP_BYTES {
        return result;
    }
    // A rendering of the same value cannot be shorter than the value, so this
    // finds the verbatim copy and not a one-line caption beside an image.
    let copy = result.content.iter().position(|block| {
        block
            .as_text()
            .is_some_and(|t| t.text.len() >= rendered.len())
    });
    let pointer = ContentBlock::text(elision_notice(rendered.len()));
    if let Some(index) = copy {
        // The copy is the liability: replace it, keep everything else.
        result.content[index] = pointer;
    } else {
        // No copy was made — the image paths deliberately never make one — so
        // append the pointer rather than leaving a text-only client with an
        // attachment and a caption, and no indication the rest exists.
        result.content.push(pointer);
    }
    result
}

/// What a client sees in place of a text copy that was too large to repeat.
fn elision_notice(omitted_bytes: usize) -> String {
    format!(
        "This answer is {omitted_bytes} bytes of text and is not repeated here: it is \
         carried in this result's structuredContent field, unmodified. The text copy is \
         omitted above {DUPLICATE_TEXT_SKIP_BYTES} bytes because the transport cannot \
         carry the same value twice. If your client does not read structuredContent, \
         request a narrower view of the same query, or retain this observation (output \
         .retain) and read it back in pages."
    )
}

/// Build an MCP tool result, promoting top-level `imageBase64` to an image block.
fn call_tool_result_from_value(tool: &str, mut value: Value) -> CallToolResult {
    if tool == "capture" {
        if let Some(samples) = value.get_mut("samples").and_then(Value::as_array_mut) {
            let mut content = Vec::new();
            for sample in samples {
                let offset = sample.get("offset").cloned().unwrap_or(Value::Null);
                if let Some(capture) = sample.get_mut("capture").and_then(Value::as_object_mut) {
                    if let Some(Value::String(b64)) = capture.remove("imageBase64") {
                        let mime = capture
                            .get("mimeType")
                            .and_then(Value::as_str)
                            .unwrap_or("image/png");
                        content.push(ContentBlock::text(format!(
                            "Capture sample offset {offset}"
                        )));
                        content.push(ContentBlock::image(b64, mime.to_owned()));
                    }
                }
            }
            if !content.is_empty() {
                let mut result = CallToolResult::success(content);
                result.structured_content = Some(value);
                return text_copy_or_pointer(result);
            }
        }
        return match try_perception_image_result(value) {
            Ok(result) => text_copy_or_pointer(result),
            Err(value) => text_copy_or_pointer(CallToolResult::structured(value)),
        };
    }
    text_copy_or_pointer(CallToolResult::structured(value))
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
        return text_copy_or_pointer(result);
    }
    text_copy_or_pointer(CallToolResult::structured_error(payload))
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

    use crate::testing::{test_resource_provider, FakeBridgeRpc};

    #[test]
    fn capture_promotes_ordered_job_samples_and_keeps_identity() {
        let value = json!({"ok":true,"jobId":"job", "samples":[
            {"offset":0,"timing":{"frame":10},"capture":{"imageBase64":"eA==","mimeType":"image/png"}},
            {"offset":3,"timing":{"frame":13},"capture":{"imageBase64":"eQ==","mimeType":"image/png"}}
        ]});
        let result = call_tool_result_from_value("capture", value);
        assert_eq!(result.content.len(), 4);
        assert!(result.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("offset 0"));
        assert!(result.content[2]
            .as_text()
            .unwrap()
            .text
            .contains("offset 3"));
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["samples"][1]["timing"]["frame"], 13);
        assert!(structured["samples"][0]["capture"]
            .get("imageBase64")
            .is_none());
    }

    #[test]
    fn session_lease_counts_acquire_and_shared_drop() {
        let state = AppState::new(
            PidRegistry::new(),
            Catalog::fallback(),
            Arc::new(FakeBridgeRpc::responding(json!({}))),
            test_resource_provider().expect("resource provider"),
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

    #[test]
    fn small_result_keeps_the_duplicate_text_block() {
        let value = json!({"ok": true, "result": 1});
        let result = call_tool_result_from_value("execute_python", value.clone());
        assert_eq!(
            result.content.len(),
            1,
            "small results keep the compat text block"
        );
        assert_eq!(
            result.content[0].as_text().expect("text").text,
            value.to_string()
        );
        assert_eq!(result.structured_content, Some(value));
    }

    #[test]
    fn oversized_result_replaces_the_duplicate_text_with_a_pointer() {
        // `Q` cannot appear in the notice text, so "does the pointer contain
        // the payload" is answerable. A filler like `x` would match the `x` in
        // "text" and make this assertion pass for the wrong reason.
        let big = "Q".repeat(DUPLICATE_TEXT_SKIP_BYTES + 1);
        let value = json!({"ok": true, "result": big});
        let result = call_tool_result_from_value("execute_python", value.clone());
        assert_eq!(
            result.content.len(),
            1,
            "the client must be told where the payload went, not left with nothing"
        );
        let text = result.content[0].as_text().expect("text").text.clone();
        assert!(
            text.len() < DUPLICATE_TEXT_SKIP_BYTES,
            "the pointer must not restate the payload"
        );
        assert!(
            text.contains("structuredContent"),
            "the pointer names no field"
        );
        assert!(!text.contains('Q'), "the pointer repeats the omitted bytes");
        assert_eq!(result.structured_content, Some(value));
    }

    #[test]
    fn oversized_error_result_also_replaces_the_duplicate_text_with_a_pointer() {
        let big = "Q".repeat(DUPLICATE_TEXT_SKIP_BYTES + 1);
        let payload = json!({"ok": false, "summary": big});
        let result = call_tool_error_result(payload.clone(), None, None);
        let text = result.content[0].as_text().expect("text").text.clone();
        assert!(
            text.contains("structuredContent"),
            "a refusal left a text-only client with no pointer: {text}"
        );
        assert_eq!(result.structured_content, Some(payload));
    }

    /// A large answer that also carries an image is the case the previous
    /// "exactly one text block" rule could not see: the image path attaches an
    /// attachment plus a one-line caption, so no block looks like a duplicate
    /// and the text copy is never made. A text-only client then receives an
    /// image and a caption, and nothing at all saying the rest of the answer
    /// exists.
    #[test]
    fn a_large_answer_with_an_image_still_points_a_text_only_client_at_the_data() {
        let big = "Q".repeat(DUPLICATE_TEXT_SKIP_BYTES + 1);
        let result = call_tool_result_from_value(
            "capture",
            json!({
                "ok": true, "blob": big,
                "path": "/project1/out1", "bytes": 4,
                "mimeType": "image/png", "imageBase64": "AAAA",
            }),
        );
        // The attachment is untouched and still exact.
        let images: Vec<_> = result
            .content
            .iter()
            .filter(|b| b.as_image().is_some())
            .collect();
        assert_eq!(images.len(), 1, "the image must survive: {result:?}");
        assert_eq!(images[0].as_image().expect("image").data, "AAAA");
        // And the pointer is present without the payload.
        let texts: Vec<&str> = result
            .content
            .iter()
            .filter_map(|b| b.as_text().map(|t| t.text.as_str()))
            .collect();
        assert!(
            texts.iter().any(|t| t.contains("structuredContent")),
            "a text-only client got no pointer beside the image: {texts:?}"
        );
        assert!(
            !texts.iter().any(|t| t.contains('Q')),
            "the pointer restates the payload"
        );
        assert_eq!(
            result
                .structured_content
                .as_ref()
                .and_then(|v| v["blob"].as_str())
                .map(str::len),
            Some(DUPLICATE_TEXT_SKIP_BYTES + 1),
            "the data itself must be untouched"
        );
    }

    /// A SMALL answer with an image is the case where adding a pointer would be
    /// wrong: there is nothing to point at, and a note about elision that is
    /// not happening is noise on every small capture.
    #[test]
    fn a_small_answer_with_an_image_gets_no_pointless_pointer() {
        let result = call_tool_result_from_value(
            "capture",
            json!({
                "ok": true, "path": "/project1/out1", "bytes": 4,
                "mimeType": "image/png", "imageBase64": "AAAA",
            }),
        );
        let texts: Vec<&str> = result
            .content
            .iter()
            .filter_map(|b| b.as_text().map(|t| t.text.as_str()))
            .collect();
        assert!(
            !texts.iter().any(|t| t.contains("structuredContent")),
            "a small capture must not claim anything was elided: {texts:?}"
        );
    }
}

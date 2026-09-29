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

use crate::admission::{admission_record, admit_image, Admission, AdmissionPolicy};
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
            Ok(value) => Ok(call_tool_result_from_value(
                &name,
                value,
                AdmissionPolicy {
                    max_image_bytes: self.state.admission_max_image_bytes,
                },
            )
            .into()),
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
                Ok(call_tool_error_result(
                    payload,
                    fail.image_base64,
                    fail.image_mime_type,
                    AdmissionPolicy {
                        max_image_bytes: self.state.admission_max_image_bytes,
                    },
                )
                .into())
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
fn call_tool_result_from_value(
    tool: &str,
    mut value: Value,
    policy: AdmissionPolicy,
) -> CallToolResult {
    if tool == "capture" {
        if let Some(samples) = value.get_mut("samples").and_then(Value::as_array_mut) {
            let mut content = Vec::new();
            let mut tally = ImageTally::default();
            for sample in samples {
                let offset = sample.get("offset").cloned().unwrap_or(Value::Null);
                if let Some(capture) = sample.get_mut("capture").and_then(Value::as_object_mut) {
                    if let Some(Value::String(b64)) = capture.get("imageBase64").cloned() {
                        let mime = capture
                            .get("mimeType")
                            .and_then(Value::as_str)
                            .unwrap_or("image/png")
                            .to_owned();
                        let label = format!("Capture sample offset {offset}");
                        // `remove` only on admission: a withheld image must
                        // stay in structuredContent, because that is the only
                        // place its bytes remain once the attachment list is
                        // skipped.
                        let bytes = encoded_len(capture, &b64);
                        let admitted = admit_image(policy, bytes) == Admission::Admitted;
                        if admitted {
                            capture.remove("imageBase64");
                            content.push(ContentBlock::text(label));
                            content.push(ContentBlock::image(b64, mime));
                        } else {
                            content.push(ContentBlock::text(withheld_label(&label, bytes)));
                        }
                        tally.record(bytes, admitted);
                    }
                }
            }
            if !content.is_empty() {
                if let Some(obj) = value.as_object_mut() {
                    obj.insert(
                        "admission".to_owned(),
                        admission_record(
                            policy,
                            tally.generated,
                            tally.admitted,
                            tally.generated_bytes,
                            tally.withheld_bytes,
                        ),
                    );
                }
                let mut result = CallToolResult::success(content);
                result.structured_content = Some(value);
                return text_copy_or_pointer(result);
            }
        }
        return match try_perception_image_result(value, policy) {
            Ok(result) => text_copy_or_pointer(result),
            // No image in this capture. The admission record is still added,
            // reporting `not_applicable`: a caller that finds no `admission`
            // key cannot tell "this capture had no image" from "this server
            // does not report admission", and those are very different facts.
            Err(mut value) => {
                if let Some(obj) = value.as_object_mut() {
                    obj.insert("admission".to_owned(), admission_record(policy, 0, 0, 0, 0));
                }
                text_copy_or_pointer(CallToolResult::structured(value))
            }
        };
    }
    text_copy_or_pointer(CallToolResult::structured(value))
}

/// Running totals for the images in one result, so the `admission` record can
/// report what the server generated and what it admitted as separate numbers.
#[derive(Debug, Default)]
struct ImageTally {
    generated: usize,
    admitted: usize,
    generated_bytes: u64,
    withheld_bytes: u64,
}

impl ImageTally {
    fn record(&mut self, bytes: usize, admitted: bool) {
        self.generated += 1;
        self.generated_bytes += bytes as u64;
        if admitted {
            self.admitted += 1;
        } else {
            self.withheld_bytes += bytes as u64;
        }
    }
}

/// Encoded byte count for an image, preferring the bridge's own `bytes`.
///
/// `bytes` is the size of the encoded image, which is what the admission bound
/// is expressed in and what the caller is shown. If it is missing the base64
/// length is an over-estimate (base64 inflates by 4/3), so the record errs
/// toward withholding — the safe direction for a bound whose job is to keep
/// oversized attachments out of the event.
fn encoded_len(capture: &serde_json::Map<String, Value>, b64: &str) -> usize {
    capture
        .get("bytes")
        .and_then(Value::as_u64)
        .map_or_else(|| decoded_len(b64), |bytes| bytes as usize)
}

/// Decoded length of a base64 payload, from its encoded length.
fn decoded_len(b64: &str) -> usize {
    b64.len() / 4 * 3
}

/// The caption that replaces a withheld image's attachment.
///
/// It has to say what the client is not getting and where it is instead, or a
/// text-rendering client sees a capture with no picture and concludes the
/// capture failed.
fn withheld_label(label: &str, bytes: usize) -> String {
    format!(
        "{label}: the image was NOT attached. It is {bytes} encoded bytes, above this \
         server's admission bound, so it was withheld from the attachment list to keep \
         this response within transport limits. The exact bytes are unchanged in \
         structuredContent, or re-capture with a smaller maxSize."
    )
}

fn call_tool_error_result(
    mut payload: Value,
    image_base64: Option<String>,
    image_mime_type: Option<String>,
    policy: AdmissionPolicy,
) -> CallToolResult {
    if let Some(b64) = image_base64.filter(|s| !s.is_empty()) {
        let mime = image_mime_type
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "image/png".into());
        // A refusal that carried an image is a real case: `black_frame` and
        // `wrong_family` return the image alongside the refusal precisely so
        // the caller can see WHY it was refused. Withholding it would remove
        // the evidence for the refusal, so the withheld caption names it.
        let bytes = decoded_len(&b64);
        let admitted = admit_image(policy, bytes) == Admission::Admitted;
        let mut tally = ImageTally::default();
        tally.record(bytes, admitted);
        if let Some(obj) = payload.as_object_mut() {
            // The image arrives here as a separate argument, so on the
            // withheld path it has to be put back into the structured payload
            // — otherwise the bytes the caption points at do not exist.
            if !admitted {
                obj.insert("imageBase64".to_owned(), Value::String(b64.clone()));
            }
            obj.insert(
                "admission".to_owned(),
                admission_record(
                    policy,
                    tally.generated,
                    tally.admitted,
                    tally.generated_bytes,
                    tally.withheld_bytes,
                ),
            );
        }
        let content = if admitted {
            vec![
                ContentBlock::image(b64, mime),
                ContentBlock::text(payload.to_string()),
            ]
        } else {
            vec![ContentBlock::text(format!(
                "{payload} — the image that came with this refusal ({bytes} encoded bytes) \
                 was NOT attached: it is above this server's admission bound, so it was \
                 withheld from the attachment list to keep the response within transport \
                 limits. The bytes are unchanged in structuredContent.imageBase64."
            ))]
        };
        let mut result = CallToolResult::error(content);
        result.structured_content = Some(payload);
        return text_copy_or_pointer(result);
    }
    text_copy_or_pointer(CallToolResult::structured_error(payload))
}

/// Promote top-level PNG (or other) payload to an image content block.
fn try_perception_image_result(
    mut value: Value,
    policy: AdmissionPolicy,
) -> Result<CallToolResult, Value> {
    let Some(obj) = value.as_object_mut() else {
        return Err(value);
    };
    // Read without removing: the bytes are only taken out of the structured
    // payload when they are actually attached, so a withheld image is still
    // recoverable from `structuredContent.imageBase64`.
    let b64 = match obj.get("imageBase64") {
        Some(Value::String(s)) if !s.is_empty() => s.clone(),
        _ => return Err(value),
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
    let bytes = obj
        .get("bytes")
        .and_then(Value::as_u64)
        .unwrap_or(decoded_len(&b64) as u64);
    // `bytes` is the size of the ENCODED image — the bridge reported the PNG
    // length — and the admission bound is expressed in the same unit. A
    // missing `bytes` falls back to the base64 length, which over-estimates
    // by 4/3 and so errs toward withholding rather than toward admitting a
    // payload nobody measured.
    let admitted = admit_image(policy, bytes as usize) == Admission::Admitted;
    let mut tally = ImageTally::default();
    tally.record(bytes as usize, admitted);
    if let Some(obj) = value.as_object_mut() {
        if admitted {
            obj.remove("imageBase64");
        }
        obj.insert(
            "admission".to_owned(),
            admission_record(
                policy,
                tally.generated,
                tally.admitted,
                tally.generated_bytes,
                tally.withheld_bytes,
            ),
        );
    }
    let content = if admitted {
        vec![
            ContentBlock::image(b64, mime.clone()),
            ContentBlock::text(format!(
                "Captured TOP image from {path} ({bytes} bytes, {mime})."
            )),
        ]
    } else {
        vec![ContentBlock::text(withheld_label(
            &format!("Captured TOP image from {path} ({bytes} bytes, {mime})"),
            bytes as usize,
        ))]
    };
    let mut result = CallToolResult::success(content);
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
        let result = call_tool_result_from_value("capture", value, AdmissionPolicy::default());
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
        let result = call_tool_result_from_value("capture", value, AdmissionPolicy::default());
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
        let result = call_tool_result_from_value("inspect", value, AdmissionPolicy::default());
        assert!(result.content.iter().all(|c| c.as_image().is_none()));
    }

    #[test]
    fn small_result_keeps_the_duplicate_text_block() {
        let value = json!({"ok": true, "result": 1});
        let result = call_tool_result_from_value(
            "execute_python",
            value.clone(),
            AdmissionPolicy::default(),
        );
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
        let result = call_tool_result_from_value(
            "execute_python",
            value.clone(),
            AdmissionPolicy::default(),
        );
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
        let result =
            call_tool_error_result(payload.clone(), None, None, AdmissionPolicy::default());
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
            AdmissionPolicy::default(),
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
            AdmissionPolicy::default(),
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

    // --- admission -------------------------------------------------------
    //
    // The load-bearing claim of every test below: a WITHHELD image is still in
    // `structuredContent`, byte for byte. Withholding is a decision about the
    // attachment list, not a deletion.

    /// The ordinary capture path: an admitted image is attached AND removed
    /// from the structured payload (as it always was), and the admission record
    /// says so.
    #[test]
    fn an_admitted_capture_says_admitted_in_both_halves() {
        let result = call_tool_result_from_value(
            "capture",
            json!({
                "ok": true, "path": "/project1/out1", "bytes": 12,
                "mimeType": "image/png", "imageBase64": "AAAA",
            }),
            AdmissionPolicy::default(),
        );
        assert_eq!(result.content[0].as_image().expect("image").data, "AAAA");
        let structured = result.structured_content.expect("structured");
        assert!(structured.pointer("/imageBase64").is_none());
        assert_eq!(structured["admission"]["admitted"]["verdict"], "admitted");
        assert_eq!(structured["admission"]["generated"]["images"], 1);
        assert_eq!(structured["admission"]["generated"]["bytes"], 12);
        assert_eq!(
            structured["admission"]["includedInModelInput"]["state"],
            "unavailable"
        );
    }

    /// The core case: over the bound, the image is NOT attached, but its bytes
    /// are still there, and a text-only client is told what happened.
    ///
    /// `bytes` is set explicitly rather than left to the base64 length, so the
    /// test exercises the reported-size path and the decision is a clean
    /// function of a number the caller can check.
    #[test]
    fn a_withheld_image_keeps_its_bytes_and_says_where_they_are() {
        let result = call_tool_result_from_value(
            "capture",
            json!({
                "ok": true, "path": "/project1/out1", "bytes": 5000,
                "mimeType": "image/png", "imageBase64": "QUJDREU=",
            }),
            AdmissionPolicy {
                max_image_bytes: 1000,
            },
        );
        // Not attached.
        assert!(
            result.content.iter().all(|b| b.as_image().is_none()),
            "an over-budget image must not be attached: {:?}",
            result.content
        );
        let structured = result.structured_content.expect("structured");
        // Still there, unchanged.
        assert_eq!(
            structured.pointer("/imageBase64").and_then(Value::as_str),
            Some("QUJDREU="),
            "a withheld image must keep its exact bytes in structuredContent"
        );
        let admission = &structured["admission"];
        assert_eq!(admission["admitted"]["verdict"], "withheld");
        assert_eq!(admission["admitted"]["images"], 0);
        assert_eq!(admission["admitted"]["withheldBytes"], 5000);
        assert_eq!(admission["generated"]["bytes"], 5000);
        // And a text-only client is told, in the same response.
        let texts: Vec<&str> = result
            .content
            .iter()
            .filter_map(|b| b.as_text().map(|t| t.text.as_str()))
            .collect();
        assert!(
            texts.iter().any(|t| t.contains("NOT attached")),
            "a text-only client must be told the picture is missing: {texts:?}"
        );
        assert!(
            texts.iter().any(|t| t.contains("structuredContent")),
            "the notice must name where the bytes are: {texts:?}"
        );
    }

    /// `imageBase64` may be ABSENT (`bytes` reported, no base64). That is not
    /// the same as an empty string, and it must not be turned into one.
    #[test]
    fn an_absent_image_is_not_an_empty_one() {
        let result = call_tool_result_from_value(
            "capture",
            json!({"ok": true, "path": "/project1/out1", "bytes": 0}),
            AdmissionPolicy::default(),
        );
        assert!(result.content.iter().all(|b| b.as_image().is_none()));
        let structured = result.structured_content.expect("structured");
        assert!(structured.pointer("/imageBase64").is_none());
        // No images at all, so admission is not applicable rather than passed.
        assert_eq!(
            structured["admission"]["admitted"]["verdict"],
            "not_applicable"
        );
    }

    /// A per-sample batch with one admitted and one withheld image: the record
    /// has to be `partial`, because that state is the one a single-image test
    /// can never reach and it is the one an evaluation would get wrong.
    #[test]
    fn a_batch_records_partial_admission_rather_than_a_single_verdict() {
        let result = call_tool_result_from_value(
            "capture",
            json!({
                "ok": true, "jobId": "job", "samples": [
                    {"offset":0, "capture":{"bytes":10, "imageBase64":"AAAA", "mimeType":"image/png"}},
                    {"offset":3, "capture":{"bytes":5000, "imageBase64":"QUJDREU=", "mimeType":"image/png"}}
                ]
            }),
            AdmissionPolicy {
                max_image_bytes: 1000,
            },
        );
        let images: Vec<_> = result
            .content
            .iter()
            .filter(|b| b.as_image().is_some())
            .collect();
        assert_eq!(images.len(), 1, "only the in-budget sample attaches");
        let structured = result.structured_content.expect("structured");
        // The admitted one is stripped, the withheld one is not. Per sample.
        assert!(structured
            .pointer("/samples/0/capture/imageBase64")
            .is_none());
        assert_eq!(
            structured
                .pointer("/samples/1/capture/imageBase64")
                .and_then(Value::as_str),
            Some("QUJDREU=")
        );
        let admission = &structured["admission"];
        assert_eq!(admission["generated"]["images"], 2);
        assert_eq!(admission["admitted"]["images"], 1);
        assert_eq!(admission["admitted"]["verdict"], "partial");
        assert_eq!(admission["admitted"]["withheldBytes"], 5000);
        // The caption order is still the sample order.
        let texts: Vec<&str> = result
            .content
            .iter()
            .filter_map(|b| b.as_text().map(|t| t.text.as_str()))
            .collect();
        assert!(texts[0].contains("offset 0"), "{texts:?}");
        assert!(texts
            .iter()
            .any(|t| t.contains("offset 3") && t.contains("NOT attached")));
    }

    /// A refusal that carried an image: withholding must not delete the
    /// evidence for the refusal. The image goes back into the structured
    /// payload, because it arrived as a SEPARATE argument, not inside it.
    #[test]
    fn a_refusal_with_an_oversized_image_still_carries_the_image() {
        let result = call_tool_error_result(
            json!({"ok": false, "code": "tdmcp.perception.black_frame", "path": "/project1/out1"}),
            Some("QUJDREU=".to_owned()),
            Some("image/png".to_owned()),
            AdmissionPolicy { max_image_bytes: 4 },
        );
        assert!(result.content.iter().all(|b| b.as_image().is_none()));
        let structured = result.structured_content.expect("structured");
        assert_eq!(
            structured.pointer("/imageBase64").and_then(Value::as_str),
            Some("QUJDREU="),
            "the refused capture's image must survive withholding: {structured}"
        );
        assert_eq!(structured["code"], "tdmcp.perception.black_frame");
        assert_eq!(structured["admission"]["admitted"]["verdict"], "withheld");
        let texts: Vec<&str> = result
            .content
            .iter()
            .filter_map(|b| b.as_text().map(|t| t.text.as_str()))
            .collect();
        assert!(
            texts
                .iter()
                .any(|t| t.contains("black_frame") && t.contains("NOT attached")),
            "the refusal text AND the withholding notice must both be present: {texts:?}"
        );
    }

    /// A refusal with an in-budget image still attaches, exactly as before.
    #[test]
    fn a_refusal_with_an_admitted_image_is_unchanged() {
        let result = call_tool_error_result(
            json!({"ok": false, "code": "tdmcp.perception.wrong_family"}),
            Some("AAAA".to_owned()),
            Some("image/png".to_owned()),
            AdmissionPolicy::default(),
        );
        assert_eq!(result.content[0].as_image().expect("image").data, "AAAA");
        let structured = result.structured_content.expect("structured");
        assert_eq!(structured["admission"]["admitted"]["verdict"], "admitted");
        // The image is NOT duplicated into the structured payload on the
        // admitted path: it was passed separately, and adding it back would
        // double every refusal-with-image response.
        assert!(structured.pointer("/imageBase64").is_none());
    }

    /// The bound is the bridge's REPORTED byte count, not the base64 length.
    /// A 4-byte image in a 6-byte base64 field must still be admitted under a
    /// 5-byte bound: measuring the wire form would withhold it and report a
    /// limit the caller cannot check against `bytes`.
    #[test]
    fn the_bound_is_measured_against_the_reported_bytes() {
        let value = json!({
            "ok": true, "path": "/project1/out1", "bytes": 4,
            "mimeType": "image/png", "imageBase64": "QUJDREU=",
        });
        let admitted = call_tool_result_from_value(
            "capture",
            value.clone(),
            AdmissionPolicy { max_image_bytes: 5 },
        );
        assert!(
            admitted.content.iter().any(|b| b.as_image().is_some()),
            "4 reported bytes under a 5-byte bound must attach"
        );
        let withheld =
            call_tool_result_from_value("capture", value, AdmissionPolicy { max_image_bytes: 3 });
        assert!(
            withheld.content.iter().all(|b| b.as_image().is_none()),
            "4 reported bytes over a 3-byte bound must be withheld"
        );
    }

    /// A MISSING `bytes` must not become zero. `bytes: 0` would compare equal to
    /// anything and read as "this image is free"; the base64 length is used
    /// instead, which over-estimates — the safe direction for a bound.
    #[test]
    fn a_missing_byte_count_is_measured_from_the_payload_not_treated_as_zero() {
        let result = call_tool_result_from_value(
            "capture",
            json!({
                "ok": true, "path": "/project1/out1",
                "mimeType": "image/png", "imageBase64": "QUJDREU=",
            }),
            AdmissionPolicy { max_image_bytes: 3 },
        );
        assert!(
            result.content.iter().all(|b| b.as_image().is_none()),
            "an unmeasured image must not be admitted on a small bound"
        );
        let structured = result.structured_content.expect("structured");
        assert_eq!(
            structured["admission"]["generated"]["bytes"], 6,
            "8 base64 chars decode to 8/4*3 = 6 bytes; the point is that it is \
             not 0, and that 6 over a 3-byte bound withholds"
        );
    }

    /// Zero is not a licence to admit everything. `with_admission` refuses the
    /// configuration, but the decision function must be total anyway.
    #[test]
    fn a_zero_bound_withholds_rather_than_admitting_everything() {
        let result = call_tool_result_from_value(
            "capture",
            json!({
                "ok": true, "path": "/project1/out1", "bytes": 1,
                "mimeType": "image/png", "imageBase64": "AA==",
            }),
            AdmissionPolicy { max_image_bytes: 0 },
        );
        assert!(result.content.iter().all(|b| b.as_image().is_none()));
        let structured = result.structured_content.expect("structured");
        assert_eq!(structured["admission"]["admitted"]["verdict"], "withheld");
    }
}

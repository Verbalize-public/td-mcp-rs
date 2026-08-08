//! Tool descriptors, dispatch, and bridge-coupled execution.
//!
//! `dispatch_tool` is async: it enqueues a task on the per-pid queue, delegates
//! the live bridge RPC to the daemon-supplied [`BridgeRpc`] impl, then records
//! the task outcome on the registry. Diagnostic mapping lives in
//! [`crate::outcomes`].

use std::sync::Arc;
use std::time::Duration;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tdmcp_core::{BridgeMethod, OpPath, Pid, PidRegistry, TaskMode};
use tdmcp_diagnostics::DiagnosticLevel;
use thiserror::Error;
use tokio::sync::Mutex;

use crate::bridge_rpc::{BridgeRpc, BridgeRpcError};
use crate::editor_context::EditorContextParams;
use crate::fleet::{fleet_summary, FleetParams};
use crate::outcomes::{
    map_api_help_outcome, map_editor_context_outcome, map_inspect_outcome, map_mutate_outcome,
    map_perception_outcome, map_script_outcome, session_busy,
};
use crate::schema::input_schema_for;
use crate::session_registry::{BridgeCallSlot, McpSessionRegistry};

/// Outer safety ceiling for awaiting a daemon bridge oneshot.
///
/// The daemon owns the real per-method budgets (`[bridge].call_timeout_secs` /
/// `script_timeout_secs`). This ceiling only fires if the oneshot never
/// completes (e.g. actor crash without reply). Keep it above the max script
/// timeout default (120s).
pub const BRIDGE_TIMEOUT: Duration = Duration::from_secs(180);

/// Soft-cap on `inspect` `paths[]` (bridge enforces; mirrored in docs).
pub const INSPECT_PATHS_LIMIT: usize = 96;

/// Soft-cap on each inspect node's direct-child roster.
pub const CHILDREN_ROSTER_LIMIT: usize = 96;

/// Soft-cap on `api_help` `queries[]` (bridge enforces; mirrored in docs).
pub const API_HELP_QUERIES_LIMIT: usize = 32;

/// Soft-cap on `editor_context` per-pane selection list.
pub const EDITOR_SELECTION_LIMIT: usize = 96;

/// Soft-cap on `editor_context` panes array.
pub const EDITOR_PANES_LIMIT: usize = 32;

/// MCP tool names — one enum for descriptors, dispatch, and schemas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolName {
    /// Fleet discovery.
    Fleet,
    /// Run Python in TD.
    ExecutePython,
    /// Structural inspect.
    Inspect,
    /// Ordered mutate steps.
    MutateNodes,
    /// Perception capture.
    Capture,
    /// Live TD Python API cards / class index.
    ApiHelp,
    /// Live editor pane / selection snapshot.
    EditorContext,
    /// Tool manifest.
    DescribeTools,
}

impl ToolName {
    /// Wire / MCP tool name string.
    #[must_use]
    pub const fn wire_str(self) -> &'static str {
        match self {
            Self::Fleet => "fleet",
            Self::ExecutePython => "execute_python",
            Self::Inspect => "inspect",
            Self::MutateNodes => "mutate_nodes",
            Self::Capture => "capture",
            Self::ApiHelp => "api_help",
            Self::EditorContext => "editor_context",
            Self::DescribeTools => "describe_tools",
        }
    }

    /// One-line description for list/describe.
    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::Fleet => {
                "Fleet view — TD processes by pid, bridge, tasks, cancelled traces"
            }
            Self::ExecutePython => {
                "Run Python in TD; failures return structured exception (type/frames/syntax); default diagnosticLevel detailed; formatMode debug adds capped locals; prints tee to Debug DAT / logs."
            }
            Self::Inspect => {
                "Structural read for an explicit paths[] batch (required, non-empty; soft-capped at 96). No auto-recursion — caller chooses nodes. Empty include defaults to nodes+errors+warnings; params opt-in; non-empty include is an allowlist. Params entries are {name, mode, val, expr?} (expr only when mode is EXPRESSION; val is evaluated and JSON-safe). Per-node summary includes a direct-child roster ({name, opType}); detailed adds path+family. Roster capped at 96 — when truncated see node.truncation. Bad paths return ok:false inline; siblings still succeed."
            }
            Self::MutateNodes => {
                "Ordered create/set/delete/connect/disconnect steps; sequential apply, stop on first hard error; later steps skipped (tdmcp.batch.skipped_dependent). Fix from failedAt only."
            }
            Self::Capture => {
                "Perception capture. top=native TOP JPEG; preview=any family via shared bridge OP Viewer TOP; chop_data=CHOP JSON; chop_image/pop=aliases of preview; auto=TOP→top, CHOP→chop_data, else preview."
            }
            Self::ApiHelp => {
                "Live TD Python API cards (not wiki dumps). Batch queries[] (soft-cap 32): class (doc/opType/family/mro/members), classes (op-like index + family/prefix), module (td thin). No help() / no param listing — use inspect include params for .par names. Case-sensitive class names."
            }
            Self::EditorContext => {
                "Live editor context — all TD panes with type/ownerPath/focused; per-pane selection as [{path, current}] (omitted when empty). Panes soft-capped at 32; selection soft-capped at 96. Bad panes return ok:false inline; siblings still succeed. Hint for mutation zone — still verify with inspect."
            }
            Self::DescribeTools => "Manifest of available tools",
        }
    }

    /// All v1 tools (descriptor / parity order).
    pub const ALL: &[Self] = &[
        Self::Fleet,
        Self::ExecutePython,
        Self::Inspect,
        Self::MutateNodes,
        Self::Capture,
        Self::ApiHelp,
        Self::EditorContext,
        Self::DescribeTools,
    ];

    /// Parse a wire tool name.
    #[must_use]
    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "fleet" => Some(Self::Fleet),
            "execute_python" => Some(Self::ExecutePython),
            "inspect" => Some(Self::Inspect),
            "mutate_nodes" => Some(Self::MutateNodes),
            "capture" => Some(Self::Capture),
            "api_help" => Some(Self::ApiHelp),
            "editor_context" => Some(Self::EditorContext),
            "describe_tools" => Some(Self::DescribeTools),
            _ => None,
        }
    }
}

/// Static tool descriptor for `describe_tools` / MCP list.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDescriptor {
    /// Tool name.
    pub name: String,
    /// One-line description.
    pub description: String,
    /// JSON Schema for arguments (derived from param types).
    pub input_schema: Map<String, Value>,
}

/// Tool call failures mapped to diagnostics.
#[derive(Debug, Error)]
pub enum ToolCallError {
    /// Unknown tool name.
    #[error("unknown tool: {0}")]
    UnknownTool(String),
    /// JSON args parse failure.
    #[error("invalid arguments: {0}")]
    InvalidArgs(String),
    /// Domain / queue / bridge failure with diagnostics.
    #[error("{0}")]
    Failed(Box<ToolFailPayload>),
}

/// Payload for [`ToolCallError::Failed`] (boxed to keep the error enum small).
#[derive(Debug)]
pub struct ToolFailPayload {
    /// Short summary.
    pub summary: String,
    /// Structured diagnostics.
    pub diagnostics: tdmcp_diagnostics::Diagnostics,
    /// Optional image (base64) when perception failed but a frame was captured
    /// (e.g. black-frame) — agents still need to see the pixels.
    pub image_base64: Option<String>,
    /// MIME type for [`Self::image_base64`] (default `image/png` at promotion).
    pub image_mime_type: Option<String>,
    /// Optional structured payload (e.g. mutate `applied` / `failedAt` / `steps`).
    pub data: Option<Value>,
}

impl std::fmt::Display for ToolFailPayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.summary)
    }
}

impl ToolFailPayload {
    /// Wrap as [`ToolCallError::Failed`].
    #[must_use]
    pub fn into_error(self) -> ToolCallError {
        ToolCallError::Failed(Box::new(self))
    }

    /// Normalize a failure into the wire structured-content shape shared by
    /// rmcp and the axum JSON fallback.
    ///
    /// Always includes top-level `"ok": false`. Serializes `diagnostics` as
    /// `{summary, items}` at the top level, then splices object keys from
    /// [`Self::data`] (e.g. mutate `applied` / `failedAt` / `steps`) flat —
    /// never nested under `"data"`. Non-object `data` is kept under `"data"`
    /// as a last resort.
    #[must_use]
    pub fn structured_content(&self) -> Value {
        let mut payload = match serde_json::to_value(&self.diagnostics) {
            Ok(Value::Object(map)) => Value::Object(map),
            Ok(_) | Err(_) => serde_json::json!({
                "summary": self.summary,
                "items": [],
            }),
        };
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("ok".into(), Value::Bool(false));
            if let Some(data) = &self.data {
                match data {
                    Value::Object(data_obj) => {
                        for (k, v) in data_obj {
                            obj.insert(k.clone(), v.clone());
                        }
                    }
                    other => {
                        obj.insert("data".into(), other.clone());
                    }
                }
            }
        }
        payload
    }
}

/// Catalogue of v1 tools with derived schemas.
#[must_use]
pub fn tool_descriptors() -> Vec<ToolDescriptor> {
    ToolName::ALL
        .iter()
        .copied()
        .map(|tool| ToolDescriptor {
            name: tool.wire_str().into(),
            description: tool.description().into(),
            input_schema: input_schema_for(tool),
        })
        .collect()
}

/// Locals capture mode for execute_python exception reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum FormatMode {
    /// Structured exception without frame locals.
    #[default]
    Normal,
    /// Include capped locals on `<string>` frames.
    Debug,
}

impl FormatMode {
    /// Wire string for the bridge.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Debug => "debug",
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_detailed() -> DiagnosticLevel {
    DiagnosticLevel::Detailed
}

/// Args for execute_python.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutePythonParams {
    /// Target pid.
    pub pid: Pid,
    /// Script body.
    pub script: String,
    /// Ignored — bridged tools always exclusive-enqueue (kept for wire compat).
    #[serde(default)]
    pub exclusive: bool,
    /// Optional context path (exposed to script as helper; not enforced).
    #[serde(default)]
    pub context_path: Option<OpPath>,
    /// When true (default), capture stdout/stderr during exec and return as `logs`.
    #[serde(default = "default_true")]
    pub include_logs: bool,
    /// Diagnostic payload size (`summary` omits raw traceback).
    /// Default for this tool is `detailed` (other tools keep global summary default).
    #[serde(default = "default_detailed")]
    pub diagnostic_level: DiagnosticLevel,
    /// Exception report locals mode (`debug` attaches capped frame locals).
    #[serde(default)]
    pub format_mode: FormatMode,
}

/// Capture mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CaptureMode {
    /// TOP → PNG (native `saveByteArray`; retains alpha).
    Top,
    /// Any family → shared bridge OP Viewer TOP (`capture_viewer`).
    Preview,
    /// TOP → top; CHOP → chop_data; everything else → preview.
    #[default]
    Auto,
    /// CHOP → capped JSON (no image).
    ChopData,
    /// Alias of `preview` (shared OP Viewer); kept for existing callers.
    ChopImage,
    /// Alias of `preview` (shared OP Viewer); kept for existing callers.
    Pop,
}

impl CaptureMode {
    /// Wire string for the bridge.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Top => "top",
            Self::Preview => "preview",
            Self::Auto => "auto",
            Self::ChopData => "chop_data",
            Self::ChopImage => "chop_image",
            Self::Pop => "pop",
        }
    }
}

/// Default longer-side cap for perception images (token / wire discipline).
pub const CAPTURE_DEFAULT_MAX_SIZE: u32 = 256;

/// Args for capture (perception).
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CaptureParams {
    /// Target pid.
    pub pid: Pid,
    /// Operator path (OpPath; relative to contextPath or /project1).
    pub path: OpPath,
    /// Capture mode.
    #[serde(default)]
    pub mode: CaptureMode,
    /// Resolution base for relative `path`.
    #[serde(default)]
    pub context_path: Option<OpPath>,
    /// Longer-side pixel cap before PNG encode. `null` = native resolution.
    /// Defaults to 256.
    #[serde(default = "default_capture_max_size")]
    pub max_size: Option<u32>,
    /// Diagnostic payload size (`summary` omits raw traceback).
    #[serde(default)]
    pub diagnostic_level: DiagnosticLevel,
}

fn default_capture_max_size() -> Option<u32> {
    Some(CAPTURE_DEFAULT_MAX_SIZE)
}

/// Sections to include in an inspect response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum InspectInclude {
    /// Node tree.
    Nodes,
    /// Parameters.
    Params,
    /// TD errors.
    Errors,
    /// TD warnings.
    Warnings,
}

/// Structural detail level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum DetailLevel {
    /// Direct-child roster as `{name, opType}` (capped at 96; see `node.truncation`).
    #[default]
    Summary,
    /// Direct-child roster as `{path, family, opType}` (same 96 cap — does not uncap).
    Detailed,
}

impl DetailLevel {
    /// Wire string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Summary => "summary",
            Self::Detailed => "detailed",
        }
    }
}

/// Args for inspect.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InspectParams {
    /// Target pid.
    pub pid: Pid,
    /// Explicit operator paths to inspect (required, non-empty). Soft-capped at 96.
    /// No auto-recursion — caller chooses exactly which nodes to fetch.
    pub paths: Vec<OpPath>,
    /// Resolution base for relative entries in `paths`.
    #[serde(default)]
    pub context_path: Option<OpPath>,
    /// Sections to include. Empty/omitted = nodes+errors+warnings; params opt-in; non-empty = allowlist.
    #[serde(default)]
    pub include: Vec<InspectInclude>,
    /// Structural detail level.
    #[serde(default)]
    pub detail_level: DetailLevel,
    /// Diagnostic payload size (`summary` omits raw traceback).
    #[serde(default)]
    pub diagnostic_level: DiagnosticLevel,
}

/// One ordered mutate step (`create` / `set` / `delete` / `connect` / `disconnect`).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "op", rename_all = "lowercase", deny_unknown_fields)]
pub enum MutateStep {
    /// Create a node at `path` with the given `opType`.
    Create {
        /// Desired node path (absolute or relative to contextPath).
        path: OpPath,
        /// TD op class name (e.g. `noiseTOP`).
        #[serde(rename = "opType")]
        op_type: String,
        /// Plain parameter values (`.par.*` only — direct OP attributes like `display`/`viewer` go in `flags`, not here).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        values: Option<Map<String, Value>>,
        /// Direct OP attribute writes (`node.<name> = val`); allowlist = TD Common Flags subset, see CONTRACT.md.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        flags: Option<Map<String, Value>>,
    },
    /// Set values / expressions / pulse / flags on an existing node.
    Set {
        /// Target node path.
        path: OpPath,
        /// Plain parameter values (`.par.*` only — direct OP attributes like `display`/`viewer` go in `flags`, not here).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        values: Option<Map<String, Value>>,
        /// Expression strings; mode is set to expression before assign.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expressions: Option<Map<String, Value>>,
        /// Parameter names to pulse.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pulse: Option<Vec<String>>,
        /// Direct OP attribute writes (`node.<name> = val`); allowlist = TD Common Flags subset, see CONTRACT.md.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        flags: Option<Map<String, Value>>,
    },
    /// Destroy a node.
    Delete {
        /// Target node path.
        path: OpPath,
    },
    /// Wire `src` output connector to `dst` input connector.
    Connect {
        /// Source operator path.
        src: OpPath,
        /// Destination operator path.
        dst: OpPath,
        /// Source output connector index (default 0).
        #[serde(default, rename = "srcOutput")]
        src_output: u32,
        /// Destination input connector index (default 0).
        #[serde(default, rename = "dstInput")]
        dst_input: u32,
    },
    /// Clear an input connector on `path`.
    Disconnect {
        /// Target operator path (destination side).
        path: OpPath,
        /// Input connector index to clear (default 0).
        #[serde(default)]
        input: u32,
    },
}

/// Args for mutate_nodes.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MutateNodesParams {
    /// Target pid.
    pub pid: Pid,
    /// Ordered steps; apply stops at the first hard failure.
    pub steps: Vec<MutateStep>,
    /// Resolution base for relative paths.
    #[serde(default)]
    pub context_path: Option<OpPath>,
    /// Ignored — bridged tools always exclusive-enqueue (kept for wire compat).
    #[serde(default)]
    pub exclusive: bool,
    /// Structural detail level for per-step echo.
    #[serde(default)]
    pub detail_level: DetailLevel,
    /// Diagnostic payload size (`summary` omits raw traceback).
    #[serde(default)]
    pub diagnostic_level: DiagnosticLevel,
}

/// Operator family filter for `api_help` `classes` queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "UPPERCASE")]
pub enum ApiHelpFamily {
    /// COMP family.
    Comp,
    /// TOP family.
    Top,
    /// CHOP family.
    Chop,
    /// SOP family.
    Sop,
    /// POP family.
    Pop,
    /// MAT family.
    Mat,
    /// DAT family.
    Dat,
}

impl ApiHelpFamily {
    /// Wire string for the bridge.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Comp => "COMP",
            Self::Top => "TOP",
            Self::Chop => "CHOP",
            Self::Sop => "SOP",
            Self::Pop => "POP",
            Self::Mat => "MAT",
            Self::Dat => "DAT",
        }
    }
}

/// One `api_help` query entry (class card / classes index / thin module).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum ApiHelpQuery {
    /// Single `td.<name>` class / type card.
    Class {
        /// Exact `td` attribute name (case-sensitive; e.g. `noiseTOP`).
        name: String,
    },
    /// Filtered index of op-like type names on `td`.
    Classes {
        /// Optional family filter (TOP / CHOP / …).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        family: Option<ApiHelpFamily>,
        /// Optional casefold prefix filter on the type name.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prefix: Option<String>,
    },
    /// Thin module card (`td` only in v1).
    Module {
        /// Module name (`td`).
        name: String,
    },
}

/// Args for api_help.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApiHelpParams {
    /// Target pid.
    pub pid: Pid,
    /// Batch of API card / index queries (required, non-empty). Soft-capped at 32.
    pub queries: Vec<ApiHelpQuery>,
    /// Caps member lists / whether wikiUrl + full mro appear (not help prose).
    #[serde(default)]
    pub detail_level: DetailLevel,
    /// Diagnostic payload size (`summary` omits raw traceback).
    #[serde(default)]
    pub diagnostic_level: DiagnosticLevel,
}

/// Outcome of a bridge-driven tool call, as reported to the mapper.
#[derive(Debug)]
pub enum BridgeOutcome {
    /// Bridge returned a result value (may encode a soft failure).
    Ok(Value),
    /// Queue rejected the enqueue (exclusive-while-busy).
    QueueBusy,
    /// Transport / timeout / disconnect failure.
    Transport(BridgeRpcError),
}

/// Optional MCP session identity for the session-chill gate.
#[derive(Debug, Clone, Copy)]
pub struct SessionGate<'a> {
    /// Streamable HTTP MCP session lease id.
    pub session_id: &'a str,
    /// Shared session registry holding `(session, pid)` in-flight slots.
    pub sessions: &'a McpSessionRegistry,
}

/// Dispatch a named tool call to a JSON result.
///
/// Never holds the registry lock across a bridge await.
/// Pass [`Some`] [`SessionGate`] from the rmcp handler; JSON fallback passes [`None`]
/// (pid exclusive still applies; session chill is skipped).
pub async fn dispatch_tool(
    registry: &Arc<Mutex<PidRegistry>>,
    catalog: &tdmcp_diagnostics::Catalog,
    bridge: &dyn BridgeRpc,
    name: &str,
    args: Value,
    session: Option<SessionGate<'_>>,
) -> Result<Value, ToolCallError> {
    let tool =
        ToolName::from_wire(name).ok_or_else(|| ToolCallError::UnknownTool(name.to_owned()))?;
    match tool {
        ToolName::Fleet => {
            let params: FleetParams = serde_json::from_value(args)
                .map_err(|e| ToolCallError::InvalidArgs(e.to_string()))?;
            let want_tasks = params.include.contains(&crate::fleet::FleetInclude::Tasks);
            let mut ipc_depths = Vec::new();
            if want_tasks {
                let pids: Vec<u32> = {
                    let reg = registry.lock().await;
                    reg.pids()
                };
                for pid in pids {
                    if let Some(depth) = bridge.job_queue_depth(pid).await {
                        if depth > 0 {
                            ipc_depths.push((pid, depth));
                        }
                    }
                }
            }
            let reg = registry.lock().await;
            Ok(
                serde_json::to_value(fleet_summary(&reg, &params, &ipc_depths))
                    .map_err(|e| ToolCallError::InvalidArgs(e.to_string()))?,
            )
        }
        ToolName::DescribeTools => Ok(serde_json::json!({ "tools": tool_descriptors() })),
        ToolName::ExecutePython => {
            let params: ExecutePythonParams = serde_json::from_value(args)
                .map_err(|e| ToolCallError::InvalidArgs(e.to_string()))?;
            let _slot = begin_session_slot(session, catalog, "execute_python", params.pid)?;
            let method = BridgeMethod::ExecutePython;
            let outcome = enqueue_and_call(
                registry,
                bridge,
                params.pid,
                method,
                serde_json::json!({
                    "script": params.script,
                    "contextPath": params.context_path,
                    "includeLogs": params.include_logs,
                    "formatMode": params.format_mode.as_str(),
                }),
            )
            .await;
            map_script_outcome(
                catalog,
                params.pid,
                outcome,
                params.diagnostic_level,
                params.format_mode,
                params.context_path.clone(),
            )
        }
        ToolName::Capture => {
            let params: CaptureParams = serde_json::from_value(args)
                .map_err(|e| ToolCallError::InvalidArgs(e.to_string()))?;
            let _slot = begin_session_slot(session, catalog, "capture", params.pid)?;
            let method = BridgeMethod::Capture;
            let outcome = enqueue_and_call(
                registry,
                bridge,
                params.pid,
                method,
                serde_json::json!({
                    "path": params.path,
                    "mode": params.mode.as_str(),
                    "contextPath": params.context_path,
                    "maxSize": params.max_size,
                }),
            )
            .await;
            map_perception_outcome(
                catalog,
                params.pid,
                params.path,
                params.context_path,
                outcome,
                params.diagnostic_level,
            )
        }
        ToolName::Inspect => {
            let params: InspectParams = serde_json::from_value(args)
                .map_err(|e| ToolCallError::InvalidArgs(e.to_string()))?;
            if params.paths.is_empty() {
                return Err(ToolCallError::InvalidArgs(
                    "inspect requires a non-empty paths array".into(),
                ));
            }
            let _slot = begin_session_slot(session, catalog, "inspect", params.pid)?;
            let method = BridgeMethod::Inspect;
            let include: Vec<&str> = params
                .include
                .iter()
                .map(|i| match i {
                    InspectInclude::Nodes => "nodes",
                    InspectInclude::Params => "params",
                    InspectInclude::Errors => "errors",
                    InspectInclude::Warnings => "warnings",
                })
                .collect();
            // Soft-cap is enforced on the bridge; still forward the full list
            // so truncation metadata can report the requested count.
            let outcome = enqueue_and_call(
                registry,
                bridge,
                params.pid,
                method,
                serde_json::json!({
                    "paths": params.paths,
                    "contextPath": params.context_path,
                    "include": include,
                    "detailLevel": params.detail_level.as_str(),
                }),
            )
            .await;
            let span_path = params.paths.first().cloned();
            map_inspect_outcome(
                catalog,
                params.pid,
                span_path,
                params.context_path,
                outcome,
                params.diagnostic_level,
            )
        }
        ToolName::MutateNodes => {
            let params: MutateNodesParams = serde_json::from_value(args)
                .map_err(|e| ToolCallError::InvalidArgs(e.to_string()))?;
            let _slot = begin_session_slot(session, catalog, "mutate_nodes", params.pid)?;
            let method = BridgeMethod::MutateNodes;
            let steps = serde_json::to_value(&params.steps)
                .map_err(|e| ToolCallError::InvalidArgs(e.to_string()))?;
            let outcome = enqueue_and_call(
                registry,
                bridge,
                params.pid,
                method,
                serde_json::json!({
                    "steps": steps,
                    "contextPath": params.context_path,
                    "detailLevel": params.detail_level.as_str(),
                }),
            )
            .await;
            map_mutate_outcome(
                catalog,
                params.pid,
                params.context_path,
                outcome,
                params.diagnostic_level,
            )
        }
        ToolName::ApiHelp => {
            let params: ApiHelpParams = serde_json::from_value(args)
                .map_err(|e| ToolCallError::InvalidArgs(e.to_string()))?;
            if params.queries.is_empty() {
                return Err(ToolCallError::InvalidArgs(
                    "api_help requires a non-empty queries array".into(),
                ));
            }
            let _slot = begin_session_slot(session, catalog, "api_help", params.pid)?;
            let queries = serde_json::to_value(&params.queries)
                .map_err(|e| ToolCallError::InvalidArgs(e.to_string()))?;
            let outcome = enqueue_and_call(
                registry,
                bridge,
                params.pid,
                BridgeMethod::ApiHelp,
                serde_json::json!({
                    "queries": queries,
                    "detailLevel": params.detail_level.as_str(),
                }),
            )
            .await;
            map_api_help_outcome(catalog, params.pid, outcome, params.diagnostic_level)
        }
        ToolName::EditorContext => {
            let params: EditorContextParams = serde_json::from_value(args)
                .map_err(|e| ToolCallError::InvalidArgs(e.to_string()))?;
            let _slot = begin_session_slot(session, catalog, "editor_context", params.pid)?;
            let outcome = enqueue_and_call(
                registry,
                bridge,
                params.pid,
                BridgeMethod::EditorContext,
                serde_json::json!({}),
            )
            .await;
            map_editor_context_outcome(catalog, params.pid, outcome, params.diagnostic_level)
        }
    }
}

fn begin_session_slot<'a>(
    session: Option<SessionGate<'a>>,
    catalog: &tdmcp_diagnostics::Catalog,
    tool: &str,
    pid: Pid,
) -> Result<Option<BridgeCallSlot<'a>>, ToolCallError> {
    let Some(gate) = session else {
        return Ok(None);
    };
    match gate
        .sessions
        .try_begin_bridge_call(gate.session_id, pid.get())
    {
        Some(slot) => Ok(Some(slot)),
        None => Err(session_busy(catalog, tool, pid)),
    }
}

/// Enqueue exclusive (always), then call the bridge with a timeout.
/// The daemon actor owns queue progression (`start_next` / `complete_task`)
/// so it stays coupled to the wire.
///
/// When the call never reaches actor completion (`NotConnected`, send-fail
/// `Disconnected`, or the outer MCP wait fires), clear the pid queue so a
/// zombie slot cannot wedge exclusive callers after dual-MCP races.
async fn enqueue_and_call(
    registry: &Arc<Mutex<PidRegistry>>,
    bridge: &dyn BridgeRpc,
    pid: Pid,
    method: BridgeMethod,
    params: Value,
) -> BridgeOutcome {
    let raw_pid = pid.get();
    {
        let mut reg = registry.lock().await;
        if let Err(e) = reg.enqueue(raw_pid, method.queue_label(), TaskMode::Exclusive) {
            return match &e {
                tdmcp_core::EnqueueError::Queue(_) => BridgeOutcome::QueueBusy,
                _ => BridgeOutcome::Transport(BridgeRpcError::NotConnected { pid: raw_pid }),
            };
        }
    }

    let call = bridge.call(raw_pid, method.wire_str(), params);
    match tokio::time::timeout(BRIDGE_TIMEOUT, call).await {
        Ok(Ok(value)) => BridgeOutcome::Ok(value),
        Ok(Err(err)) => {
            if matches!(
                &err,
                BridgeRpcError::NotConnected { .. } | BridgeRpcError::Disconnected { .. }
            ) {
                clear_queue_keep_connected(registry, raw_pid).await;
            }
            BridgeOutcome::Transport(err)
        }
        Err(_) => {
            clear_queue_keep_connected(registry, raw_pid).await;
            BridgeOutcome::Transport(BridgeRpcError::Timeout {
                pid: raw_pid,
                budget_ms: BRIDGE_TIMEOUT.as_millis() as u64,
            })
        }
    }
}

async fn clear_queue_keep_connected(registry: &Arc<Mutex<PidRegistry>>, pid: u32) {
    let mut reg = registry.lock().await;
    let _ = reg.cancel_queue_keep_connected(pid);
}

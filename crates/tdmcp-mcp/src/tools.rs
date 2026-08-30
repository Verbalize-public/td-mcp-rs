//! Tool descriptors, dispatch, and bridge-coupled execution.
//!
//! `dispatch_tool` is async: it enqueues a task on the per-pid queue, delegates
//! the live bridge RPC to the daemon-supplied [`BridgeRpc`] impl, then records
//! the task outcome on the registry. Diagnostic mapping lives in
//! [`crate::outcomes`].

use std::ops::ControlFlow;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tdmcp_core::{
    AggregatedFleetProcess, BridgeMethod, DaemonId, OpPath, Pid, PidRegistry, PidResolve,
    SlaveReachability, TaskMode,
};
use tdmcp_diagnostics::{codes, DiagnosticLevel};
use thiserror::Error;
use tokio::sync::Mutex;

use crate::args_diag::{coded_failure, parse_args, serialize_failed};
use crate::bridge_rpc::{BridgeRpc, BridgeRpcError};
use crate::editor_context::EditorContextParams;
use crate::fleet::{fleet_summary, FleetParams, FleetProcess, FleetResponse};
use crate::outcomes::{
    ambiguous_pid, map_api_help_outcome, map_editor_context_outcome, map_inspect_outcome,
    map_mutate_outcome, map_perception_outcome, map_script_outcome, session_busy,
    slave_unreachable,
};
use crate::schema::input_schema_for;
use crate::server::FederationCtx;
use crate::session_registry::{BridgeCallSlot, McpSessionRegistry, DAEMON_SCOPE_LOCAL};

/// Outer safety ceiling for awaiting a daemon bridge oneshot.
///
/// The daemon owns the real per-method budgets (`[bridge].call_timeout_secs` /
/// `script_timeout_secs`). This ceiling only fires if the oneshot never
/// completes (e.g. actor crash without reply). Keep it above the max script
/// timeout default (120s). Historical fixed fallback — see
/// [`init_bridge_timeouts`] for the config-derived value actually used once
/// the daemon has started.
pub const BRIDGE_TIMEOUT: Duration = Duration::from_secs(180);

/// Master→slave HTTP proxy timeout (bridge script budget + margin).
/// Historical fixed fallback — see [`init_bridge_timeouts`].
pub const PROXY_TIMEOUT: Duration = Duration::from_secs(130);

/// Process-wide override for [`BRIDGE_TIMEOUT`] / [`PROXY_TIMEOUT`], set once
/// at daemon startup by [`init_bridge_timeouts`]. `OnceLock` rather than
/// threading a value through `dispatch_tool` → `enqueue_and_call` /
/// `maybe_proxy_bridged` (a dozen call sites) for what is explicitly an
/// outer safety net, not the primary timeout path. Each keeps its own
/// historical fallback (180s / 130s) when uninitialized, so tests — which
/// never call `init_bridge_timeouts` — see unchanged behavior.
static DERIVED_BRIDGE_TIMEOUT: std::sync::OnceLock<Duration> = std::sync::OnceLock::new();
static DERIVED_PROXY_TIMEOUT: std::sync::OnceLock<Duration> = std::sync::OnceLock::new();

/// Derive [`BRIDGE_TIMEOUT`] / [`PROXY_TIMEOUT`] from the daemon's
/// `[bridge].script_timeout_secs` config: `script_timeout_secs + 60s`
/// margin, each floored at its own historical constant so raising
/// `script_timeout_secs` (e.g. toward the 600s in `docs/LIMITS_AUDIT.md`
/// §3.4) can never silently re-open the "hidden glass ceiling" the outer
/// safety net used to hit first (§2.4 / §5 Phase 2.1), and an unconfigured
/// deployment never gets a *smaller* safety net than before. Call once,
/// before serving; a second call is a silent no-op.
pub fn init_bridge_timeouts(script_timeout_secs: u64) {
    let _ = DERIVED_BRIDGE_TIMEOUT.set(derive_timeout(script_timeout_secs, BRIDGE_TIMEOUT));
    let _ = DERIVED_PROXY_TIMEOUT.set(derive_timeout(script_timeout_secs, PROXY_TIMEOUT));
}

/// `script_timeout_secs + 60s` margin, floored at `floor`. Pure so
/// [`init_bridge_timeouts`]'s arithmetic is testable without touching the
/// process-wide `OnceLock`s it feeds.
fn derive_timeout(script_timeout_secs: u64, floor: Duration) -> Duration {
    Duration::from_secs(script_timeout_secs.saturating_add(60)).max(floor)
}

fn effective_bridge_timeout() -> Duration {
    DERIVED_BRIDGE_TIMEOUT
        .get()
        .copied()
        .unwrap_or(BRIDGE_TIMEOUT)
}

fn effective_proxy_timeout() -> Duration {
    DERIVED_PROXY_TIMEOUT
        .get()
        .copied()
        .unwrap_or(PROXY_TIMEOUT)
}

/// Soft-cap on `inspect` `paths[]` (bridge enforces; mirrored in docs).
pub const INSPECT_PATHS_LIMIT: usize = 256;

/// Soft-cap on each inspect node's direct-child roster.
pub const CHILDREN_ROSTER_LIMIT: usize = 256;

/// Soft-cap on `api_help` `queries[]` (bridge enforces; mirrored in docs).
pub const API_HELP_QUERIES_LIMIT: usize = 64;

/// Soft-cap on `editor_context` per-pane selection list.
pub const EDITOR_SELECTION_LIMIT: usize = 256;

/// Soft-cap on `editor_context` panes array.
pub const EDITOR_PANES_LIMIT: usize = 64;

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
    /// TouchDesigner installations on disk (offline discovery).
    TdInstalls,
    /// Offline .toe/.tox -> expand dir via official toeexpand.
    ProjectUnpack,
    /// Expand dir -> packed .toe/.tox via official toecollapse.
    ProjectPack,
    /// OS popup list/describe/dismiss for a TD pid.
    Dialogs,
    /// Spawn TouchDesigner + deterministic handshake wait.
    SpawnTd,
    /// Kill a known TD pid (graceful→force ladder).
    KillTd,
    /// Sanity-check an expand dir / packed project.
    ProjectLint,
    /// Install/override the tdmcp bridge inside a packed project.
    ProjectInstallBridge,
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
            Self::TdInstalls => "td_installs",
            Self::ProjectUnpack => "project_unpack",
            Self::ProjectPack => "project_pack",
            Self::Dialogs => "dialogs",
            Self::SpawnTd => "spawn_td",
            Self::KillTd => "kill_td",
            Self::ProjectLint => "project_lint",
            Self::ProjectInstallBridge => "project_install_bridge",
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
                "Structural read for an explicit paths[] batch (required, non-empty; soft-capped at 256). No auto-recursion — caller chooses nodes. Empty include defaults to nodes+errors+warnings; params and content opt-in; non-empty include is an allowlist. When nodes is included, each ok node includes positional inputs/outputs peer lists ({path, name, opType} or null per connector; [] when empty). Params entries are {name, mode, val, expr?} (expr only when mode is EXPRESSION; val is evaluated and JSON-safe). Content (opt-in) returns DAT .text bodies (text+table) and GLSL shader stages by following DAT refs plus compileResult — no size cap; omit content key on non-eligible ops. DAT content also carries shader consumers[] diagnostics ({severity note|error, code tdmcp.shader.*, consumer, role, lines[]}; caps 2048 ops scanned / 64 consumers — see consumersTruncated); GLSL content carries classified compileState compiled|error. Reading compileResult forces a synchronous recompile of that consumer. Every node also returns comment (OP.comment) when non-empty — read it first, it is the operator's own account of its role (capped 1024 chars, commentTruncated when cut). Per-node summary includes a direct-child roster ({name, opType}, plus each child's comment when set — capped 160 chars); detailed adds path+family. Roster capped at 256 — when truncated see node.truncation. Bad paths return ok:false inline; siblings still succeed."
            }
            Self::MutateNodes => {
                "Ordered create/set/delete/connect/disconnect steps; sequential apply, stop on first hard error; later steps skipped (tdmcp.batch.skipped_dependent). Fix from failedAt only. create/set accept text: DAT body write (applied first; non-DAT target = hard error tdmcp.mutate.not_dat; create rolls back). create/set also accept comment: OP.comment, the node's own account of what it does and why — any family, an empty string clears it, and inspect returns it. Comment every non-obvious node you create: it is how the next agent (and the user) reads the network. After each successful text write the tool lints consuming GLSL ops and attaches per-step shaderDiagnostics[] ({severity note|error, code tdmcp.shader.*, consumer, consumerOpType, role, message, lines[]} for errors); summary adds shaderNotes/shaderErrors counts. Lint reads compileResult, forcing a synchronous recompile of each consumer; never flips ok."
            }
            Self::Capture => {
                "Perception capture. top=native TOP JPEG; preview=any family via shared bridge OP Viewer TOP; chop_data=CHOP JSON; chop_image/pop=aliases of preview; auto=TOP→top, CHOP→chop_data, else preview. maxSize is hard-capped at 1536px longer side (tdmcp.perception.max_size_too_large); null (native) is only honored when native resolution is already under the cap."
            }
            Self::ApiHelp => {
                "Live TD Python API cards (not wiki dumps). Batch queries[] (soft-cap 64): class (doc/opType/family/mro/members), classes (op-like index + family/prefix), module (td thin). No help() / no param listing — use inspect include params for .par names. Case-sensitive class names."
            }
            Self::EditorContext => {
                "Live editor context — all TD panes with type/ownerPath/focused; per-pane selection as [{path, current}] (omitted when empty). Panes soft-capped at 64; selection soft-capped at 256. Bad panes return ok:false inline; siblings still succeed. Hint for mutation zone — still verify with inspect."
            }
            Self::DescribeTools => "Manifest of available tools",
            Self::TdInstalls => {
                "List TouchDesigner installations on disk (version dirs, exe path, which official tools exist). Offline; complete=false marks stub installs. default=true on the newest usable install."
            }
            Self::ProjectUnpack => {
                "Expand a packed .toe/.tox into a directory tree via the installed official toeexpand. Success verified by filesystem evidence (dir + strict-LF toc), never by exit code. overwrite=replace stashes prior artifacts and restores them on failure. Default destination: <source>.dir beside the input."
            }
            Self::ProjectPack => {
                "Collapse an expand directory back into a packed .toe/.tox via official toecollapse (output verified non-empty). Guards against build skew between the source .build and the selected install unless allowBuildSkew=true."
            }
            Self::Dialogs => {
                "List/describe/dismiss OS popups owned by a TD pid. list returns popups+windowStatus; dismiss runs the ladder (button label/id optional, default button otherwise) and verifies the window is gone. Main chrome is protected."
            }
            Self::SpawnTd => {
                "Spawn TouchDesigner and deterministically wait for THAT pid's bridge handshake (never another instance). Registers pre-handshake so fleet shows it immediately. Non-handshake outcomes return ok:false with an `outcome` field (`wait_timeout` + stillAlive, or `exited_early` + exitCode) — not a diagnostic code. Startup popups ride along as `startupDialogs`, surfaced and never auto-dismissed: a wait_timeout carrying startupDialogs means a modal is blocking the handshake — dismiss via `dialogs`, then poll fleet for that pid rather than spawning again."
            }
            Self::KillTd => {
                "Kill a known TouchDesigner pid: graceful WM_CLOSE first (graceMs window), then mode=force as explicit opt-in. Refuses pids that are neither registered nor TouchDesigner.exe."
            }
            Self::ProjectLint => {
                "Sanity-check a project: .toc strict-LF parse + filesystem consistency, duplicate entries, and tdmcp_rs bridge DAT presence. A packed .toe/.tox is auto-expanded into a private temp staging dir (cleaned up; the input is never touched). Optionally delegates deep checks to td-cli when available."
            }
            Self::ProjectInstallBridge => {
                "Install/override the tdmcp bridge inside a packed .toe/.tox: backs up the original, rewrites the three bridge DAT bodies (bootstrap/callbacks/tdmcp_exec) with the daemon's embedded sources, verifies by targeted re-expand, then replaces atomically. When the project has no bridge, one is created from the shipped bootstrap.tox under an unambiguous host COMP (returns created:true); an ambiguous project fails tdmcp.project.bridge_subtree_missing instead of guessing. strategy defaults to force; ensure skips when payloads already match."
            }
        }
    }

    /// All tools (descriptor / parity order).
    pub const ALL: &[Self] = &[
        Self::Fleet,
        Self::ExecutePython,
        Self::Inspect,
        Self::MutateNodes,
        Self::Capture,
        Self::ApiHelp,
        Self::EditorContext,
        Self::DescribeTools,
        Self::TdInstalls,
        Self::ProjectUnpack,
        Self::ProjectPack,
        Self::Dialogs,
        Self::SpawnTd,
        Self::KillTd,
        Self::ProjectLint,
        Self::ProjectInstallBridge,
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
            "td_installs" => Some(Self::TdInstalls),
            "project_unpack" => Some(Self::ProjectUnpack),
            "project_pack" => Some(Self::ProjectPack),
            "dialogs" => Some(Self::Dialogs),
            "spawn_td" => Some(Self::SpawnTd),
            "kill_td" => Some(Self::KillTd),
            "project_lint" => Some(Self::ProjectLint),
            "project_install_bridge" => Some(Self::ProjectInstallBridge),
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
    /// Optional federated daemon id (omit for local / unique remote resolve).
    #[serde(default)]
    pub daemon_id: Option<String>,
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
pub const CAPTURE_DEFAULT_MAX_SIZE: u32 = 512;

/// Hard pre-flight reject for `maxSize` (mirrors bridge
/// `constants.CAPTURE_MAX_SIZE`) — enforced bridge-side (see `capture.py`),
/// kept here for parity/documentation only. `null` (native resolution) is
/// only honored when native is already under this cap; larger native
/// captures must be an explicit downscale instead.
#[allow(
    dead_code,
    reason = "documents the bridge-side hard cap; not enforced Rust-side, mirrors SCRIPT_MAX_BYTES"
)]
pub const CAPTURE_MAX_SIZE: u32 = 1536;

/// Args for capture (perception).
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CaptureParams {
    /// Target pid.
    pub pid: Pid,
    /// Optional federated daemon id (omit for local / unique remote resolve).
    #[serde(default)]
    pub daemon_id: Option<String>,
    /// Operator path (OpPath; relative to contextPath or /project1).
    pub path: OpPath,
    /// Capture mode.
    #[serde(default)]
    pub mode: CaptureMode,
    /// Resolution base for relative `path`.
    #[serde(default)]
    pub context_path: Option<OpPath>,
    /// Longer-side pixel cap before PNG encode. `null` = native resolution.
    /// Defaults to 512. Hard-capped at 1536 (`tdmcp.perception.max_size_too_large`);
    /// `null` is only honored when native resolution is already under the cap.
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
    /// DAT text/table bodies and GLSL shader stages (opt-in; not in empty default).
    Content,
}

/// Structural detail level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum DetailLevel {
    /// Direct-child roster as `{name, opType}` (capped at 256; see `node.truncation`).
    #[default]
    Summary,
    /// Direct-child roster as `{path, family, opType}` (same 256 cap — does not uncap).
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
    /// Optional federated daemon id (omit for local / unique remote resolve).
    #[serde(default)]
    pub daemon_id: Option<String>,
    /// Explicit operator paths to inspect (required, non-empty). Soft-capped at 256.
    /// No auto-recursion — caller chooses exactly which nodes to fetch.
    pub paths: Vec<OpPath>,
    /// Resolution base for relative entries in `paths`.
    #[serde(default)]
    pub context_path: Option<OpPath>,
    /// Sections to include. Empty/omitted = nodes+errors+warnings; params/content opt-in; non-empty = allowlist.
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
        /// DAT body write (applied before `values`; target must be a DAT, else hard error `tdmcp.mutate.not_dat`). Attaches shader-consumer diagnostics (`shaderDiagnostics`) on success.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        /// Operator comment (`OP.comment`) — the node's own one-line account of what it does and why. Any family; `""` clears it. Set one on every non-obvious node you create.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        comment: Option<String>,
        /// Plain parameter values (`.par.*` only — direct OP attributes like `display`/`viewer` go in `flags`, not here).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        values: Option<Map<String, Value>>,
        /// Direct OP attribute writes (`node.<name> = val`); allowlist = TD Common Flags subset, see CONTRACT.md.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        flags: Option<Map<String, Value>>,
    },
    /// Set text / comment / values / expressions / pulse / flags on an existing node.
    Set {
        /// Target node path.
        path: OpPath,
        /// DAT body write (applied before `values`; target must be a DAT, else hard error `tdmcp.mutate.not_dat`). Attaches shader-consumer diagnostics (`shaderDiagnostics`) on success.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        /// Operator comment (`OP.comment`) — the node's own one-line account of what it does and why. Any family; `""` clears it. Set one on every non-obvious node you create.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        comment: Option<String>,
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
    /// Optional federated daemon id (omit for local / unique remote resolve).
    #[serde(default)]
    pub daemon_id: Option<String>,
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
    /// Optional federated daemon id (omit for local / unique remote resolve).
    #[serde(default)]
    pub daemon_id: Option<String>,
    /// Batch of API card / index queries (required, non-empty). Soft-capped at 64.
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
    /// Shared session registry holding `(session, daemon_scope, pid)` in-flight slots.
    pub sessions: &'a McpSessionRegistry,
}

/// Dispatch a named tool call to a JSON result.
///
/// Never holds the registry lock across a bridge await.
/// Pass [`Some`] [`SessionGate`] from the rmcp handler; JSON fallback passes [`None`]
/// (pid exclusive still applies; session chill is skipped).
///
/// When `federation` is set, bridged tools may proxy to a registered slave.
pub async fn dispatch_tool(
    registry: &Arc<Mutex<PidRegistry>>,
    catalog: &tdmcp_diagnostics::Catalog,
    bridge: &dyn BridgeRpc,
    name: &str,
    args: Value,
    session: Option<SessionGate<'_>>,
    federation: Option<&FederationCtx>,
) -> Result<Value, ToolCallError> {
    // One boundary record per call — the only per-call trace agents and the
    // tray Logs view ever see (observability spec §5.7 rule 3).
    let started = std::time::Instant::now();
    let result =
        dispatch_tool_inner(registry, catalog, bridge, name, args, session, federation).await;
    let elapsed_ms = started.elapsed().as_millis() as u64;
    match &result {
        Ok(_) => tracing::info!(tool = %name, elapsed_ms, "tool call complete"),
        Err(ToolCallError::UnknownTool(_)) => {
            tracing::warn!(tool = %name, elapsed_ms, "tool call failed: unknown tool")
        }
        Err(ToolCallError::Failed(fail)) => {
            let code = fail
                .diagnostics
                .items
                .first()
                .map(|i| i.code.as_str())
                .unwrap_or("unknown");
            tracing::warn!(tool = %name, code, elapsed_ms, "tool call failed");
        }
    }
    result
}

async fn dispatch_tool_inner(
    registry: &Arc<Mutex<PidRegistry>>,
    catalog: &tdmcp_diagnostics::Catalog,
    bridge: &dyn BridgeRpc,
    name: &str,
    args: Value,
    session: Option<SessionGate<'_>>,
    federation: Option<&FederationCtx>,
) -> Result<Value, ToolCallError> {
    let tool =
        ToolName::from_wire(name).ok_or_else(|| ToolCallError::UnknownTool(name.to_owned()))?;
    match tool {
        ToolName::Fleet => {
            let params: FleetParams = parse_args(catalog, tool, args)?;
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
            let local = {
                let reg = registry.lock().await;
                let dialogs_popups = crate::dialogs::get()
                    .map(|d| d.snapshots.lock().unwrap_or_else(|p| p.into_inner()));
                fleet_summary(&reg, &params, &ipc_depths, dialogs_popups.as_deref())
            };
            if let Some(fed) = federation {
                Ok(
                    serde_json::to_value(federated_fleet_summary(fed, local).await)
                        .map_err(|e| serialize_failed(catalog, tool, "fleet summary", &e))?,
                )
            } else {
                Ok(serde_json::to_value(local)
                    .map_err(|e| serialize_failed(catalog, tool, "fleet summary", &e))?)
            }
        }
        ToolName::DescribeTools => Ok(serde_json::json!({ "tools": tool_descriptors() })),
        ToolName::TdInstalls => {
            // Absent args arrive as null; the tool takes none.
            let args = if args.is_null() {
                serde_json::json!({})
            } else {
                args
            };
            let _params: crate::td_installs::TdInstallsParams = parse_args(catalog, tool, args)?;
            Ok(crate::td_installs::run(&tdmcp_projectio::resolve::std_env))
        }
        ToolName::ProjectUnpack => {
            // Typed parse for arg-shape errors; run() re-parses the same value.
            let _params: crate::project_unpack::ProjectUnpackParams =
                parse_args(catalog, tool, args.clone())?;
            crate::project_unpack::run(args)
                .map_err(|e| coded_failure(catalog, tool, e.code, "sourcePath", e.message))
        }
        ToolName::ProjectPack => {
            let _params: crate::project_pack::ProjectPackParams =
                parse_args(catalog, tool, args.clone())?;
            crate::project_pack::run(args)
                .map_err(|e| coded_failure(catalog, tool, e.code, "srcDir", e.message))
        }
        ToolName::Dialogs => {
            let params: crate::dialogs_tool::DialogsParams = parse_args(catalog, tool, args)?;
            crate::dialogs_tool::run(params)
                .map_err(|e| coded_failure(catalog, tool, e.0, "pid", e.1))
        }
        ToolName::SpawnTd => {
            // All-optional args: absent arrives as null.
            let args = if args.is_null() {
                serde_json::json!({})
            } else {
                args
            };
            let params: crate::lifecycle::SpawnTdParams = parse_args(catalog, tool, args.clone())?;
            let cfg = tdmcp_config::load(&tdmcp_config::default_config_path()).map_err(|e| {
                coded_failure(
                    catalog,
                    tool,
                    "spawn.spawn_failed",
                    "exePath",
                    format!("config load: {e}"),
                )
            })?;
            crate::lifecycle::spawn_td(
                registry,
                &cfg,
                params.exe_path.as_deref(),
                params.install_id.as_deref(),
                params.project_path.as_deref(),
                &params.args,
                params.wait_timeout_ms,
            )
            .await
            .map_err(|e| {
                let code = match e.code {
                    "spawn.exe_incomplete" => tdmcp_diagnostics::codes::SPAWN_EXE_INCOMPLETE,
                    "spawn.spawn_failed" => tdmcp_diagnostics::codes::SPAWN_FAILED,
                    "spawn.wait_timeout" => tdmcp_diagnostics::codes::SPAWN_WAIT_TIMEOUT,
                    _ => tdmcp_diagnostics::codes::SPAWN_WAIT_TIMEOUT,
                };
                coded_failure(catalog, tool, code, "exePath", e.message)
            })
        }
        ToolName::KillTd => {
            let params: crate::lifecycle::KillTdParams = parse_args(catalog, tool, args)?;
            let source = crate::dialogs::get().map(|d| d.source.as_ref());
            crate::lifecycle::kill_td(registry, source, params.pid, params.mode, params.grace_ms)
                .await
                .map_err(|e| {
                    let code = match e.code {
                        "kill.not_td_pid" => tdmcp_diagnostics::codes::KILL_NOT_TD_PID,
                        "kill.graceful_timeout" => tdmcp_diagnostics::codes::KILL_GRACEFUL_TIMEOUT,
                        _ => tdmcp_diagnostics::codes::KILL_ACCESS_DENIED,
                    };
                    coded_failure(catalog, tool, code, "pid", e.message)
                })
        }
        ToolName::ProjectLint => {
            let params: crate::project_lint::ProjectLintParams = parse_args(catalog, tool, args)?;
            let target = std::path::PathBuf::from(&params.target_path);
            // Only pay the config+tool-scan cost when the target is packed.
            let looks_packed =
                !target.is_dir() && tdmcp_projectio::sniff::sniff_packed(&target).is_ok();
            let tools = if looks_packed {
                let cfg =
                    tdmcp_config::load(&tdmcp_config::default_config_path()).map_err(|e| {
                        coded_failure(
                            catalog,
                            tool,
                            "project.io_failed",
                            "targetPath",
                            format!("config load: {e}"),
                        )
                    })?;
                Some(
                    crate::project_unpack::resolve_official_tools(&cfg, None).map_err(|e| {
                        coded_failure(catalog, tool, e.code, "targetPath", e.message)
                    })?,
                )
            } else {
                None
            };
            Ok(crate::project_lint::run(&params, None, tools.as_ref()))
        }
        ToolName::ProjectInstallBridge => {
            let params: crate::project_install::ProjectInstallBridgeParams =
                parse_args(catalog, tool, args.clone())?;
            let cfg = tdmcp_config::load(&tdmcp_config::default_config_path()).map_err(|e| {
                coded_failure(
                    catalog,
                    tool,
                    "project.io_failed",
                    "targetPath",
                    format!("config load: {e}"),
                )
            })?;
            let tools = crate::project_unpack::resolve_official_tools(&cfg, None)
                .map_err(|e| coded_failure(catalog, tool, e.code, "targetPath", e.message))?;
            crate::project_install::run(&params, &tools)
                .map_err(|e| coded_failure(catalog, tool, e.1, "targetPath", e.0))
        }
        ToolName::ExecutePython => {
            let params: ExecutePythonParams = parse_args(catalog, tool, args.clone())?;
            if let ControlFlow::Break(v) = maybe_proxy_bridged(
                federation,
                registry,
                catalog,
                "execute_python",
                args,
                params.daemon_id.as_deref(),
                params.pid,
                session,
            )
            .await?
            {
                return Ok(v);
            }
            let _slot = begin_session_slot(
                session,
                catalog,
                "execute_python",
                DAEMON_SCOPE_LOCAL,
                params.pid,
            )?;
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
            let params: CaptureParams = parse_args(catalog, tool, args.clone())?;
            if let ControlFlow::Break(v) = maybe_proxy_bridged(
                federation,
                registry,
                catalog,
                "capture",
                args,
                params.daemon_id.as_deref(),
                params.pid,
                session,
            )
            .await?
            {
                return Ok(v);
            }
            let _slot =
                begin_session_slot(session, catalog, "capture", DAEMON_SCOPE_LOCAL, params.pid)?;
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
            let params: InspectParams = parse_args(catalog, tool, args.clone())?;
            if params.paths.is_empty() {
                return Err(coded_failure(
                    catalog,
                    tool,
                    codes::OP_PATHS_REQUIRED,
                    "paths",
                    "inspect requires a non-empty paths array",
                ));
            }
            if let ControlFlow::Break(v) = maybe_proxy_bridged(
                federation,
                registry,
                catalog,
                "inspect",
                args,
                params.daemon_id.as_deref(),
                params.pid,
                session,
            )
            .await?
            {
                return Ok(v);
            }
            let _slot =
                begin_session_slot(session, catalog, "inspect", DAEMON_SCOPE_LOCAL, params.pid)?;
            let method = BridgeMethod::Inspect;
            let include: Vec<&str> = params
                .include
                .iter()
                .map(|i| match i {
                    InspectInclude::Nodes => "nodes",
                    InspectInclude::Params => "params",
                    InspectInclude::Errors => "errors",
                    InspectInclude::Warnings => "warnings",
                    InspectInclude::Content => "content",
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
            let params: MutateNodesParams = parse_args(catalog, tool, args.clone())?;
            if let ControlFlow::Break(v) = maybe_proxy_bridged(
                federation,
                registry,
                catalog,
                "mutate_nodes",
                args,
                params.daemon_id.as_deref(),
                params.pid,
                session,
            )
            .await?
            {
                return Ok(v);
            }
            let _slot = begin_session_slot(
                session,
                catalog,
                "mutate_nodes",
                DAEMON_SCOPE_LOCAL,
                params.pid,
            )?;
            let method = BridgeMethod::MutateNodes;
            let steps = serde_json::to_value(&params.steps)
                .map_err(|e| serialize_failed(catalog, tool, "mutate steps", &e))?;
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
            let params: ApiHelpParams = parse_args(catalog, tool, args.clone())?;
            if params.queries.is_empty() {
                return Err(coded_failure(
                    catalog,
                    tool,
                    codes::API_HELP_QUERIES_REQUIRED,
                    "queries",
                    "api_help requires a non-empty queries array",
                ));
            }
            if let ControlFlow::Break(v) = maybe_proxy_bridged(
                federation,
                registry,
                catalog,
                "api_help",
                args,
                params.daemon_id.as_deref(),
                params.pid,
                session,
            )
            .await?
            {
                return Ok(v);
            }
            let _slot =
                begin_session_slot(session, catalog, "api_help", DAEMON_SCOPE_LOCAL, params.pid)?;
            let queries = serde_json::to_value(&params.queries)
                .map_err(|e| serialize_failed(catalog, tool, "api_help queries", &e))?;
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
            let params: EditorContextParams = parse_args(catalog, tool, args.clone())?;
            if let ControlFlow::Break(v) = maybe_proxy_bridged(
                federation,
                registry,
                catalog,
                "editor_context",
                args,
                params.daemon_id.as_deref(),
                params.pid,
                session,
            )
            .await?
            {
                return Ok(v);
            }
            let _slot = begin_session_slot(
                session,
                catalog,
                "editor_context",
                DAEMON_SCOPE_LOCAL,
                params.pid,
            )?;
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

async fn federated_fleet_summary(fed: &FederationCtx, local: FleetResponse) -> FleetResponse {
    let tagged: Vec<AggregatedFleetProcess> = local
        .processes
        .iter()
        .map(|p| AggregatedFleetProcess {
            pid: p.pid.get(),
            title: p.title.clone(),
            toe_path: p.toe_path.clone(),
            bridge: p.bridge,
            daemon_id: Some(fed.local_daemon_id.clone()),
            hostname: Some(fed.local_hostname.clone()),
        })
        .collect();
    let mut slaves = fed.slaves.lock().await;
    slaves.tick_stale(Utc::now());
    let aggregated = slaves.aggregate_fleet(&fed.local_daemon_id, &fed.local_hostname, tagged);
    let mut by_pid: std::collections::HashMap<u32, FleetProcess> = local
        .processes
        .into_iter()
        .map(|mut p| {
            p.daemon_id = Some(fed.local_daemon_id.as_str().to_owned());
            p.hostname = Some(fed.local_hostname.clone());
            (p.pid.get(), p)
        })
        .collect();
    let mut processes = Vec::with_capacity(aggregated.len());
    for row in aggregated {
        let is_local =
            row.daemon_id.as_ref().map(DaemonId::as_str) == Some(fed.local_daemon_id.as_str());
        if is_local {
            if let Some(local_row) = by_pid.remove(&row.pid) {
                processes.push(local_row);
                continue;
            }
        }
        processes.push(FleetProcess {
            pid: Pid::new(row.pid),
            title: row.title,
            window_status: None,
            toe_path: row.toe_path,
            bridge: row.bridge,
            // Spawn provenance is daemon-local; remote rows carry none.
            spawn: None,
            // Popups are daemon-local too.
            popups: Vec::new(),
            tasks: None,
            ipc_queue_depth: None,
            resurrected: false,
            last_disconnect_at: None,
            cancelled_tasks: Vec::new(),
            daemon_id: row.daemon_id.map(DaemonId::into_inner),
            hostname: row.hostname,
        });
    }
    FleetResponse { processes }
}

/// Resolve whether a bridged call should run locally or be proxied to a slave.
///
/// [`ControlFlow::Continue`] → run locally. [`ControlFlow::Break`] → proxied result.
#[allow(clippy::too_many_arguments, reason = "proxy dispatch wiring")]
async fn maybe_proxy_bridged(
    federation: Option<&FederationCtx>,
    registry: &Arc<Mutex<PidRegistry>>,
    catalog: &tdmcp_diagnostics::Catalog,
    tool_name: &str,
    mut args: Value,
    daemon_id: Option<&str>,
    pid: Pid,
    session: Option<SessionGate<'_>>,
) -> Result<ControlFlow<Value>, ToolCallError> {
    let Some(fed) = federation else {
        return Ok(ControlFlow::Continue(()));
    };
    let local_id = fed.local_daemon_id.as_str();

    let target_slave_id = if let Some(id) = daemon_id {
        if id == local_id {
            return Ok(ControlFlow::Continue(()));
        }
        Some(id.to_owned())
    } else {
        let local_has = {
            let reg = registry.lock().await;
            reg.get(pid.get()).is_some()
        };
        let resolve = {
            let slaves = fed.slaves.lock().await;
            slaves.resolve_pid(pid.get())
        };
        match (local_has, resolve) {
            (true, PidResolve::Unique(remote_id)) => {
                let slaves = fed.slaves.lock().await;
                let remote_host = slaves
                    .get(&remote_id)
                    .map(|e| e.hostname.clone())
                    .unwrap_or_default();
                return Err(ambiguous_pid(
                    catalog,
                    tool_name,
                    pid,
                    &[
                        (fed.local_daemon_id.clone(), fed.local_hostname.clone()),
                        (remote_id, remote_host),
                    ],
                ));
            }
            (true, PidResolve::Ambiguous(mut hits)) => {
                hits.insert(0, (fed.local_daemon_id.clone(), fed.local_hostname.clone()));
                return Err(ambiguous_pid(catalog, tool_name, pid, &hits));
            }
            (false, PidResolve::Ambiguous(hits)) => {
                return Err(ambiguous_pid(catalog, tool_name, pid, &hits));
            }
            (false, PidResolve::Unique(remote_id)) => Some(remote_id.into_inner()),
            (true, PidResolve::Local) => {
                return Ok(ControlFlow::Continue(()));
            }
            (false, PidResolve::Local) => {
                // Fall through to local unknown_pid path.
                return Ok(ControlFlow::Continue(()));
            }
        }
    };

    let Some(slave_id_str) = target_slave_id else {
        return Ok(ControlFlow::Continue(()));
    };
    let slave_id = DaemonId::new(slave_id_str.clone());

    let (base_url, auth_token) = {
        let slaves = fed.slaves.lock().await;
        let Some(entry) = slaves.get(&slave_id) else {
            return Err(slave_unreachable(catalog, tool_name, pid, &slave_id_str));
        };
        if entry.reachability == SlaveReachability::Unreachable {
            return Err(slave_unreachable(catalog, tool_name, pid, &slave_id_str));
        }
        (entry.base_url.clone(), entry.auth_token.clone())
    };

    let _slot = begin_session_slot(session, catalog, tool_name, &slave_id_str, pid)?;

    if let Some(obj) = args.as_object_mut() {
        obj.remove("daemonId");
    }

    let url = format!("{}/mcp/tools/call", base_url.trim_end_matches('/'));
    let body = serde_json::json!({
        "name": tool_name,
        "arguments": args,
    });
    let mut req = fed
        .http
        .post(&url)
        .json(&body)
        .timeout(effective_proxy_timeout());
    if !auth_token.is_empty() {
        req = req.bearer_auth(&auth_token);
    }

    let resp = match req.send().await {
        Ok(r) => r,
        Err(_) => {
            return Err(slave_unreachable(catalog, tool_name, pid, &slave_id_str));
        }
    };
    if !resp.status().is_success() {
        return Err(slave_unreachable(catalog, tool_name, pid, &slave_id_str));
    }
    let body: Value = match resp.json().await {
        Ok(v) => v,
        Err(_) => {
            return Err(slave_unreachable(catalog, tool_name, pid, &slave_id_str));
        }
    };

    if body.get("ok") == Some(&Value::Bool(true)) {
        let mut data = body.get("data").cloned().unwrap_or(Value::Null);
        inject_routed(&mut data);
        return Ok(ControlFlow::Break(data));
    }

    // Soft/hard tool failure from slave — return as Failed preserving diagnostics.
    Err(proxy_slave_tool_failed(catalog, tool_name, pid, body))
}

fn inject_routed(value: &mut Value) {
    if let Some(obj) = value.as_object_mut() {
        obj.insert("routed".into(), Value::Bool(true));
    }
}

fn proxy_slave_tool_failed(
    catalog: &tdmcp_diagnostics::Catalog,
    tool: &str,
    pid: Pid,
    body: Value,
) -> ToolCallError {
    use crate::outcomes::{build_diag, failed_one_with_image_and_data};
    use tdmcp_diagnostics::{DiagnosticContext, DiagnosticLayer, DiagnosticSpan};

    let code = body
        .pointer("/items/0/code")
        .and_then(Value::as_str)
        .unwrap_or(tdmcp_diagnostics::codes::FEDERATION_SLAVE_UNREACHABLE);
    let message = body
        .get("summary")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            body.pointer("/items/0/message")
                .and_then(Value::as_str)
                .map(str::to_owned)
        });
    let item = build_diag(
        catalog,
        code,
        DiagnosticSpan {
            tool: tool.into(),
            mutation_index: None,
            field: None,
            line: None,
            column: None,
            snippet: None,
        },
        message,
        DiagnosticContext {
            pid: Some(pid.get()),
            op_path: None,
            context_path: None,
            logs: None,
        },
        DiagnosticLayer::Fleet,
    );
    failed_one_with_image_and_data(item, None, Some(body))
}

/// D3 interception gate: fail fast while an OS modal wedges the pid's main
/// thread (`[dialogs].intercept`). Fail-open on any probe problem.
fn dialogs_blocking_gate(
    catalog: &tdmcp_diagnostics::Catalog,
    tool: &str,
    pid: u32,
) -> Result<(), ToolCallError> {
    let Some(d) = crate::dialogs::get() else {
        return Ok(());
    };
    if !d.intercept {
        return Ok(());
    }
    let cached = d
        .snapshots
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(&pid)
        .cloned();
    // Cache miss -> one bounded refresh (snapshot budget lives in the backend).
    let snap = match cached {
        Some(s) => Some(s),
        None => Some(d.source.snapshot(pid)),
    };
    if let Some(snap) = snap {
        if !snap.popups.is_empty() {
            let list: Vec<String> = snap
                .popups
                .iter()
                .map(|p| format!("[{:?}] {}", p.severity, p.title))
                .collect();
            return Err(coded_failure(
                catalog,
                ToolName::from_wire(tool).unwrap_or(ToolName::Fleet),
                tdmcp_diagnostics::codes::DIALOG_BLOCKING,
                "pid",
                format!(
                    "{} modal popup(s) block TD's main thread: {} — run dialogs {{pid}} action=list, dismiss via action=dismiss",
                    snap.popups.len(),
                    list.join("; ")
                ),
            ));
        }
    }
    Ok(())
}

fn begin_session_slot<'a>(
    session: Option<SessionGate<'a>>,
    catalog: &tdmcp_diagnostics::Catalog,
    tool: &str,
    daemon_scope: &str,
    pid: Pid,
) -> Result<Option<BridgeCallSlot<'a>>, ToolCallError> {
    // Interception first: a modal wedges every main-thread dispatch.
    dialogs_blocking_gate(catalog, tool, pid.get())?;
    let Some(gate) = session else {
        return Ok(None);
    };
    match gate
        .sessions
        .try_begin_bridge_call(gate.session_id, daemon_scope, pid.get())
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
    match tokio::time::timeout(effective_bridge_timeout(), call).await {
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
                budget_ms: effective_bridge_timeout().as_millis() as u64,
            })
        }
    }
}

async fn clear_queue_keep_connected(registry: &Arc<Mutex<PidRegistry>>, pid: u32) {
    let mut reg = registry.lock().await;
    let _ = reg.cancel_queue_keep_connected(pid);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod timeout_tests {
    use super::{derive_timeout, BRIDGE_TIMEOUT, PROXY_TIMEOUT};
    use std::time::Duration;

    #[test]
    fn derive_timeout_adds_margin_over_the_configured_script_budget() {
        // 600s (docs/LIMITS_AUDIT.md §3.4 proposed script_timeout_secs) + 60s
        // margin must clear the historical 180s/130s floors comfortably.
        assert_eq!(
            derive_timeout(600, BRIDGE_TIMEOUT),
            Duration::from_secs(660)
        );
        assert_eq!(derive_timeout(600, PROXY_TIMEOUT), Duration::from_secs(660));
    }

    #[test]
    fn derive_timeout_never_drops_below_the_historical_floor() {
        // A short/unconfigured script_timeout_secs must not shrink the
        // safety net below what it's always been.
        assert_eq!(derive_timeout(10, BRIDGE_TIMEOUT), Duration::from_secs(180));
        assert_eq!(derive_timeout(10, PROXY_TIMEOUT), Duration::from_secs(130));
    }

    #[test]
    fn derive_timeout_matches_the_default_script_timeout_at_the_floor() {
        // 120s default + 60s margin == 180s == the historical BRIDGE_TIMEOUT
        // const exactly — the derivation must not regress today's behavior.
        assert_eq!(
            derive_timeout(120, BRIDGE_TIMEOUT),
            Duration::from_secs(180)
        );
    }
}

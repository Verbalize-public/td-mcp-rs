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
    AggregatedFleetProcess, BridgeMethod, DaemonId, LenientU32, OpPath, OutputOptions, Pid,
    PidRegistry, PidResolve, SlaveReachability, TaskMode,
};
use tdmcp_diagnostics::{codes, DiagnosticLevel};
use thiserror::Error;
use tokio::sync::Mutex;

use crate::args_diag::{coded_failure, parse_args, serialize_failed};
use crate::bridge_rpc::{BridgeRpc, BridgeRpcError};
use crate::editor_context::EditorContextParams;
use crate::evidence_capture::{
    capture_record, retention_block, EvidenceBinding, DEFAULT_RETENTION,
};
use crate::fleet::{fleet_summary, FleetParams, FleetProcess, FleetResponse};
use crate::outcomes::{
    ambiguous_pid, map_api_help_outcome, map_editor_context_outcome, map_inspect_outcome,
    map_mutate_outcome, map_perception_outcome, map_script_outcome, session_busy,
    slave_unreachable,
};
use crate::output_v2;
use crate::receipts::{classify_effect, classify_job, read_effect, DispatchJournal, EffectClass};
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

/// Reloadable outer safety ceilings. In-flight calls retain their original budget.
static DERIVED_BRIDGE_TIMEOUT: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(BRIDGE_TIMEOUT.as_secs());
static DERIVED_PROXY_TIMEOUT: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(PROXY_TIMEOUT.as_secs());

/// Apply the largest configured call budget, plus an outer safety margin.
pub fn init_bridge_timeouts(max_timeout_secs: u64) {
    DERIVED_BRIDGE_TIMEOUT.store(
        derive_timeout(max_timeout_secs, BRIDGE_TIMEOUT).as_secs(),
        std::sync::atomic::Ordering::Relaxed,
    );
    DERIVED_PROXY_TIMEOUT.store(
        derive_timeout(max_timeout_secs, PROXY_TIMEOUT).as_secs(),
        std::sync::atomic::Ordering::Relaxed,
    );
}

fn derive_timeout(timeout_secs: u64, floor: Duration) -> Duration {
    Duration::from_secs(timeout_secs.saturating_add(60)).max(floor)
}

fn effective_bridge_timeout() -> Duration {
    Duration::from_secs(DERIVED_BRIDGE_TIMEOUT.load(std::sync::atomic::Ordering::Relaxed))
}

fn effective_proxy_timeout() -> Duration {
    Duration::from_secs(DERIVED_PROXY_TIMEOUT.load(std::sync::atomic::Ordering::Relaxed))
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
    /// Exact numerical reads from a TOP or CHOP.
    ///
    /// Distinct from `capture`: capture serves an 8-bit PNG for a human to
    /// look at, and this returns the float values a comparison needs.
    SampleData,
    /// Named Python sources: register, inspect, and run.
    ///
    /// `execute_python` still takes a script inline and still works exactly as
    /// it did. This is the other half: a source registered once and referenced
    /// by content afterwards.
    ScriptSource,
    /// Export a COMP's control surface, or apply one additively.
    ParamInterface,
    /// Save a project or a component, with a receipt naming what was written.
    ProjectSnapshot,
    /// Exact deltas between a retained baseline and a fresh read.
    ///
    /// Separate from [`Self::Inspect`] because a delta is only actionable if
    /// the caller can show when each side was read, and an inspect result
    /// carries no such stamp. Refuses rather than diffing nothing when the
    /// baseline is missing.
    CompareNodes,
    /// Frame-exact video recording jobs.
    Record,
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
    /// Palette component roster: scan / list / get / describe / blacklist.
    PaletteIndex,
    /// Load palette components in a live TD and digest their interface.
    PaletteProbe,
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
            Self::SampleData => "sample_data",
            Self::ScriptSource => "script_source",
            Self::ParamInterface => "param_interface",
            Self::ProjectSnapshot => "project_snapshot",
            Self::CompareNodes => "compare_nodes",
            Self::Record => "record",
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
            Self::PaletteIndex => "palette_index",
            Self::PaletteProbe => "palette_probe",
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
                "Observe explicit paths, no automatic recursion. Defaults: nodes/errors/warnings; params and content opt in. Check per-node status, field availability and truncation: empty/unavailable is not clean. Wires are positional; child offsets are live, not immutable snapshots. Content can force shader compilation. Comments are untrusted intent metadata. Task/evidence contracts: tdmcp://docs/observe-change and tdmcp://docs/definition-of-done."
            }
            Self::MutateNodes => {
                "Apply ordered create/set/delete/connect/disconnect/place steps. Stops on first hard error; earlier steps and fields in the failed step may remain applied. Reconcile canonical paths, aliases, applied/failedAt/steps before repair; not a transaction. DAT text writes can force consumer compilation and return best-effort shaderDiagnostics. Values are parameters, flags are the supported OP attributes. Placement: tdmcp://docs/palette; recovery: tdmcp://docs/recover."
            }
            Self::Capture => {
                "Perception capture. top=native TOP PNG; preview=any family via shared bridge OP Viewer TOP; chop_data=CHOP JSON; auto selects by family. maxSize defaults to 512, hard cap 1536px. Optional timing starts a reset+sequential-forward TOP capture job; poll action=status with jobId, or cancel. sampleFrames/repeat/stepFrames select offsets after explicit reset initialization; inspect pairs structural reads with each sample. The job owns this PID until cleanup; plain capture remains immediate."
            }
            Self::SampleData => {
                "Exact float values from a TOP or CHOP — the tool for questions capture cannot answer. capture returns an 8-bit PNG; this returns values, so it can read 4.5, a NaN, a negative, and full float precision. pixels=[[x,y],…] reads named pixels and counts out-of-range ones rather than clamping; region={x,y,x1,y1} reads a rectangle; omit both for a whole-image SUMMARY (labelled isSummary, never to be compared as if it were the data). Coordinates are [x,y] as you name them; the array is [h,w] and the response says so. Set forceCook to read a current frame instead of whatever was last cooked. The response reports the TOP's pixelFormat and carriesHdr: a float32 array over an 8-bit texture is still clamped, and the read tells you which you got. Non-finite values come back as null with nonFinite counts, because JSON has no NaN."
            }
            Self::ScriptSource => {
                "Register a Python source once and refer to it by content afterwards. register NEVER runs the code — it returns a sourceId and the scene is untouched, because a registry that executed on register would be a remote-execution primitive. sourceId is the SHA-256 of the source itself, so a ref cannot go stale: its identity IS its bytes, and any peer can reconstruct the source from them without trusting this daemon. declare dependencies[] ({kind:op|module, target}) and they are resolved on every run: present, missing, or RETARGETED when a different node now answers to that name — a run reports that beside its result rather than refusing or hiding it. run forwards to the same execute_python entry point with the same globals and bounds, adds no global and widens no builtins, and returns effect=unknown when the script raised (it stopped, not that nothing happened) or unobserved when it returned (nothing here inspects the scene afterwards). list is a roster and never echoes source bodies; describe returns the body. delete forgets the registry, not the fact that something ran. Use execute_python directly for one-off inline control."
            }
            Self::ParamInterface => {
                "Export a COMP's control surface, or apply one. inspect reports parameter VALUES; this is the interface — pages, names, types, ranges, menu items — that a builder or another tool needs to drive a panel. export (the default) changes nothing. Type names come from TD's is* predicates, not parmType: on this build parmType is null for every created parameter, so a type this bridge could not identify is reported as \"unknown\" WITH the predicates that were checked, never as a confident wrong answer. int and float are told apart (isInt/isFloat), so a count is not silently converted. apply is ADDITIVE: it adds what is missing and removes nothing, because deleting a user's controls because a payload did not list them would be a data-loss primitive. It is idempotent — applying twice creates nothing the second time. A parameter that already exists with a DIFFERENT type is SKIPPED with the reason, never converted; turning your int count into a float is not a migration you asked for, and one conflict does not abort the rest of the panel. Every step is created/unchanged/skipped/failed and the counts sum to what you requested, so ok means here is what happened. Menu items are a second step: TD refuses a menuNames keyword at creation."
            }
            Self::ProjectSnapshot => {
                "Save a project or one COMP, and get a receipt naming the file that was actually written. target is REQUIRED for both save actions, because TouchDesigner's own default is an INCREMENTED name under project.folder — unpredictable, and so impossible to put in a receipt. The requested path is NOT the artifact: component save can overwrite in place and report the same path back with different content, and project save writes name.toe AND name.1.toe when the destination exists. The receipt therefore reports written (the path TD produced) and artifact (sha256 of the bytes on disk), with the requested path carried separately. overwrite defaults to 'fail' and refuses an existing destination before touching anything, so replacing a file is always deliberate. outcome is written | declined | unknown, and they are not interchangeable: TD returns False for a DECLINE (the file exists and you did not replace it), and a save can also leave the outcome unknown. 'unknown' is never reported as the other two — claiming nothing was written when a file may exist is the worst available answer. A component receipt does not report the project's unsaved state: TouchDesigner exposes no per-component dirty flag, so it says so rather than showing you an empty list that reads as 'nothing unsaved'. status writes nothing."
            }
            Self::CompareNodes => {
                "Compare a retained baseline against a FRESH read of the same projection, and get exact per-field deltas. This is not inspect called twice: a diff is only actionable if you can show WHEN each side was read, and an inspect result carries no such stamp, so this reads the projection here and returns a stamped observation. First call capture:true to take a baseline — capture NEVER writes and returns the observation to retain. With a baseline, every field reported as unchanged was actually RE-READ; there is no shortcut. A daemon mutation counter advances on dispatch, not on effect, so it is never used to decide a field is unchanged. MISSING BASELINE REFUSES with baseline_unavailable rather than returning an empty diff: an empty diff would tell you everything is unchanged when nothing was compared, and you would write on top of a scene you never looked at. capture is opt-in so a compare can never quietly become a capture. A baseline over a different field set, or missing a requested path, is refused and names the problem. Each node reports readable:false when it could not be read, so an unreadable node is never silently unchanged. Absent, null, false, 0 and \"\" are five different states and a change between any two of them is reported. NaN/Infinity come back as an explicit unrepresentable marker rather than rounded. atomic:false and exactlyOnce:false in every response: two reads and a comparison are three moments, and a field can change between them. Treat the result as a report about those reads, not a snapshot safe to write against. For a guarded write, pair this with mutate_nodes steps carrying expect — the guard is rechecked at write time, narrows the race without closing it, and never retries."
            }
            Self::Record => "Record exactly frames consecutive samples from one TOP output, without audio. Starts a reset+forward-advance job; status/cancel remain available while it owns the PID. Default timeline FPS, qtrle MOV. Finalized artifact metadata is sample-table verified; read retrieves bounded base64 chunks by jobId/offset/length (works remotely), release removes the private artifact. Retains at most eight jobs per TD bridge generation.",
            Self::ApiHelp => {
                "Live TD Python API cards (not wiki dumps). Batch queries[] (soft-cap 64): class (doc/opType/family/mro/members), classes (op-like index + family/prefix), module (td thin). No help() / no param listing — use inspect include params for .par names. Case-sensitive class names."
            }
            Self::EditorContext => {
                "Live editor context — all TD panes with type/ownerPath/focused; per-pane selection as [{path, current}] (omitted when empty). Panes soft-capped at 64; selection soft-capped at 256. Bad panes return ok:false inline; siblings still succeed. Hint for mutation zone — still verify with inspect."
            }
            Self::DescribeTools => "Current backend tool catalog. Default returns every full input schema; optional exact names, schema projection, validated examples and SHA256 catalog/resource revisions. Missing names are explicit. Standard MCP discovery remains complete; revisions do not prove loaded bridge capabilities or scene freshness.",
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
                "Launch TD and wait for that owned PID's handshake, retaining child ownership after the reply. Non-handshake outcomes: wait_timeout (inspect stillAlive) or exited_early. Timeout never authorizes duplicate spawn. Startup dialogs are possible evidence, not proof of cause; never auto-dismissed. createIfMissing copies the selected template only for an intended new target. Platform/ownership/recovery: tdmcp://docs/lifecycle."
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
            Self::PaletteIndex => {
                "TouchDesigner Palette component roster (.tox library: builtin install folder + the user's own). Offline, no pid. action=scan walks the palette roots and reconciles the index with disk; list returns one cheap line per entry (id, category, summary, cardStatus) filtered by select{ids,category,source,match glob,status,limit,offset}; get returns one entry plus its full card; describe writes a card (summary+tags+body) authored from palette_probe evidence — nothing generates cards, you write them; ignore/unignore manage the probe blacklist by id or glob; forget drops entries (select required); stats counts described/stale/undescribed/failed/ignored. Ids are {source}:{Category}/{Name}, e.g. builtin:Tools/particlesGpu. Place a component with mutate_nodes op=place. Read tdmcp://docs/palette first."
            }
            Self::PaletteProbe => {
                "Load palette components into a scratch COMP in a live TD, digest their interface (opType, custom parameter pages, In/Out pins, child roster, extensions, About), and destroy them — the evidence you turn into cards via palette_index action=describe. Same select{} as palette_index; batch is capped small because one wedging component takes its whole batch with it. Blacklisted entries are skipped unless includeIgnored=true. Probe in a throwaway project, never the user's work. Read tdmcp://docs/palette-scan first."
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
        Self::SampleData,
        Self::ScriptSource,
        Self::ParamInterface,
        Self::ProjectSnapshot,
        Self::CompareNodes,
        Self::Record,
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
        Self::PaletteIndex,
        Self::PaletteProbe,
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
            "sample_data" => Some(Self::SampleData),
            "script_source" => Some(Self::ScriptSource),
            "param_interface" => Some(Self::ParamInterface),
            "project_snapshot" => Some(Self::ProjectSnapshot),
            "compare_nodes" => Some(Self::CompareNodes),
            "record" => Some(Self::Record),
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
            "palette_index" => Some(Self::PaletteIndex),
            "palette_probe" => Some(Self::PaletteProbe),
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
    #[serde(default)]
    pub path: Option<OpPath>,
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
    pub max_size: Option<LenientU32>,
    /// Start (default), status, cancel, or release a timed capture job.
    #[serde(default)]
    pub action: crate::timing::JobAction,
    /// Job returned by a prior timed capture. Omit on status/cancel to discover the active job.
    #[serde(default)]
    pub job_id: Option<String>,
    #[doc = "Status only: include stored samples (default true); false returns metadata without consuming samples."]
    #[serde(default = "default_true")]
    pub include_samples: bool,
    #[doc = "Status only: zero-based stored-sample index, not a timeline offset (default 0)."]
    #[serde(default)]
    pub sample_offset: u32,
    /// Optional reset and sequential advance schedule; omitted means immediate capture.
    #[serde(default)]
    pub timing: Option<crate::timing::TimingOptions>,
    /// Optional structural inspection paired with each timed sample.
    #[serde(default)]
    pub inspect: Option<crate::timing::SampleInspect>,
    /// Diagnostic payload size (`summary` omits raw traceback).
    #[serde(default)]
    pub diagnostic_level: DiagnosticLevel,
}

fn default_capture_max_size() -> Option<LenientU32> {
    Some(LenientU32(CAPTURE_DEFAULT_MAX_SIZE))
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

#[doc = "Parameter inspection mode."]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ParamsMode {
    #[doc = "Evaluate selected parameters (default)."]
    #[default]
    Values,
    #[doc = "Return names only, without parameter evaluation or metadata reads."]
    Names,
}

fn deserialize_child_limit<'de, D>(deserializer: D) -> Result<Option<LenientU32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let limit = Option::<LenientU32>::deserialize(deserializer)?;
    if limit.is_some_and(|n| n.get() == 0 || n.get() > CHILDREN_ROSTER_LIMIT as u32) {
        return Err(serde::de::Error::custom("childLimit must be in 1..256"));
    }
    Ok(limit)
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
    #[doc = "Exact case-sensitive parameter names; omitted/null = all, [] = none. Requires include params."]
    #[serde(default)]
    pub param_names: Option<Vec<String>>,
    #[doc = "values (default) evaluates selected parameters; names returns only names. Requires include params."]
    #[serde(default)]
    pub params_mode: ParamsMode,
    #[doc = "Zero-based direct-child offset, default 0. Offsets can shift on mutable graphs. Requires include nodes."]
    #[serde(default)]
    pub child_offset: Option<LenientU32>,
    #[doc = "Direct-child page size, default 256; must be 1..256. Requires include nodes."]
    #[serde(default, deserialize_with = "deserialize_child_limit")]
    pub child_limit: Option<LenientU32>,
    /// Structural detail level.
    #[serde(default)]
    pub detail_level: DetailLevel,
    /// Diagnostic payload size (`summary` omits raw traceback).
    #[serde(default)]
    pub diagnostic_level: DiagnosticLevel,
    /// Opt in to the version 2 output contract. Omitting this returns the
    /// existing response unchanged; it never implicitly selects a contract.
    #[serde(default)]
    pub output: Option<OutputOptions>,
}

fn bridge_inspect_params(params: &InspectParams) -> Value {
    let mut wire = serde_json::json!({
        "paths": params.paths,
        "contextPath": params.context_path,
        "include": params.include,
        "detailLevel": params.detail_level.as_str(),
    });
    if let Some(names) = &params.param_names {
        wire["paramNames"] = serde_json::json!(names);
    }
    if params.params_mode != ParamsMode::Values {
        wire["paramsMode"] = serde_json::json!(params.params_mode);
    }
    if let Some(offset) = params.child_offset {
        wire["childOffset"] = serde_json::json!(offset);
    }
    if let Some(limit) = params.child_limit {
        wire["childLimit"] = serde_json::json!(limit);
    }
    wire
}

/// Policy for connecting to an input that already has connections.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum OnOccupied {
    /// Preserve legacy TD connect behavior; report the occupied input.
    #[default]
    Replace,
    /// Refuse occupied or unreadable inputs without changing any connection.
    Error,
}

/// Preconditions a guarded step rechecks against a fresh read.
///
/// A guard narrows the window between "you looked" and "you wrote". It does
/// not close it: a callback, a second script or a user can still change the
/// field after the recheck and before the write. A guarded write is therefore
/// never atomic and never exactly-once, and the pass result says so rather
/// than implying a scene-wide lock.
///
/// `fields` is REQUIRED, not optional. The bridge treats a missing or empty
/// block as "no guard", which would let a caller's requested precondition be
/// silently skipped; requiring it here turns that into a decode error the
/// caller can see.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StepPrecondition {
    /// Field path to expected value, e.g. `{"par.gain": 0.5}`.
    ///
    /// Presence-aware: an expected `null`, `false`, `0` or `""` is a real
    /// expectation, distinct from a field that is absent. A field that cannot
    /// be read fails the guard closed — nothing is written over a value nobody
    /// observed.
    pub fields: Map<String, Value>,
}

impl<'de> Deserialize<'de> for StepPrecondition {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            fields: Map<String, Value>,
        }

        let raw = Raw::deserialize(deserializer)?;
        // An empty `fields` reads as a guard and behaves as none: the bridge
        // treats it as no precondition at all. Accepting it would let a
        // caller request a guard and get an unguarded write, so refuse it at
        // the boundary instead of silently dropping the intent.
        if raw.fields.is_empty() {
            return Err(serde::de::Error::custom(
                "expect.fields must name at least one field; an empty block \
                 would be ignored and the write would run unguarded",
            ));
        }
        Ok(Self { fields: raw.fields })
    }
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
        /// Preconditions rechecked against a FRESH read immediately before the
        /// write. On any mismatch the step is refused and NOTHING is written.
        ///
        /// This narrows the race; it does not close it. A value can still change
        /// between the recheck and the write, so a guarded write is not atomic
        /// and not exactly-once. It is never retried, because retrying a write
        /// whose effect is unknown turns one intent into two mutations.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expect: Option<StepPrecondition>,
    },
    /// Destroy a node.
    Delete {
        /// Target node path.
        path: OpPath,
        /// Preconditions rechecked before the delete (see `Set::expect`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expect: Option<StepPrecondition>,
    },
    /// Wire `src` output connector to `dst` input connector.
    Connect {
        /// Source operator path.
        src: OpPath,
        /// Destination operator path.
        dst: OpPath,
        /// Source output connector index (default 0).
        #[serde(default, rename = "srcOutput")]
        src_output: LenientU32,
        /// Destination input connector index (default 0).
        #[serde(default, rename = "dstInput")]
        dst_input: LenientU32,
        /// Occupied-input policy (default replace); error also rejects unreadable occupancy.
        #[serde(default, rename = "onOccupied")]
        on_occupied: OnOccupied,
    },
    /// Place a Palette component (`.tox`) into the network.
    ///
    /// `paletteId` is resolved to a file on the daemon *before* the bridge is
    /// touched, so an unknown id never reaches TouchDesigner.
    Place {
        /// Desired node path; the leaf is the name you want.
        path: OpPath,
        /// Indexed component id, e.g. `builtin:Tools/particlesGpu` (see `palette_index`). Exactly one of `paletteId` / `toxPath`.
        #[serde(default, rename = "paletteId", skip_serializing_if = "Option::is_none")]
        palette_id: Option<String>,
        /// Absolute `.tox` path for a component outside the palette. Exactly one of `paletteId` / `toxPath`.
        #[serde(default, rename = "toxPath", skip_serializing_if = "Option::is_none")]
        tox_path: Option<String>,
        /// Operator comment (`OP.comment`) — say which component this is and why it is here.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        comment: Option<String>,
        /// Custom parameter values applied after load (`.par.*` only).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        values: Option<Map<String, Value>>,
        /// Direct OP attribute writes (`node.<name> = val`); allowlist = TD Common Flags subset.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        flags: Option<Map<String, Value>>,
    },
    /// Clear an input connector on `path`.
    Disconnect {
        /// Target operator path (destination side).
        path: OpPath,
        /// Input connector index to clear (default 0).
        #[serde(default)]
        input: LenientU32,
    },
}

// Old Python bridges ignore unknown step fields. Use a distinct internal op for
// safe connects so an old bridge rejects the step instead of silently rewiring.
fn bridge_mutate_steps(steps: &[MutateStep]) -> Result<Value, serde_json::Error> {
    steps
        .iter()
        .map(|step| {
            let mut wire = serde_json::to_value(step)?;
            if matches!(
                step,
                MutateStep::Connect {
                    on_occupied: OnOccupied::Error,
                    ..
                }
            ) {
                wire["op"] = Value::String("connect_checked".to_owned());
            }
            Ok(wire)
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Value::Array)
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

/// Swap each probe row's inline thumbnail bytes for the path they were stored at.
///
/// The bytes have already been written to the palette store by `record_probe`,
/// so keeping them in the reply would send every picture twice — once to disk
/// and once down the wire — for a caller that only needs to know where it went.
fn rehome_thumbnails(mapped: &mut Value, thumbs: &crate::palette::StoredThumbs) {
    let Some(rows) = mapped.get_mut("results").and_then(Value::as_array_mut) else {
        return;
    };
    for row in rows {
        let Some(obj) = row.as_object_mut() else {
            continue;
        };
        let had_bytes = obj.remove("thumbnailBase64").is_some();
        obj.remove("thumbnailMime");
        if !had_bytes {
            continue;
        }
        if let Some(id) = obj.get("paletteId").and_then(Value::as_str) {
            match thumbs.get(id) {
                Some(path) => {
                    obj.insert("thumb".into(), serde_json::json!(path));
                }
                // Rendered but unstorable (a full or read-only store dir):
                // say so rather than imply a file that is not there.
                None => {
                    obj.insert(
                        "thumbError".into(),
                        serde_json::json!("thumbnail could not be stored"),
                    );
                }
            }
        }
    }
}

/// Rewrite every `place` step's `paletteId` into an absolute `.tox` path.
///
/// Runs before the bridge call so an unknown id, a vanished file, or an
/// ambiguous step fails on the daemon and never reaches TouchDesigner.
fn resolve_place_steps(
    catalog: &tdmcp_diagnostics::Catalog,
    tool: ToolName,
    steps: &mut [MutateStep],
) -> Result<(), ToolCallError> {
    // Validate every place step first: an ill-formed step is a schema-shaped
    // error that must not depend on whether a config or an index exists.
    let mut needs_lookup = false;
    for (i, step) in steps.iter().enumerate() {
        let MutateStep::Place {
            palette_id,
            tox_path,
            ..
        } = step
        else {
            continue;
        };
        match (non_empty(palette_id), non_empty(tox_path)) {
            (Some(_), Some(_)) => {
                return Err(coded_failure(
                    catalog,
                    tool,
                    codes::ARGS_UNKNOWN_FIELD,
                    &format!("steps[{i}].toxPath"),
                    "place takes paletteId or toxPath, never both",
                ))
            }
            (None, None) => {
                return Err(coded_failure(
                    catalog,
                    tool,
                    codes::ARGS_MISSING_FIELD,
                    &format!("steps[{i}].paletteId"),
                    "place requires paletteId (from palette_index) or an absolute toxPath",
                ))
            }
            (Some(_), None) => needs_lookup = true,
            (None, Some(_)) => {}
        }
    }
    if !needs_lookup {
        return Ok(()); // Nothing to look up: never pay for a config load.
    }

    let cfg = tdmcp_config::load(&tdmcp_config::default_config_path()).map_err(|e| {
        coded_failure(
            catalog,
            tool,
            codes::PALETTE_STORE_FAILED,
            "steps",
            format!("config load: {e}"),
        )
    })?;
    for (i, step) in steps.iter_mut().enumerate() {
        let MutateStep::Place {
            palette_id,
            tox_path,
            ..
        } = step
        else {
            continue;
        };
        let Some(id) = non_empty(palette_id) else {
            continue;
        };
        let resolved = crate::palette::resolve_tox_path(&cfg, id).map_err(|e| {
            coded_failure(
                catalog,
                tool,
                e.code,
                &format!("steps[{i}].paletteId"),
                e.message,
            )
        })?;
        *tox_path = Some(resolved.to_string_lossy().into_owned());
    }
    Ok(())
}

/// Commit one observation to the bound store, off the executor.
///
/// [`EvidenceStore`](crate::evidence::EvidenceStore) is a SYNCHRONOUS port
/// that performs blocking disk and database work, so calling it straight from
/// this async function would occupy an executor worker for the whole upload —
/// a `MAX` MB observation is minutes of one worker, and the daemon serves
/// every other client from the same pool. It runs on the blocking pool here.
///
/// The block is produced in ALL THREE cases:
///
/// * no binding — `retained: false`, because a caller that never learns the
///   status cannot tell "not retained" from "retained but unmentioned";
/// * refused claim or unopenable store — a declined capture, with the store's
///   own words;
/// * a task that did not come back — an UNCERTAIN outcome, named as one,
///   because a commit that may have landed must not be reported as a clean
///   failure the caller would simply retry.
async fn capture_off_executor(
    evidence: Option<EvidenceBinding>,
    record: &output_v2::ObservationRecord,
    tool: &str,
    legacy: &Value,
    requested: bool,
) -> Value {
    let Some(binding) = evidence else {
        return retention_block(&crate::evidence_capture::CaptureOutcome::NotRetained {
            reason: "this call is not attributable to a client, so there is no principal to \
                     scope a retained observation to"
                .to_owned(),
        });
    };
    if !requested {
        // Retention is explicit, exactly as a compact view is. A store being
        // BOUND is not a request: writing a durable blob for every read would
        // spend the owner's quota on calls nobody asked to keep, and the
        // resulting `retained: true` would be something the caller never
        // consented to. This is reported, not silent, so "declined" stays
        // distinguishable from "not mentioned".
        return retention_block(&crate::evidence_capture::CaptureOutcome::NotRetained {
            reason: "retention was not requested for this call (omit the option or pass \
                     retain:false)"
                .to_owned(),
        });
    }
    let record = record.clone();
    let legacy = legacy.clone();
    let tool = tool.to_owned();
    let task = tokio::task::spawn_blocking(move || {
        retention_block(&capture_record(
            binding.store().as_ref(),
            binding.owner(),
            &record,
            &tool,
            &legacy,
            // The lease the producer asks for. A store clamps by REFUSING
            // anything above its own limit, so a store with a shorter ceiling
            // turns this into a reported failure rather than a silently
            // shortened retention the caller was never told about.
            DEFAULT_RETENTION,
        ))
    });
    match task.await {
        Ok(block) => block,
        // The task was cancelled or panicked. The commit may or may not have
        // landed, and this is the one outcome that must not be dressed up as a
        // clean refusal.
        Err(_) => crate::evidence_capture::uncertain_block(
            "the retention task did not complete, so whether this observation was \
             committed is unknown; inspect the store before retrying"
                .to_owned(),
        ),
    }
}

/// A trimmed non-empty view of an optional string field.
fn non_empty(value: &Option<String>) -> Option<&str> {
    value.as_deref().map(str::trim).filter(|s| !s.is_empty())
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
/// Dispatch one tool call and record what it actually did.
///
/// `journal` is the durable receipt port, supplied by the daemon and bound to
/// the caller's authenticated client identity. Pass `None` for a call that is
/// genuinely not attributable to a client — a boot-time self-check, a test — and
/// note that such a call gets no receipt: there is no principal to record it
/// under, and inventing one would be worse than the missing row.
///
/// `evidence` is the retained-evidence seam: the store plus the OWNER this
/// call's authenticated identity resolved to. The owner is bound by the
/// transport layer from the same `x-tdmcp-client` claim the receipt port uses
/// and is never a request argument — a caller that could name its own owner
/// could read another principal's evidence. `None` means this call is not
/// attributable to a client, and it therefore retains nothing.
#[allow(
    clippy::too_many_arguments,
    reason = "one parameter per dispatch input"
)]
pub async fn dispatch_tool_recorded(
    registry: &Arc<Mutex<PidRegistry>>,
    catalog: &tdmcp_diagnostics::Catalog,
    bridge: &dyn BridgeRpc,
    name: &str,
    args: Value,
    session: Option<SessionGate<'_>>,
    federation: Option<&FederationCtx>,
    journal: Option<&dyn DispatchJournal>,
    evidence: Option<EvidenceBinding>,
) -> Result<Value, ToolCallError> {
    // One boundary record per call — the only per-call trace agents and the
    // tray Logs view ever see (observability spec §5.7 rule 3).
    let started = std::time::Instant::now();
    let result = dispatch_tool_inner(
        registry, catalog, bridge, name, args, session, federation, journal, evidence,
    )
    .await;
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

/// Dispatch one tool call without a receipt.
///
/// Kept as the existing entry point so the many unrecorded callers — tests,
/// federation probes, describe_tools — keep working unchanged. A daemon serving
/// a real client should use [`dispatch_tool_recorded`].
pub async fn dispatch_tool(
    registry: &Arc<Mutex<PidRegistry>>,
    catalog: &tdmcp_diagnostics::Catalog,
    bridge: &dyn BridgeRpc,
    name: &str,
    args: Value,
    session: Option<SessionGate<'_>>,
    federation: Option<&FederationCtx>,
) -> Result<Value, ToolCallError> {
    dispatch_tool_recorded(
        registry, catalog, bridge, name, args, session, federation, None, None,
    )
    .await
}

#[allow(
    clippy::too_many_arguments,
    reason = "one parameter per dispatch input"
)]
async fn dispatch_tool_inner(
    registry: &Arc<Mutex<PidRegistry>>,
    catalog: &tdmcp_diagnostics::Catalog,
    bridge: &dyn BridgeRpc,
    name: &str,
    args: Value,
    session: Option<SessionGate<'_>>,
    federation: Option<&FederationCtx>,
    journal: Option<&dyn DispatchJournal>,
    evidence: Option<EvidenceBinding>,
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
        ToolName::DescribeTools => {
            let args = if args.is_null() {
                serde_json::json!({})
            } else {
                args
            };
            let params: crate::discovery::DescribeToolsParams = parse_args(catalog, tool, args)?;
            crate::discovery::describe(params)
                .map_err(|error| serialize_failed(catalog, tool, "discovery result", &error))
        }
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
                params.wait_timeout_ms.get(),
                params.create_if_missing,
                params.template_path.as_deref(),
            )
            .await
            .map_err(|e| {
                let code = match e.code {
                    "spawn.exe_incomplete" => tdmcp_diagnostics::codes::SPAWN_EXE_INCOMPLETE,
                    "spawn.spawn_failed" => tdmcp_diagnostics::codes::SPAWN_FAILED,
                    "spawn.wait_timeout" => tdmcp_diagnostics::codes::SPAWN_WAIT_TIMEOUT,
                    "spawn.target_not_found" => tdmcp_diagnostics::codes::SPAWN_TARGET_NOT_FOUND,
                    "spawn.template_not_found" => {
                        tdmcp_diagnostics::codes::SPAWN_TEMPLATE_NOT_FOUND
                    }
                    "spawn.template_invalid" => tdmcp_diagnostics::codes::SPAWN_TEMPLATE_INVALID,
                    "spawn.create_failed" => tdmcp_diagnostics::codes::SPAWN_CREATE_FAILED,
                    _ => tdmcp_diagnostics::codes::SPAWN_WAIT_TIMEOUT,
                };
                let field = match e.code {
                    "spawn.target_not_found" => "projectPath",
                    "spawn.template_not_found" | "spawn.template_invalid" => "templatePath",
                    "spawn.create_failed" => "projectPath",
                    _ => "exePath",
                };
                coded_failure(catalog, tool, code, field, e.message)
            })
        }
        ToolName::KillTd => {
            let params: crate::lifecycle::KillTdParams = parse_args(catalog, tool, args)?;
            let source = crate::dialogs::get().map(|d| d.source.as_ref());
            crate::lifecycle::kill_td(
                registry,
                source,
                params.pid.get(),
                params.mode,
                params.grace_ms.get(),
            )
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
        ToolName::PaletteIndex => {
            let params: crate::palette::PaletteIndexParams = parse_args(catalog, tool, args)?;
            crate::palette::run(&params)
                .map_err(|e| coded_failure(catalog, tool, e.code, e.field, e.message))
        }
        ToolName::PaletteProbe => {
            let params: crate::palette::PaletteProbeParams =
                parse_args(catalog, tool, args.clone())?;
            if let ControlFlow::Break(v) = maybe_proxy_bridged(
                federation,
                registry,
                catalog,
                "palette_probe",
                args,
                params.daemon_id.as_deref(),
                params.pid,
                session,
            )
            .await?
            {
                return Ok(v);
            }
            let cfg = tdmcp_config::load(&tdmcp_config::default_config_path()).map_err(|e| {
                coded_failure(
                    catalog,
                    tool,
                    codes::PALETTE_STORE_FAILED,
                    "pid",
                    format!("config load: {e}"),
                )
            })?;
            // Plan on the daemon: the batch is chosen, checked against disk,
            // and recorded as in-flight before TD is asked to load anything.
            let plan = crate::palette::plan_probe(&cfg, &params)
                .map_err(|e| coded_failure(catalog, tool, e.code, e.field, e.message))?;
            let skipped: Vec<Value> = plan
                .skipped
                .iter()
                .map(|(id, reason)| {
                    serde_json::json!({ "paletteId": id, "status": "skipped", "reason": reason })
                })
                .collect();
            if plan.targets.is_empty() {
                // Nothing loadable is a legitimate end-of-loop, not a failure —
                // but say *why* nothing ran, or an all-blacklisted selection
                // looks like an empty palette.
                let mut out = serde_json::json!({
                    "ok": true,
                    "results": [],
                    "skipped": skipped,
                    "skippedTotal": plan.skipped_total,
                    "remaining": plan.remaining,
                });
                out["note"] = serde_json::json!(if plan.skipped_total > 0 {
                    format!(
                        "nothing probed: {} matching component(s) are on the blacklist or missing from disk — pass includeIgnored:true to probe them anyway, or clear them with palette_index action=unignore",
                        plan.skipped_total
                    )
                } else {
                    "nothing to probe: no component matched this selection — widen it, or check palette_index action=stats"
                        .to_owned()
                });
                return Ok(out);
            }
            let _slot = begin_session_slot(
                session,
                catalog,
                "palette_probe",
                DAEMON_SCOPE_LOCAL,
                params.pid,
            )?;
            let targets: Vec<Value> = plan
                .targets
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "paletteId": t.palette_id,
                        "toxPath": t.tox_path.to_string_lossy(),
                    })
                })
                .collect();
            let outcome = enqueue_and_call(
                registry,
                bridge,
                params.pid,
                BridgeMethod::PaletteProbe,
                classify_effect(BridgeMethod::PaletteProbe.wire_str()),
                serde_json::json!({
                    "targets": targets,
                    "detailLevel": params.detail_level.as_str(),
                    "thumbnails": params.thumbnails,
                }),
                journal,
            )
            .await;
            // Only a timeout or a mid-call disconnect means the batch may have
            // actually run and wedged TD — that is the signal the breadcrumb
            // exists for. Everything else never reached TouchDesigner, so drop
            // the breadcrumb rather than blame those components later.
            let never_dispatched = matches!(
                &outcome,
                BridgeOutcome::QueueBusy
                    | BridgeOutcome::Transport(
                        BridgeRpcError::NotConnected { .. } | BridgeRpcError::Unknown { .. }
                    )
            );
            if never_dispatched {
                let _ = crate::palette::clear_inflight(&cfg);
            }
            let mut mapped = crate::outcomes::map_palette_probe_outcome(
                catalog,
                params.pid,
                outcome,
                params.diagnostic_level,
            )?;
            // Fold the ledger back in — including auto-ignore on repeat failure
            // — and clear the in-flight breadcrumb.
            let results = mapped
                .get("results")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            match crate::palette::record_probe(&cfg, &results) {
                // The PNG is now a file in the store; echoing the bytes back as
                // well would double a batch's payload for nothing. Swap each
                // row's base64 for the path it landed at.
                Ok(thumbs) => rehome_thumbnails(&mut mapped, &thumbs),
                Err(e) => mapped["ledgerError"] = serde_json::json!(e.to_string()),
            }
            if !skipped.is_empty() {
                mapped["skipped"] = Value::Array(skipped);
                mapped["skippedTotal"] = serde_json::json!(plan.skipped_total);
            }
            mapped["remaining"] = serde_json::json!(plan.remaining);
            Ok(mapped)
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
                classify_effect(method.wire_str()),
                serde_json::json!({
                    "script": params.script,
                    "contextPath": params.context_path,
                    "includeLogs": params.include_logs,
                    "formatMode": params.format_mode.as_str(),
                }),
                journal,
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
            let sample_options = crate::timing::capture_sample_options(&params, &args);
            if params.action == crate::timing::JobAction::Start && params.path.is_none() {
                return Err(coded_failure(
                    catalog,
                    tool,
                    codes::ARGS_MISSING_FIELD,
                    "path",
                    "capture start requires path",
                ));
            }
            if params.inspect.is_some() && params.timing.is_none() {
                return Err(coded_failure(
                    catalog,
                    tool,
                    codes::TIMING_INVALID,
                    "inspect",
                    "paired inspect requires timing",
                ));
            }
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
                classify_job(params.action),
                {
                    let mut wire = serde_json::json!({
                        "path": params.path,
                        "mode": params.mode.as_str(),
                        "contextPath": params.context_path,
                        "maxSize": params.max_size,
                        "action": params.action,
                        "jobId": params.job_id,
                        "timing": params.timing,
                        "inspect": params.inspect,
                    });
                    for (key, value) in sample_options {
                        wire[key] = value;
                    }
                    wire
                },
                journal,
            )
            .await;
            if params.timing.is_some() || params.action != crate::timing::JobAction::Start {
                return crate::outcomes::map_timing_outcome(
                    catalog, params.pid, "capture", outcome,
                );
            }
            map_perception_outcome(
                catalog,
                params.pid,
                params.path.unwrap_or_else(|| OpPath::from("")),
                params.context_path,
                outcome,
                params.diagnostic_level,
            )
        }
        ToolName::SampleData => {
            let params: crate::sample_data::SampleDataParams =
                parse_args(catalog, tool, args.clone())?;
            if let ControlFlow::Break(v) = maybe_proxy_bridged(
                federation,
                registry,
                catalog,
                "sample_data",
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
                "sample_data",
                DAEMON_SCOPE_LOCAL,
                params.pid,
            )?;
            let outcome = enqueue_and_call(
                registry,
                bridge,
                params.pid,
                BridgeMethod::SampleData,
                // A read. It cannot change the scene, so its receipt effect is
                // `not_applicable` rather than unknown -- even a forced cook
                // advances TD's cook, not the network, and nothing here writes.
                EffectClass::Read,
                serde_json::json!({
                    "path": params.path,
                    "pixels": params.pixels,
                    "region": params.region,
                    // The SOP arm. Without these the bridge's exact point read
                    // would be unreachable from a tool call.
                    "point": params.point,
                    "points": params.points,
                    "forceCook": params.force_cook,
                    "readMethod": params.read_method,
                }),
                journal,
            )
            .await;
            crate::outcomes::map_sample_data_outcome(
                catalog,
                params.pid,
                outcome,
                params.diagnostic_level,
            )
        }
        ToolName::ScriptSource => {
            let params: crate::script_source::ScriptSourceParams =
                parse_args(catalog, tool, args.clone())?;
            if let ControlFlow::Break(v) = maybe_proxy_bridged(
                federation,
                registry,
                catalog,
                "script_source",
                args,
                params.daemon_id.as_deref(),
                params.pid,
                session,
            )
            .await?
            {
                return Ok(v);
            }
            // A session slot is taken for the whole tool, including the actions
            // that cannot run anything. The pid queue is what keeps a `run`
            // exclusive against other bridged work, and a registered source
            // must not be able to acquire one.
            let _slot = begin_session_slot(
                session,
                catalog,
                "script_source",
                DAEMON_SCOPE_LOCAL,
                params.pid,
            )?;
            let outcome = enqueue_and_call(
                registry,
                bridge,
                params.pid,
                BridgeMethod::ScriptSource,
                // Unclassified on purpose. Most actions (register, list,
                // describe, check, delete) cannot change the scene at all, and
                // `run` can change anything -- so this class is not knowable
                // here without guessing, and an effect nobody established is
                // what `Unknown` is for.
                EffectClass::Unknown,
                serde_json::json!({
                    "action": params.action.as_str(),
                    "source": params.source,
                    "name": params.name,
                    "dependencies": params.dependencies.as_ref().map(|deps| {
                        deps.iter()
                            .map(|dep| serde_json::json!({
                                "kind": dep.kind(),
                                "target": dep.target(),
                            }))
                            .collect::<Vec<_>>()
                    }),
                    "sourceId": params.source_id,
                    "contextPath": params.context_path,
                    "includeLogs": params.include_logs,
                }),
                journal,
            )
            .await;
            map_script_outcome(
                catalog,
                params.pid,
                outcome,
                params.diagnostic_level,
                // `normal`: a registered source's failure is its own, and the
                // `debug` mode attaches frame locals from a call this tool did
                // not make.
                crate::tools::FormatMode::Normal,
                params.context_path,
            )
        }
        ToolName::ParamInterface => {
            let params: crate::param_interface::ParamInterfaceParams =
                parse_args(catalog, tool, args.clone())?;
            if let ControlFlow::Break(v) = maybe_proxy_bridged(
                federation,
                registry,
                catalog,
                "param_interface",
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
                "param_interface",
                DAEMON_SCOPE_LOCAL,
                params.pid,
            )?;
            // Not classifiable from this layer: `export` cannot change anything
            // and `apply` creates parameters, which changes the operator. An
            // effect this layer cannot establish is `Unknown`, and the bridge's
            // per-step reporting is what actually says what happened.
            let outcome = enqueue_and_call(
                registry,
                bridge,
                params.pid,
                BridgeMethod::ParamInterface,
                EffectClass::Unknown,
                serde_json::json!({
                    "action": params.action.as_str(),
                    "path": params.path,
                    "contextPath": params.context_path,
                    "definitions": params.definitions,
                }),
                journal,
            )
            .await;
            map_script_outcome(
                catalog,
                params.pid,
                outcome,
                params.diagnostic_level,
                crate::tools::FormatMode::Normal,
                params.context_path,
            )
        }
        ToolName::CompareNodes => {
            let params: crate::compare_nodes::CompareNodesParams =
                parse_args(catalog, tool, args.clone())?;
            if let ControlFlow::Break(v) = maybe_proxy_bridged(
                federation,
                registry,
                catalog,
                "compare_nodes",
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
                "compare_nodes",
                DAEMON_SCOPE_LOCAL,
                params.pid,
            )?;
            let outcome = enqueue_and_call(
                registry,
                bridge,
                params.pid,
                BridgeMethod::CompareNodes,
                // A read in both directions: capturing only observes, and
                // comparing only observes. Nothing here writes, so the receipt
                // effect is not_applicable rather than unknown.
                EffectClass::Read,
                serde_json::json!({
                    "paths": params.paths,
                    "fields": params.fields,
                    "baseline": params.baseline,
                    "capture": params.capture,
                    "contextPath": params.context_path,
                }),
                journal,
            )
            .await;
            crate::outcomes::map_compare_nodes_outcome(
                catalog,
                params.pid,
                outcome,
                params.diagnostic_level,
            )
        }
        ToolName::ProjectSnapshot => {
            let params: crate::project_snapshot::ProjectSnapshotParams =
                parse_args(catalog, tool, args.clone())?;
            if let ControlFlow::Break(v) = maybe_proxy_bridged(
                federation,
                registry,
                catalog,
                "project_snapshot",
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
                "project_snapshot",
                DAEMON_SCOPE_LOCAL,
                params.pid,
            )?;
            // `status` writes nothing, `project` and `component` write to the
            // user's disk. The class is not knowable from here without
            // branching on the action, and an effect nobody established is
            // what `Unknown` is for — a save is exactly the operation where
            // claiming more than was observed would be worst.
            let outcome = enqueue_and_call(
                registry,
                bridge,
                params.pid,
                BridgeMethod::ProjectSnapshot,
                EffectClass::Unknown,
                serde_json::json!({
                    "action": params.action.as_str(),
                    "target": params.target,
                    "path": params.path,
                    "overwrite": params.overwrite.as_str(),
                    "contextPath": params.context_path,
                }),
                journal,
            )
            .await;
            map_script_outcome(
                catalog,
                params.pid,
                outcome,
                params.diagnostic_level,
                crate::tools::FormatMode::Normal,
                params.context_path,
            )
        }
        ToolName::Record => {
            let params: crate::timing::RecordParams = parse_args(catalog, tool, args.clone())?;
            if let ControlFlow::Break(v) = maybe_proxy_bridged(
                federation,
                registry,
                catalog,
                "record",
                args.clone(),
                params.daemon_id.as_deref(),
                params.pid,
                session,
            )
            .await?
            {
                return Ok(v);
            }
            let _slot =
                begin_session_slot(session, catalog, "record", DAEMON_SCOPE_LOCAL, params.pid)?;
            let outcome = enqueue_and_call(
                registry,
                bridge,
                params.pid,
                BridgeMethod::Record,
                classify_job(params.action),
                args,
                journal,
            )
            .await;
            crate::outcomes::map_timing_outcome(catalog, params.pid, "record", outcome)
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
            // Soft-cap is enforced on the bridge; still forward the full list
            // so truncation metadata can report the requested count.
            let started_at = Utc::now();
            let started_mono = output_v2::monotonic_nanos();
            let outcome = enqueue_and_call(
                registry,
                bridge,
                params.pid,
                method,
                classify_effect(method.wire_str()),
                bridge_inspect_params(&params),
                journal,
            )
            .await;
            let completed_at = Utc::now();
            let completed_mono = output_v2::monotonic_nanos();
            let span_path = params.paths.first().cloned();
            // Captured before the mapping call, which consumes `context_path`.
            // The query hash must cover the selection actually sent.
            let query_value =
                serde_json::to_value(bridge_inspect_params(&params)).unwrap_or(Value::Null);
            let legacy = map_inspect_outcome(
                catalog,
                params.pid,
                span_path,
                params.context_path,
                outcome,
                params.diagnostic_level,
            )?;
            // No `output` option means no contract was requested, so the legacy
            // value is returned exactly as it was.
            let Some(options) = params.output else {
                return Ok(legacy);
            };
            // `retain: true` is answered HERE, where the record exists, rather
            // than refused for the whole build: the only honest reason to
            // refuse is that there is no store bound to an authenticated
            // principal for THIS call, and that is a per-call fact. The refusal
            // stays a refusal — a caller who asked for a durable observation
            // never receives a value that will be dropped.
            if options.retain == Some(true) && evidence.is_none() {
                return Err(coded_failure(
                    catalog,
                    tool,
                    codes::ARGS_WRONG_TYPE,
                    "output.retain",
                    output_v2::retention_rejection(&options)
                        .unwrap_or("retain:true is not available for this call"),
                ));
            }
            // Read the identity fields under the lock rather than cloning the
            // whole entry, so a concurrent handshake cannot make the reported
            // fingerprint and generation come from two different connections.
            let (target, bridge_generation) = {
                let reg = registry.lock().await;
                let entry = reg.get(params.pid.get());
                let launcher_pid = entry.and_then(|e| e.spawn.as_ref()).map(|_| params.pid);
                let generation = entry.map(|e| e.bridge_identity.connection_generation);
                (
                    output_v2::target_for(entry, params.pid, launcher_pid),
                    generation,
                )
            };
            let record = output_v2::record_observation(
                &query_value,
                &legacy,
                started_at,
                completed_at,
                Some((started_mono, completed_mono)),
                target,
                bridge_generation,
            );
            // Retention happens HERE, where both the record and the exact
            // legacy value are in hand, and BEFORE `wrap_v2` consumes the
            // value. The stored bytes are the legacy value a non-opt-in caller
            // would have received, so a retained page decodes to the same thing
            // `payloadHash` digests.
            let retention = capture_off_executor(
                evidence,
                &record,
                tool.wire_str(),
                &legacy,
                options.retain == Some(true),
            )
            .await;
            let mut wrapped = output_v2::wrap_v2(&record, legacy, &options).map_err(|error| {
                coded_failure(
                    catalog,
                    tool,
                    codes::OP_OBSERVATION_UNAVAILABLE,
                    "output.view",
                    format!("the requested output view could not be built: {error}"),
                )
            })?;
            // The block is inserted here, outside `wrap_v2`, because retention
            // is not a property of the document: the same bytes are wrapped
            // identically whether or not the caller asked for a durable copy.
            if let Some(map) = wrapped.as_object_mut() {
                map.insert("retention".to_owned(), retention);
            }
            Ok(wrapped)
        }
        ToolName::MutateNodes => {
            let mut params: MutateNodesParams = parse_args(catalog, tool, args.clone())?;
            resolve_place_steps(catalog, tool, &mut params.steps)?;
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
            let steps = bridge_mutate_steps(&params.steps)
                .map_err(|e| serialize_failed(catalog, tool, "mutate steps", &e))?;
            let outcome = enqueue_and_call(
                registry,
                bridge,
                params.pid,
                method,
                classify_effect(method.wire_str()),
                serde_json::json!({
                    "steps": steps,
                    "contextPath": params.context_path,
                    "detailLevel": params.detail_level.as_str(),
                }),
                journal,
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
                classify_effect(BridgeMethod::ApiHelp.wire_str()),
                serde_json::json!({
                    "queries": queries,
                    "detailLevel": params.detail_level.as_str(),
                }),
                journal,
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
                classify_effect(BridgeMethod::EditorContext.wire_str()),
                serde_json::json!({}),
                journal,
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
            // Full identity forwarding is not implemented for aggregated remote rows yet.
            identity: None,
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
///
/// `journal` is the durable receipt port, present only for callers that supply
/// one. Its absence is not a silent loss: the tools below pass it through, and
/// a tool call with no journal behaves exactly as it did before.
async fn enqueue_and_call(
    registry: &Arc<Mutex<PidRegistry>>,
    bridge: &dyn BridgeRpc,
    pid: Pid,
    method: BridgeMethod,
    effect_class: EffectClass,
    params: Value,
    journal: Option<&dyn DispatchJournal>,
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

    // Admitted only once the enqueue succeeded: before that point nothing was
    // going to be sent, so a receipt here would describe a dispatch that never
    // existed. `begin` failing is not a reason to proceed — a dispatch the
    // daemon cannot remember is exactly what this receipt exists to catch.
    let mut receipt = match journal {
        Some(journal) => match journal.begin(method.wire_str()) {
            Ok(guard) => Some(guard),
            Err(error) => {
                // Refuse the call rather than perform an unrecordable one. The
                // failure is returned as the call's own error so the caller sees
                // why nothing happened.
                clear_queue_keep_connected(registry, raw_pid).await;
                return BridgeOutcome::Transport(BridgeRpcError::ReceiptUnavailable {
                    message: error,
                });
            }
        },
        None => None,
    };

    let call = bridge.call(raw_pid, method.wire_str(), params);
    let outcome = match tokio::time::timeout(effective_bridge_timeout(), call).await {
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
    };

    // Settle the receipt from what was actually observed. Dropping the guard
    // without this would record "unobserved" even for a call that returned, so
    // every successful mutation would look uncertain.
    if let Some(guard) = receipt.as_mut() {
        match &outcome {
            BridgeOutcome::Ok(value) => {
                let ok = value.get("ok").and_then(Value::as_bool).unwrap_or(true);
                let effect = read_effect(effect_class, value);
                if let Err(error) = guard.settled(ok, effect, None) {
                    // The work is done and the receipt failed. Returning the
                    // work's result is right — discarding a successful mutation
                    // because bookkeeping failed would be worse — but the
                    // failure must reach the logs, because the receipt on disk
                    // now understates what happened.
                    tracing::warn!(
                        tool = method.wire_str(),
                        pid = raw_pid,
                        %error,
                        "the dispatch returned but its receipt was not recorded"
                    );
                }
            }
            // A timeout is not a failure and a disconnect is not a failure:
            // the request may still be running inside TouchDesigner. Dropping
            // the guard records an unsettled receipt, which is the honest
            // answer and what a restart later reports as an unknown effect.
            BridgeOutcome::Transport(_) | BridgeOutcome::QueueBusy => {}
        }
    }
    outcome
}

async fn clear_queue_keep_connected(registry: &Arc<Mutex<PidRegistry>>, pid: u32) {
    let mut reg = registry.lock().await;
    let _ = reg.cancel_queue_keep_connected(pid);
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "unit tests"
)]
mod output_v2_forwarding_tests {
    use super::{bridge_inspect_params, InspectParams};
    use serde_json::json;

    #[test]
    fn omitting_output_leaves_the_legacy_request_untouched() {
        let params: InspectParams = serde_json::from_value(json!({
            "pid": 1, "paths": ["/project1"]
        }))
        .unwrap();
        assert!(params.output.is_none());
        // Nothing about the opt-in reaches the bridge: the wire payload is
        // byte-identical to the pre-contract request.
        assert_eq!(
            bridge_inspect_params(&params),
            json!({
                "paths": ["/project1"], "contextPath": null, "include": [], "detailLevel": "summary"
            })
        );
    }

    #[test]
    fn requesting_a_contract_does_not_change_what_the_bridge_receives() {
        let without: InspectParams = serde_json::from_value(json!({
            "pid": 1, "paths": ["/project1"]
        }))
        .unwrap();
        let with: InspectParams = serde_json::from_value(json!({
            "pid": 1, "paths": ["/project1"], "output": {"contract": "v2"}
        }))
        .unwrap();
        assert!(with.output.is_some());
        // The contract selects how THIS backend presents the result. It must not
        // change the work the bridge performs, or two callers asking the same
        // question would observe different scenes.
        assert_eq!(
            bridge_inspect_params(&without),
            bridge_inspect_params(&with)
        );
    }

    #[test]
    fn an_unknown_output_option_is_rejected_rather_than_ignored() {
        let err = serde_json::from_value::<InspectParams>(json!({
            "pid": 1, "paths": ["/project1"], "output": {"contract": "v2", "viev": "compact"}
        }));
        assert!(
            err.is_err(),
            "a misspelled option must not silently change fidelity"
        );
    }

    #[test]
    fn an_unknown_contract_version_is_rejected() {
        let err = serde_json::from_value::<InspectParams>(json!({
            "pid": 1, "paths": ["/project1"], "output": {"contract": "v3"}
        }));
        assert!(err.is_err());
    }

    #[test]
    fn retain_false_is_accepted_and_recorded_explicitly() {
        let params: InspectParams = serde_json::from_value(json!({
            "pid": 1, "paths": ["/project1"], "output": {"contract": "v2", "retain": false}
        }))
        .unwrap();
        let options = params.output.unwrap();
        assert_eq!(options.retain, Some(false));
        assert!(crate::output_v2::retention_rejection(&options).is_none());
    }

    #[test]
    fn retain_true_is_accepted_by_the_parser_and_refused_by_the_producer() {
        // Parsing succeeds; the refusal happens where the backend knows it
        // cannot honor it. A parse-time rejection would report a type error
        // for what is really an unsupported capability.
        let params: InspectParams = serde_json::from_value(json!({
            "pid": 1, "paths": ["/project1"], "output": {"contract": "v2", "retain": true}
        }))
        .unwrap();
        let options = params.output.unwrap();
        let reason = crate::output_v2::retention_rejection(&options);
        assert!(reason.is_some());
        // The refusal must name the actual missing thing. The old text blamed
        // the build ("this build captures in memory only"), which was a claim
        // about every call; the real reason is that THIS call has no
        // principal to scope a retained observation to.
        assert!(reason.unwrap().contains("no client identity"), "{reason:?}");
        assert!(
            !reason.unwrap().contains("this build"),
            "the refusal must not blame the build for a per-call fact"
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod timeout_tests {
    #[test]
    fn inspect_forwarding_preserves_defaults_and_normalizes_options() {
        use super::{bridge_inspect_params, InspectParams};
        use serde_json::json;
        let defaults: InspectParams = serde_json::from_value(json!({
            "pid": 1, "paths": ["/project1"]
        }))
        .unwrap();
        assert_eq!(
            bridge_inspect_params(&defaults),
            json!({
                "paths": ["/project1"], "contextPath": null, "include": [], "detailLevel": "summary"
            })
        );
        let selected: InspectParams = serde_json::from_value(json!({
            "pid": 1, "paths": ["/project1"], "paramNames": [], "paramsMode": "names",
            "childOffset": " 256 ", "childLimit": "1", "include": ["nodes", "params"]
        }))
        .unwrap();
        assert_eq!(
            bridge_inspect_params(&selected),
            json!({
                "paths": ["/project1"], "contextPath": null, "include": ["nodes", "params"],
                "detailLevel": "summary", "paramNames": [], "paramsMode": "names",
                "childOffset": 256, "childLimit": 1
            })
        );
        let explicit: InspectParams = serde_json::from_value(json!({
            "pid": 1, "paths": ["/project1"], "paramNames": ["gain"], "paramsMode": "values",
            "childOffset": 0, "childLimit": 256
        }))
        .unwrap();
        let wire = bridge_inspect_params(&explicit);
        assert_eq!(wire["paramNames"], json!(["gain"]));
        assert_eq!(wire["childOffset"], 0);
        assert_eq!(wire["childLimit"], 256);
        assert!(wire.get("paramsMode").is_none());
    }

    #[test]
    fn safe_connect_rejects_stale_bridge_instead_of_downgrading() {
        use super::{bridge_mutate_steps, MutateStep};
        let steps: Vec<MutateStep> = serde_json::from_value(serde_json::json!([
            {"op":"connect", "src":"a", "dst":"b", "onOccupied":"error"},
            {"op":"connect", "src":"a", "dst":"b"}
        ]))
        .unwrap();
        let wire = bridge_mutate_steps(&steps).unwrap();
        assert_eq!(wire[0]["op"], "connect_checked");
        assert_eq!(wire[0]["onOccupied"], "error");
        assert_eq!(wire[1]["op"], "connect");
        assert!(
            serde_json::from_value::<MutateStep>(wire[0].clone()).is_err(),
            "internal op must not leak into the public MCP schema"
        );
    }

    #[test]
    fn a_guarded_step_decodes_and_reaches_the_wire_unchanged() {
        use super::{bridge_mutate_steps, MutateStep};
        use serde_json::json;

        // The whole point of the guard is that the bridge receives it. A
        // decode that dropped or renamed `expect` would leave a caller
        // believing it was protected while writing unguarded.
        let steps: Vec<MutateStep> = serde_json::from_value(json!([
            {"op": "set", "path": "/project1/audio_gain", "values": {"gain": 0.4},
             "expect": {"fields": {"par.gain": 0.5}}}
        ]))
        .unwrap();
        let wire = bridge_mutate_steps(&steps).unwrap();
        assert_eq!(wire[0]["expect"]["fields"]["par.gain"], json!(0.5));
        assert_eq!(wire[0]["values"]["gain"], json!(0.4));
    }

    #[test]
    fn an_empty_precondition_is_refused_instead_of_writing_unguarded() {
        use super::MutateStep;
        use serde_json::json;

        // The bridge treats an empty block as "no guard". Accepting it here
        // would be a request for protection answered with none.
        let err = serde_json::from_value::<MutateStep>(json!({
            "op": "set", "path": "/p", "expect": {"fields": {}}
        }))
        .expect_err("an empty fields map must not decode");
        assert!(
            err.to_string().contains("unguarded"),
            "refusal should say why: {err}"
        );
    }

    #[test]
    fn a_guard_absent_from_the_request_stays_absent() {
        use super::{bridge_mutate_steps, MutateStep};
        use serde_json::json;

        // Existing callers must be unaffected: a step with no `expect` is
        // serialized without one, not with a fabricated empty guard.
        let steps: Vec<MutateStep> =
            serde_json::from_value(json!([{"op": "set", "path": "/p", "values": {"a": 1}}]))
                .unwrap();
        let wire = bridge_mutate_steps(&steps).unwrap();
        assert!(wire[0].get("expect").is_none(), "{wire}");
    }

    #[test]
    fn create_cannot_carry_a_guard_it_would_silently_skip() {
        use super::MutateStep;
        use serde_json::json;

        // The bridge skips the precondition for create/place because there is
        // no prior state. Accepting the field would promise a check that
        // never runs, so the decoder must refuse it.
        assert!(
            serde_json::from_value::<MutateStep>(
                json!({"op": "create", "path": "/p", "opType": "constantTOP",
                      "expect": {"fields": {"par.gain": 0.5}}})
            )
            .is_err(),
            "create must not accept an expect it would ignore"
        );
    }

    #[test]
    fn a_guard_can_expect_a_falsy_value() {
        use super::{bridge_mutate_steps, MutateStep};
        use serde_json::json;

        // null/false/0/"" are real expectations, and an absent field is a
        // fifth distinct state. A decoder that dropped falsy values would let
        // a clobbering write through.
        let steps: Vec<MutateStep> = serde_json::from_value(json!([{
            "op": "set", "path": "/p",
            "expect": {"fields": {"comment": "", "par.bypass": false, "par.raw": null}}
        }]))
        .unwrap();
        let wire = bridge_mutate_steps(&steps).unwrap();
        assert_eq!(wire[0]["expect"]["fields"]["comment"], json!(""));
        assert_eq!(wire[0]["expect"]["fields"]["par.bypass"], json!(false));
        assert_eq!(wire[0]["expect"]["fields"]["par.raw"], json!(null));
    }

    use super::{derive_timeout, BRIDGE_TIMEOUT, PROXY_TIMEOUT};
    use std::time::Duration;

    #[test]
    fn derive_timeout_adds_margin_over_the_configured_script_budget() {
        // 600s (the proposed script_timeout_secs target) + 60s
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

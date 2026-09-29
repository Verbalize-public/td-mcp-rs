//! Stable `tdmcp.*` diagnostic code constants.
//!
//! Every code emitted by daemon or bridge must appear here **and** in
//! [`diagnostics/catalog.yaml`](../../../diagnostics/catalog.yaml). The
//! completeness test in this module gates drift.

/// Unknown or dead pid.
pub const BRIDGE_UNKNOWN_PID: &str = "tdmcp.bridge.unknown_pid";
/// Bridge IPC link lost.
pub const BRIDGE_LOST: &str = "tdmcp.bridge.lost";
/// Completed bridge reply exceeded the IPC frame budget (session retained).
pub const BRIDGE_RESPONSE_TOO_LARGE: &str = "tdmcp.bridge.response_too_large";
/// Completed bridge reply was not valid JSON (session retained).
pub const BRIDGE_RESPONSE_INVALID: &str = "tdmcp.bridge.response_invalid";
/// Queued or in-flight bridge work was cancelled.
pub const BRIDGE_CANCELLED: &str = "tdmcp.bridge.cancelled";
/// Bridge gave up waiting for TD main thread (paused / hung).
pub const BRIDGE_MAIN_THREAD_TIMEOUT: &str = "tdmcp.bridge.main_thread_timeout";
/// Exclusive request rejected — queue non-empty.
pub const BRIDGE_QUEUE_BUSY: &str = "tdmcp.bridge.queue_busy";
/// The daemon refused a dispatch because it could not durably record that the
/// work was about to happen.
///
/// Distinct from every bridge code: no bridge saw this call, so reporting it as
/// a bridge failure would blame a component that was never involved. Nothing was
/// dispatched, which is what makes a retry safe here and unsafe everywhere else.
pub const RECEIPT_UNAVAILABLE: &str = "tdmcp.receipt.unavailable";
/// MCP session already has an in-flight bridged tool against this pid.
pub const MCP_SESSION_BUSY: &str = "tdmcp.mcp.session_busy";
/// Another deferred timing job owns the TD transport.
pub const TIMING_BUSY: &str = "tdmcp.timing.busy";
/// Invalid reset, source, schedule or artifact request.
pub const TIMING_INVALID: &str = "tdmcp.timing.invalid";
/// Timing job or artifact no longer retained.
pub const TIMING_NOT_FOUND: &str = "tdmcp.timing.not_found";
/// Timing/render/encoder/finalization failure.
pub const TIMING_FAILED: &str = "tdmcp.timing.failed";
/// Deadline, disconnect or external transport/source interference.
pub const TIMING_INTERRUPTED: &str = "tdmcp.timing.interrupted";
/// Official-tools scan could not enumerate any location.
pub const INSTALLS_SCAN_FAILED: &str = "tdmcp.installs.scan_failed";

/// Offline project I/O family (v2).
pub const PROJECT_IO_FAILED: &str = "tdmcp.project.io_failed";
/// Packed source path does not exist.
pub const PROJECT_SOURCE_NOT_FOUND: &str = "tdmcp.project.source_not_found";
/// Packed magic check failed.
pub const PROJECT_NOT_PACKED_FORMAT: &str = "tdmcp.project.not_packed_format";
/// Official tools not found after full resolution.
pub const PROJECT_TOOL_MISSING: &str = "tdmcp.project.tool_missing";
/// Only one of expand/collapse paths configured.
pub const PROJECT_TOOL_PAIR_PARTIAL: &str = "tdmcp.project.tool_pair_partial";
/// toeexpand produced no usable artifacts.
pub const PROJECT_EXPAND_FAILED: &str = "tdmcp.project.expand_failed";
/// toecollapse produced no packed output.
pub const PROJECT_COLLAPSE_FAILED: &str = "tdmcp.project.collapse_failed";
/// Destination exists while overwrite=fail.
pub const PROJECT_DEST_EXISTS: &str = "tdmcp.project.dest_exists";
/// Source is not a valid expand directory.
pub const PROJECT_SRC_NOT_EXPAND_DIR: &str = "tdmcp.project.src_not_expand_dir";
/// Toc has BOM/CR or failed to parse.
pub const PROJECT_TOC_INVALID: &str = "tdmcp.project.toc_invalid";
/// Toc entry escapes its root.
pub const PROJECT_TOC_ESCAPE: &str = "tdmcp.project.toc_escape";
/// Project build differs from selected install build.
pub const PROJECT_BUILD_SKEW: &str = "tdmcp.project.build_skew";
/// Targeted re-expand verification found differences.
pub const PROJECT_ROUNDTRIP_MISMATCH: &str = "tdmcp.project.roundtrip_mismatch";
/// Pre-replace backup creation failed.
pub const PROJECT_BACKUP_FAILED: &str = "tdmcp.project.backup_failed";
/// No tdmcp_rs COMP subtree present in the project.
pub const PROJECT_BRIDGE_SUBTREE_MISSING: &str = "tdmcp.project.bridge_subtree_missing";

/// OS dialogs family (DIALOGS.md 5.5).
pub const DIALOG_UNSUPPORTED: &str = "tdmcp.dialog.unsupported";
/// Popup id not found (stale list / closed window).
pub const DIALOG_NOT_FOUND: &str = "tdmcp.dialog.not_found";
/// Popup survived the dismiss ladder verification.
pub const DIALOG_DISMISS_FAILED: &str = "tdmcp.dialog.dismiss_failed";
/// Target is TD main chrome — protected.
pub const DIALOG_CHROME_PROTECTED: &str = "tdmcp.dialog.chrome_protected";
/// Modal popup is blocking the pid's main thread (interception gate).
pub const DIALOG_BLOCKING: &str = "tdmcp.dialog.blocking";
/// macOS Accessibility (TCC) not granted for AX automation.
pub const DIALOG_PERMISSION_DENIED: &str = "tdmcp.dialog.permission_denied";

/// TD process lifecycle family (v2).
/// Selected install lacks official tools.
pub const SPAWN_EXE_INCOMPLETE: &str = "tdmcp.spawn.exe_incomplete";
/// OS spawn of TouchDesigner failed.
pub const SPAWN_FAILED: &str = "tdmcp.spawn.spawn_failed";
/// Handshake did not arrive within the wait budget.
pub const SPAWN_WAIT_TIMEOUT: &str = "tdmcp.spawn.wait_timeout";
/// Startup popups are blocking the handshake (surfaced, not auto-dismissed).
pub const SPAWN_BLOCKED_BY_DIALOG: &str = "tdmcp.spawn.blocked_by_dialog";
/// Process exited before handshaking.
pub const SPAWN_EXITED_EARLY: &str = "tdmcp.spawn.exited_early";
/// Project target missing and createIfMissing is false.
pub const SPAWN_TARGET_NOT_FOUND: &str = "tdmcp.spawn.target_not_found";
/// Template .toe not found.
pub const SPAWN_TEMPLATE_NOT_FOUND: &str = "tdmcp.spawn.template_not_found";
/// Template .toe looks invalid.
pub const SPAWN_TEMPLATE_INVALID: &str = "tdmcp.spawn.template_invalid";
/// Failed to copy template to target.
pub const SPAWN_CREATE_FAILED: &str = "tdmcp.spawn.create_failed";
/// Pid is neither registered nor a TD image.
pub const KILL_NOT_TD_PID: &str = "tdmcp.kill.not_td_pid";
/// Process survived the graceful close window.
pub const KILL_GRACEFUL_TIMEOUT: &str = "tdmcp.kill.graceful_timeout";
/// TerminateProcess denied (elevation/UIPI).
pub const KILL_ACCESS_DENIED: &str = "tdmcp.kill.access_denied";

/// No palette root found (no TD install with a Palette folder, no user root).
pub const PALETTE_NO_ROOTS: &str = "tdmcp.palette.no_roots";
/// The palette index is empty or absent — nothing has been scanned yet.
pub const PALETTE_NOT_INDEXED: &str = "tdmcp.palette.not_indexed";
/// Requested paletteId is not in the index.
pub const PALETTE_UNKNOWN_ID: &str = "tdmcp.palette.unknown_id";
/// Indexed component's `.tox` is no longer on disk.
pub const PALETTE_TOX_MISSING: &str = "tdmcp.palette.tox_missing";
/// Component is on the probe blacklist and was not loaded.
pub const PALETTE_IGNORED: &str = "tdmcp.palette.ignored";
/// TouchDesigner failed to load the `.tox`.
pub const PALETTE_LOAD_FAILED: &str = "tdmcp.palette.load_failed";
/// Probe could not digest the component.
pub const PALETTE_PROBE_FAILED: &str = "tdmcp.palette.probe_failed";
/// Palette index / card store could not be read or written.
pub const PALETTE_STORE_FAILED: &str = "tdmcp.palette.store_failed";

/// Tool result failed to serialize (internal; never the caller's fault).
pub const MCP_SERIALIZE_FAILED: &str = "tdmcp.mcp.serialize_failed";

/// Tool-call arguments missing a required field.
pub const ARGS_MISSING_FIELD: &str = "tdmcp.args.missing_field";
/// Tool-call argument key not in the schema (`deny_unknown_fields`).
pub const ARGS_UNKNOWN_FIELD: &str = "tdmcp.args.unknown_field";
/// Argument value not a valid enum variant (e.g. `include` / `detailLevel` / step `op`).
pub const ARGS_UNKNOWN_VARIANT: &str = "tdmcp.args.unknown_variant";
/// Argument value has the wrong JSON type for its schema slot.
pub const ARGS_WRONG_TYPE: &str = "tdmcp.args.wrong_type";
/// Lint: similar argument key found (typo / near-miss).
pub const ARGS_SIMILAR_FIELD: &str = "tdmcp.args.similar_field";
/// Daemon wait timed out.
pub const BRIDGE_TIMEOUT: &str = "tdmcp.bridge.timeout";
/// Bridge package version incompatible with daemon.
pub const BRIDGE_VERSION: &str = "tdmcp.bridge.version";
/// Bridge handshake protocol_version not supported by the daemon (T-6/D5).
pub const BRIDGE_PROTOCOL_MISMATCH: &str = "tdmcp.bridge.protocol_mismatch";
/// Bridge handshake frame missing/unreadable on the TCP bridge socket.
pub const BRIDGE_HANDSHAKE_INVALID: &str = "tdmcp.bridge.handshake_invalid";
/// Peer connected but sent no handshake frame within the 5s budget.
pub const BRIDGE_HANDSHAKE_TIMEOUT: &str = "tdmcp.bridge.handshake_timeout";

/// Stdio proxy ↔ daemon HTTP link lost / unreachable (reconnect-only; no upsert).
pub const DAEMON_UNREACHABLE: &str = "tdmcp.daemon.unreachable";

/// Auth token missing or wrong on a protected remote path.
pub const REMOTE_UNAUTHORIZED: &str = "tdmcp.remote.unauthorized";

/// Slave registration rejected by master (PSK mismatch / auth failure).
pub const FEDERATION_AUTH_REJECTED: &str = "tdmcp.federation.auth_rejected";
/// Master cannot reach a slave for a proxied tool call.
pub const FEDERATION_SLAVE_UNREACHABLE: &str = "tdmcp.federation.slave_unreachable";
/// Pid matches multiple daemons; response includes candidates.
pub const FEDERATION_AMBIGUOUS_PID: &str = "tdmcp.federation.ambiguous_pid";

/// OpPath resolution failed.
pub const OP_NOT_FOUND: &str = "tdmcp.op.not_found";
/// Similar node name found nearby (lint).
pub const OP_SIMILAR_NAME: &str = "tdmcp.op.similar_name";
/// Create auto-renamed by TD; actual path differs from requested (lint).
pub const OP_RENAMED: &str = "tdmcp.op.renamed";
/// Path outside authorized mutation zone.
pub const OP_OUTSIDE_ZONE: &str = "tdmcp.op.outside_zone";
/// Inspect direct-child roster capped (soft; payload `truncation` block).
pub const OP_CHILDREN_TRUNCATED: &str = "tdmcp.op.children_truncated";
/// Inspect `paths` batch capped (soft; payload `truncation` block).
pub const OP_PATHS_TRUNCATED: &str = "tdmcp.op.paths_truncated";
/// Inspect called without a non-empty `paths` array.
pub const OP_PATHS_REQUIRED: &str = "tdmcp.op.paths_required";
/// Per-path inspect shaping failed after resolve.
pub const OP_INSPECT_FAILED: &str = "tdmcp.op.inspect_failed";
/// An inspection field could not be read; empty fallback is not clean evidence.
pub const OP_OBSERVATION_UNAVAILABLE: &str = "tdmcp.op.observation_unavailable";
/// Unknown or unresolved opType for create.
pub const OP_UNKNOWN_TYPE: &str = "tdmcp.op.unknown_type";

/// Top-level editor_context handler failed.
pub const EDITOR_CONTEXT_FAILED: &str = "tdmcp.editor.context_failed";
/// Per-pane editor_context shaping failed (inline soft error).
pub const EDITOR_PANE_FAILED: &str = "tdmcp.editor.pane_failed";
/// editor_context per-pane selection list capped (soft; payload `truncation` block).
pub const EDITOR_SELECTION_TRUNCATED: &str = "tdmcp.editor.selection_truncated";
/// editor_context panes array capped (soft; payload `truncation` block).
pub const EDITOR_PANES_TRUNCATED: &str = "tdmcp.editor.panes_truncated";

/// api_help query target not found / unsupported.
pub const API_HELP_NOT_FOUND: &str = "tdmcp.api_help.not_found";
/// api_help called without a non-empty `queries` array.
pub const API_HELP_QUERIES_REQUIRED: &str = "tdmcp.api_help.queries_required";
/// api_help `queries` batch capped (soft; payload `truncation` block).
pub const API_HELP_QUERIES_TRUNCATED: &str = "tdmcp.api_help.queries_truncated";
/// api_help `classes` names index capped (soft; payload `truncation` block).
pub const API_HELP_CLASSES_TRUNCATED: &str = "tdmcp.api_help.classes_truncated";

/// Later batch step skipped after prior failure.
pub const BATCH_SKIPPED_DEPENDENT: &str = "tdmcp.batch.skipped_dependent";

/// Unknown parameter on node.
pub const PAR_UNKNOWN: &str = "tdmcp.par.unknown";
/// Requested value would be silently coerced by a built-in closed menu.
pub const PAR_INVALID_MENU_VALUE: &str = "tdmcp.par.invalid_menu_value";
/// Unknown operator flag name (not in the operate-relevant Common Flags subset).
pub const FLAG_UNKNOWN: &str = "tdmcp.flag.unknown";
/// Lint: name belongs under flags, not values/expressions/pulse.
pub const PAR_WRONG_COLLECTION: &str = "tdmcp.par.wrong_collection";
/// Lint: name belongs under values, not flags.
pub const FLAG_WRONG_COLLECTION: &str = "tdmcp.flag.wrong_collection";
/// Lint: similar .par name found (typo / near-miss).
pub const PAR_SIMILAR_NAME: &str = "tdmcp.par.similar_name";
/// Soft inspect enrichment: custom parameter enableExpr failed to evaluate.
pub const PAR_ENABLE_EXPR_FAILED: &str = "tdmcp.par.enable_expr_failed";
/// Lint: similar opType found (case / near-miss).
pub const OP_SIMILAR_TYPE: &str = "tdmcp.op.similar_type";
/// A guarded write was refused: the field did not hold the expected value.
pub const MUTATE_PRECONDITION_FAILED: &str = "tdmcp.mutate.precondition_failed";
/// A guarded write was refused: the precondition could not be read at all.
pub const MUTATE_PRECONDITION_UNREADABLE: &str = "tdmcp.mutate.precondition_unreadable";
/// `compare_nodes` had no usable baseline, so nothing was compared.
///
/// Its own code, never collapsed into a bridge fault: "I have nothing to
/// compare against" and "the bridge sent nonsense" call for opposite
/// responses, and a caller who cannot tell them apart will retry a request
/// that was never going to succeed.
pub const COMPARE_BASELINE_UNAVAILABLE: &str = "tdmcp.compare.baseline_unavailable";
/// `compare_nodes` arguments were self-contradictory (empty/repeated paths or fields).
pub const COMPARE_INVALID: &str = "tdmcp.compare.invalid";
/// The baseline was taken over a different field set than the one requested.
pub const COMPARE_PROJECTION_MISMATCH: &str = "tdmcp.compare.projection_mismatch";
/// The comparison exceeded the node or field bound.
pub const COMPARE_TOO_LARGE: &str = "tdmcp.compare.too_large";
/// Mutate step failed with a TD-side exception (catch-all).
pub const MUTATE_STEP_FAILED: &str = "tdmcp.mutate.step_failed";
/// Text write (`create`/`set` `text`) targeted a non-DAT operator.
pub const MUTATE_NOT_DAT: &str = "tdmcp.mutate.not_dat";
/// `param_interface` received an action it does not implement.
pub const INTERFACE_UNKNOWN_ACTION: &str = "tdmcp.interface.unknown_action";
/// `param_interface` definitions were not a non-empty array of parameters.
pub const INTERFACE_BAD_DEFINITIONS: &str = "tdmcp.interface.bad_definitions";
/// The apply exceeded the per-call parameter cap.
pub const INTERFACE_TOO_MANY_STEPS: &str = "tdmcp.interface.too_many_steps";
/// An interface belongs on a COMP; the target resolved to another family.
pub const INTERFACE_UNSUPPORTED_FAMILY: &str = "tdmcp.interface.unsupported_family";
/// The interface could not be read back from the target.
pub const INTERFACE_EXPORT_FAILED: &str = "tdmcp.interface.export_failed";
/// `project_snapshot` received an action it does not implement.
pub const SNAPSHOT_UNKNOWN_ACTION: &str = "tdmcp.snapshot.unknown_action";
/// An explicit save/load target is required (the default is unnameable).
pub const SNAPSHOT_TARGET_REQUIRED: &str = "tdmcp.snapshot.target_required";
/// `overwrite` was neither `fail` nor `replace`.
pub const SNAPSHOT_BAD_OVERWRITE: &str = "tdmcp.snapshot.bad_overwrite";
/// The target exists and `overwrite` is `fail`; nothing was written.
pub const SNAPSHOT_TARGET_EXISTS: &str = "tdmcp.snapshot.target_exists";
/// The existing file could not be set aside, so the save was not attempted.
pub const SNAPSHOT_RECOVERY_FAILED: &str = "tdmcp.snapshot.recovery_failed";
/// TouchDesigner raised while saving.
pub const SNAPSHOT_SAVE_FAILED: &str = "tdmcp.snapshot.save_failed";
/// TouchDesigner declined to save.
pub const SNAPSHOT_NOT_SAVED: &str = "tdmcp.snapshot.not_saved";
/// The save returned no path, so no artifact identity is available.
pub const SNAPSHOT_NO_PATH_RETURNED: &str = "tdmcp.snapshot.no_path_returned";
/// The save reported a path but no file is there — effect unknown, not success.
pub const SNAPSHOT_NOT_WRITTEN: &str = "tdmcp.snapshot.not_written";
/// The load target is not on disk; the load was refused before any attempt.
pub const SNAPSHOT_TARGET_MISSING: &str = "tdmcp.snapshot.target_missing";
/// The load raised, or returned nothing identifiable.
pub const SNAPSHOT_LOAD_FAILED: &str = "tdmcp.snapshot.load_failed";
/// TouchDesigner offers no safe way to replace the open project.
pub const SNAPSHOT_LOAD_NOT_OFFERED: &str = "tdmcp.snapshot.load_not_offered";
/// A component save/load needs a COMP; the target resolved to another family.
pub const SNAPSHOT_UNSUPPORTED_FAMILY: &str = "tdmcp.snapshot.unsupported_family";
/// The operator has no resolution, channels or samples to read.
pub const SAMPLE_EMPTY: &str = "tdmcp.sample.empty";
/// The forced cook raised before any value could be read.
pub const SAMPLE_COOK_FAILED: &str = "tdmcp.sample.cook_failed";
/// This build exposes no exact read path for this operator.
pub const SAMPLE_UNSUPPORTED: &str = "tdmcp.sample.unsupported";
/// Exact sampling supports TOP, CHOP and SOP only.
pub const SAMPLE_UNSUPPORTED_FAMILY: &str = "tdmcp.sample.unsupported_family";
/// The region is malformed or falls outside the image.
pub const SAMPLE_BAD_REGION: &str = "tdmcp.sample.bad_region";
/// The region covers more pixels than the per-call cap.
pub const SAMPLE_REGION_TOO_LARGE: &str = "tdmcp.sample.region_too_large";
/// More pixels were requested than the per-call cap.
pub const SAMPLE_TOO_MANY_PIXELS: &str = "tdmcp.sample.too_many_pixels";
/// More points were requested than the per-call cap.
pub const SAMPLE_TOO_MANY_POINTS: &str = "tdmcp.sample.too_many_points";
/// The point index is not a whole number, or is outside the roster.
pub const SAMPLE_BAD_POINT: &str = "tdmcp.sample.bad_point";
/// The requested coordinates do not apply to this operator family.
pub const SAMPLE_WRONG_COORDINATES: &str = "tdmcp.sample.wrong_coordinates";
/// A channel or point could not be read — the value is unknown, not zero.
pub const SAMPLE_READ_FAILED: &str = "tdmcp.sample.read_failed";
/// `script_source` received an action it does not implement.
pub const SCRIPT_SOURCE_UNKNOWN_ACTION: &str = "tdmcp.script.unknown_action";
/// `script_source` requires the content-addressed `sourceId`.
pub const SCRIPT_SOURCE_MISSING_SOURCE_ID: &str = "tdmcp.script.missing_source_id";
/// No registered source for this ref.
pub const SCRIPT_SOURCE_UNKNOWN_SOURCE: &str = "tdmcp.script.unknown_source";
/// A declared dependency could not be resolved.
pub const SCRIPT_SOURCE_BAD_DEPENDENCY: &str = "tdmcp.script.bad_dependency";
/// More dependencies were declared than the per-call cap.
pub const SCRIPT_SOURCE_TOO_MANY_DEPENDENCIES: &str = "tdmcp.script.too_many_dependencies";
/// The source registry is at its cap.
pub const SCRIPT_SOURCE_LIMIT: &str = "tdmcp.script.source_limit";
/// Connector index out of range for connect/disconnect.
pub const WIRE_BAD_INDEX: &str = "tdmcp.wire.bad_index";
/// Connector connect/disconnect raised a TD-side error.
pub const WIRE_CONNECT_FAILED: &str = "tdmcp.wire.connect_failed";
/// Destination input is occupied.
pub const WIRE_INPUT_OCCUPIED: &str = "tdmcp.wire.input_occupied";
/// Destination occupancy could not be observed.
pub const WIRE_OCCUPANCY_UNKNOWN: &str = "tdmcp.wire.occupancy_unknown";

/// Shader lint: consumer compiled successfully (soft note).
pub const SHADER_COMPILED: &str = "tdmcp.shader.compiled";
/// No affirmative, complete shader compile evidence is available.
pub const SHADER_STATE_UNKNOWN: &str = "tdmcp.shader.state_unknown";
/// Shader lint: consumer compile failed; item `lines[]` carries verbatim ERROR: lines.
pub const SHADER_COMPILE_FAILED: &str = "tdmcp.shader.compile_failed";
/// Shader lint: consumer exposes no compile-status surface (e.g. glslPOP).
pub const SHADER_UNSUPPORTED_CONSUMER: &str = "tdmcp.shader.unsupported_consumer";
/// Shader lint consumer scan/diagnostics capped (soft; payload `truncation` block).
pub const SHADER_CONSUMERS_TRUNCATED: &str = "tdmcp.shader.consumers_truncated";

/// DAT or followed shader source exceeds the inspect text-preview cap (soft).
pub const OP_CONTENT_TRUNCATED: &str = "tdmcp.op.content_truncated";

/// Python script execution failed.
pub const SCRIPT_EXECUTION_FAILED: &str = "tdmcp.script.execution_failed";
/// Lint: AttributeError on None — likely missing op() / bad path.
pub const SCRIPT_NONE_OP: &str = "tdmcp.script.none_op";
/// execute_python script UTF-8 size exceeds bridge cap.
pub const SCRIPT_TOO_LARGE: &str = "tdmcp.script.too_large";
/// execute_python result JSON UTF-8 size exceeds bridge cap — truncated
/// (`truncation` block on an `ok:true` response), never discarded: the
/// script already ran.
pub const SCRIPT_RESULT_TOO_LARGE: &str = "tdmcp.script.result_too_large";

/// GLSL compile / cook error from TD.
pub const TD_GLSL_COMPILE: &str = "tdmcp.td.glsl_compile";

/// Captured TOP frame is black.
pub const PERCEPTION_BLACK_FRAME: &str = "tdmcp.perception.black_frame";
/// Captured TOP frame is a uniform solid color (non-black).
pub const PERCEPTION_UNIFORM_FRAME: &str = "tdmcp.perception.uniform_frame";
/// No perception path for COMP.
pub const PERCEPTION_NO_PATH: &str = "tdmcp.perception.no_path";
/// Capture mode does not match resolved operator family.
pub const PERCEPTION_WRONG_FAMILY: &str = "tdmcp.perception.wrong_family";
/// CHOP has no channels or samples.
pub const PERCEPTION_EMPTY_CHOP: &str = "tdmcp.perception.empty_chop";
/// CHOP capture capped (soft; payload `truncation` block).
pub const PERCEPTION_CHOP_TRUNCATED: &str = "tdmcp.perception.chop_truncated";
/// Shared OP Viewer / legacy converter capture path failed.
pub const PERCEPTION_CONVERTER_FAILED: &str = "tdmcp.perception.converter_failed";
/// capture `maxSize` (or native resolution) exceeds the hard pre-flight cap.
pub const PERCEPTION_MAX_SIZE_TOO_LARGE: &str = "tdmcp.perception.max_size_too_large";

/// All codes that must exist in the catalog (compile-time enumeration).
pub const ALL: &[&str] = &[
    RECEIPT_UNAVAILABLE,
    TIMING_BUSY,
    TIMING_INVALID,
    TIMING_NOT_FOUND,
    TIMING_FAILED,
    TIMING_INTERRUPTED,
    INSTALLS_SCAN_FAILED,
    PROJECT_IO_FAILED,
    PROJECT_SOURCE_NOT_FOUND,
    PROJECT_NOT_PACKED_FORMAT,
    PROJECT_TOOL_MISSING,
    PROJECT_TOOL_PAIR_PARTIAL,
    PROJECT_EXPAND_FAILED,
    PROJECT_COLLAPSE_FAILED,
    PROJECT_DEST_EXISTS,
    PROJECT_SRC_NOT_EXPAND_DIR,
    PROJECT_TOC_INVALID,
    PROJECT_TOC_ESCAPE,
    PROJECT_BUILD_SKEW,
    PROJECT_ROUNDTRIP_MISMATCH,
    PROJECT_BACKUP_FAILED,
    PROJECT_BRIDGE_SUBTREE_MISSING,
    DIALOG_UNSUPPORTED,
    DIALOG_NOT_FOUND,
    DIALOG_DISMISS_FAILED,
    DIALOG_CHROME_PROTECTED,
    DIALOG_BLOCKING,
    DIALOG_PERMISSION_DENIED,
    SPAWN_EXE_INCOMPLETE,
    SPAWN_FAILED,
    SPAWN_WAIT_TIMEOUT,
    SPAWN_BLOCKED_BY_DIALOG,
    SPAWN_EXITED_EARLY,
    SPAWN_TARGET_NOT_FOUND,
    SPAWN_TEMPLATE_NOT_FOUND,
    SPAWN_TEMPLATE_INVALID,
    SPAWN_CREATE_FAILED,
    KILL_NOT_TD_PID,
    KILL_GRACEFUL_TIMEOUT,
    KILL_ACCESS_DENIED,
    BRIDGE_UNKNOWN_PID,
    BRIDGE_LOST,
    BRIDGE_RESPONSE_TOO_LARGE,
    BRIDGE_RESPONSE_INVALID,
    BRIDGE_CANCELLED,
    BRIDGE_MAIN_THREAD_TIMEOUT,
    BRIDGE_QUEUE_BUSY,
    MCP_SESSION_BUSY,
    PALETTE_NO_ROOTS,
    PALETTE_NOT_INDEXED,
    PALETTE_UNKNOWN_ID,
    PALETTE_TOX_MISSING,
    PALETTE_IGNORED,
    PALETTE_LOAD_FAILED,
    PALETTE_PROBE_FAILED,
    PALETTE_STORE_FAILED,
    MCP_SERIALIZE_FAILED,
    ARGS_MISSING_FIELD,
    ARGS_UNKNOWN_FIELD,
    ARGS_UNKNOWN_VARIANT,
    ARGS_WRONG_TYPE,
    ARGS_SIMILAR_FIELD,
    BRIDGE_TIMEOUT,
    BRIDGE_VERSION,
    BRIDGE_PROTOCOL_MISMATCH,
    BRIDGE_HANDSHAKE_INVALID,
    BRIDGE_HANDSHAKE_TIMEOUT,
    DAEMON_UNREACHABLE,
    REMOTE_UNAUTHORIZED,
    FEDERATION_AUTH_REJECTED,
    FEDERATION_SLAVE_UNREACHABLE,
    FEDERATION_AMBIGUOUS_PID,
    OP_NOT_FOUND,
    OP_SIMILAR_NAME,
    OP_RENAMED,
    OP_OUTSIDE_ZONE,
    OP_CHILDREN_TRUNCATED,
    OP_PATHS_TRUNCATED,
    OP_PATHS_REQUIRED,
    OP_INSPECT_FAILED,
    OP_OBSERVATION_UNAVAILABLE,
    OP_UNKNOWN_TYPE,
    EDITOR_CONTEXT_FAILED,
    EDITOR_PANE_FAILED,
    EDITOR_SELECTION_TRUNCATED,
    EDITOR_PANES_TRUNCATED,
    API_HELP_NOT_FOUND,
    API_HELP_QUERIES_REQUIRED,
    API_HELP_QUERIES_TRUNCATED,
    API_HELP_CLASSES_TRUNCATED,
    BATCH_SKIPPED_DEPENDENT,
    PAR_UNKNOWN,
    PAR_INVALID_MENU_VALUE,
    FLAG_UNKNOWN,
    PAR_WRONG_COLLECTION,
    FLAG_WRONG_COLLECTION,
    PAR_SIMILAR_NAME,
    PAR_ENABLE_EXPR_FAILED,
    OP_SIMILAR_TYPE,
    MUTATE_PRECONDITION_FAILED,
    MUTATE_PRECONDITION_UNREADABLE,
    COMPARE_BASELINE_UNAVAILABLE,
    COMPARE_INVALID,
    COMPARE_PROJECTION_MISMATCH,
    COMPARE_TOO_LARGE,
    INTERFACE_UNKNOWN_ACTION,
    INTERFACE_BAD_DEFINITIONS,
    INTERFACE_TOO_MANY_STEPS,
    INTERFACE_UNSUPPORTED_FAMILY,
    INTERFACE_EXPORT_FAILED,
    SNAPSHOT_UNKNOWN_ACTION,
    SNAPSHOT_TARGET_REQUIRED,
    SNAPSHOT_BAD_OVERWRITE,
    SNAPSHOT_TARGET_EXISTS,
    SNAPSHOT_RECOVERY_FAILED,
    SNAPSHOT_SAVE_FAILED,
    SNAPSHOT_NOT_SAVED,
    SNAPSHOT_NO_PATH_RETURNED,
    SNAPSHOT_NOT_WRITTEN,
    SNAPSHOT_TARGET_MISSING,
    SNAPSHOT_LOAD_FAILED,
    SNAPSHOT_LOAD_NOT_OFFERED,
    SNAPSHOT_UNSUPPORTED_FAMILY,
    SAMPLE_EMPTY,
    SAMPLE_COOK_FAILED,
    SAMPLE_UNSUPPORTED,
    SAMPLE_UNSUPPORTED_FAMILY,
    SAMPLE_BAD_REGION,
    SAMPLE_REGION_TOO_LARGE,
    SAMPLE_TOO_MANY_PIXELS,
    SAMPLE_TOO_MANY_POINTS,
    SAMPLE_BAD_POINT,
    SAMPLE_WRONG_COORDINATES,
    SAMPLE_READ_FAILED,
    SCRIPT_SOURCE_UNKNOWN_ACTION,
    SCRIPT_SOURCE_MISSING_SOURCE_ID,
    SCRIPT_SOURCE_UNKNOWN_SOURCE,
    SCRIPT_SOURCE_BAD_DEPENDENCY,
    SCRIPT_SOURCE_TOO_MANY_DEPENDENCIES,
    SCRIPT_SOURCE_LIMIT,
    MUTATE_STEP_FAILED,
    MUTATE_NOT_DAT,
    WIRE_BAD_INDEX,
    WIRE_CONNECT_FAILED,
    WIRE_INPUT_OCCUPIED,
    WIRE_OCCUPANCY_UNKNOWN,
    SHADER_COMPILED,
    SHADER_STATE_UNKNOWN,
    SHADER_COMPILE_FAILED,
    SHADER_UNSUPPORTED_CONSUMER,
    SHADER_CONSUMERS_TRUNCATED,
    OP_CONTENT_TRUNCATED,
    SCRIPT_EXECUTION_FAILED,
    SCRIPT_NONE_OP,
    SCRIPT_TOO_LARGE,
    SCRIPT_RESULT_TOO_LARGE,
    TD_GLSL_COMPILE,
    PERCEPTION_BLACK_FRAME,
    PERCEPTION_UNIFORM_FRAME,
    PERCEPTION_NO_PATH,
    PERCEPTION_WRONG_FAMILY,
    PERCEPTION_EMPTY_CHOP,
    PERCEPTION_CHOP_TRUNCATED,
    PERCEPTION_CONVERTER_FAILED,
    PERCEPTION_MAX_SIZE_TOO_LARGE,
];

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;
    use crate::Catalog;

    #[test]
    fn every_code_constant_is_in_catalog() {
        let cat = Catalog::fallback();
        for code in ALL {
            assert!(
                cat.contains(code),
                "code {code} missing from diagnostics/catalog.yaml"
            );
        }
    }

    #[test]
    fn catalog_has_no_orphan_codes_beyond_constants() {
        // Every catalog entry must have a constant, not merely "eventually".
        // The transition allowance this test used to carry is now empty: the
        // codes it covered (interface / snapshot / sample / script-source)
        // are real codes the bridge emits, and a constant is what lets Rust
        // name one. Leaving them as orphans meant nothing in Rust could
        // reference them, which is how they went uncatalogued in the first
        // place.
        let cat = Catalog::fallback();
        for code in cat.codes() {
            assert!(
                ALL.contains(&code),
                "catalog entry {code} has no codes::* constant — add it or remove the entry"
            );
        }
    }

    /// M6 §5.7 rule 8: every `tdmcp.*`-shaped string literal in **production**
    /// Rust source (everything before this codebase's own `#[cfg(test)] mod
    /// tests { ... }` tail — the convention every file here follows) must be
    /// a real catalog entry. Catches a typo'd or ad-hoc code written as a raw
    /// literal instead of a `codes::*` constant (which
    /// `every_code_constant_is_in_catalog` alone can't see, since it only
    /// walks the constants that exist). Test-only fixture codes (e.g. an
    /// intentionally-fake code exercising an "unknown code" fallback path)
    /// are out of scope by construction, not by an allowlist.
    #[test]
    fn no_unregistered_code_literals_in_source() {
        let cat = Catalog::fallback();
        let crates_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates");
        let mut offenders = Vec::new();
        scan_dir_for_code_literals(&crates_dir, &cat, &mut offenders);
        assert!(
            offenders.is_empty(),
            "code literal(s) not in diagnostics/catalog.yaml (typo, or missing catalog \
             entry — add one or route through a codes::* constant): {offenders:#?}"
        );
    }

    fn scan_dir_for_code_literals(
        dir: &std::path::Path,
        cat: &Catalog,
        offenders: &mut Vec<String>,
    ) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().and_then(|n| n.to_str()) == Some("target") {
                    continue;
                }
                scan_dir_for_code_literals(&path, cat, offenders);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                // Production code only — everything up to this codebase's
                // `#[cfg(test)]` tail convention (see doc comment above).
                let production = text.split("#[cfg(test)]").next().unwrap_or(&text);
                for code in extract_tdmcp_code_literals(production) {
                    if !cat.contains(&code) {
                        offenders.push(format!("{}: {code}", path.display()));
                    }
                }
            }
        }
    }

    /// M6 §5.7 rule 8, extended to the half of the codebase the original test
    /// could not see: **the Python bridge emits codes too**, and a code that is
    /// absent from the catalog is not merely undocumented — `outcomes.rs` looks
    /// the code up with `catalog.contains(code)` and falls back to
    /// `tdmcp.bridge.response_invalid` when it misses. A deliberate refusal
    /// ("no baseline supplied, so nothing was compared") therefore surfaces as
    /// a malformed-response fault, which tells the caller the opposite of what
    /// happened and invites a retry that can never succeed.
    ///
    /// Found live, not by inspection: `compare_nodes` shipped four codes
    /// (`baseline_unavailable`, `invalid`, `projection_mismatch`, `too_large`)
    /// that no Rust constant covered, and every honest refusal was relabelled.
    #[test]
    fn every_code_the_bridge_emits_is_in_the_catalog() {
        let cat = Catalog::fallback();
        let bridge_dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bridge/tdmcp_bridge");
        let Ok(entries) = std::fs::read_dir(&bridge_dir) else {
            // Packaged builds may not carry the bridge sources; skip rather than
            // assert on a directory that was never going to exist here.
            return;
        };
        let mut offenders = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("py") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            for code in extract_tdmcp_code_literals(&text) {
                if !cat.contains(&code) {
                    offenders.push(format!("{}: {code}", path.display()));
                }
            }
        }
        offenders.sort();
        offenders.dedup();
        assert!(
            offenders.is_empty(),
            "bridge code literal(s) not in diagnostics/catalog.yaml — a missing entry \
             makes outcomes.rs relabel that refusal as tdmcp.bridge.response_invalid: {offenders:#?}"
        );
    }

    /// Every `"tdmcp.foo.bar"`-shaped string literal in `text` (the catalog's
    /// own namespace — nothing else in this codebase uses a `tdmcp.` dotted
    /// prefix, as opposed to `tdmcp_*` crate/target names or `tdmcp://` URIs).
    fn extract_tdmcp_code_literals(text: &str) -> Vec<String> {
        let mut out = Vec::new();
        let bytes = text.as_bytes();
        let mut i = 0;
        while let Some(rel) = text[i..].find("\"tdmcp.") {
            let start = i + rel + 1; // skip the opening quote
            let mut end = start;
            while end < bytes.len() && bytes[end] != b'"' && bytes[end] != b'\n' {
                end += 1;
            }
            if end < bytes.len() && bytes[end] == b'"' {
                out.push(text[start..end].to_owned());
            }
            i = end.max(start + 1);
        }
        out
    }
}

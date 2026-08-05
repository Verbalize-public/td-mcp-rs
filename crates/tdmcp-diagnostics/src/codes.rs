//! Stable `tdmcp.*` diagnostic code constants.
//!
//! Every code emitted by daemon or bridge must appear here **and** in
//! [`diagnostics/catalog.yaml`](../../../diagnostics/catalog.yaml). The
//! completeness test in this module gates drift.

/// Unknown or dead pid.
pub const BRIDGE_UNKNOWN_PID: &str = "tdmcp.bridge.unknown_pid";
/// Bridge IPC link lost.
pub const BRIDGE_LOST: &str = "tdmcp.bridge.lost";
/// Exclusive request rejected — queue non-empty.
pub const BRIDGE_QUEUE_BUSY: &str = "tdmcp.bridge.queue_busy";
/// MCP session already has an in-flight bridged tool against this pid.
pub const MCP_SESSION_BUSY: &str = "tdmcp.mcp.session_busy";
/// Daemon wait timed out.
pub const BRIDGE_TIMEOUT: &str = "tdmcp.bridge.timeout";
/// Bridge package version incompatible with daemon.
pub const BRIDGE_VERSION: &str = "tdmcp.bridge.version";

/// Stdio proxy ↔ daemon HTTP link lost / unreachable (reconnect-only; no upsert).
pub const DAEMON_UNREACHABLE: &str = "tdmcp.daemon.unreachable";

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
/// Mutate step failed with a TD-side exception (catch-all).
pub const MUTATE_STEP_FAILED: &str = "tdmcp.mutate.step_failed";
/// Connector index out of range for connect/disconnect.
pub const WIRE_BAD_INDEX: &str = "tdmcp.wire.bad_index";
/// Connector connect/disconnect raised a TD-side error.
pub const WIRE_CONNECT_FAILED: &str = "tdmcp.wire.connect_failed";

/// Python script execution failed.
pub const SCRIPT_EXECUTION_FAILED: &str = "tdmcp.script.execution_failed";
/// Lint: AttributeError on None — likely missing op() / bad path.
pub const SCRIPT_NONE_OP: &str = "tdmcp.script.none_op";
/// execute_python script UTF-8 size exceeds bridge cap.
pub const SCRIPT_TOO_LARGE: &str = "tdmcp.script.too_large";
/// execute_python result JSON UTF-8 size exceeds bridge cap.
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

/// All codes that must exist in the catalog (compile-time enumeration).
pub const ALL: &[&str] = &[
    BRIDGE_UNKNOWN_PID,
    BRIDGE_LOST,
    BRIDGE_QUEUE_BUSY,
    MCP_SESSION_BUSY,
    BRIDGE_TIMEOUT,
    BRIDGE_VERSION,
    DAEMON_UNREACHABLE,
    OP_NOT_FOUND,
    OP_SIMILAR_NAME,
    OP_RENAMED,
    OP_OUTSIDE_ZONE,
    OP_CHILDREN_TRUNCATED,
    OP_PATHS_TRUNCATED,
    OP_PATHS_REQUIRED,
    OP_INSPECT_FAILED,
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
    FLAG_UNKNOWN,
    PAR_WRONG_COLLECTION,
    FLAG_WRONG_COLLECTION,
    PAR_SIMILAR_NAME,
    PAR_ENABLE_EXPR_FAILED,
    OP_SIMILAR_TYPE,
    MUTATE_STEP_FAILED,
    WIRE_BAD_INDEX,
    WIRE_CONNECT_FAILED,
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
        // Soft check: every catalog entry should eventually have a constant.
        // Orphans are allowed only if listed here during a transition.
        let cat = Catalog::fallback();
        for code in cat.codes() {
            assert!(
                ALL.contains(&code),
                "catalog entry {code} has no codes::* constant — add it or remove the entry"
            );
        }
    }
}

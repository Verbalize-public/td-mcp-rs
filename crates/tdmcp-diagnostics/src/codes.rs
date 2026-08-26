//! Stable `tdmcp.*` diagnostic code constants.
//!
//! Every code emitted by daemon or bridge must appear here **and** in
//! [`diagnostics/catalog.yaml`](../../../diagnostics/catalog.yaml). The
//! completeness test in this module gates drift.

/// Unknown or dead pid.
pub const BRIDGE_UNKNOWN_PID: &str = "tdmcp.bridge.unknown_pid";
/// Bridge IPC link lost.
pub const BRIDGE_LOST: &str = "tdmcp.bridge.lost";
/// Queued or in-flight bridge work was cancelled.
pub const BRIDGE_CANCELLED: &str = "tdmcp.bridge.cancelled";
/// Bridge gave up waiting for TD main thread (paused / hung).
pub const BRIDGE_MAIN_THREAD_TIMEOUT: &str = "tdmcp.bridge.main_thread_timeout";
/// Exclusive request rejected — queue non-empty.
pub const BRIDGE_QUEUE_BUSY: &str = "tdmcp.bridge.queue_busy";
/// MCP session already has an in-flight bridged tool against this pid.
pub const MCP_SESSION_BUSY: &str = "tdmcp.mcp.session_busy";
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
pub const DIALOG_NOT_FOUND: &str = "tdmcp.dialog.not_found";
pub const DIALOG_DISMISS_FAILED: &str = "tdmcp.dialog.dismiss_failed";
pub const DIALOG_CHROME_PROTECTED: &str = "tdmcp.dialog.chrome_protected";
pub const DIALOG_BLOCKING: &str = "tdmcp.dialog.blocking";

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
/// Text write (`create`/`set` `text`) targeted a non-DAT operator.
pub const MUTATE_NOT_DAT: &str = "tdmcp.mutate.not_dat";
/// Connector index out of range for connect/disconnect.
pub const WIRE_BAD_INDEX: &str = "tdmcp.wire.bad_index";
/// Connector connect/disconnect raised a TD-side error.
pub const WIRE_CONNECT_FAILED: &str = "tdmcp.wire.connect_failed";

/// Shader lint: consumer compiled successfully (soft note).
pub const SHADER_COMPILED: &str = "tdmcp.shader.compiled";
/// Shader lint: consumer compile failed; item `lines[]` carries verbatim ERROR: lines.
pub const SHADER_COMPILE_FAILED: &str = "tdmcp.shader.compile_failed";
/// Shader lint: consumer exposes no compile-status surface (e.g. glslPOP).
pub const SHADER_UNSUPPORTED_CONSUMER: &str = "tdmcp.shader.unsupported_consumer";
/// Shader lint consumer scan/diagnostics capped (soft; payload `truncation` block).
pub const SHADER_CONSUMERS_TRUNCATED: &str = "tdmcp.shader.consumers_truncated";

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
    BRIDGE_UNKNOWN_PID,
    BRIDGE_LOST,
    BRIDGE_CANCELLED,
    BRIDGE_MAIN_THREAD_TIMEOUT,
    BRIDGE_QUEUE_BUSY,
    MCP_SESSION_BUSY,
    MCP_SERIALIZE_FAILED,
    ARGS_MISSING_FIELD,
    ARGS_UNKNOWN_FIELD,
    ARGS_UNKNOWN_VARIANT,
    ARGS_WRONG_TYPE,
    ARGS_SIMILAR_FIELD,
    BRIDGE_TIMEOUT,
    BRIDGE_VERSION,
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
    MUTATE_NOT_DAT,
    WIRE_BAD_INDEX,
    WIRE_CONNECT_FAILED,
    SHADER_COMPILED,
    SHADER_COMPILE_FAILED,
    SHADER_UNSUPPORTED_CONSUMER,
    SHADER_CONSUMERS_TRUNCATED,
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

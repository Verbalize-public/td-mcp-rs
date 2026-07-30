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
/// Daemon wait timed out.
pub const BRIDGE_TIMEOUT: &str = "tdmcp.bridge.timeout";
/// Bridge package version incompatible with daemon.
pub const BRIDGE_VERSION: &str = "tdmcp.bridge.version";

/// OpPath resolution failed.
pub const OP_NOT_FOUND: &str = "tdmcp.op.not_found";
/// Similar node name found nearby (lint).
pub const OP_SIMILAR_NAME: &str = "tdmcp.op.similar_name";
/// Path outside authorized mutation zone.
pub const OP_OUTSIDE_ZONE: &str = "tdmcp.op.outside_zone";
/// Inspect direct-child roster capped (soft; payload `truncation` block).
pub const OP_CHILDREN_TRUNCATED: &str = "tdmcp.op.children_truncated";

/// Later batch step skipped after prior failure.
pub const BATCH_SKIPPED_DEPENDENT: &str = "tdmcp.batch.skipped_dependent";

/// Unknown parameter on node.
pub const PAR_UNKNOWN: &str = "tdmcp.par.unknown";

/// Python script execution failed.
pub const SCRIPT_EXECUTION_FAILED: &str = "tdmcp.script.execution_failed";

/// GLSL compile / cook error from TD.
pub const TD_GLSL_COMPILE: &str = "tdmcp.td.glsl_compile";

/// Captured TOP frame is black.
pub const PERCEPTION_BLACK_FRAME: &str = "tdmcp.perception.black_frame";
/// No perception path for COMP.
pub const PERCEPTION_NO_PATH: &str = "tdmcp.perception.no_path";

/// All codes that must exist in the catalog (compile-time enumeration).
pub const ALL: &[&str] = &[
    BRIDGE_UNKNOWN_PID,
    BRIDGE_LOST,
    BRIDGE_QUEUE_BUSY,
    BRIDGE_TIMEOUT,
    BRIDGE_VERSION,
    OP_NOT_FOUND,
    OP_SIMILAR_NAME,
    OP_OUTSIDE_ZONE,
    OP_CHILDREN_TRUNCATED,
    BATCH_SKIPPED_DEPENDENT,
    PAR_UNKNOWN,
    SCRIPT_EXECUTION_FAILED,
    TD_GLSL_COMPILE,
    PERCEPTION_BLACK_FRAME,
    PERCEPTION_NO_PATH,
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

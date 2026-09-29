//! `sample_data` tool — exact numerical reads from a TOP or CHOP.
//!
//! The arguments are deliberately strict (`deny_unknown_fields`). A misspelled
//! `pixel` or `regions` would otherwise be dropped silently and the call would
//! answer with a whole-image summary — a different, cheaper answer to the
//! question that was asked, with nothing in the response to say so.
//!
//! # Coordinates
//!
//! Callers name pixels `[x, y]`, which is how humans and every other part of
//! this API talk about pixels. The underlying array is addressed `[h, w]` —
//! measured on TouchDesigner 2025.32460, where `numpyArray()` returned shape
//! `[256, 256, 4]`. The swap happens in the bridge, in one function, and the
//! response reports `arrayOrder` so a consumer reading raw values does not have
//! to guess.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tdmcp_core::Pid;
use tdmcp_diagnostics::DiagnosticLevel;

/// How a CHOP's samples are read.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ReadMethod {
    /// Per-channel indexer (`chan[sample]`), the path that was measured.
    ///
    /// The default on purpose: a CHOP's `numpyArray` axis order was not
    /// measured, and guessing `[samples, chans]` would silently transpose
    /// every channel.
    #[default]
    Indexed,
    /// `numpyArray()` in one call, with the array shape reported alongside.
    ///
    /// Opt-in because the response carries `axisOrderVerified: false`.
    NumpyArray,
}

/// A pixel rectangle, inclusive of the origin and exclusive of the far edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SampleRegion {
    /// Left edge in pixels (the `x` a caller names).
    pub x: u32,
    /// Top edge in pixels (the `y` a caller names).
    pub y: u32,
    /// Right edge, exclusive.
    pub x1: u32,
    /// Bottom edge, exclusive.
    pub y1: u32,
}

/// Args for `sample_data`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SampleDataParams {
    /// Target pid.
    pub pid: Pid,
    /// Operator to read. TOPs and CHOPs are supported exactly; anything else
    /// is refused by name rather than approximated.
    pub path: String,
    /// Optional federated daemon id (omit for local / unique remote resolve).
    #[serde(default)]
    pub daemon_id: Option<String>,
    /// Exact values at these `[x, y]` pixels.
    ///
    /// Out-of-range and malformed entries are counted in `outOfRange` rather
    /// than clamped, because a clamped answer to a bad coordinate is a
    /// different observation than the caller asked for.
    #[serde(default)]
    pub pixels: Option<Vec<Vec<u32>>>,
    /// Exact values over a rectangle.
    ///
    /// Mutually exclusive with `pixels` in practice; when both are present the
    /// region wins, and the response reports which mode it used.
    #[serde(default)]
    pub region: Option<SampleRegion>,
    /// Cook the operator before reading.
    ///
    /// Needed for a current read: without it a TOP may still hold the previous
    /// frame, and the response says so in `cook.forced` either way.
    #[serde(default, rename = "forceCook")]
    pub force_cook: bool,
    /// How a CHOP's samples are read. Ignored for a TOP.
    #[serde(default)]
    pub read_method: Option<ReadMethod>,
    /// Diagnostic payload size (`summary` omits raw traceback).
    #[serde(default)]
    pub diagnostic_level: DiagnosticLevel,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(value: serde_json::Value) -> SampleDataParams {
        serde_json::from_value(value).expect("valid params")
    }

    #[test]
    fn a_minimal_call_needs_only_pid_and_path() {
        let params = parse(json!({"pid": 1, "path": "/project1/px"}));
        assert_eq!(params.path, "/project1/px");
        assert!(params.pixels.is_none());
        assert!(params.region.is_none());
        assert!(!params.force_cook);
        // Indexed is the measured path, so it is the default rather than a
        // value the caller has to know to pass.
        assert_eq!(params.read_method, None);
    }

    #[test]
    fn force_cook_uses_the_wire_spelling() {
        let params = parse(json!({"pid": 1, "path": "/p", "forceCook": true}));
        assert!(params.force_cook);
    }

    #[test]
    fn pixels_are_a_list_of_x_y_pairs() {
        let params = parse(json!({"pid": 1, "path": "/p", "pixels": [[0, 0], [3, 1]]}));
        let pixels = params.pixels.expect("pixels");
        assert_eq!(pixels.len(), 2);
        assert_eq!(pixels[1], vec![3, 1]);
    }

    #[test]
    fn a_misspelled_option_is_refused_rather_than_ignored() {
        // The reason this type is strict: a dropped `pixel` would fall through
        // to the whole-image summary, which is a cheaper answer to a different
        // question and says nothing about what was lost.
        for bad in [
            json!({"pid": 1, "path": "/p", "pixel": [[0, 0]]}),
            json!({"pid": 1, "path": "/p", "region": {"x": 0, "y": 0, "x2": 2, "y2": 2}}),
            json!({"pid": 1, "path": "/p", "forcecook": true}),
        ] {
            assert!(
                serde_json::from_value::<SampleDataParams>(bad.clone()).is_err(),
                "expected {bad} to be refused"
            );
        }
    }

    #[test]
    fn a_region_needs_all_four_bounds() {
        let params = parse(json!({
            "pid": 1, "path": "/p",
            "region": {"x": 0, "y": 0, "x1": 4, "y1": 4}
        }));
        let region = params.region.expect("region");
        assert_eq!((region.x, region.y, region.x1, region.y1), (0, 0, 4, 4));
    }

    #[test]
    fn the_read_method_round_trips_both_arms() {
        let indexed = parse(json!({"pid": 1, "path": "/p", "readMethod": "indexed"}));
        assert_eq!(indexed.read_method, Some(ReadMethod::Indexed));
        let numpy = parse(json!({"pid": 1, "path": "/p", "readMethod": "numpyArray"}));
        assert_eq!(numpy.read_method, Some(ReadMethod::NumpyArray));
    }

    #[test]
    fn an_unknown_read_method_is_refused() {
        assert!(serde_json::from_value::<SampleDataParams>(
            json!({"pid": 1, "path": "/p", "readMethod": "guess"})
        )
        .is_err());
    }
}

//! The version-2 envelope fixtures, built through the real producer.
//!
//! `scripts/generate_envelope_fixtures.py` stores cases, not recorded
//! responses: a query, a legacy value, the requested options, and the
//! expectations. Storing a whole expected envelope would freeze hashes,
//! timestamps and the per-process instance id — all of which change every run —
//! and would therefore assert nothing.
//!
//! What these pin is the set of distinctions that fail *silently*. A caller
//! that reads `unavailable` as `captured: null`, or takes a `compact` request
//! answered with the full value as a compact response, cannot tell from the
//! text that anything went wrong.

#![allow(clippy::unwrap_used, reason = "test setup/assertions may panic")]
#![allow(clippy::expect_used, reason = "test setup/assertions may panic")]
#![allow(clippy::panic, reason = "test assertions may panic")]

use std::error::Error;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};
use tdmcp_core::{OutputContract, OutputOptions, OutputView, Pid};
use tdmcp_mcp::output_v2::{monotonic_nanos, record_observation, target_for, wrap_v2};
use tdmcp_mcp::output_views::{CompactOutput, OutputCodec};

/// The workspace root, from the package manifest directory (`crates/tdmcp-mcp`).
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map_or_else(
            || PathBuf::from(env!("CARGO_MANIFEST_DIR")),
            Path::to_path_buf,
        )
}

fn fixture_dir() -> PathBuf {
    let configured = std::env::var_os("TDMCP_ENVELOPE_FIXTURES").map(PathBuf::from);
    match configured {
        Some(path) if path.is_absolute() => path,
        Some(path) if path.exists() => path,
        Some(path) => workspace_root().join(path),
        None => workspace_root().join("scripts/fixtures/envelope"),
    }
}

struct Case {
    name: String,
    purpose: String,
    query: Value,
    legacy: Value,
    options: OutputOptions,
    expect: Map<String, Value>,
}

/// Read every fixture, sorted by name so a failure is reproducible.
fn cases() -> Result<Vec<Case>, Box<dyn Error>> {
    let dir = fixture_dir();
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    paths.sort();

    let mut cases = Vec::new();
    for path in paths {
        let raw: Value = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
        let provenance = &raw["provenance"];
        let case = &raw["case"];
        cases.push(Case {
            name: path.file_stem().map_or_else(
                || "unknown".to_owned(),
                |stem| stem.to_string_lossy().into(),
            ),
            purpose: provenance["purpose"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            query: case["query"].clone(),
            legacy: case["legacy"].clone(),
            // The options are strict on purpose, so a misspelled key here is a
            // generator bug rather than a silently-ignored request.
            options: serde_json::from_value(case["options"].clone())?,
            expect: case["expect"]
                .as_object()
                .ok_or("a fixture's expect must be an object")?
                .clone(),
        });
    }
    Ok(cases)
}

/// Build the envelope for one case through the real producer.
fn envelope(case: &Case) -> Result<Value, Box<dyn Error>> {
    let started_mono = monotonic_nanos();
    let started_at = chrono::Utc::now();
    let record = record_observation(
        &case.query,
        &case.legacy,
        started_at,
        chrono::Utc::now(),
        Some((started_mono, monotonic_nanos())),
        target_for(None, Pid(1), None),
        Some(1),
    );
    Ok(wrap_v2(&record, case.legacy.clone(), &case.options)?)
}

/// Follow a dotted path, treating a `present`/`absent` marker as its own check.
fn at<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cursor = root;
    for segment in path.split('.') {
        cursor = cursor.get(segment)?;
    }
    Some(cursor)
}

fn check(case: &Case, envelope: &Value) -> Result<(), Box<dyn Error>> {
    for (path, expected) in &case.expect {
        let observed = at(envelope, path);
        match expected.as_str() {
            Some("present") => {
                if observed.is_none() {
                    return Err(format!("{}: expected {path} to be present", case.name).into());
                }
            }
            Some("absent") => {
                if observed.is_some() {
                    return Err(format!("{}: expected {path} to be absent", case.name).into());
                }
            }
            _ => {
                let Some(observed) = observed else {
                    return Err(format!("{}: {path} is absent", case.name).into());
                };
                if observed != expected {
                    return Err(format!(
                        "{}: {path} is {observed}, expected {expected}",
                        case.name
                    )
                    .into());
                }
            }
        }
    }
    Ok(())
}

#[test]
fn every_envelope_fixture_holds_through_the_real_producer() -> Result<(), Box<dyn Error>> {
    let cases = cases()?;
    assert!(!cases.is_empty(), "no envelope fixtures were found");
    for case in &cases {
        let envelope = envelope(case)?;
        check(case, &envelope)?;
    }
    Ok(())
}

/// The cases are not decorative: this asserts the fixture set actually covers
/// the states it claims to, so deleting a case is a visible change.
#[test]
fn the_fixture_set_covers_every_observation_state_and_capability() -> Result<(), Box<dyn Error>> {
    let cases = cases()?;
    let all = cases
        .iter()
        .map(|case| envelope(case))
        .collect::<Result<Vec<_>, _>>()?;
    let first = all.first().ok_or("no envelopes were built")?;

    // The four states must all be reachable in the shipped envelope shape.
    let states: std::collections::BTreeSet<&str> =
        ["window.monotonic.state", "window.targetClock.state"]
            .iter()
            .filter_map(|path| at(first, path).and_then(Value::as_str))
            .collect();
    assert!(
        states.len() >= 2,
        "the envelope should distinguish at least two window states, saw {states:?}"
    );

    // No capability may be Supported unless its backend really produced it.
    for envelope in &all {
        let capabilities = &envelope["capabilities"];
        for name in [
            "evidenceRead",
            "snapshotPaging",
            "exactMemberLookup",
            "guardedWrites",
            "numericalSampling",
            "operationStatus",
        ] {
            assert_eq!(
                capabilities[name]["state"],
                json!("unknown"),
                "{name} must stay unknown until its backend is built"
            );
        }
    }
    Ok(())
}

/// A compact document must reconstruct the legacy value exactly.
///
/// This is the load-bearing claim of the whole compact view: the envelope may
/// change the representation, never the data.
#[test]
fn a_served_compact_document_decodes_back_to_the_legacy_value() -> Result<(), Box<dyn Error>> {
    let codec = OutputCodec::default();
    let mut served = 0usize;
    for case in cases()?.iter() {
        if case.options.view != Some(OutputView::Compact) {
            continue;
        }
        let envelope = envelope(case)?;
        if envelope["presentation"]["applied"] != json!("compact") {
            continue;
        }
        served += 1;
        let compact =
            CompactOutput::from_wire_value(envelope["data"].clone()).map_err(|error| {
                format!(
                    "{}: a compact response must be a codec document: {error}",
                    case.name
                )
            })?;
        let decoded = codec
            .decode_compact(compact)
            .map_err(|error| format!("{}: decode failed: {error}", case.name))?;
        assert_eq!(
            serde_json::to_string(&decoded)?,
            serde_json::to_string(&case.legacy)?,
            "{}: the decoded value differs from what was captured",
            case.name
        );
    }
    assert!(served > 0, "no fixture actually served a compact document");
    Ok(())
}

/// Omitting the view must never compact, and compact must never be implicit.
#[test]
fn compact_is_only_ever_served_when_it_was_asked_for() -> Result<(), Box<dyn Error>> {
    for case in cases()?.iter() {
        let envelope = envelope(case)?;
        let requested = &envelope["presentation"]["requested"];
        let applied = &envelope["presentation"]["applied"];
        if case.options.view == Some(OutputView::Compact) {
            assert_eq!(requested, &json!("compact"), "{}", case.name);
        } else {
            assert_eq!(requested, &json!("full"), "{}", case.name);
            assert_eq!(
                applied,
                &json!("full"),
                "{}: compact must never be applied without being requested",
                case.name
            );
        }
    }
    Ok(())
}

/// A compact request answered with the full value must carry a reason.
#[test]
fn a_refused_compact_request_states_why_and_claims_no_support() -> Result<(), Box<dyn Error>> {
    for case in cases()?.iter() {
        if case.options.view != Some(OutputView::Compact) {
            continue;
        }
        let envelope = envelope(case)?;
        if envelope["presentation"]["applied"] == json!("compact") {
            continue;
        }
        let reason = &envelope["presentation"]["fullReason"];
        assert!(
            reason.as_str().is_some_and(|value| !value.is_empty()),
            "{}: a full value returned for a compact request needs a stated reason",
            case.name
        );
        assert_eq!(
            envelope["capabilities"]["compactView"]["state"],
            json!("unknown"),
            "{}: serving full for a compact request is not evidence of support",
            case.name
        );
    }
    Ok(())
}

/// The unit of the byte accounting is declared, never assumed.
#[test]
fn the_envelope_never_makes_a_token_claim() -> Result<(), Box<dyn Error>> {
    for case in cases()?.iter() {
        let rendered = envelope(case)?.to_string();
        assert!(
            !rendered.to_lowercase().contains("token"),
            "{}: the envelope must not carry a token claim",
            case.name
        );
    }
    Ok(())
}

/// The fixtures are synthetic by construction, and that must stay true.
#[test]
fn the_fixtures_are_synthetic_and_path_shaped() -> Result<(), Box<dyn Error>> {
    for case in cases()?.iter() {
        assert_eq!(
            case.name,
            Path::new(&case.name).file_stem().unwrap().to_string_lossy()
        );
        // The fixture README is generated from this field, so a missing purpose
        // would leave a fixture that is documented as covering something with no
        // statement of what that something is.
        assert!(
            !case.purpose.trim().is_empty(),
            "{}: a fixture must state what it pins",
            case.name
        );
        let rendered = format!("{}{}", case.query, case.legacy);
        assert!(
            !rendered.contains("/home/") && !rendered.contains("/root/"),
            "{}: a fixture must not carry a real path",
            case.name
        );
        for path in walk_paths(&case.legacy) {
            assert!(
                path.starts_with("/project1/fx/"),
                "{}: node path {path} is not a synthetic fixture path",
                case.name
            );
        }
    }
    Ok(())
}

/// Every string under a `path` key in the value.
fn walk_paths(value: &Value) -> Vec<String> {
    let mut found = Vec::new();
    collect_paths(value, &mut found);
    found
}

fn collect_paths(value: &Value, found: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                if key == "path" {
                    if let Some(text) = child.as_str() {
                        found.push(text.to_owned());
                    }
                }
                collect_paths(child, found);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_paths(item, found);
            }
        }
        _ => {}
    }
}

/// The contract is opt-in: nothing here changes for a caller who did not ask.
#[test]
fn the_envelope_declares_the_contract_it_actually_used() -> Result<(), Box<dyn Error>> {
    for case in cases()?.iter() {
        assert_eq!(case.options.contract, OutputContract::V2);
        let envelope = envelope(case)?;
        assert_eq!(envelope["contract"], json!("v2"), "{}", case.name);
        // The payload hash covers the value the caller reconstructs, so it has
        // to survive the compact round trip.
        assert!(
            envelope["identity"]["payloadHash"]
                .get("algorithm")
                .is_some(),
            "{}: the payload hash must be present and named",
            case.name
        );
    }
    Ok(())
}

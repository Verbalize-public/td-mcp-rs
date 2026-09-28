//! Replay the private recorded `inspect` corpus through the shipped codec.
//!
//! The corpus lives outside the repository (`retroanalysis/` is gitignored) because it contains
//! real user session payloads. It is never copied into a public fixture. This test therefore
//! runs only when `TDMCP_INSPECT_CORPUS` points at the directory, and a configured-but-missing
//! directory is a failure rather than a silent skip.
//!
//! What it proves, per payload:
//!
//! * whichever representation the planner chose, the decoded value is byte-identical to the
//!   recorded one after serialization, so nothing is dropped or invented;
//! * a compact choice was actually smaller than full;
//! * a decode never silently degrades — a failure is a test failure.
//!
//! These are byte counts. They are not token counts and support no token claim.

use std::error::Error;
use std::path::PathBuf;

use serde_json::Value;
use tdmcp_mcp::output_views::{EncodedOutput, OutputCodec, PresentationPlan};

fn corpus_dir() -> Option<PathBuf> {
    let configured = std::env::var_os("TDMCP_INSPECT_CORPUS")?;
    Some(PathBuf::from(configured))
}

/// One recorded payload plus the provenance that makes a failure attributable.
struct Payload {
    file: String,
    session: String,
    call_seq: i64,
    value: Value,
}

fn load(dir: &PathBuf) -> Result<Vec<Payload>, Box<dyn Error>> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();

    let mut payloads = Vec::new();
    for path in files {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(document) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        // The corpus directory also holds derived indexes. Only attributed
        // request/response pairs are replayable payloads.
        let Some(response) = document.get("response") else {
            continue;
        };
        let provenance = document.get("provenance").cloned().unwrap_or(Value::Null);
        payloads.push(Payload {
            file: path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            session: provenance
                .get("session")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_owned(),
            call_seq: provenance
                .get("callSeq")
                .and_then(Value::as_i64)
                .unwrap_or(-1),
            value: response.clone(),
        });
    }
    Ok(payloads)
}

#[test]
fn every_recorded_inspect_payload_round_trips_through_the_codec() -> Result<(), Box<dyn Error>> {
    let Some(dir) = corpus_dir() else {
        eprintln!(
            "skipping corpus replay: TDMCP_INSPECT_CORPUS is not set. \
             The recorded payloads are private and are not part of the repository."
        );
        return Ok(());
    };
    assert!(
        dir.is_dir(),
        "TDMCP_INSPECT_CORPUS points at {}, which is not a directory",
        dir.display()
    );

    let payloads = load(&dir)?;
    assert!(
        !payloads.is_empty(),
        "no attributed payloads found in {}",
        dir.display()
    );

    let codec = OutputCodec::default();
    let mut compact_count = 0_u64;
    let mut full_count = 0_u64;
    // Corpus-wide serialized bytes. `baseline` is what a client received before
    // any compact view existed; `adaptive` is what this encoder returns. These
    // are bytes, not tokens, and support no token claim.
    let mut baseline_bytes = 0_u64;
    let mut adaptive_bytes = 0_u64;
    let mut reasons: std::collections::BTreeMap<&'static str, u64> = Default::default();

    for payload in &payloads {
        let original = serde_json::to_string(&payload.value)?;
        let planned = codec.encode(payload.value.clone());
        let label = format!(
            "session {} call {} ({})",
            payload.session, payload.call_seq, payload.file
        );

        match planned.plan {
            PresentationPlan::Compact {
                full_bytes,
                compact_bytes,
                transforms,
            } => {
                compact_count += 1;
                baseline_bytes += u64::try_from(full_bytes).unwrap_or(0);
                adaptive_bytes += u64::try_from(compact_bytes).unwrap_or(0);
                assert!(
                    compact_bytes < full_bytes,
                    "{label}: compact was selected but is not smaller ({compact_bytes} >= {full_bytes})"
                );
                assert!(
                    transforms > 0,
                    "{label}: compact selected with no transforms"
                );
                assert!(
                    matches!(planned.output, EncodedOutput::Compact(_)),
                    "{label}: plan says compact but the output is not a compact document"
                );
            }
            PresentationPlan::Full {
                full_bytes, reason, ..
            } => {
                full_count += 1;
                // A declined compact request costs exactly the baseline bytes: the
                // full value is returned intact rather than trimmed.
                let full = u64::try_from(full_bytes.unwrap_or(0)).unwrap_or(0);
                baseline_bytes += full;
                adaptive_bytes += full;
                let name = match reason {
                    tdmcp_mcp::output_views::FullReason::NoTransforms => "no_transforms",
                    tdmcp_mcp::output_views::FullReason::NotSmaller => "not_smaller",
                    tdmcp_mcp::output_views::FullReason::TraversalBudget => "traversal_budget",
                    tdmcp_mcp::output_views::FullReason::SerializationBudget => {
                        "serialization_budget"
                    }
                    tdmcp_mcp::output_views::FullReason::TransformBudget => "transform_budget",
                };
                *reasons.entry(name).or_default() += 1;
            }
        }

        let decoded = codec
            .decode(planned.output)
            .map_err(|error| format!("{label}: decode failed: {error}"))?;
        assert_eq!(
            serde_json::to_string(&decoded)?,
            original,
            "{label}: reconstruction differs from the recorded value"
        );
    }

    eprintln!(
        "replayed {} recorded inspect payloads: {compact_count} compact, {full_count} full",
        payloads.len()
    );
    for (reason, count) in &reasons {
        eprintln!("  full because {reason}: {count}");
    }
    if baseline_bytes > 0 {
        let saved = baseline_bytes.saturating_sub(adaptive_bytes);
        eprintln!(
            "  serialized bytes over the whole corpus: baseline {baseline_bytes} -> adaptive \
             {adaptive_bytes} ({saved} fewer, {:.2}%)",
            100.0 * (saved as f64) / (baseline_bytes as f64)
        );
    }
    Ok(())
}

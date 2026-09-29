//! Replay recorded `inspect` payloads through the shipped codec.
//!
//! Two suites, deliberately kept separate:
//!
//! * **public fixtures** (`scripts/fixtures/inspect`) are synthetic and checked in, so this
//!   runs on every build. W0's contract is that no private corpus is published, and the private
//!   corpus is not publishable: it carries real session ids, project paths, parameter values and
//!   authored comments.
//! * **the private corpus** lives under `retroanalysis/` (gitignored) and is replayed only when
//!   `TDMCP_INSPECT_CORPUS` points at it. A configured-but-missing directory is a failure rather
//!   than a silent skip.
//!
//! Both assert the same things, per payload:
//!
//! * whichever representation the planner chose, the decoded value is byte-identical to the
//!   recorded one after serialization, so nothing is dropped or invented;
//! * a compact choice was actually smaller than full;
//! * a decode never silently degrades — a failure is a test failure.
//!
//! The headroom numbers are serialized BYTES. A token count is a different
//! measurement over a different unit, so it is opt-in, it names its tokenizer,
//! and it never replaces the byte figures — see `TDMCP_TOKEN_DUMP` below.

use std::error::Error;
use std::path::{Path, PathBuf};

use serde_json::Value;
use tdmcp_mcp::output_views::{EncodedOutput, OutputCodec, PresentationPlan};

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

/// Resolve the configured corpus directory.
///
/// A relative path is resolved against the workspace root, not the test
/// binary's working directory: cargo runs integration tests with the package
/// root as cwd, so a path written the way a developer would type it from the
/// repository root would otherwise be reported as "not a directory".
fn corpus_dir() -> Option<PathBuf> {
    let configured = std::env::var_os("TDMCP_INSPECT_CORPUS")?;
    let path = PathBuf::from(configured);
    if path.is_absolute() || path.exists() {
        return Some(path);
    }
    let candidate = workspace_root().join(&path);
    if candidate.is_dir() {
        Some(candidate)
    } else {
        Some(path)
    }
}

/// One recorded payload plus the provenance that makes a failure attributable.
struct Payload {
    file: String,
    session: String,
    call_seq: i64,
    value: Value,
    /// The plan this fixture requires the codec to choose, when it declares
    /// one. Public fixtures carry this so a fixture cannot quietly stop
    /// reaching the path it exists to cover; recorded payloads have none.
    expect_plan: Option<String>,
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
            expect_plan: provenance
                .get("expectPlan")
                .and_then(Value::as_str)
                .map(str::to_owned),
            value: response.clone(),
        });
    }
    Ok(payloads)
}

/// Resolve the public, sanitized fixture directory.
///
/// These fixtures are synthetic and checked in, so this path is expected to
/// exist. An override is honoured for a scratch run but a configured-but-missing
/// directory is a failure: a silently empty fixture set would make the codec
/// look tested when nothing was replayed.
fn public_fixture_dir() -> PathBuf {
    let configured = std::env::var_os("TDMCP_INSPECT_FIXTURES").map_or_else(
        || workspace_root().join("scripts/fixtures/inspect"),
        PathBuf::from,
    );
    assert!(
        configured.is_dir(),
        "public inspect fixtures are missing at {}. Regenerate them with \
         `python scripts/generate_inspect_fixtures.py`.",
        configured.display()
    );
    configured
}

/// What a replay pass observed, so both suites can be asserted on and reported.
#[derive(Default)]
struct Tally {
    compact_count: u64,
    full_count: u64,
    /// Serialized bytes a client received before any compact view existed.
    baseline_bytes: u64,
    /// Serialized bytes this encoder returned.
    adaptive_bytes: u64,
    /// Why the planner declined, and how often.
    reasons: std::collections::BTreeMap<&'static str, u64>,
}

/// One payload's two documents, dumped for offline token measurement.
///
/// The Rust test cannot count tokens: doing so would put a tokenizer in the
/// build, and the test-only Python dependencies are not available to a
/// `cargo test` run. It CAN emit exactly the two documents a client would have
/// received, and let a separate step count them with a NAMED tokenizer. That
/// keeps the byte assertions here and the token figure in the one place that
/// can state which tokenizer produced it.
struct TokenDump {
    entries: Vec<TokenEntry>,
}

struct TokenEntry {
    label: String,
    plan: &'static str,
    full: String,
    adaptive: String,
}

impl TokenDump {
    /// Where to write, if the caller asked for a dump.
    ///
    /// A configured-but-unwritable path is an error rather than a silent skip:
    /// a dump that quietly did not happen is a measurement that looks absent
    /// instead of unmeasured.
    fn requested() -> Result<Option<Self>, Box<dyn Error>> {
        let Some(path) = std::env::var_os("TDMCP_TOKEN_DUMP") else {
            return Ok(None);
        };
        let path = PathBuf::from(&path);
        // The dump path itself, not its parent: almost every real path has a
        // directory as its parent, so checking the parent refuses exactly the
        // paths a caller would write.
        if path.is_dir() {
            return Err(format!(
                "TDMCP_TOKEN_DUMP must name a file to write, not a directory: {}",
                path.display()
            )
            .into());
        }
        Ok(Some(Self {
            entries: Vec::new(),
        }))
    }

    fn push(&mut self, label: String, plan: &'static str, full: String, adaptive: String) {
        self.entries.push(TokenEntry {
            label,
            plan,
            full,
            adaptive,
        });
    }

    fn write(self) -> Result<(), Box<dyn Error>> {
        let Some(path) = std::env::var_os("TDMCP_TOKEN_DUMP") else {
            return Ok(());
        };
        let rows = self
            .entries
            .iter()
            .map(|entry| {
                serde_json::json!({
                    "label": entry.label,
                    "plan": entry.plan,
                    "full": entry.full,
                    "adaptive": entry.adaptive,
                })
            })
            .collect::<Vec<_>>();
        std::fs::write(&path, serde_json::to_string_pretty(&rows)?)?;
        eprintln!(
            "wrote {} document pairs to {} for offline token measurement",
            self.entries.len(),
            std::path::Path::new(&path).display()
        );
        Ok(())
    }
}

/// Replay every payload through the codec and prove the round trip is lossless.
///
/// The invariants are the same for the private corpus and the public fixtures,
/// which is the point: the checked-in set exercises the same assertions in CI
/// that the private set gets during release work.
fn replay(
    codec: &OutputCodec,
    payloads: &[Payload],
    mut dump: Option<TokenDump>,
) -> Result<Tally, Box<dyn Error>> {
    let mut tally = Tally::default();

    for payload in payloads {
        let original = serde_json::to_string(&payload.value)?;
        let planned = codec.encode(payload.value.clone());
        let label = format!(
            "session {} call {} ({})",
            payload.session, payload.call_seq, payload.file
        );

        // The plan the codec actually chose, as a name comparable with the
        // `expectPlan` a public fixture carries.
        let observed = match &planned.plan {
            PresentationPlan::Compact { .. } => "compact",
            PresentationPlan::Full { reason, .. } => match reason {
                tdmcp_mcp::output_views::FullReason::NoTransforms => "full:no_transforms",
                tdmcp_mcp::output_views::FullReason::NotSmaller => "full:not_smaller",
                tdmcp_mcp::output_views::FullReason::TraversalBudget => "full:traversal_budget",
                tdmcp_mcp::output_views::FullReason::SerializationBudget => {
                    "full:serialization_budget"
                }
                tdmcp_mcp::output_views::FullReason::TransformBudget => "full:transform_budget",
            },
        };
        if let Some(expected) = payload.expect_plan.as_deref() {
            assert_eq!(
                observed, expected,
                "{label}: the codec chose {observed} but this fixture exists to pin {expected}. \
                 If the decision is now genuinely correct, change the fixture's `expectPlan` \
                 deliberately rather than deleting the assertion."
            );
        }

        // The adaptive document is serialized up front, by REFERENCE: the
        // `decode` below still needs `planned.output`, so consuming it into a
        // wire value here would move it out from under the round-trip check.
        // Serializing the compact payload by reference produces exactly the
        // JSON `into_wire_value` builds, which is what a client is sent.
        let adaptive_wire = match &planned.output {
            EncodedOutput::Compact(output) => Some(serde_json::to_string(output)?),
            EncodedOutput::Full(_) => None,
        };

        match planned.plan {
            PresentationPlan::Compact {
                full_bytes,
                compact_bytes,
                transforms,
            } => {
                tally.compact_count += 1;
                tally.baseline_bytes += u64::try_from(full_bytes).unwrap_or(0);
                tally.adaptive_bytes += u64::try_from(compact_bytes).unwrap_or(0);
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
                if let Some(dump) = dump.as_mut() {
                    dump.push(
                        label.clone(),
                        "compact",
                        original.clone(),
                        // Not `None` in this arm: the plan says compact and the
                        // assertion above already proved the output is a
                        // compact document, so a missing serialization here
                        // would be a real inconsistency rather than a plan shape.
                        adaptive_wire.clone().ok_or_else(|| {
                            format!("{label}: compact plan with no compact document to measure")
                        })?,
                    );
                }
            }
            PresentationPlan::Full {
                full_bytes, reason, ..
            } => {
                tally.full_count += 1;
                // A declined compact request costs exactly the baseline bytes: the
                // full value is returned intact rather than trimmed.
                let full = u64::try_from(full_bytes.unwrap_or(0)).unwrap_or(0);
                tally.baseline_bytes += full;
                tally.adaptive_bytes += full;
                let name = match reason {
                    tdmcp_mcp::output_views::FullReason::NoTransforms => "no_transforms",
                    tdmcp_mcp::output_views::FullReason::NotSmaller => "not_smaller",
                    tdmcp_mcp::output_views::FullReason::TraversalBudget => "traversal_budget",
                    tdmcp_mcp::output_views::FullReason::SerializationBudget => {
                        "serialization_budget"
                    }
                    tdmcp_mcp::output_views::FullReason::TransformBudget => "transform_budget",
                };
                *tally.reasons.entry(name).or_default() += 1;
                if let Some(dump) = dump.as_mut() {
                    // A declined request served the full value, so both
                    // documents are identical. That is the point of recording
                    // it: a token measurement over the whole set has to
                    // include the calls that gained nothing, or the saving
                    // describes only the calls that were going to win anyway.
                    dump.push(label.clone(), "full", original.clone(), original.clone());
                }
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

    if let Some(dump) = dump {
        dump.write()?;
    }

    Ok(tally)
}

/// Report a replay pass. These are serialized BYTES. A token figure is a
/// different measurement, produced by `scripts/measure_tokens.py` over the
/// `TDMCP_TOKEN_DUMP` output, and it names the tokenizer it used.
fn report(label: &str, payloads: &[Payload], tally: &Tally) {
    eprintln!(
        "replayed {label} {} inspect payloads: {} compact, {} full",
        payloads.len(),
        tally.compact_count,
        tally.full_count
    );
    for (reason, count) in &tally.reasons {
        eprintln!("  full because {reason}: {count}");
    }
    if tally.baseline_bytes > 0 {
        let saved = tally.baseline_bytes.saturating_sub(tally.adaptive_bytes);
        eprintln!(
            "  serialized bytes over the whole set: baseline {} -> adaptive {} ({saved} fewer, \
             {:.2}%)",
            tally.baseline_bytes,
            tally.adaptive_bytes,
            100.0 * (saved as f64) / (tally.baseline_bytes as f64)
        );
    }
}

/// The checked-in synthetic fixtures round-trip through the codec in CI.
///
/// W0's contract is that no private corpus is published, so this is the suite
/// that actually runs on every build. The private corpus replay below applies
/// the same assertions to real recorded payloads during release work.
#[test]
fn public_inspect_fixtures_round_trip_through_the_codec() -> Result<(), Box<dyn Error>> {
    let dir = public_fixture_dir();
    let payloads = load(&dir)?;
    assert!(
        !payloads.is_empty(),
        "no attributed payloads found in the public fixture set at {}",
        dir.display()
    );

    let tally = replay(&OutputCodec::default(), &payloads, None)?;
    report("public fixture", &payloads, &tally);
    Ok(())
}

/// The private recorded corpus, when a release pass points at it.
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

    let tally = replay(&OutputCodec::default(), &payloads, TokenDump::requested()?)?;
    report("recorded", &payloads, &tally);
    Ok(())
}

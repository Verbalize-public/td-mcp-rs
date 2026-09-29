//! Runtime composition of the opt-in version 2 output contract.
//!
//! [`tdmcp_core::output`] defines the data contract. This module is the first
//! real producer for it: it wraps a legacy tool response in the typed identity,
//! window, coverage, and operation outcome that the contract requires, without
//! changing any legacy response.
//!
//! Three rules constrain everything here.
//!
//! * **Opt-in only.** A request without `output` gets the legacy value back,
//!   byte for byte. Nothing in this module runs unless the caller asked.
//! * **No invented evidence.** A field the backend did not report is
//!   `unavailable` with a reason, never a default, a zero, or an empty
//!   collection. The contract exists to keep those five states apart.
//! * **An interval, not a snapshot.** [`ObservationWindow`]
//!   records a capture interval. Equal start and end frame values do not prove
//!   the scene was still, and the wrapper never claims they do.

use std::sync::atomic::{AtomicU64, Ordering};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use tdmcp_core::{
    CapabilityManifest, CapabilityState, CapabilitySupport, EffectState, ExtensionData,
    FingerprintStrength, HashIdentity, MonotonicWindow, ObservationCoverage, ObservationIdentity,
    ObservationTarget, ObservationWindow, ObservedValue, OperationOutcome, OperationState,
    OutputContract, OutputOptions, OutputView, Pid, PidCorrelation, PidEntry, TargetFingerprint,
};
use uuid::Uuid;

use crate::output_views::{
    EncodedOutput, FullReason, OutputCodec, PlannedOutput, PresentationPlan,
};

/// Process-local monotonic clock, scoped by daemon instance identity.
///
/// Monotonic time is the only clock here that cannot be moved backwards by an
/// NTP step. The identity field is the clock's scope: two daemons may both
/// report `started: 0` without describing the same interval.
static CLOCK_START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
static OBSERVATION_SEQ: AtomicU64 = AtomicU64::new(0);
static DAEMON_INSTANCE: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Nanoseconds elapsed since this daemon process started.
#[must_use]
pub fn monotonic_nanos() -> u64 {
    let start = CLOCK_START.get_or_init(std::time::Instant::now);
    u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

/// Identity of this daemon process boot, distinct from any stable routing id.
///
/// Two daemons on one machine can share a routing name; they cannot share a
/// boot. Callers comparing a retained capture against a later one need this
/// to tell "the same process re-answered" from "some daemon re-answered".
///
/// The value is fixed for the lifetime of the process. Minting a fresh
/// identifier per call would make every capture claim a different daemon and
/// would defeat the comparison this exists to support.
#[must_use]
pub fn daemon_instance_id() -> &'static str {
    DAEMON_INSTANCE.get_or_init(|| format!("daemon-{}-{}", std::process::id(), Uuid::new_v4()))
}

/// SHA-256 over a canonicalized value, as a fully qualified digest identity.
///
/// The algorithm and encoding travel with the value on purpose: a bare hex
/// string cannot be compared against a base64url one, and comparing digests
/// from different algorithms would silently pass or fail at random.
#[must_use]
pub fn hash_identity(value: &Value) -> HashIdentity {
    let bytes = serde_json::to_vec(value).unwrap_or_default();
    let digest = Sha256::digest(&bytes);
    HashIdentity {
        algorithm: "sha256".to_owned(),
        encoding: "lower_hex".to_owned(),
        value: digest.iter().map(|b| format!("{b:02x}")).collect(),
        extensions: ExtensionData::new(),
    }
}

/// One call's result: the legacy value plus the metadata the contract needs.
#[derive(Debug, Clone)]
pub struct ObservationRecord {
    /// Hash of the normalized request selection and options.
    pub query_hash: HashIdentity,
    /// Hash of the exact legacy value that is being returned.
    pub payload_hash: HashIdentity,
    /// Per-path accounting derived from the bridge's own node roster.
    pub coverage: ObservationCoverage,
    /// Capture interval.
    pub window: ObservationWindow,
    /// When the capture started, before the bridge call.
    pub started_at: chrono::DateTime<Utc>,
    /// When the capture completed, after the bridge call returned.
    pub completed_at: chrono::DateTime<Utc>,
    /// Monotonic start and completion counters, when captured.
    pub monotonic: Option<(u64, u64)>,
    /// Target identity for the pid the request addressed.
    pub target: ObservationTarget,
    /// Loaded-bridge connection generation, when one is registered.
    pub bridge_generation: Option<u64>,
}

impl ObservationRecord {
    /// Unique capture identity.
    ///
    /// The counter makes the id unique within a daemon boot even for byte
    /// identical payloads, and the daemon instance keeps it unique across
    /// daemons. Two captures with the same payload still have different
    /// `observationId` values, which is the point: a retained capture is an
    /// event, not a content address.
    #[must_use]
    pub fn observation_id(&self) -> String {
        let seq = OBSERVATION_SEQ.fetch_add(1, Ordering::Relaxed);
        format!("{}-{seq}", Uuid::new_v4())
    }

    /// Assemble the identity block for a completed capture.
    #[must_use]
    pub fn identity(&self) -> ObservationIdentity {
        ObservationIdentity {
            observation_id: self.observation_id(),
            contract: OutputContract::V2,
            daemon_instance: daemon_instance_id().to_owned(),
            bridge_generation: self.bridge_generation,
            target: self.target.clone(),
            query_hash: self.query_hash.clone(),
            payload_hash: Some(self.payload_hash.clone()),
            extensions: ExtensionData::new(),
        }
    }
}

/// Build a capture record for a completed legacy call.
///
/// `value` must be the exact legacy value being returned; its digest is what
/// makes a retained capture verifiable later.
#[must_use]
pub fn record_observation(
    query: &Value,
    value: &Value,
    started_at: chrono::DateTime<Utc>,
    completed_at: chrono::DateTime<Utc>,
    monotonic: Option<(u64, u64)>,
    target: ObservationTarget,
    bridge_generation: Option<u64>,
) -> ObservationRecord {
    ObservationRecord {
        query_hash: hash_identity(query),
        payload_hash: hash_identity(value),
        coverage: coverage_from_value(value),
        window: ObservationWindow {
            started_at,
            completed_at,
            monotonic: match monotonic {
                Some((started, done)) => ObservedValue::captured(MonotonicWindow {
                    clock_id: "process_monotonic".to_owned(),
                    started,
                    completed: done,
                    unit: "nanoseconds".to_owned(),
                    extensions: ExtensionData::new(),
                }),
                None => ObservedValue::unavailable("no monotonic interval was captured"),
            },
            target_clock: ObservedValue::unavailable(
                "this tool does not request a target clock or frame interval",
            ),
            extensions: ExtensionData::new(),
        },
        started_at,
        completed_at,
        monotonic,
        target,
        bridge_generation,
    }
}

/// Derive per-path accounting from the bridge's inspect roster.
///
/// `missing` and `unavailable` are counted separately because they mean
/// different things: a path that does not exist is not the same evidence as a
/// path that exists and could not be read.
#[must_use]
pub fn coverage_from_value(value: &Value) -> ObservationCoverage {
    let Value::Object(map) = value else {
        return ObservationCoverage {
            requested: 0,
            accounted_for: 0,
            returned: 0,
            missing: 0,
            unavailable: 0,
            truncated: bool_of(value, "pathsTruncated"),
            extensions: ExtensionData::new(),
        };
    };
    let Some(Value::Array(nodes)) = map.get("nodes") else {
        return ObservationCoverage {
            requested: 0,
            accounted_for: 0,
            returned: 0,
            missing: 0,
            unavailable: 0,
            truncated: bool_of(value, "pathsTruncated"),
            extensions: ExtensionData::new(),
        };
    };

    let mut missing = 0_u64;
    let mut unavailable = 0_u64;
    let mut returned = 0_u64;
    for node in nodes {
        match node.get("ok") {
            Some(Value::Bool(true)) => returned = returned.saturating_add(1),
            // The bridge reports a per-path failure with a code. `not_found`
            // is the only one that means the target does not exist; every
            // other failure is "found but could not be observed".
            Some(Value::Bool(false)) => {
                if node.get("code").and_then(Value::as_str) == Some(OP_NOT_FOUND_CODE) {
                    missing = missing.saturating_add(1);
                } else {
                    unavailable = unavailable.saturating_add(1);
                }
            }
            _ => unavailable = unavailable.saturating_add(1),
        }
    }

    let requested = u64::try_from(nodes.len()).unwrap_or(u64::MAX);
    ObservationCoverage {
        requested,
        accounted_for: returned.saturating_add(missing).saturating_add(unavailable),
        returned,
        missing,
        unavailable,
        truncated: bool_of(value, "pathsTruncated"),
        extensions: ExtensionData::new(),
    }
}

/// The bridge's own not-found code, which coverage accounting must not
/// hardcode in two places.
const OP_NOT_FOUND_CODE: &str = tdmcp_diagnostics::codes::OP_NOT_FOUND;

fn bool_of(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// Target identity for a registry entry, with honest fingerprint strength.
///
/// A fingerprint drawn from a title alone does not rule out pid reuse, so it
/// is reported `weak`; only the daemon's own native start identity earns
/// `strong`. A registered pid with no attributes at all is `missing`, not
/// `weak`, because there is nothing to weigh.
#[must_use]
pub fn target_for(
    entry: Option<&PidEntry>,
    bridge_pid: Pid,
    launcher_pid: Option<Pid>,
) -> ObservationTarget {
    let fingerprint = entry.map_or_else(ProcessFingerprintParts::default, |e| {
        ProcessFingerprintParts {
            title: e.process.fingerprint.title.clone(),
            image: e.process.fingerprint.image.clone(),
            start_time: e.process.fingerprint.start_time.clone(),
        }
    });
    let observed = [
        fingerprint.title.is_some(),
        fingerprint.image.is_some(),
        fingerprint.start_time.is_some(),
    ];
    let strength = match observed {
        [false, false, false] => FingerprintStrength::Missing,
        // No start identity means pid reuse stays plausible regardless of how
        // specific the image path is.
        [_, _, false] => FingerprintStrength::Weak,
        _ => FingerprintStrength::Strong,
    };
    ObservationTarget {
        bridge_pid,
        launcher_pid,
        // Nothing here compares the two namespaces, so the honest state is
        // unknown. A Wine launcher PID is a different namespace from the
        // Windows PID the bridge reports.
        pid_correlation: PidCorrelation::Unknown,
        fingerprint_strength: strength,
        fingerprint: TargetFingerprint {
            title: fingerprint.title,
            image: fingerprint.image,
            start_time: fingerprint.start_time,
            extensions: ExtensionData::new(),
        },
        extensions: ExtensionData::new(),
    }
}

#[derive(Debug, Default, Clone)]
struct ProcessFingerprintParts {
    title: Option<String>,
    image: Option<String>,
    start_time: Option<String>,
}

/// Representation a caller asked for, and the one actually returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Presentation {
    /// The complete selected data in its ordinary representation.
    Full,
    /// A lossless compact document with an explicit codec header.
    Compact,
}

/// Size accounting behind a representation decision.
///
/// These are counted serialized bytes, not tokens. Nothing in this response
/// may be used to claim a token saving.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresentationBytes {
    /// Counted bytes of the full value, when it fit the budget.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub full: Option<usize>,
    /// Counted bytes of the complete compact document, when one was built.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compact: Option<usize>,
    /// Number of control transforms in the compact document.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transforms: Option<usize>,
}

/// What the caller asked for, what was returned, and why they differ.
///
/// A request for `compact` that receives the full value is recorded as such
/// rather than passed off as a compact response. The planner is allowed to
/// keep the full value, but the caller has to be able to see that it did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresentationBlock {
    /// Representation the caller requested.
    pub requested: Presentation,
    /// Representation actually returned.
    pub applied: Presentation,
    /// Stable reason the full value was kept despite a compact request.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub full_reason: Option<&'static str>,
    /// Size accounting for the decision.
    pub bytes: PresentationBytes,
}

/// Wrap a legacy value in the version 2 envelope.
///
/// `legacy` is the exact value a non-opt-in caller would have received, and it
/// is what `payloadHash` in the identity digests. When a compact document is
/// returned under `data`, decoding it yields exactly this value, so the digest
/// still verifies against what the caller reconstructs.
///
/// Fails only when a selected compact document cannot be serialized. That is
/// reported rather than downgraded to the full value, because returning full
/// data under a `view: "compact"` request would satisfy the request on paper
/// while giving the caller a different representation than the one it asked
/// for.
pub fn wrap_v2(
    record: &ObservationRecord,
    legacy: Value,
    options: &OutputOptions,
) -> Result<Value, crate::output_views::CodecError> {
    let identity = record.identity();
    let outcome = OperationOutcome {
        state: OperationState::Completed,
        // A read ran to completion. It reports no effect because it made
        // none, which is different from not being able to tell.
        effect: EffectState::None,
        extensions: ExtensionData::new(),
    };

    // Omission requests the full representation. Compact is never implicit.
    let requested = match options.view {
        Some(OutputView::Compact) => Presentation::Compact,
        _ => Presentation::Full,
    };
    let planned = match requested {
        // An explicit full request is not re-planned; a compact document here
        // would contradict what the caller asked for.
        Presentation::Full => None,
        Presentation::Compact => Some(OutputCodec::default().encode(legacy.clone())),
    };

    // Destructure the plan once. `bytes` and `full_reason` are both derived
    // from it, and `PresentationPlan` is not `Copy`.
    let (planned, bytes, full_reason) = match planned {
        Some(PlannedOutput { plan, output }) => {
            let reason = match &plan {
                PresentationPlan::Full { reason, .. } => Some(reason_str(*reason)),
                PresentationPlan::Compact { .. } => None,
            };
            let bytes = match plan {
                PresentationPlan::Full {
                    full_bytes,
                    candidate_bytes,
                    reason: _,
                } => PresentationBytes {
                    full: full_bytes,
                    compact: candidate_bytes,
                    transforms: None,
                },
                PresentationPlan::Compact {
                    full_bytes,
                    compact_bytes,
                    transforms,
                } => PresentationBytes {
                    full: Some(full_bytes),
                    compact: Some(compact_bytes),
                    transforms: Some(transforms),
                },
            };
            (Some(output), bytes, reason)
        }
        None => (
            None,
            PresentationBytes {
                full: None,
                compact: None,
                transforms: None,
            },
            None,
        ),
    };

    // The representation actually returned is decided by `output` alone, not
    // by the planner's intent, so an unexpected pairing degrades to a reported
    // full value instead of trusting an invariant the types do not enforce.
    let (data, applied) = match planned {
        Some(EncodedOutput::Compact(compact)) => {
            // A compact document that cannot be serialized is reported as an
            // error. Falling back to the full value here would hand back a
            // representation the caller did not ask for while reporting the
            // request as satisfied.
            let wire = serde_json::to_value(compact).map_err(|error| {
                crate::output_views::CodecError::Serialization(error.to_string())
            })?;
            (wire, Presentation::Compact)
        }
        Some(EncodedOutput::Full(full)) => (full, Presentation::Full),
        None => (legacy, Presentation::Full),
    };
    let presentation = PresentationBlock {
        requested,
        applied,
        full_reason,
        bytes,
    };

    // Feed the process-wide tally. The per-response block above is what a single
    // caller sees; this is what makes the aggregate answerable, and it is
    // recorded in serialized bytes only — see `crate::cost`.
    crate::cost::global_ledger().record(crate::cost::Observation {
        requested_compact: requested == Presentation::Compact,
        applied_compact: applied == Presentation::Compact,
        served_bytes: presentation.bytes.compact.or(presentation.bytes.full),
        full_bytes: presentation.bytes.full,
        full_reason,
    });

    let mut out = Map::new();
    out.insert("contract".to_owned(), Value::String("v2".to_owned()));
    out.insert("identity".to_owned(), to_value(&identity));
    out.insert("window".to_owned(), to_value(&record.window));
    out.insert("coverage".to_owned(), to_value(&record.coverage));
    out.insert("outcome".to_owned(), to_value(&outcome));
    out.insert("presentation".to_owned(), to_value(&presentation));
    out.insert("capabilities".to_owned(), to_value(&manifest_for(applied)));
    out.insert("data".to_owned(), data);
    Ok(Value::Object(out))
}

fn reason_str(reason: FullReason) -> &'static str {
    match reason {
        FullReason::NoTransforms => "no_transforms",
        FullReason::NotSmaller => "not_smaller",
        FullReason::TraversalBudget => "traversal_budget",
        FullReason::SerializationBudget => "serialization_budget",
        FullReason::TransformBudget => "transform_budget",
    }
}

/// Capability manifest matching what this producer actually served.
///
/// `compact_view` is reported supported only when a compact document was
/// really returned. When the planner kept the full value, the caller can see
/// both that in `presentation` and that the capability was not exercised.
fn manifest_for(applied: Presentation) -> CapabilityManifest {
    let mut manifest = supported_manifest();
    manifest.compact_view = match applied {
        Presentation::Compact => CapabilitySupport {
            state: CapabilityState::Supported,
            reason: Some("a compact document was returned for this request".to_owned()),
            extensions: ExtensionData::new(),
        },
        Presentation::Full => CapabilitySupport {
            state: CapabilityState::Unknown,
            reason: Some(
                "the compact representation was not returned for this request; see presentation"
                    .to_owned(),
            ),
            extensions: ExtensionData::new(),
        },
    };
    manifest
}

fn to_value<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

/// Reject a request that asks for retention the backend does not provide.
///
/// `retain: true` must not be answered with an inline-only capture: the caller
/// asked for something durable, and quietly returning a value that will be
/// dropped is exactly the silent substitution the contract forbids.
///
/// Whether the backend *can* honour it is a per-call fact, not a property of
/// the build: retention needs a store bound to an authenticated principal, and
/// a call with no client identity has neither. The producer therefore consults
/// this only when it has no store for this call, and a producer holding a store
/// asks it instead of asking this.
#[must_use]
pub fn retention_rejection(options: &OutputOptions) -> Option<&'static str> {
    if options.retain == Some(true) {
        Some(
            "retain:true is not available for this call: the request carried no client \
             identity, so there is no principal to scope a retained observation to",
        )
    } else {
        None
    }
}

/// Capability manifest for a backend that serves the v2 inspect contract.
///
/// `output_v2` is supported because this module can compose it. Everything
/// else stays `unknown`: the evidence store, snapshot paging, guarded writes,
/// and sampling either have no caller-facing route yet or are not reachable
/// from this producer, and a manifest that claims them would be advertising
/// capability the backend has not been shown to have.
#[must_use]
pub fn supported_manifest() -> CapabilityManifest {
    CapabilityManifest {
        output_v2: CapabilitySupport {
            state: CapabilityState::Supported,
            reason: Some("v2 inspect envelope is composed by this backend".to_owned()),
            extensions: ExtensionData::new(),
        },
        ..CapabilityManifest::default()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;
    use tdmcp_core::{PidRegistry, ProcessAttrs, ProcessFingerprint};

    use crate::output_views::CompactOutput;

    fn legacy_inspect() -> Value {
        serde_json::json!({
            "ok": true,
            "nodes": [
                {"ok": true, "path": "/project1/a", "params": []},
                {"ok": false, "path": "/project1/missing", "code": "tdmcp.op.not_found"},
                {"ok": false, "path": "/project1/broken", "code": "tdmcp.op.inspect_failed"},
                {"ok": true, "path": "/project1/b"},
            ],
        })
    }

    fn record(value: &Value, entry: Option<&PidEntry>) -> ObservationRecord {
        record_observation(
            &serde_json::json!({"paths": ["/project1"]}),
            value,
            Utc::now(),
            Utc::now(),
            Some((10, 20)),
            target_for(entry, Pid(42), None),
            Some(7),
        )
    }

    fn entry_with(fingerprint: ProcessFingerprint) -> PidRegistry {
        let mut reg = PidRegistry::new();
        reg.handshake(
            42,
            ProcessAttrs {
                fingerprint,
                ..Default::default()
            },
            Some("1".into()),
        );
        reg
    }

    fn options(view: Option<OutputView>) -> OutputOptions {
        OutputOptions {
            contract: OutputContract::V2,
            view,
            retain: None,
        }
    }

    /// Envelope for a request that named no view, which must never compact.
    fn full_envelope(record: &ObservationRecord, legacy: Value) -> Value {
        wrap_v2(record, legacy, &options(None)).unwrap()
    }

    #[test]
    fn coverage_separates_missing_from_unavailable() {
        let coverage = coverage_from_value(&legacy_inspect());
        assert_eq!(coverage.requested, 4);
        assert_eq!(coverage.accounted_for, 4);
        assert_eq!(coverage.returned, 2);
        assert_eq!(coverage.missing, 1);
        // A path that exists but could not be shaped is not "missing". Reading
        // it as missing would tell a caller to stop asking about a live node.
        assert_eq!(coverage.unavailable, 1);
        assert!(!coverage.truncated);
    }

    #[test]
    fn coverage_reports_paths_truncation() {
        let mut value = legacy_inspect();
        value["pathsTruncated"] = Value::Bool(true);
        assert!(coverage_from_value(&value).truncated);
    }

    #[test]
    fn coverage_does_not_invent_accounting_for_a_non_node_value() {
        let coverage = coverage_from_value(&serde_json::json!({"ok": true}));
        assert_eq!(coverage.requested, 0);
        assert_eq!(coverage.accounted_for, 0);
    }

    #[test]
    fn an_empty_fingerprint_is_missing_not_weak() {
        let reg = entry_with(ProcessFingerprint::default());
        let target = target_for(reg.get(42), Pid(42), None);
        assert_eq!(target.fingerprint_strength, FingerprintStrength::Missing);
    }

    #[test]
    fn a_fingerprint_without_start_identity_is_weak() {
        let reg = entry_with(ProcessFingerprint {
            title: Some("proj".into()),
            image: Some("TouchDesigner.exe".into()),
            start_time: None,
        });
        let target = target_for(reg.get(42), Pid(42), None);
        // An exact image path does not rule out pid reuse on its own.
        assert_eq!(target.fingerprint_strength, FingerprintStrength::Weak);
    }

    #[test]
    fn a_fingerprint_with_start_identity_is_strong() {
        let reg = entry_with(ProcessFingerprint {
            title: Some("proj".into()),
            image: Some("TouchDesigner.exe".into()),
            start_time: Some("t0".into()),
        });
        let target = target_for(reg.get(42), Pid(42), None);
        assert_eq!(target.fingerprint_strength, FingerprintStrength::Strong);
    }

    #[test]
    fn pid_correlation_is_unknown_until_something_compares_the_namespaces() {
        let reg = entry_with(ProcessFingerprint::default());
        let target = target_for(reg.get(42), Pid(42), Some(Pid(99)));
        // A Wine launcher pid and the Windows pid the bridge reports are not
        // comparable here, so verified-same would be an invented claim.
        assert_eq!(target.pid_correlation, PidCorrelation::Unknown);
    }

    #[test]
    fn the_envelope_embeds_the_legacy_value_unchanged() {
        let legacy = legacy_inspect();
        let wrapped = full_envelope(&record(&legacy, None), legacy.clone());
        assert_eq!(wrapped["data"], legacy);
        assert_eq!(wrapped["contract"], "v2");
    }

    #[test]
    fn the_envelope_keeps_missing_unavailable_and_captured_distinct() {
        let legacy = legacy_inspect();
        let wrapped = full_envelope(&record(&legacy, None), legacy);
        let window = &wrapped["window"];
        // Captured: the producer really did read a monotonic interval. The
        // tagged form nests the payload under `value`, which is what keeps a
        // captured `null` distinguishable from the absence of a capture.
        assert_eq!(window["monotonic"]["state"], "captured");
        assert_eq!(window["monotonic"]["value"]["started"], 10);
        assert_eq!(window["monotonic"]["value"]["completed"], 20);
        assert_eq!(window["monotonic"]["value"]["unit"], "nanoseconds");
        // Unavailable: nothing was requested, and it is not a captured zero.
        assert_eq!(window["targetClock"]["state"], "unavailable");
        assert_ne!(window["targetClock"]["state"], "captured");
    }

    #[test]
    fn an_absent_monotonic_interval_is_unavailable_not_zero() {
        let record = record_observation(
            &serde_json::json!({}),
            &legacy_inspect(),
            Utc::now(),
            Utc::now(),
            None,
            target_for(None, Pid(1), None),
            None,
        );
        let wrapped = full_envelope(&record, legacy_inspect());
        assert_eq!(wrapped["window"]["monotonic"]["state"], "unavailable");
    }

    #[test]
    fn a_read_reports_no_effect_rather_than_an_unknown_one() {
        let legacy = legacy_inspect();
        let wrapped = full_envelope(&record(&legacy, None), legacy);
        assert_eq!(wrapped["outcome"]["state"], "completed");
        assert_eq!(wrapped["outcome"]["effect"], "none");
    }

    #[test]
    fn the_payload_hash_identifies_the_exact_returned_bytes() {
        let legacy = legacy_inspect();
        let wrapped = full_envelope(&record(&legacy, None), legacy.clone());
        let digest = &wrapped["identity"]["payloadHash"];
        assert_eq!(digest["algorithm"], "sha256");
        assert_eq!(digest["encoding"], "lower_hex");
        assert_eq!(digest["value"], hash_identity(&legacy).value);
    }

    #[test]
    fn two_captures_of_identical_bytes_get_distinct_observation_ids() {
        let legacy = legacy_inspect();
        let first = full_envelope(&record(&legacy, None), legacy.clone());
        let second = full_envelope(&record(&legacy, None), legacy.clone());
        // Same payload hash, different capture identity: a capture is an event.
        assert_eq!(
            first["identity"]["payloadHash"],
            second["identity"]["payloadHash"]
        );
        assert_ne!(
            first["identity"]["observationId"],
            second["identity"]["observationId"]
        );
    }

    #[test]
    fn the_daemon_instance_is_stable_for_the_process_lifetime() {
        assert_eq!(daemon_instance_id(), daemon_instance_id());
    }

    #[test]
    fn the_query_hash_covers_the_selection_that_was_sent() {
        let one = hash_identity(&serde_json::json!({"paths": ["/a"]}));
        let two = hash_identity(&serde_json::json!({"paths": ["/b"]}));
        assert_ne!(one.value, two.value);
    }

    #[test]
    fn retain_true_is_refused_but_omission_and_false_are_not() {
        let retain = |r: Option<bool>| OutputOptions {
            contract: OutputContract::V2,
            view: None,
            retain: r,
        };
        assert!(retention_rejection(&retain(Some(true))).is_some());
        assert!(retention_rejection(&retain(Some(false))).is_none());
        assert!(retention_rejection(&retain(None)).is_none());
        // The refusal is about the CALL, not the document: the same options are
        // refused for a reason naming the missing identity, so a caller can act
        // on it (authenticate) rather than conclude the backend is incapable.
        let reason = retention_rejection(&retain(Some(true))).unwrap();
        assert!(reason.contains("no client identity"), "{reason}");
    }

    #[test]
    fn only_the_composed_contract_is_advertised_as_supported() {
        let manifest = supported_manifest();
        assert_eq!(manifest.output_v2.state, CapabilityState::Supported);
        // Everything else stays unknown. The manifest is built by a producer
        // that cannot serve evidence paging or guarded writes.
        for state in [
            manifest.compact_view.state,
            manifest.evidence_read.state,
            manifest.snapshot_paging.state,
            manifest.guarded_writes.state,
            manifest.numerical_sampling.state,
        ] {
            assert_eq!(state, CapabilityState::Unknown);
        }
    }

    #[test]
    fn the_envelope_reports_capability_state_so_a_caller_can_negotiate() {
        let legacy = legacy_inspect();
        let wrapped = full_envelope(&record(&legacy, None), legacy);
        let capabilities = &wrapped["capabilities"];
        assert_eq!(capabilities["outputV2"]["state"], "supported");
        // Everything this producer cannot serve stays `unknown` on the wire,
        // so a client can tell "not offered" from "not answered yet".
        assert_eq!(capabilities["evidenceRead"]["state"], "unknown");
        assert_eq!(capabilities["snapshotPaging"]["state"], "unknown");
    }

    #[test]
    fn the_envelope_round_trips_through_the_contract_types() {
        let legacy = legacy_inspect();
        let wrapped = full_envelope(&record(&legacy, None), legacy);
        let identity: ObservationIdentity =
            serde_json::from_value(wrapped["identity"].clone()).unwrap();
        assert_eq!(identity.contract, OutputContract::V2);
        assert_eq!(identity.bridge_generation, Some(7));
        assert_eq!(identity.target.bridge_pid.get(), 42);
        let coverage: ObservationCoverage =
            serde_json::from_value(wrapped["coverage"].clone()).unwrap();
        assert_eq!(coverage.requested, 4);
    }

    /// A payload the version 1 codec genuinely factors into a compact document.
    ///
    /// Measured on this codec: 4 nodes x 3 params is `NotSmaller` (553 vs 721)
    /// because the transform legend costs more than the repetition saves, while
    /// 4 x 8 is `Compact` (1113 vs 1101). The compact view is chosen by
    /// measurement, not assumed, which is why the fixture is sized to the
    /// point where factoring actually pays.
    fn fat_legacy() -> Value {
        let node = |index: usize| {
            let params: Vec<Value> = (0..8)
                .map(|p| serde_json::json!({"name": format!("parameter_name_{p}")}))
                .collect();
            serde_json::json!({
                "ok": true,
                "path": format!("/project1/node_{index}"),
                "params": params,
            })
        };
        serde_json::json!({
            "ok": true,
            "nodes": [node(0), node(1), node(2), node(3)],
        })
    }

    #[test]
    fn omitting_the_view_returns_full_data_and_never_compacts() {
        let legacy = fat_legacy();
        let wrapped = full_envelope(&record(&legacy, None), legacy.clone());
        assert_eq!(wrapped["presentation"]["requested"], "full");
        assert_eq!(wrapped["presentation"]["applied"], "full");
        assert_eq!(wrapped["data"], legacy);
        // No byte accounting was performed for a representation nobody asked for.
        assert!(wrapped["presentation"]["bytes"].get("full").is_none());
    }

    #[test]
    fn a_compact_request_that_compacts_reports_both_and_decodes_back_exactly() {
        let legacy = fat_legacy();
        let wrapped = wrap_v2(
            &record(&legacy, None),
            legacy.clone(),
            &options(Some(OutputView::Compact)),
        )
        .unwrap();
        let presentation = &wrapped["presentation"];
        assert_eq!(presentation["requested"], "compact");
        assert_eq!(presentation["applied"], "compact");
        assert!(presentation["fullReason"].is_null());

        let full_bytes = presentation["bytes"]["full"].as_u64().unwrap();
        let compact_bytes = presentation["bytes"]["compact"].as_u64().unwrap();
        assert!(
            compact_bytes < full_bytes,
            "compact was applied but was not smaller: {compact_bytes} vs {full_bytes}"
        );
        assert!(presentation["bytes"]["transforms"].as_u64().unwrap() > 0);

        // The compact document is what the caller receives, and it decodes back
        // to exactly the value a non-compact caller would have received.
        let decoded = OutputCodec::default()
            .decode_compact(CompactOutput::from_wire_value(wrapped["data"].clone()).unwrap())
            .unwrap();
        assert_eq!(decoded, legacy);
    }

    #[test]
    fn a_compact_request_that_cannot_compact_is_reported_not_passed_off_as_compact() {
        // A payload with nothing the codec can factor: one node, no roster.
        let legacy = serde_json::json!({"ok": true, "nodes": [{"ok": true, "path": "/p"}]});
        let wrapped = wrap_v2(
            &record(&legacy, None),
            legacy.clone(),
            &options(Some(OutputView::Compact)),
        )
        .unwrap();
        let presentation = &wrapped["presentation"];
        // The caller asked for compact and received full. That must be visible.
        assert_eq!(presentation["requested"], "compact");
        assert_eq!(presentation["applied"], "full");
        assert!(!presentation["fullReason"].is_null());
        // The full value is intact, not trimmed to make compact look plausible.
        assert_eq!(wrapped["data"], legacy);
    }

    #[test]
    fn compact_view_is_only_advertised_as_supported_when_it_was_returned() {
        let legacy = fat_legacy();
        let compact = wrap_v2(
            &record(&legacy, None),
            legacy.clone(),
            &options(Some(OutputView::Compact)),
        )
        .unwrap();
        assert_eq!(compact["capabilities"]["compactView"]["state"], "supported");

        let full = full_envelope(&record(&legacy, None), legacy);
        assert_eq!(full["capabilities"]["compactView"]["state"], "unknown");
    }

    #[test]
    fn a_compact_response_keeps_the_payload_hash_verifiable_after_decoding() {
        let legacy = fat_legacy();
        let wrapped = wrap_v2(
            &record(&legacy, None),
            legacy.clone(),
            &options(Some(OutputView::Compact)),
        )
        .unwrap();
        let decoded = OutputCodec::default()
            .decode_compact(CompactOutput::from_wire_value(wrapped["data"].clone()).unwrap())
            .unwrap();
        // The digest covers the legacy value, so a caller that reconstructs it
        // from the compact document can still verify what it received.
        assert_eq!(
            wrapped["identity"]["payloadHash"]["value"],
            hash_identity(&decoded).value
        );
    }
}

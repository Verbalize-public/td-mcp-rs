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
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use tdmcp_core::{
    CapabilityManifest, CapabilityState, CapabilitySupport, EffectState, ExtensionData,
    FingerprintStrength, HashIdentity, MonotonicWindow, ObservationCoverage, ObservationIdentity,
    ObservationTarget, ObservationWindow, ObservedValue, OperationOutcome, OperationState,
    OutputContract, OutputOptions, Pid, PidCorrelation, PidEntry, TargetFingerprint,
};
use uuid::Uuid;

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

/// Wrap a legacy value in the version 2 envelope.
///
/// The legacy value is embedded under `data` unchanged; the identity,
/// window, coverage, and outcome blocks are siblings. A caller that wants the
/// old shape can read `data` and see exactly what it would have seen.
#[must_use]
pub fn wrap_v2(record: &ObservationRecord, legacy: Value) -> Value {
    let identity = record.identity();
    let outcome = OperationOutcome {
        state: OperationState::Completed,
        // A read ran to completion. It reports no effect because it made
        // none, which is different from not being able to tell.
        effect: EffectState::None,
        extensions: ExtensionData::new(),
    };
    let mut out = Map::new();
    out.insert("contract".to_owned(), Value::String("v2".to_owned()));
    out.insert("identity".to_owned(), to_value(&identity));
    out.insert("window".to_owned(), to_value(&record.window));
    out.insert("coverage".to_owned(), to_value(&record.coverage));
    out.insert("outcome".to_owned(), to_value(&outcome));
    out.insert("capabilities".to_owned(), to_value(&supported_manifest()));
    out.insert("data".to_owned(), legacy);
    Value::Object(out)
}

fn to_value<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

/// Reject a request that asks for retention the backend does not provide.
///
/// `retain: true` must not be answered with an inline-only capture: the caller
/// asked for something durable, and quietly returning a value that will be
/// dropped is exactly the silent substitution the contract forbids.
#[must_use]
pub fn retention_rejection(options: &OutputOptions) -> Option<&'static str> {
    if options.retain == Some(true) {
        Some(
            "retain:true is not available: this build captures in memory only and no \
             permitted reader route exists for a retained projection",
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
        let wrapped = wrap_v2(&record(&legacy, None), legacy.clone());
        assert_eq!(wrapped["data"], legacy);
        assert_eq!(wrapped["contract"], "v2");
    }

    #[test]
    fn the_envelope_keeps_missing_unavailable_and_captured_distinct() {
        let legacy = legacy_inspect();
        let wrapped = wrap_v2(&record(&legacy, None), legacy);
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
        let wrapped = wrap_v2(&record, legacy_inspect());
        assert_eq!(wrapped["window"]["monotonic"]["state"], "unavailable");
    }

    #[test]
    fn a_read_reports_no_effect_rather_than_an_unknown_one() {
        let legacy = legacy_inspect();
        let wrapped = wrap_v2(&record(&legacy, None), legacy);
        assert_eq!(wrapped["outcome"]["state"], "completed");
        assert_eq!(wrapped["outcome"]["effect"], "none");
    }

    #[test]
    fn the_payload_hash_identifies_the_exact_returned_bytes() {
        let legacy = legacy_inspect();
        let wrapped = wrap_v2(&record(&legacy, None), legacy.clone());
        let digest = &wrapped["identity"]["payloadHash"];
        assert_eq!(digest["algorithm"], "sha256");
        assert_eq!(digest["encoding"], "lower_hex");
        assert_eq!(digest["value"], hash_identity(&legacy).value);
    }

    #[test]
    fn two_captures_of_identical_bytes_get_distinct_observation_ids() {
        let legacy = legacy_inspect();
        let first = wrap_v2(&record(&legacy, None), legacy.clone());
        let second = wrap_v2(&record(&legacy, None), legacy.clone());
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
        let wrapped = wrap_v2(&record(&legacy, None), legacy);
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
        let wrapped = wrap_v2(&record(&legacy, None), legacy);
        let identity: ObservationIdentity =
            serde_json::from_value(wrapped["identity"].clone()).unwrap();
        assert_eq!(identity.contract, OutputContract::V2);
        assert_eq!(identity.bridge_generation, Some(7));
        assert_eq!(identity.target.bridge_pid.get(), 42);
        let coverage: ObservationCoverage =
            serde_json::from_value(wrapped["coverage"].clone()).unwrap();
        assert_eq!(coverage.requested, 4);
    }
}

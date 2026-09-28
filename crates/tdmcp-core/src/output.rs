//! Pure data contracts for opt-in version 2 tool output.
//!
//! These types do not change legacy result shapes. Producers may compose only
//! the pieces a result needs instead of wrapping every response in a universal
//! envelope.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Pid;

/// Opaque JSON fields retained for forwards-compatible output decoding.
///
/// Input option types deliberately do not use this alias: unknown request
/// options are errors rather than silently accepted extensions.
pub type ExtensionData = BTreeMap<String, Value>;

/// Output contract selected by an opt-in request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum OutputContract {
    /// Typed version 2 output.
    V2,
}

/// Representation requested for a version 2 result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OutputView {
    /// Complete selected data using its ordinary representation.
    Full,
    /// A lossless readable compact representation when supported and smaller.
    Compact,
}

/// Per-request opt-in to version 2 output.
///
/// Omission of this entire object keeps the existing legacy response. The
/// options are strict so a misspelled or future unsupported option cannot
/// silently change response fidelity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutputOptions {
    /// Requested response contract. Currently only `v2` is defined.
    pub contract: OutputContract,
    /// Requested representation. Omission requests the full representation;
    /// it never implicitly enables compact output.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub view: Option<OutputView>,
    /// Request retention of the captured projection. Omission does not request
    /// retention; `Some(false)` records an explicit inline-only preference. A
    /// producer must reject `Some(true)` when retention is not implemented or
    /// no permitted reader route exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retain: Option<bool>,
}

/// Availability of one requested field and, when captured, its exact value.
///
/// The tagged representation keeps an unavailable or unrequested field
/// distinct from a captured JSON `null`, `false`, zero, or empty collection.
/// Unknown fields within a known state are retained as opaque JSON.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ObservedValue<T> {
    /// The caller did not ask the producer to observe this field.
    Unrequested {
        /// Forward-compatible metadata retained without interpretation.
        #[serde(flatten)]
        extensions: ExtensionData,
    },
    /// The requested target or member did not exist.
    Missing {
        /// Forward-compatible metadata retained without interpretation.
        #[serde(flatten)]
        extensions: ExtensionData,
    },
    /// The field was requested but could not be observed.
    Unavailable {
        /// Stable human-readable reason; diagnostics may carry richer detail.
        reason: String,
        /// Forward-compatible metadata retained without interpretation.
        #[serde(flatten)]
        extensions: ExtensionData,
    },
    /// The field was captured, including when the value is JSON `null`.
    Captured {
        /// Exact captured value.
        value: T,
        /// Forward-compatible metadata retained without interpretation.
        #[serde(flatten)]
        extensions: ExtensionData,
    },
}

impl<T> ObservedValue<T> {
    /// Construct a captured value without extension fields.
    pub fn captured(value: T) -> Self {
        Self::Captured {
            value,
            extensions: ExtensionData::new(),
        }
    }

    /// Construct an unrequested value without extension fields.
    #[must_use]
    pub fn unrequested() -> Self {
        Self::Unrequested {
            extensions: ExtensionData::new(),
        }
    }

    /// Construct a missing value without extension fields.
    #[must_use]
    pub fn missing() -> Self {
        Self::Missing {
            extensions: ExtensionData::new(),
        }
    }

    /// Construct an unavailable value without extension fields.
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self::Unavailable {
            reason: reason.into(),
            extensions: ExtensionData::new(),
        }
    }
}

/// A digest together with the algorithm and representation it identifies.
///
/// The value alone is never sufficient identity: callers must compare the
/// algorithm and encoding too.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HashIdentity {
    /// Hash algorithm name and version, for example `sha256`.
    pub algorithm: String,
    /// Encoding of `value`, for example `lower_hex` or `base64url`.
    pub encoding: String,
    /// Encoded digest bytes.
    pub value: String,
    /// Forward-compatible metadata retained without interpretation.
    #[serde(flatten)]
    pub extensions: ExtensionData,
}

/// Evidentiary strength of a process fingerprint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FingerprintStrength {
    /// No usable process fingerprint was observed.
    Missing,
    /// Some identifying fields were observed but PID reuse remains plausible.
    Weak,
    /// The producer verified the process using its strongest available native
    /// identity. This is still scoped to that producer and observation.
    Strong,
}

/// Explicit relationship between a launcher PID and bridge-reported PID.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PidCorrelation {
    /// No verified PID relationship is known. This is the safe default,
    /// including for Wine launchers.
    #[default]
    Unknown,
    /// The producer verified that both identifiers refer to the same process.
    VerifiedSame,
    /// The producer verified that the identifiers refer to different process
    /// identities or PID namespaces.
    VerifiedDifferent,
}

/// Best-effort target fingerprint captured with version 2 observations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TargetFingerprint {
    /// Project identity hint when observed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Process image or executable path when observed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// Opaque OS-specific process start identity when observed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_time: Option<String>,
    /// Forward-compatible metadata retained without interpretation.
    #[serde(flatten)]
    pub extensions: ExtensionData,
}

/// Target identity recorded with an observation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ObservationTarget {
    /// PID reported by or associated with the loaded bridge.
    pub bridge_pid: Pid,
    /// OS process initially launched by the daemon, when one exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub launcher_pid: Option<Pid>,
    /// Verified relationship between bridge and launcher PIDs.
    pub pid_correlation: PidCorrelation,
    /// Explicit quality of `fingerprint`.
    pub fingerprint_strength: FingerprintStrength,
    /// Best-effort fingerprint fields captured for the target.
    pub fingerprint: TargetFingerprint,
    /// Forward-compatible metadata retained without interpretation.
    #[serde(flatten)]
    pub extensions: ExtensionData,
}

/// Identity of one independently captured projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ObservationIdentity {
    /// Opaque identity unique to this capture, even when payload bytes match.
    pub observation_id: String,
    /// Contract used to interpret the capture.
    pub contract: OutputContract,
    /// Daemon process-boot identity, distinct from stable routing identity.
    pub daemon_instance: String,
    /// Loaded bridge connection/build generation when known.
    ///
    /// This records provenance only and does not establish scene freshness.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bridge_generation: Option<u64>,
    /// Target process identity and fingerprint quality.
    pub target: ObservationTarget,
    /// Hash of normalized selection and relevant execution options.
    pub query_hash: HashIdentity,
    /// Hash of exact captured payload bytes when materialized.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload_hash: Option<HashIdentity>,
    /// Forward-compatible metadata retained without interpretation.
    #[serde(flatten)]
    pub extensions: ExtensionData,
}

/// Monotonic capture interval scoped to one named clock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MonotonicWindow {
    /// Clock identity, scoped by the observation's daemon instance.
    pub clock_id: String,
    /// Counter at capture start.
    pub started: u64,
    /// Counter at capture completion.
    pub completed: u64,
    /// Counter unit, for example `nanoseconds`.
    pub unit: String,
    /// Forward-compatible metadata retained without interpretation.
    #[serde(flatten)]
    pub extensions: ExtensionData,
}

/// Target clock/frame evidence actually observed during capture.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TargetClockWindow {
    /// Target clock or path name, such as a TD timeline identifier.
    pub clock: String,
    /// Observed value at capture start.
    pub started: Value,
    /// Observed value at capture completion.
    pub completed: Value,
    /// Forward-compatible metadata retained without interpretation.
    #[serde(flatten)]
    pub extensions: ExtensionData,
}

/// Time window over which a projection was captured.
///
/// This is an interval, not a claim of atomic scene snapshotting. Equal start
/// and completion frame values do not prove that callbacks or edits did not
/// occur.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ObservationWindow {
    /// Human-readable wall-clock capture start.
    pub started_at: DateTime<Utc>,
    /// Human-readable wall-clock capture completion.
    pub completed_at: DateTime<Utc>,
    /// Monotonic interval when captured by the producer.
    pub monotonic: ObservedValue<MonotonicWindow>,
    /// Target-specific clock/frame interval when requested.
    pub target_clock: ObservedValue<TargetClockWindow>,
    /// Forward-compatible metadata retained without interpretation.
    #[serde(flatten)]
    pub extensions: ExtensionData,
}

/// Accounting for the targets or members selected by a query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ObservationCoverage {
    /// Number of items explicitly requested.
    pub requested: u64,
    /// Number accounted for by returned, missing, or unavailable states.
    pub accounted_for: u64,
    /// Number with captured data in this projection.
    pub returned: u64,
    /// Number requested but not found.
    pub missing: u64,
    /// Number found but unavailable to observe.
    pub unavailable: u64,
    /// Whether acquisition or presentation stopped before full accounting.
    pub truncated: bool,
    /// Forward-compatible metadata retained without interpretation.
    #[serde(flatten)]
    pub extensions: ExtensionData,
}

/// Lifecycle state of an operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OperationState {
    /// Accepted by policy and admission checks.
    Admitted,
    /// Waiting for its execution gate.
    Queued,
    /// Journaled and sent to an executor; effects may have begun.
    Dispatched,
    /// Known to be executing.
    Running,
    /// The caller's wait ended while execution may continue.
    WaitExpired,
    /// Execution returned successfully.
    Completed,
    /// Execution returned a failure.
    Failed,
    /// Cancellation reached a terminal state; consult effect state rather than
    /// assuming work was prevented.
    Cancelled,
    /// The producer cannot establish a terminal result after possible dispatch.
    UnknownEffect,
}

/// What is known about externally visible operation effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EffectState {
    /// Effect reporting does not apply to this operation.
    NotApplicable,
    /// Dispatch was verifiably prevented, so the operation did not run.
    PreventedBeforeDispatch,
    /// The operation ran and verified that it made no externally visible change.
    None,
    /// All declared effects were applied.
    Applied,
    /// Some effects were applied and others were not or remain unresolved.
    PartiallyApplied,
    /// Dispatch or execution may have produced effects, but their state is not
    /// known. This state must not trigger automatic replay.
    Unknown,
}

/// Small composable receipt for lifecycle and effect uncertainty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OperationOutcome {
    /// Current lifecycle state.
    pub state: OperationState,
    /// Current effect knowledge.
    pub effect: EffectState,
    /// Forward-compatible metadata retained without interpretation.
    #[serde(flatten)]
    pub extensions: ExtensionData,
}

/// Support state for one negotiated capability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CapabilitySupport {
    /// Whether support is established for the owning backend and loaded bridge.
    pub state: CapabilityState,
    /// Explanation for unsupported or unknown support when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Forward-compatible metadata retained without interpretation.
    #[serde(flatten)]
    pub extensions: ExtensionData,
}

impl Default for CapabilitySupport {
    fn default() -> Self {
        Self {
            state: CapabilityState::Unknown,
            reason: None,
            extensions: ExtensionData::new(),
        }
    }
}

impl CapabilitySupport {
    /// Construct an explicitly unsupported capability with a reason.
    pub fn unsupported(reason: impl Into<String>) -> Self {
        Self {
            state: CapabilityState::Unsupported,
            reason: Some(reason.into()),
            extensions: ExtensionData::new(),
        }
    }
}

/// Negotiated support state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityState {
    /// Support has not been established. This is the safe default.
    #[default]
    Unknown,
    /// The owning backend explicitly does not support the capability.
    Unsupported,
    /// The owning backend and, where needed, loaded bridge support it.
    Supported,
}

/// Bounded capability manifest for the version 2 output workstreams.
///
/// Every built-in capability defaults to [`CapabilityState::Unknown`]. A
/// producer must opt in only after the end-to-end backend route is available;
/// constructing this manifest alone does not advertise support.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityManifest {
    /// Typed version 2 output contract.
    pub output_v2: CapabilitySupport,
    /// Lossless readable compact views.
    pub compact_view: CapabilitySupport,
    /// Owner-scoped immutable evidence retrieval.
    pub evidence_read: CapabilitySupport,
    /// Paging over a materialized immutable projection.
    pub snapshot_paging: CapabilitySupport,
    /// Exact documentation/member lookup.
    pub exact_member_lookup: CapabilitySupport,
    /// Typed per-step guarded writes.
    pub guarded_writes: CapabilitySupport,
    /// Exact typed numerical sampling.
    pub numerical_sampling: CapabilitySupport,
    /// Asynchronous operation status and receipts.
    pub operation_status: CapabilitySupport,
    /// Future capability entries retained without being interpreted as support.
    #[serde(flatten)]
    pub extensions: ExtensionData,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;
    use schemars::schema_for;
    use serde_json::json;

    fn extensions() -> ExtensionData {
        ExtensionData::new()
    }

    #[test]
    fn output_options_are_explicit_and_strict() {
        let options: OutputOptions = serde_json::from_value(json!({
            "contract": "v2",
            "view": "compact",
            "retain": false
        }))
        .unwrap();
        assert_eq!(options.contract, OutputContract::V2);
        assert_eq!(options.view, Some(OutputView::Compact));
        assert_eq!(options.retain, Some(false));
        assert_eq!(serde_json::to_value(options).unwrap()["retain"], false);

        assert!(serde_json::from_value::<OutputOptions>(json!({"contract": "v3"})).is_err());
        assert!(serde_json::from_value::<OutputOptions>(json!({
            "contract": "v2",
            "view": "summary"
        }))
        .is_err());
        assert!(serde_json::from_value::<OutputOptions>(json!({
            "contract": "v2",
            "retian": true
        }))
        .is_err());
    }

    #[test]
    fn observed_values_distinguish_null_false_zero_and_negative_states() {
        let values = vec![
            ObservedValue::unrequested(),
            ObservedValue::missing(),
            ObservedValue::unavailable("getter not present"),
            ObservedValue::captured(Value::Null),
            ObservedValue::captured(Value::Bool(false)),
            ObservedValue::captured(json!(0)),
            ObservedValue::captured(json!([])),
            ObservedValue::captured(json!({
                "futurePayload": {"ordered": [null, false, 0, ""]}
            })),
        ];
        let encoded = serde_json::to_value(&values).unwrap();
        assert_eq!(encoded[0], json!({"state": "unrequested"}));
        assert_eq!(encoded[1], json!({"state": "missing"}));
        assert_eq!(encoded[2]["state"], "unavailable");
        assert_eq!(encoded[3], json!({"state": "captured", "value": null}));
        assert_eq!(encoded[4], json!({"state": "captured", "value": false}));
        assert_eq!(encoded[5], json!({"state": "captured", "value": 0}));
        assert_eq!(encoded[6], json!({"state": "captured", "value": []}));
        assert_eq!(
            encoded[7]["value"],
            json!({"futurePayload": {"ordered": [null, false, 0, ""]}})
        );
        assert_eq!(
            serde_json::from_value::<Vec<ObservedValue<Value>>>(encoded).unwrap(),
            values
        );
        assert!(serde_json::from_value::<ObservedValue<Value>>(json!({
            "state": "captured"
        }))
        .is_err());
        assert!(serde_json::from_value::<ObservedValue<Value>>(json!({
            "state": "future_state",
            "value": null
        }))
        .is_err());
    }

    #[test]
    fn unknown_observation_extensions_survive_roundtrip() {
        let fixture = json!({
            "observationId": "o_1",
            "contract": "v2",
            "daemonInstance": "boot_1",
            "bridgeGeneration": 7,
            "target": {
                "bridgePid": 4321,
                "launcherPid": 1234,
                "pidCorrelation": "unknown",
                "fingerprintStrength": "weak",
                "fingerprint": {"image": "TouchDesigner.exe"},
                "vendorTarget": {"namespace": "wine"}
            },
            "queryHash": {
                "algorithm": "sha256",
                "encoding": "lower_hex",
                "value": "abcd",
                "vendorHash": [1, null, false]
            },
            "vendorObservation": {"future": 0}
        });
        let observation: ObservationIdentity = serde_json::from_value(fixture.clone()).unwrap();
        let encoded = serde_json::to_value(observation).unwrap();
        assert_eq!(encoded["vendorObservation"], fixture["vendorObservation"]);
        assert_eq!(
            encoded["target"]["vendorTarget"],
            fixture["target"]["vendorTarget"]
        );
        assert_eq!(
            encoded["queryHash"]["vendorHash"],
            fixture["queryHash"]["vendorHash"]
        );
        assert!(encoded.get("payloadHash").is_none());
    }

    #[test]
    fn window_coverage_and_operation_roundtrip() {
        let window = ObservationWindow {
            started_at: "2026-01-01T00:00:00Z".parse().unwrap(),
            completed_at: "2026-01-01T00:00:01Z".parse().unwrap(),
            monotonic: ObservedValue::captured(MonotonicWindow {
                clock_id: "daemon-boot-1".into(),
                started: 10,
                completed: 20,
                unit: "nanoseconds".into(),
                extensions: extensions(),
            }),
            target_clock: ObservedValue::unavailable("target clock was not exposed"),
            extensions: extensions(),
        };
        let coverage = ObservationCoverage {
            requested: 3,
            accounted_for: 3,
            returned: 1,
            missing: 1,
            unavailable: 1,
            truncated: false,
            extensions: extensions(),
        };
        let outcome = OperationOutcome {
            state: OperationState::WaitExpired,
            effect: EffectState::Unknown,
            extensions: extensions(),
        };

        let window_value = serde_json::to_value(&window).unwrap();
        let coverage_value = serde_json::to_value(&coverage).unwrap();
        let outcome_value = serde_json::to_value(&outcome).unwrap();
        assert_eq!(
            serde_json::from_value::<ObservationWindow>(window_value).unwrap(),
            window
        );
        assert_eq!(
            serde_json::from_value::<ObservationCoverage>(coverage_value).unwrap(),
            coverage
        );
        assert_eq!(
            serde_json::from_value::<OperationOutcome>(outcome_value.clone()).unwrap(),
            outcome
        );
        assert_eq!(
            outcome_value,
            json!({"state": "wait_expired", "effect": "unknown"})
        );
        assert!(serde_json::from_value::<OperationState>(json!("future_state")).is_err());
    }

    #[test]
    fn capability_manifest_is_unknown_until_a_producer_opts_in() {
        let manifest = CapabilityManifest::default();
        for capability in [
            &manifest.output_v2,
            &manifest.compact_view,
            &manifest.evidence_read,
            &manifest.snapshot_paging,
            &manifest.exact_member_lookup,
            &manifest.guarded_writes,
            &manifest.numerical_sampling,
            &manifest.operation_status,
        ] {
            assert_eq!(capability.state, CapabilityState::Unknown);
        }

        let mut fixture = serde_json::to_value(manifest).unwrap();
        fixture["futureCapability"] = json!({"state": "supported", "revision": 3});
        let decoded: CapabilityManifest = serde_json::from_value(fixture.clone()).unwrap();
        assert_eq!(
            serde_json::to_value(decoded).unwrap()["futureCapability"],
            fixture["futureCapability"]
        );
        assert_eq!(
            CapabilitySupport::unsupported("old bridge").state,
            CapabilityState::Unsupported
        );
    }

    #[test]
    fn schemas_cover_strict_options_and_composable_contracts() {
        let options = serde_json::to_value(schema_for!(OutputOptions)).unwrap();
        assert_eq!(options["additionalProperties"], false);
        assert!(options["properties"].get("contract").is_some());
        assert!(options["properties"].get("view").is_some());
        assert!(options["properties"].get("retain").is_some());

        for schema in [
            serde_json::to_value(schema_for!(ObservationIdentity)).unwrap(),
            serde_json::to_value(schema_for!(ObservationWindow)).unwrap(),
            serde_json::to_value(schema_for!(ObservationCoverage)).unwrap(),
            serde_json::to_value(schema_for!(OperationOutcome)).unwrap(),
            serde_json::to_value(schema_for!(CapabilityManifest)).unwrap(),
            serde_json::to_value(schema_for!(ObservedValue<Value>)).unwrap(),
        ] {
            assert!(schema.is_object());
            assert!(schema.get("$schema").is_some());
        }
    }
}

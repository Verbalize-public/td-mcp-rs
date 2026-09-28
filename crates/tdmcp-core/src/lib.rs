//! Domain logic for td-mcp-rs: pid registry, task queues, resurrection.
//!
//! This crate has **zero I/O** — no `rmcp`, `axum`, or IPC types.

#![warn(missing_docs)]

mod bridge_identity;
mod bridge_method;
mod dialogs;
mod federation;
mod fingerprint;
mod ids;
mod numeric;
mod output;
mod registry;
mod resurrection;
mod task_queue;

pub use bridge_identity::{
    validate_ready, BridgeBundleSnapshot, BridgeCapabilityError, BridgeCapabilityState,
    BridgeHeartbeatObservation, BridgeIdentitySnapshot, BridgeIdentityState, BridgeObservedAge,
    BridgeProtocolIdentity, BridgeReadyEvent, BridgeRuntimeIdentity, BridgeSourceSnapshot,
    LoadedCodeIdentity,
};
pub use bridge_method::BridgeMethod;
pub use dialogs::{
    DialogError, DialogSeverity, DialogSnapshot, DialogSource, DismissOutcome, NullDialogSource,
    PopupButton, PopupInfo, PopupKind, WindowStatus,
};
pub use federation::{
    AggregatedFleetProcess, DaemonId, DaemonIdConflict, PidResolve, RemoteFleetProcess, SlaveEntry,
    SlaveReachability, SlaveRegistry,
};
pub use fingerprint::ProcessFingerprint;
pub use ids::{OpPath, Pid};
pub use numeric::{LenientU32, LenientU64};
pub use output::{
    CapabilityManifest, CapabilityState, CapabilitySupport, EffectState, ExtensionData,
    FingerprintStrength, HashIdentity, MonotonicWindow, ObservationCoverage, ObservationIdentity,
    ObservationTarget, ObservationWindow, ObservedValue, OperationOutcome, OperationState,
    OutputContract, OutputOptions, OutputView, PidCorrelation, TargetClockWindow,
    TargetFingerprint,
};
pub use registry::{BridgeStatus, EnqueueError, PidEntry, PidRegistry, ProcessAttrs, SpawnRecord};
pub use resurrection::{CancelReason, CancelledTask, ResurrectionState};
pub use task_queue::{QueueError, TaskInfo, TaskMode, TaskQueue, TaskResult};

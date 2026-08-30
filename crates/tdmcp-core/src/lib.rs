//! Domain logic for td-mcp-rs: pid registry, task queues, resurrection.
//!
//! This crate has **zero I/O** — no `rmcp`, `axum`, or IPC types.

#![warn(missing_docs)]

mod bridge_method;
mod dialogs;
mod federation;
mod fingerprint;
mod ids;
mod numeric;
mod registry;
mod resurrection;
mod task_queue;

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
pub use registry::{BridgeStatus, EnqueueError, PidEntry, PidRegistry, ProcessAttrs, SpawnRecord};
pub use resurrection::{CancelReason, CancelledTask, ResurrectionState};
pub use task_queue::{QueueError, TaskInfo, TaskMode, TaskQueue, TaskResult};

//! Domain logic for td-mcp-rs: pid registry, task queues, resurrection.
//!
//! This crate has **zero I/O** — no `rmcp`, `axum`, or IPC types.

#![warn(missing_docs)]

mod bridge_method;
mod federation;
mod fingerprint;
mod ids;
mod registry;
mod resurrection;
mod task_queue;

pub use bridge_method::BridgeMethod;
pub use federation::{
    AggregatedFleetProcess, DaemonId, DaemonIdConflict, PidResolve,
    RemoteFleetProcess, SlaveEntry, SlaveReachability, SlaveRegistry,
};
pub use fingerprint::ProcessFingerprint;
pub use ids::{OpPath, Pid};
pub use registry::{BridgeStatus, EnqueueError, PidEntry, PidRegistry, ProcessAttrs};
pub use resurrection::{CancelReason, CancelledTask, ResurrectionState};
pub use task_queue::{QueueError, TaskInfo, TaskMode, TaskQueue, TaskResult};

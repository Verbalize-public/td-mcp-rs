//! Library target exposing daemon wiring (bridge sessions, admin router,
//! config, tracing init) so integration tests can drive the real accept loop
//! + actor without depending on the binary.

#![warn(missing_docs)]

pub mod admin;
pub mod bridge;
pub mod config;
pub mod tracing_init;

pub use admin::RestartArgs;
pub use bridge::{run_ipc_accept, BridgeSessions};

//! Library target exposing daemon wiring (bridge sessions, admin router,
//! config, tracing init) so integration tests can drive the real accept loop
//! + actor without depending on the binary.

#![warn(missing_docs)]

pub mod admin;
pub mod bridge;
pub mod config;
pub mod ensure;
pub mod install;
pub mod tracing_init;

pub use admin::RestartArgs;
pub use bridge::{
    run_ipc_accept, BridgeSessions, HeartbeatConfig, DISCONNECTED_TTL, HEARTBEAT_INTERVAL,
    IDLE_DEAD, PONG_TIMEOUT,
};
pub use ensure::{
    daemon_lock_path, ensure_daemon, health_ok, pid_alive, read_daemon_lock_pid,
    reclaim_stale_daemon_lock, refuse_if_daemon_owned, EnsureOptions, EnsureResult,
};
pub use install::{default_data_dir, ensure_installed, InstallOutcome};

//! Library target exposing daemon wiring (bridge sessions, admin router,
//! config, tracing init) so integration tests can drive the real accept loop
//! + actor without depending on the binary.

#![warn(missing_docs)]

pub mod admin;
pub mod autostart;
pub mod bridge;
pub mod config;
pub mod ensure;
pub mod http_util;
pub mod idle;
pub mod install;
pub mod tracing_init;

pub use admin::RestartArgs;
pub use bridge::{
    run_ipc_accept, BridgeSessions, BridgeTimeouts, HeartbeatConfig, CALL_TIMEOUT,
    DISCONNECTED_TTL, HEARTBEAT_INTERVAL, IDLE_DEAD, JOB_CHANNEL_CAPACITY, PONG_TIMEOUT,
    SCRIPT_TIMEOUT,
};
pub use ensure::{
    configure_detached_spawn, configure_detached_spawn_with_log, daemon_lock_path, ensure_daemon,
    health_ok, pid_alive, read_daemon_lock_pid, reclaim_stale_daemon_lock, refuse_if_daemon_owned,
    EnsureOptions, EnsureResult,
};
pub use install::{
    copy_daemon_binary, copy_skills_to, default_data_dir, ensure_installed, skills_dir,
    InstallOutcome,
};

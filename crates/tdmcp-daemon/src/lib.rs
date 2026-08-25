//! Library target exposing daemon wiring (bridge sessions, admin router,
//! config, tracing init) so integration tests can drive the real accept loop
//! + actor without depending on the binary.

#![warn(missing_docs)]

pub mod admin;
pub mod autostart;
pub mod bridge;
pub mod config;
pub mod crashreport;
pub mod ensure;
pub mod federation;
pub mod http_util;
pub mod idle;
pub mod install;
pub mod logrecord;
pub mod logring;
pub mod middleware;
pub mod tracing_init;

pub use admin::RestartArgs;
pub use bridge::{
    run_ipc_accept, BridgeSessions, BridgeTimeouts, HeartbeatConfig, CALL_TIMEOUT,
    DISCONNECTED_TTL, HEARTBEAT_INTERVAL, IDLE_DEAD, JOB_CHANNEL_CAPACITY, PONG_TIMEOUT,
    SCRIPT_TIMEOUT,
};
pub use ensure::{
    configure_detached_spawn, daemon_lock_path, ensure_daemon, health_ok, pid_alive,
    read_daemon_lock_pid, reclaim_stale_daemon_lock, refuse_if_daemon_owned, request_shutdown,
    running_version, wait_until_unhealthy, EnsureOptions, EnsureResult,
};
pub use install::{
    copy_daemon_binary, default_data_dir, ensure_installed, render_skills_to, skills_dir,
    verify_installed_version, InstallOutcome,
};
pub use logrecord::{from_line as record_from_line, to_line as record_to_line, Level, Record, Src};
pub use logring::{LogRing, LogSink};

//! Bridge RPC abstraction for tool dispatch.
//!
//! `tdmcp-mcp` owns tool semantics and diagnostics; the actual transport to a
//! live TouchDesigner bridge is supplied by the daemon via this trait. This
//! keeps the IPC wire (named pipe / UDS) out of the MCP crate, per the
//! constitution crate boundaries (`tdmcp-mcp` must not own IPC transports).

use async_trait::async_trait;
use serde_json::Value;
use thiserror::Error;

/// Failures from a bridge RPC call (daemon → TD peer).
#[derive(Debug, Error)]
pub enum BridgeRpcError {
    /// No connected bridge session for the pid, but the registry has seen
    /// this pid before (resurrection is a real possibility — retrying may help).
    #[error("no connected bridge for pid {pid}")]
    NotConnected {
        /// Target pid.
        pid: u32,
    },
    /// No connected bridge session for the pid, and the registry has never
    /// seen it either (never registered, or its record already expired past
    /// the disconnected-pid TTL). Distinct from [`Self::NotConnected`] so the
    /// agent-facing mitigation doesn't suggest waiting for a resurrection
    /// that has nothing to resurrect. See `docs/LIMITS_AUDIT.md` §4.6 / §5
    /// Phase 2.4.
    #[error("pid {pid} was never registered (or its record expired) — call fleet")]
    Unknown {
        /// Target pid.
        pid: u32,
    },
    /// The wait was cancelled because the bridge disconnected mid-call.
    #[error("bridge for pid {pid} disconnected during call")]
    Disconnected {
        /// Target pid.
        pid: u32,
    },
    /// The bridge did not respond within the per-call budget. Per the v1
    /// contract, a timeout fails the **wait** — it does not claim TD cancelled.
    #[error("bridge rpc timed out for pid {pid} after {budget_ms}ms")]
    Timeout {
        /// Target pid.
        pid: u32,
        /// Budget that elapsed, in milliseconds.
        budget_ms: u64,
    },
    /// The bridge returned a structured error payload.
    #[error("bridge error: {message}")]
    BridgeReturned {
        /// Human-readable summary.
        message: String,
        /// Stable `tdmcp.*` code if the bridge supplied one.
        #[allow(dead_code)]
        code: Option<String>,
    },
}

/// Send a method call to a connected TD bridge for a given pid.
///
/// Implementations must be cheap to clone and safe to share across axum tasks.
#[async_trait]
pub trait BridgeRpc: Send + Sync {
    /// Invoke `method` with `params` on the bridge owning `pid`.
    ///
    /// Returns the bridge result value on success, or a [`BridgeRpcError`] on
    /// transport/timeout failure. A bridge-returned structured error (e.g. a
    /// script execution failure) is delivered as `Ok` with the error payload
    /// inside the `Value`, so the tool layer can map it to diagnostics.
    async fn call(&self, pid: u32, method: &str, params: Value) -> Result<Value, BridgeRpcError>;

    /// Approximate depth of jobs waiting in the per-pid actor inbox (not yet
    /// in-flight on the registry queue). Default: unknown / not applicable.
    async fn job_queue_depth(&self, _pid: u32) -> Option<usize> {
        None
    }
}

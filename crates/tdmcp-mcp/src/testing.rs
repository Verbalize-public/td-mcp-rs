//! Test fakes for the bridge RPC layer.
//!
//! Public so the daemon's integration tests and this crate's own tests share
//! one fake. Not part of the production surface.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::Mutex;

use crate::bridge_rpc::{BridgeRpc, BridgeRpcError};

/// A fake bridge that returns a canned value, optionally gated by a held lock
/// so callers can keep a call in-flight (e.g. to exercise exclusive-while-busy).
pub struct FakeBridgeRpc {
    gate: Arc<Mutex<()>>,
    canned: Value,
    /// If set, `call()` returns this error instead of the canned value.
    failure: Option<BridgeRpcFailure>,
}

/// Transport failure kind for the fake.
#[derive(Debug, Clone)]
pub enum BridgeRpcFailure {
    /// Act as if no bridge is connected for this pid.
    NotConnected,
    /// Act as if the bridge disconnected mid-call.
    Disconnected,
    /// Act as if the call timed out.
    Timeout,
}

impl FakeBridgeRpc {
    /// Fake that responds immediately with `result` (clone per call).
    #[must_use]
    pub fn responding(result: Value) -> Self {
        Self {
            gate: Arc::new(Mutex::new(())),
            canned: result,
            failure: None,
        }
    }

    /// Fake that responds with `result` only once the test releases the gate.
    /// Hold the guard returned by [`gate`](Self::gate) to keep calls pending.
    #[must_use]
    pub fn gated(result: Value) -> Self {
        Self {
            gate: Arc::new(Mutex::new(())),
            canned: result,
            failure: None,
        }
    }

    /// Fake that always fails the transport.
    #[must_use]
    pub fn failing(kind: BridgeRpcFailure, pid: u32) -> Self {
        Self {
            gate: Arc::new(Mutex::new(())),
            canned: Value::Null,
            failure: Some(kind),
        }
        .with_pid(pid)
    }

    fn with_pid(self, _pid: u32) -> Self {
        self
    }

    /// Acquire the gate lock to hold all calls in-flight; drop the guard to
    /// release them.
    pub async fn gate(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.gate.lock().await
    }

    /// Clone of the inner gate handle, so a test can hold the gate without
    /// borrowing the fake (the fake can then move into `Arc<dyn BridgeRpc>`).
    #[must_use]
    pub fn gate_handle(&self) -> Arc<Mutex<()>> {
        self.gate.clone()
    }
}

#[async_trait]
impl BridgeRpc for FakeBridgeRpc {
    async fn call(&self, pid: u32, _method: &str, _params: Value) -> Result<Value, BridgeRpcError> {
        let _g = self.gate.lock().await;
        match &self.failure {
            Some(BridgeRpcFailure::NotConnected) => Err(BridgeRpcError::NotConnected { pid }),
            Some(BridgeRpcFailure::Disconnected) => Err(BridgeRpcError::Disconnected { pid }),
            Some(BridgeRpcFailure::Timeout) => Err(BridgeRpcError::Timeout {
                pid,
                budget_ms: 30_000,
            }),
            None => Ok(self.canned.clone()),
        }
    }
}

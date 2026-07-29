//! Per-pid bridge sessions: own the IPC stream and drive queue progression.
//!
//! One actor task per connected TD peer. The actor receives [`TaskJob`]s from
//! the MCP layer, promotes the head pending task to in-flight, sends a framed
//! request, awaits the framed response (with a per-call timeout), records the
//! task outcome on the registry, and replies. Disconnect is detected on the
//! next call attempt; the actor then tears down, calls `on_bridge_lost`, and
//! removes itself from the session map. (Prompt liveness detection without a
//! pending call is a P0.x follow-up.)

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use tdmcp_core::{PidRegistry, ProcessAttrs, TaskResult};
use tdmcp_ipc::{BridgeEndpoint, IpcListener, IpcStream, Message};
use tokio::sync::{mpsc, oneshot, Mutex};
use tracing::{info, warn};

use tdmcp_mcp::{BridgeRpc, BridgeRpcError};

const CALL_TIMEOUT: Duration = Duration::from_secs(30);

static CALL_ID: AtomicU64 = AtomicU64::new(1);

/// One queued tool call awaiting the bridge.
struct TaskJob {
    method: String,
    params: Value,
    reply: oneshot::Sender<Result<Value, BridgeRpcError>>,
}

/// Handle to a per-pid actor's inbox.
#[derive(Clone)]
struct BridgeHandle {
    job_tx: mpsc::Sender<TaskJob>,
}

/// Map of pid → bridge session handle. Cheap to clone (Arc-backed).
#[derive(Clone, Default)]
pub struct BridgeSessions {
    sessions: Arc<Mutex<HashMap<u32, BridgeHandle>>>,
    registry: Arc<Mutex<PidRegistry>>,
}

impl BridgeSessions {
    /// Construct with the shared registry (same Arc the MCP layer uses).
    #[must_use]
    pub fn new(registry: Arc<Mutex<PidRegistry>>) -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            registry,
        }
    }

    /// Spawn an actor for an accepted, handshaken stream.
    pub async fn spawn(&self, pid: u32, stream: IpcStream) {
        let (job_tx, job_rx) = mpsc::channel::<TaskJob>(32);
        {
            let mut s = self.sessions.lock().await;
            s.insert(pid, BridgeHandle { job_tx });
        }
        let sessions = self.sessions.clone();
        let registry = self.registry.clone();
        tokio::spawn(async move {
            run_session(pid, stream, job_rx, registry, sessions).await;
        });
    }
}

#[async_trait]
impl BridgeRpc for BridgeSessions {
    async fn call(&self, pid: u32, method: &str, params: Value) -> Result<Value, BridgeRpcError> {
        let handle = {
            let sessions = self.sessions.lock().await;
            sessions.get(&pid).cloned()
        };
        let Some(handle) = handle else {
            return Err(BridgeRpcError::NotConnected { pid });
        };
        let (tx, rx) = oneshot::channel();
        handle
            .job_tx
            .send(TaskJob {
                method: method.to_owned(),
                params,
                reply: tx,
            })
            .await
            .map_err(|_| BridgeRpcError::Disconnected { pid })?;
        rx.await.map_err(|_| BridgeRpcError::Disconnected { pid })?
    }
}

async fn run_session(
    pid: u32,
    mut stream: IpcStream,
    mut job_rx: mpsc::Receiver<TaskJob>,
    registry: Arc<Mutex<PidRegistry>>,
    sessions: Arc<Mutex<HashMap<u32, BridgeHandle>>>,
) {
    info!(pid, "bridge session started");
    while let Some(job) = job_rx.recv().await {
        // Promote the head pending task (this job, FIFO) to in-flight.
        {
            let mut reg = registry.lock().await;
            let _ = reg.start_next(pid);
        }

        let id = CALL_ID.fetch_add(1, Ordering::Relaxed).to_string();
        let req = Message::Request {
            id: id.clone(),
            method: job.method.clone(),
            params: job.params.clone(),
        };

        let outcome = match stream.send(&req).await {
            Ok(()) => match tokio::time::timeout(CALL_TIMEOUT, stream.recv_message()).await {
                Ok(Ok(Message::Response {
                    id: rid,
                    result,
                    error,
                })) if rid == id => {
                    if let Some(err) = error {
                        let msg = err
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("bridge returned an error")
                            .to_owned();
                        let code = err.get("code").and_then(Value::as_str).map(str::to_owned);
                        Err(BridgeRpcError::BridgeReturned { message: msg, code })
                    } else {
                        Ok(result.unwrap_or(Value::Null))
                    }
                }
                Ok(Ok(_other)) => Err(BridgeRpcError::Disconnected { pid }),
                Ok(Err(_e)) => break,
                Err(_) => Err(BridgeRpcError::Timeout {
                    pid,
                    budget_ms: CALL_TIMEOUT.as_millis() as u64,
                }),
            },
            Err(_e) => break,
        };

        let success = matches!(&outcome, Ok(v) if !is_bridge_error(v));
        {
            let mut reg = registry.lock().await;
            let result = if success {
                TaskResult::Success
            } else {
                TaskResult::Failed
            };
            let _ = reg.complete_task(pid, result);
        }

        let _ = job.reply.send(outcome);
    }

    teardown(pid, registry, sessions).await;
}

async fn teardown(
    pid: u32,
    registry: Arc<Mutex<PidRegistry>>,
    sessions: Arc<Mutex<HashMap<u32, BridgeHandle>>>,
) {
    {
        let mut reg = registry.lock().await;
        reg.on_bridge_lost(pid, chrono::Utc::now());
    }
    {
        let mut s = sessions.lock().await;
        s.remove(&pid);
    }
    warn!(
        pid,
        "bridge session ended — disconnected, cancelled tasks stacked"
    );
}

fn is_bridge_error(value: &Value) -> bool {
    value
        .get("ok")
        .and_then(Value::as_bool)
        .is_some_and(|ok| !ok)
}

/// Bind the local bridge endpoint and spawn a per-pid session actor for each
/// handshaken TD peer. Runs until the listener fails irrecoverably.
pub async fn run_ipc_accept(
    endpoint: BridgeEndpoint,
    bridge_dir: PathBuf,
    registry: Arc<Mutex<PidRegistry>>,
    sessions: BridgeSessions,
) {
    let listener = match IpcListener::bind(endpoint).await {
        Ok(l) => l,
        Err(e) => {
            warn!(error = %e, "ipc listener bind failed — no bridges will connect");
            return;
        }
    };
    let bridge_dir_str = bridge_dir.to_string_lossy().to_string();
    let version = env!("CARGO_PKG_VERSION").to_string();
    info!("ipc listener bound — waiting for TD bridges");

    loop {
        match listener.accept_handshake(&bridge_dir_str, &version).await {
            Ok(stream) => {
                let pid = stream.pid;
                let handshake = stream.handshake.clone();
                {
                    let mut reg = registry.lock().await;
                    reg.handshake(
                        pid,
                        ProcessAttrs {
                            title: handshake.title.clone(),
                            toe_path: handshake.toe_path.clone(),
                            fingerprint: tdmcp_core::ProcessFingerprint {
                                title: handshake.title.clone(),
                                image: handshake.image.clone(),
                                start_time: handshake.start_time.clone(),
                            },
                            ..Default::default()
                        },
                        Some(handshake.protocol_version.clone()),
                        chrono::Utc::now(),
                    );
                }
                sessions.spawn(pid, stream).await;
            }
            Err(e) => {
                warn!(error = %e, "ipc accept_handshake failed — retrying");
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    }
}

#[allow(unused_imports)]
use tdmcp_ipc::HandshakeRequest as _HandshakeRequestReexport;

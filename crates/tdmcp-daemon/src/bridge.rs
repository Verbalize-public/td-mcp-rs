//! Per-pid bridge sessions: own the IPC stream and drive queue progression.
//!
//! One actor task per connected TD peer. The actor receives [`TaskJob`]s from
//! the MCP layer, promotes the head pending task to in-flight, sends a framed
//! request, awaits the framed response (with a per-call timeout), records the
//! task outcome on the registry, and replies.
//!
//! While idle, the actor probes the peer with wire `ping` (outside the task
//! queue). Missed pongs or inbound silence past [`HeartbeatConfig::idle_dead`]
//! tear the session down via `on_bridge_lost`. After loss, the pid is evicted
//! from the fleet when [`DISCONNECTED_TTL`] elapses or any handshake succeeds.
//! Session generations prevent a superseded actor's teardown from clobbering a
//! newer connection for the same pid.
//!
//! On call timeout the wait fails but the session stays up. Stale responses
//! from timed-out calls are discarded under the next call's budget so they
//! cannot be mistaken for `bridge_lost`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::Value;
use tdmcp_core::{BridgeMethod, PidRegistry, ProcessAttrs, TaskResult};
use tdmcp_ipc::{BridgeEndpoint, IpcListener, IpcStream, Message};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::time::{timeout, MissedTickBehavior};
use tracing::{info, warn};

use tdmcp_mcp::{BridgeRpc, BridgeRpcError};

/// Production default for `ping` / `inspect` / `capture` waits.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(45);
/// Production default for `execute_python` / `mutate_nodes` waits.
pub const SCRIPT_TIMEOUT: Duration = Duration::from_secs(120);

/// Production idle heartbeat: ping every 5s; dead after 20s silence.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
/// Max wait for a heartbeat pong.
pub const PONG_TIMEOUT: Duration = Duration::from_secs(8);
/// Either side assumes the bridge dead after this much inbound silence.
pub const IDLE_DEAD: Duration = Duration::from_secs(20);
/// After bridge loss, drop the pid from the fleet if still disconnected.
pub const DISCONNECTED_TTL: Duration = Duration::from_secs(15);

static CALL_ID: AtomicU64 = AtomicU64::new(1);
static SESSION_GENERATION: AtomicU64 = AtomicU64::new(1);

/// Tunable idle liveness for a bridge session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeartbeatConfig {
    /// When false, no idle pings and no idle-dead teardown (tests that drive
    /// the peer only on tool calls).
    pub enabled: bool,
    /// Send `ping` when the session has been quiet at least this long.
    pub interval: Duration,
    /// Max wait for the matching pong after a heartbeat ping.
    pub pong_timeout: Duration,
    /// Tear down if no inbound framed traffic for this long.
    pub idle_dead: Duration,
}

impl HeartbeatConfig {
    /// Production defaults (5s / 8s / 20s).
    #[must_use]
    pub const fn production() -> Self {
        Self {
            enabled: true,
            interval: HEARTBEAT_INTERVAL,
            pong_timeout: PONG_TIMEOUT,
            idle_dead: IDLE_DEAD,
        }
    }

    /// Disable idle probes (existing integration tests that only touch the
    /// stream during tool calls).
    #[must_use]
    pub const fn disabled() -> Self {
        Self {
            enabled: false,
            interval: Duration::from_secs(5),
            pong_timeout: Duration::from_secs(8),
            idle_dead: Duration::from_secs(20),
        }
    }
}

impl Default for HeartbeatConfig {
    fn default() -> Self {
        Self::production()
    }
}

/// Per-call wait budgets (default vs long-running script/mutate methods).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BridgeTimeouts {
    /// Budget for `ping` / `inspect` / `capture`.
    pub call: Duration,
    /// Budget for `execute_python` / `mutate_nodes`.
    pub script: Duration,
}

impl BridgeTimeouts {
    /// Production defaults (45s call / 120s script).
    #[must_use]
    pub const fn production() -> Self {
        Self {
            call: CALL_TIMEOUT,
            script: SCRIPT_TIMEOUT,
        }
    }

    /// Resolve the wait budget for a wire method name.
    #[must_use]
    pub fn for_method(self, method: &str) -> Duration {
        match BridgeMethod::from_wire(method) {
            Some(BridgeMethod::ExecutePython | BridgeMethod::MutateNodes) => self.script,
            _ => self.call,
        }
    }
}

impl Default for BridgeTimeouts {
    fn default() -> Self {
        Self::production()
    }
}

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
    /// Monotonic generation so a superseded actor's teardown cannot clobber
    /// a newer connection for the same pid.
    generation: u64,
}

/// Map of pid → bridge session handle. Cheap to clone (Arc-backed).
#[derive(Clone)]
pub struct BridgeSessions {
    sessions: Arc<Mutex<HashMap<u32, BridgeHandle>>>,
    registry: Arc<Mutex<PidRegistry>>,
    heartbeat: HeartbeatConfig,
    timeouts: BridgeTimeouts,
    disconnected_ttl: Duration,
}

impl BridgeSessions {
    /// Construct with the shared registry (same Arc the MCP layer uses).
    /// Uses production heartbeat and call-timeout defaults.
    #[must_use]
    pub fn new(registry: Arc<Mutex<PidRegistry>>) -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            registry,
            heartbeat: HeartbeatConfig::production(),
            timeouts: BridgeTimeouts::production(),
            disconnected_ttl: DISCONNECTED_TTL,
        }
    }

    /// Override idle heartbeat (tests: short intervals or [`HeartbeatConfig::disabled`]).
    #[must_use]
    pub fn with_heartbeat(mut self, heartbeat: HeartbeatConfig) -> Self {
        self.heartbeat = heartbeat;
        self
    }

    /// Override per-call wait budgets (tests: short call/script timeouts).
    #[must_use]
    pub fn with_timeouts(mut self, timeouts: BridgeTimeouts) -> Self {
        self.timeouts = timeouts;
        self
    }

    /// Override post-disconnect fleet eviction TTL (tests: short grace).
    #[must_use]
    pub fn with_disconnected_ttl(mut self, ttl: Duration) -> Self {
        self.disconnected_ttl = ttl;
        self
    }

    /// Idle-dead budget forwarded to connecting bridges via handshake.
    #[must_use]
    pub fn idle_dead_secs(&self) -> u64 {
        self.heartbeat.idle_dead.as_secs().max(1)
    }

    /// Number of live bridge session actors (connected IPC peers).
    pub async fn connected_count(&self) -> usize {
        self.sessions.lock().await.len()
    }

    /// Spawn an actor for an accepted, handshaken stream.
    pub async fn spawn(&self, pid: u32, stream: IpcStream) {
        let (job_tx, job_rx) = mpsc::channel::<TaskJob>(32);
        let generation = SESSION_GENERATION.fetch_add(1, Ordering::Relaxed);
        {
            let mut s = self.sessions.lock().await;
            s.insert(pid, BridgeHandle { job_tx, generation });
        }
        let sessions = self.sessions.clone();
        let registry = self.registry.clone();
        let heartbeat = self.heartbeat;
        let timeouts = self.timeouts;
        let disconnected_ttl = self.disconnected_ttl;
        tokio::spawn(async move {
            run_session(
                pid,
                generation,
                stream,
                job_rx,
                registry,
                sessions,
                heartbeat,
                timeouts,
                disconnected_ttl,
            )
            .await;
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

#[allow(clippy::too_many_arguments, reason = "session actor wiring")]
async fn run_session(
    pid: u32,
    generation: u64,
    mut stream: IpcStream,
    mut job_rx: mpsc::Receiver<TaskJob>,
    registry: Arc<Mutex<PidRegistry>>,
    sessions: Arc<Mutex<HashMap<u32, BridgeHandle>>>,
    heartbeat: HeartbeatConfig,
    timeouts: BridgeTimeouts,
    disconnected_ttl: Duration,
) {
    info!(pid, generation, "bridge session started");
    let mut last_activity = Instant::now();

    let mut ticker = tokio::time::interval(if heartbeat.enabled {
        heartbeat.interval.max(Duration::from_millis(1))
    } else {
        Duration::from_secs(3600)
    });
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    // Consume the immediate first tick so we wait a full interval before probing.
    ticker.tick().await;

    loop {
        let until_idle_dead = if heartbeat.enabled {
            heartbeat
                .idle_dead
                .saturating_sub(last_activity.elapsed())
                .max(Duration::from_millis(1))
        } else {
            Duration::from_secs(3600)
        };

        tokio::select! {
            biased;
            job = job_rx.recv() => {
                let Some(job) = job else {
                    break;
                };
                match run_tool_job(pid, &mut stream, &registry, timeouts, job).await {
                    JobLoop::Continue { activity } => {
                        if activity {
                            last_activity = Instant::now();
                        }
                    }
                    JobLoop::Disconnect => break,
                }
            }
            _ = tokio::time::sleep(until_idle_dead), if heartbeat.enabled => {
                if last_activity.elapsed() >= heartbeat.idle_dead {
                    warn!(pid, "bridge idle-dead — no inbound traffic");
                    break;
                }
            }
            _ = ticker.tick(), if heartbeat.enabled => {
                if last_activity.elapsed() < heartbeat.interval {
                    continue;
                }
                match run_heartbeat_ping(pid, &mut stream, heartbeat.pong_timeout).await {
                    Ok(()) => {
                        last_activity = Instant::now();
                    }
                    Err(()) => break,
                }
            }
        }
    }

    teardown(pid, generation, registry, sessions, disconnected_ttl).await;
}

enum JobLoop {
    Continue { activity: bool },
    Disconnect,
}

async fn run_tool_job(
    pid: u32,
    stream: &mut IpcStream,
    registry: &Arc<Mutex<PidRegistry>>,
    timeouts: BridgeTimeouts,
    job: TaskJob,
) -> JobLoop {
    // Promote the head pending task (this job, FIFO) to in-flight.
    {
        let mut reg = registry.lock().await;
        let _ = reg.start_next(pid);
    }

    let budget = timeouts.for_method(&job.method);
    let id = CALL_ID.fetch_add(1, Ordering::Relaxed).to_string();
    let req = Message::Request {
        id: id.clone(),
        method: job.method.clone(),
        params: job.params.clone(),
    };

    let outcome = match stream.send(&req).await {
        Ok(()) => match await_matching_response(pid, stream, &id, budget).await {
            RecvOutcome::Matched(result, error) => {
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
            RecvOutcome::TimedOut => Err(BridgeRpcError::Timeout {
                pid,
                budget_ms: budget.as_millis() as u64,
            }),
            RecvOutcome::Disconnected => {
                let _ = job.reply.send(Err(BridgeRpcError::Disconnected { pid }));
                return JobLoop::Disconnect;
            }
        },
        Err(_e) => {
            let _ = job.reply.send(Err(BridgeRpcError::Disconnected { pid }));
            return JobLoop::Disconnect;
        }
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

    // Any framed response (ok or bridge error) counts as inbound activity.
    let activity = matches!(&outcome, Ok(_) | Err(BridgeRpcError::BridgeReturned { .. }));
    let _ = job.reply.send(outcome);
    JobLoop::Continue { activity }
}

enum RecvOutcome {
    Matched(Option<Value>, Option<Value>),
    TimedOut,
    Disconnected,
}

/// Wait for the response whose `id` matches `want_id`, discarding stale frames.
///
/// Single-flight wire: a mismatched Response is always a leftover from an
/// earlier timed-out call — discard and keep reading until match, IO error,
/// or the budget elapses.
async fn await_matching_response(
    pid: u32,
    stream: &mut IpcStream,
    want_id: &str,
    budget: Duration,
) -> RecvOutcome {
    let deadline = Instant::now() + budget;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return RecvOutcome::TimedOut;
        }
        match timeout(remaining, stream.recv_message()).await {
            Ok(Ok(Message::Response {
                id: rid,
                result,
                error,
            })) if rid == want_id => {
                return RecvOutcome::Matched(result, error);
            }
            Ok(Ok(Message::Response { id: rid, .. })) => {
                warn!(
                    pid,
                    want = %want_id,
                    got = %rid,
                    "discarding stale bridge response (prior timeout)"
                );
                continue;
            }
            Ok(Ok(_other)) => {
                warn!(
                    pid,
                    "unexpected non-response frame while awaiting tool reply"
                );
                return RecvOutcome::Disconnected;
            }
            Ok(Err(_e)) => return RecvOutcome::Disconnected,
            Err(_) => return RecvOutcome::TimedOut,
        }
    }
}

/// Internal liveness probe — not registered on the task queue / fleet.
async fn run_heartbeat_ping(
    pid: u32,
    stream: &mut IpcStream,
    pong_timeout: Duration,
) -> Result<(), ()> {
    let id = CALL_ID.fetch_add(1, Ordering::Relaxed).to_string();
    let req = Message::Request {
        id: id.clone(),
        method: BridgeMethod::Ping.wire_str().to_owned(),
        params: Value::Object(serde_json::Map::new()),
    };
    if stream.send(&req).await.is_err() {
        warn!(pid, "bridge heartbeat send failed");
        return Err(());
    }
    match await_matching_response(pid, stream, &id, pong_timeout).await {
        RecvOutcome::Matched(result, None) => {
            let ok = result
                .as_ref()
                .and_then(|v| v.get("ok"))
                .and_then(Value::as_bool)
                .unwrap_or(true);
            if ok {
                Ok(())
            } else {
                warn!(pid, "bridge heartbeat pong not ok");
                Err(())
            }
        }
        RecvOutcome::Matched(_, Some(_)) => {
            warn!(pid, "bridge heartbeat pong carried error");
            Err(())
        }
        RecvOutcome::TimedOut => {
            warn!(pid, "bridge heartbeat pong timeout");
            Err(())
        }
        RecvOutcome::Disconnected => {
            warn!(pid, "bridge heartbeat recv failed");
            Err(())
        }
    }
}

async fn teardown(
    pid: u32,
    generation: u64,
    registry: Arc<Mutex<PidRegistry>>,
    sessions: Arc<Mutex<HashMap<u32, BridgeHandle>>>,
    disconnected_ttl: Duration,
) {
    {
        let mut s = sessions.lock().await;
        match s.get(&pid) {
            Some(handle) if handle.generation == generation => {
                s.remove(&pid);
            }
            _ => {
                // Superseded by a newer session for this pid — do not touch registry.
                warn!(
                    pid,
                    generation, "bridge session ended — superseded, skip loss"
                );
                return;
            }
        }
    }
    {
        let mut reg = registry.lock().await;
        reg.on_bridge_lost(pid, chrono::Utc::now());
    }
    warn!(
        pid,
        "bridge session ended — disconnected, cancelled tasks stacked"
    );

    let registry = registry.clone();
    tokio::spawn(async move {
        tokio::time::sleep(disconnected_ttl).await;
        let mut reg = registry.lock().await;
        if reg.evict_if_disconnected(pid) {
            warn!(pid, "disconnected pid evicted from fleet after TTL");
        }
    });
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
    let idle_dead_secs = Some(sessions.idle_dead_secs());
    info!("ipc listener bound — waiting for TD bridges");

    loop {
        match listener
            .accept_handshake(&bridge_dir_str, &version, idle_dead_secs)
            .await
        {
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

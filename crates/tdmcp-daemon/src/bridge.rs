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
//! On call timeout the wait fails but the session stays up — `last_activity` is
//! refreshed so a budget longer than `idle_dead` cannot immediately tear the
//! session down. Stale responses from timed-out calls are discarded under the
//! next call's budget so they cannot be mistaken for `bridge_lost`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::Value;
use tdmcp_core::{BridgeMethod, PidRegistry, ProcessAttrs, TaskResult};
use tdmcp_ipc::{BridgeEndpoint, HandshakeOffer, IpcListener, IpcStream, Message};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::time::{timeout, MissedTickBehavior};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use tdmcp_mcp::{BridgeRpc, BridgeRpcError};

use crate::logring::{ingest_bridge_logs, LogSink};

/// Capacity of the per-pid job mpsc (MCP → session actor).
pub const JOB_CHANNEL_CAPACITY: usize = 128;

/// Production default for `ping` / `inspect` / `capture` waits.
/// After bridge loss, drop the pid from the fleet if still disconnected.
pub const DISCONNECTED_TTL: Duration = Duration::from_secs(15);

/// Must match [`tdmcp_config::BridgeSection::default`] (enforced by unit test).
pub const CALL_TIMEOUT: Duration = Duration::from_secs(45);
/// Must match [`tdmcp_config::BridgeSection::default`].
pub const SCRIPT_TIMEOUT: Duration = Duration::from_secs(120);
/// Must match [`tdmcp_config::BridgeSection::default`].
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
/// Must match [`tdmcp_config::BridgeSection::default`].
pub const PONG_TIMEOUT: Duration = Duration::from_secs(8);
/// Must match [`tdmcp_config::BridgeSection::default`].
pub const IDLE_DEAD: Duration = Duration::from_secs(20);

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
    /// Production defaults from [`tdmcp_config::BridgeSection::default`].
    #[must_use]
    pub fn production() -> Self {
        Self::from(&tdmcp_config::BridgeSection::default())
    }

    /// Disable idle probes (existing integration tests that only touch the
    /// stream during tool calls).
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Self::production()
        }
    }
}

impl From<&tdmcp_config::BridgeSection> for HeartbeatConfig {
    fn from(b: &tdmcp_config::BridgeSection) -> Self {
        Self {
            enabled: true,
            interval: Duration::from_secs(b.heartbeat_interval_secs),
            pong_timeout: Duration::from_secs(b.pong_timeout_secs),
            idle_dead: Duration::from_secs(b.idle_dead_secs),
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
    /// Production defaults from [`tdmcp_config::BridgeSection::default`].
    #[must_use]
    pub fn production() -> Self {
        Self::from(&tdmcp_config::BridgeSection::default())
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

impl From<&tdmcp_config::BridgeSection> for BridgeTimeouts {
    fn from(b: &tdmcp_config::BridgeSection) -> Self {
        Self {
            call: Duration::from_secs(b.call_timeout_secs),
            script: Duration::from_secs(b.script_timeout_secs),
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
    /// Cancelled when a newer session supersedes this actor.
    cancel: CancellationToken,
}

/// Map of pid → bridge session handle. Cheap to clone (Arc-backed).
#[derive(Clone)]
pub struct BridgeSessions {
    sessions: Arc<Mutex<HashMap<u32, BridgeHandle>>>,
    registry: Arc<Mutex<PidRegistry>>,
    heartbeat: HeartbeatConfig,
    timeouts: BridgeTimeouts,
    disconnected_ttl: Duration,
    log_sink: Option<LogSink>,
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
            log_sink: None,
        }
    }

    /// Wire the central log sink so bridge-uplinked `log` events (M2) land in
    /// the same ring + JSONL file as in-process records.
    #[must_use]
    pub fn with_log_sink(mut self, sink: LogSink) -> Self {
        self.log_sink = Some(sink);
        self
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

    /// Max main-thread wait budget (seconds) forwarded via handshake.
    #[must_use]
    pub fn max_call_wait_secs(&self) -> u64 {
        self.timeouts
            .call
            .max(self.timeouts.script)
            .as_secs()
            .max(1)
    }

    /// Number of live bridge session actors (connected IPC peers).
    pub async fn connected_count(&self) -> usize {
        self.sessions.lock().await.len()
    }

    /// Approximate depth of jobs waiting in the actor mpsc (not yet in-flight).
    pub async fn job_queue_depth(&self, pid: u32) -> Option<usize> {
        let sessions = self.sessions.lock().await;
        sessions
            .get(&pid)
            .map(|h| JOB_CHANNEL_CAPACITY.saturating_sub(h.job_tx.capacity()))
    }

    /// Spawn an actor for an accepted, handshaken stream.
    ///
    /// If a session already exists for `pid`, its cancel token is fired and its
    /// job channel is dropped so the prior actor exits even when blocked in a
    /// tool wait (does not rely solely on the Python peer closing the old pipe).
    pub async fn spawn(&self, pid: u32, stream: IpcStream) {
        let (job_tx, job_rx) = mpsc::channel::<TaskJob>(JOB_CHANNEL_CAPACITY);
        let generation = SESSION_GENERATION.fetch_add(1, Ordering::Relaxed);
        let cancel = CancellationToken::new();
        let previous = {
            let mut s = self.sessions.lock().await;
            s.insert(
                pid,
                BridgeHandle {
                    job_tx,
                    generation,
                    cancel: cancel.clone(),
                },
            )
        };
        if let Some(prev) = previous {
            info!(
                pid,
                prev_generation = prev.generation,
                "superseding bridge session"
            );
            prev.cancel.cancel();
        }
        let sessions = self.sessions.clone();
        let registry = self.registry.clone();
        let heartbeat = self.heartbeat;
        let timeouts = self.timeouts;
        let disconnected_ttl = self.disconnected_ttl;
        let log_sink = self.log_sink.clone();
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
                cancel,
                log_sink,
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
            // Distinguish "known pid, bridge currently down" (resurrection
            // is plausible — worth a retry) from "never registered / TTL-
            // evicted" (nothing to resurrect, retrying wastes a call).
            let ever_seen = self.registry.lock().await.get(pid).is_some();
            return Err(if ever_seen {
                BridgeRpcError::NotConnected { pid }
            } else {
                BridgeRpcError::Unknown { pid }
            });
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

    async fn job_queue_depth(&self, pid: u32) -> Option<usize> {
        BridgeSessions::job_queue_depth(self, pid).await
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
    cancel: CancellationToken,
    log_sink: Option<LogSink>,
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
            () = cancel.cancelled() => {
                info!(pid, generation, "bridge session cancelled (superseded)");
                break;
            }
            job = job_rx.recv() => {
                let Some(job) = job else {
                    break;
                };
                match run_tool_job(pid, &mut stream, &registry, timeouts, &cancel, job, log_sink.as_ref()).await {
                    // Always refresh on Continue — including call Timeout. A timed-out
                    // wait otherwise leaves last_activity stale across a budget longer
                    // than idle_dead, and the next select iteration immediately
                    // idle-dead teardowns (dual-MCP amplification).
                    JobLoop::Continue => {
                        last_activity = Instant::now();
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
                match run_heartbeat_ping(pid, &mut stream, heartbeat.pong_timeout, &cancel, log_sink.as_ref()).await {
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
    Continue,
    Disconnect,
}

#[allow(clippy::too_many_arguments, reason = "session actor wiring")]
async fn run_tool_job(
    pid: u32,
    stream: &mut IpcStream,
    registry: &Arc<Mutex<PidRegistry>>,
    timeouts: BridgeTimeouts,
    cancel: &CancellationToken,
    job: TaskJob,
    log_sink: Option<&LogSink>,
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
        Ok(()) => match await_matching_response(pid, stream, &id, budget, cancel, log_sink).await {
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

    let _ = job.reply.send(outcome);
    JobLoop::Continue
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
    cancel: &CancellationToken,
    log_sink: Option<&LogSink>,
) -> RecvOutcome {
    let deadline = Instant::now() + budget;
    loop {
        if cancel.is_cancelled() {
            return RecvOutcome::Disconnected;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return RecvOutcome::TimedOut;
        }
        tokio::select! {
            () = cancel.cancelled() => return RecvOutcome::Disconnected,
            recv = timeout(remaining, stream.recv_message()) => {
                match recv {
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
                    // Log uplink (M2): never a disconnect signal, on any wait.
                    Ok(Ok(Message::Event { name, payload })) if name == "log" => {
                        if let Some(sink) = log_sink {
                            ingest_bridge_logs(pid, &payload, sink);
                        }
                        continue;
                    }
                    Ok(Ok(Message::Event { name, .. })) => {
                        debug!(pid, event = %name, "unrecognized bridge event");
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
    }
}

/// Internal liveness probe — not registered on the task queue / fleet.
async fn run_heartbeat_ping(
    pid: u32,
    stream: &mut IpcStream,
    pong_timeout: Duration,
    cancel: &CancellationToken,
    log_sink: Option<&LogSink>,
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
    match await_matching_response(pid, stream, &id, pong_timeout, cancel, log_sink).await {
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
    let superseded = {
        let mut s = sessions.lock().await;
        match s.get(&pid) {
            Some(handle) if handle.generation == generation => {
                s.remove(&pid);
                false
            }
            _ => true,
        }
    };
    if superseded {
        // Do not mark Disconnected / start eviction TTL (a newer actor owns
        // the pid), but clear stale queue slots so exclusive calls are not
        // wedged forever.
        let cancelled = {
            let mut reg = registry.lock().await;
            reg.cancel_queue_keep_connected(pid)
        };
        warn!(
            pid,
            generation,
            cancelled = cancelled.len(),
            "bridge session ended — superseded, queue cleared"
        );
        return;
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
    let offer = HandshakeOffer {
        idle_dead_secs: Some(sessions.idle_dead_secs()),
        max_call_wait_secs: Some(sessions.max_call_wait_secs()),
    };
    info!("ipc listener bound — waiting for TD bridges");

    loop {
        match listener
            .accept_handshake(&bridge_dir_str, &version, offer)
            .await
        {
            Ok(stream) => {
                // Registry + session spawn off the accept loop so the next
                // pipe/UDS accept can proceed immediately.
                let registry = registry.clone();
                let sessions = sessions.clone();
                tokio::spawn(async move {
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
                        );
                    }
                    sessions.spawn(pid, stream).await;
                });
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

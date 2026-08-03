//! Tiered concurrency fuses: FakeTdPeer + BridgeSessions + dispatch_tool.
//!
//! No live TD. Harness law: Notify/watch phase barriers, poll-with-budget,
//! outer + caller timeouts. Medium+ use multi_thread runtime.
//! Subscribe to `Notify` **before** spawning the work that notifies
//! (`let w = n.notified(); pin!(w); …; w.await`) — awaiting after spawn can miss.
//!
//! Ladder: Easy → Medium → Hard → Extreme. See docs/TESTING.md and
//! docs/CONCURRENCY_FUSES_BASELINE.md.

#![allow(clippy::unwrap_used, reason = "test setup/assertions may panic")]
#![allow(clippy::expect_used, reason = "test setup/assertions may panic")]
#![allow(clippy::panic, reason = "test assertions may panic")]

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tdmcp_core::{BridgeStatus, PidRegistry, ProcessAttrs, ProcessFingerprint};
use tdmcp_daemon::bridge::{BridgeSessions, HeartbeatConfig};
use tdmcp_daemon::JOB_CHANNEL_CAPACITY;
use tdmcp_diagnostics::Catalog;
use tdmcp_ipc::{IpcStream, Message};
use tdmcp_mcp::{dispatch_tool, ToolCallError};
use tdmcp_test_support::FakeTdPeer;
use tokio::sync::{Mutex, Notify};
use tokio::task::JoinHandle;

const K_STORM: usize = 8;
const TEST_TIMEOUT: Duration = Duration::from_secs(10);
const EXTREME_TIMEOUT: Duration = Duration::from_secs(15);
/// Per-caller budget for saturate/disconnect paths. Must cover hold+fill
/// phase, not only post-release drain (timeout starts when the call begins).
const CALLER_TIMEOUT: Duration = Duration::from_secs(8);
const POLL_BUDGET: Duration = Duration::from_secs(3);

fn attrs() -> ProcessAttrs {
    ProcessAttrs {
        title: Some("proj".into()),
        fingerprint: ProcessFingerprint {
            title: Some("proj".into()),
            image: Some("TouchDesigner.exe".into()),
            start_time: Some("t0".into()),
        },
        ..Default::default()
    }
}

async fn with_test_timeout<F, T>(budget: Duration, fut: F) -> T
where
    F: Future<Output = T>,
{
    tokio::time::timeout(budget, fut)
        .await
        .unwrap_or_else(|_| panic!("test exceeded outer timeout {budget:?}"))
}

async fn wait_until_async<F, Fut>(mut pred: F, budget: Duration, label: &str)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + budget;
    loop {
        if pred().await {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("wait_until_async({label}) exceeded {budget:?}");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn setup_pid(pid: u32) -> (Arc<Mutex<PidRegistry>>, BridgeSessions, FakeTdPeer) {
    let registry = Arc::new(Mutex::new(PidRegistry::new()));
    let sessions = BridgeSessions::new(registry.clone()).with_heartbeat(HeartbeatConfig::disabled());
    let (peer, server) = FakeTdPeer::pair(pid);
    let server_task = tokio::spawn(async move {
        IpcStream::accept_memory_handshake(server, "/bridge", "0.1.0")
            .await
            .expect("server handshake")
    });
    let mut peer = peer;
    peer.handshake("proj").await.expect("client handshake");
    let ipc_stream = server_task.await.expect("join server");
    {
        let mut reg = registry.lock().await;
        reg.handshake(pid, attrs(), Some("1".into()), chrono::Utc::now());
    }
    sessions.spawn(pid, ipc_stream).await;
    (registry, sessions, peer)
}

async fn setup_two_pids(
    pid_a: u32,
    pid_b: u32,
) -> (
    Arc<Mutex<PidRegistry>>,
    BridgeSessions,
    FakeTdPeer,
    FakeTdPeer,
) {
    let registry = Arc::new(Mutex::new(PidRegistry::new()));
    let sessions = BridgeSessions::new(registry.clone()).with_heartbeat(HeartbeatConfig::disabled());

    let spawn_one = |pid: u32| {
        let registry = registry.clone();
        let sessions = sessions.clone();
        async move {
            let (peer, server) = FakeTdPeer::pair(pid);
            let server_task = tokio::spawn(async move {
                IpcStream::accept_memory_handshake(server, "/bridge", "0.1.0")
                    .await
                    .expect("server handshake")
            });
            let mut peer = peer;
            peer.handshake("proj").await.expect("client handshake");
            let ipc_stream = server_task.await.expect("join server");
            {
                let mut reg = registry.lock().await;
                reg.handshake(pid, attrs(), Some("1".into()), chrono::Utc::now());
            }
            sessions.spawn(pid, ipc_stream).await;
            peer
        }
    };

    let peer_a = spawn_one(pid_a).await;
    let peer_b = spawn_one(pid_b).await;
    (registry, sessions, peer_a, peer_b)
}

async fn rehandshake_and_spawn(
    registry: &Arc<Mutex<PidRegistry>>,
    sessions: &BridgeSessions,
    pid: u32,
) -> FakeTdPeer {
    let (peer, server) = FakeTdPeer::pair(pid);
    let server_task = tokio::spawn(async move {
        IpcStream::accept_memory_handshake(server, "/bridge", "0.1.0")
            .await
            .expect("server handshake")
    });
    let mut peer = peer;
    peer.handshake("proj").await.expect("client handshake");
    let ipc_stream = server_task.await.expect("join server");
    {
        let mut reg = registry.lock().await;
        reg.handshake(pid, attrs(), Some("1".into()), chrono::Utc::now());
    }
    sessions.spawn(pid, ipc_stream).await;
    peer
}

fn marker_script(i: usize) -> String {
    format!("m{i}")
}

fn echo_result_from_params(params: &Value) -> Value {
    let marker = params
        .get("script")
        .and_then(Value::as_str)
        .unwrap_or("1");
    json!({"ok": true, "result": marker})
}

/// Hold the first request until `release` is notified, then answer it (and
/// optionally `extra` more with echo). Notifies `held` once the first frame
/// is received.
fn spawn_hold_then_echo(
    mut peer: FakeTdPeer,
    held: Arc<Notify>,
    release: Arc<Notify>,
    extra: usize,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let msg = match peer.recv_message().await {
            Ok(m) => m,
            Err(_) => return,
        };
        let Message::Request { id, params, .. } = msg else {
            return;
        };
        held.notify_waiters();
        release.notified().await;
        let _ = peer.send_response(id, echo_result_from_params(&params)).await;
        for _ in 0..extra {
            let msg = match peer.recv_message().await {
                Ok(m) => m,
                Err(_) => break,
            };
            let Message::Request { id, params, .. } = msg else {
                continue;
            };
            if peer
                .send_response(id, echo_result_from_params(&params))
                .await
                .is_err()
            {
                break;
            }
        }
    })
}

/// Hold forever after first recv (until peer drop). Used for saturate/disconnect.
fn spawn_hold_forever(mut peer: FakeTdPeer, held: Arc<Notify>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let msg = match peer.recv_message().await {
            Ok(m) => m,
            Err(_) => return,
        };
        let Message::Request { .. } = msg else {
            return;
        };
        held.notify_waiters();
        // Park until the peer/stream is dropped by the test.
        std::future::pending::<()>().await;
    })
}

/// Answer N requests echoing script markers in wire recv order.
fn spawn_echo_n(mut peer: FakeTdPeer, n: usize) -> JoinHandle<()> {
    tokio::spawn(async move {
        for _ in 0..n {
            let msg = match peer.recv_message().await {
                Ok(m) => m,
                Err(_) => break,
            };
            let Message::Request { id, params, .. } = msg else {
                continue;
            };
            if peer
                .send_response(id, echo_result_from_params(&params))
                .await
                .is_err()
            {
                break;
            }
        }
    })
}

async fn queue_empty(registry: &Arc<Mutex<PidRegistry>>, pid: u32) -> bool {
    let reg = registry.lock().await;
    reg.get(pid).is_some_and(|e| e.queue.is_empty())
}

fn is_queue_busy(err: &ToolCallError) -> bool {
    match err {
        ToolCallError::Failed(fail) => fail
            .diagnostics
            .items
            .first()
            .is_some_and(|i| i.code == "tdmcp.bridge.queue_busy"),
        _ => false,
    }
}

fn call_exec(
    registry: Arc<Mutex<PidRegistry>>,
    catalog: Catalog,
    sessions: BridgeSessions,
    pid: u32,
    script: impl Into<String>,
    exclusive: bool,
) -> JoinHandle<Result<Value, ToolCallError>> {
    let script = script.into();
    tokio::spawn(async move {
        dispatch_tool(
            &registry,
            &catalog,
            &sessions,
            "execute_python",
            json!({"pid": pid, "script": script, "exclusive": exclusive}),
        )
        .await
    })
}

fn call_exec_timed(
    registry: Arc<Mutex<PidRegistry>>,
    catalog: Catalog,
    sessions: BridgeSessions,
    pid: u32,
    script: impl Into<String>,
    exclusive: bool,
) -> JoinHandle<Result<Result<Value, ToolCallError>, tokio::time::error::Elapsed>> {
    let script = script.into();
    tokio::spawn(async move {
        tokio::time::timeout(
            CALLER_TIMEOUT,
            dispatch_tool(
                &registry,
                &catalog,
                &sessions,
                "execute_python",
                json!({"pid": pid, "script": script, "exclusive": exclusive}),
            ),
        )
        .await
    })
}

// ---------------------------------------------------------------------------
// Easy
// ---------------------------------------------------------------------------

#[tokio::test]
async fn easy_two_shared_same_pid_ok() {
    with_test_timeout(TEST_TIMEOUT, async {
        let (registry, sessions, peer) = setup_pid(1001).await;
        let _driver = peer.spawn_auto_pong();
        let catalog = Catalog::fallback();

        let a = call_exec(
            registry.clone(),
            catalog.clone(),
            sessions.clone(),
            1001,
            "m0",
            false,
        );
        let b = call_exec(
            registry.clone(),
            catalog.clone(),
            sessions.clone(),
            1001,
            "m1",
            false,
        );

        let va = a.await.unwrap().expect("a ok");
        let vb = b.await.unwrap().expect("b ok");
        assert_eq!(va["ok"], true);
        assert_eq!(vb["ok"], true);

        wait_until_async(|| queue_empty(&registry, 1001), POLL_BUDGET, "queue empty").await;
    })
    .await;
}

#[tokio::test]
async fn easy_exclusive_rejects_while_shared_held() {
    with_test_timeout(TEST_TIMEOUT, async {
        let (registry, sessions, peer) = setup_pid(1002).await;
        let held = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let _driver = spawn_hold_then_echo(peer, held.clone(), release.clone(), 0);
        let catalog = Catalog::fallback();

        // Subscribe before spawn so we cannot miss a fast notify.
        let held_wait = held.notified();
        tokio::pin!(held_wait);
        let shared = call_exec(
            registry.clone(),
            catalog.clone(),
            sessions.clone(),
            1002,
            "held",
            false,
        );
        held_wait.await;

        let err = dispatch_tool(
            &registry,
            &catalog,
            &sessions,
            "execute_python",
            json!({"pid": 1002, "script": "excl", "exclusive": true}),
        )
        .await
        .expect_err("exclusive must fail");
        assert!(is_queue_busy(&err), "expected queue_busy, got {err:?}");

        release.notify_waiters();
        let v = shared.await.unwrap().expect("shared completes");
        assert_eq!(v["ok"], true);
        assert_eq!(v["result"], "held");
        wait_until_async(|| queue_empty(&registry, 1002), POLL_BUDGET, "queue empty").await;
    })
    .await;
}

#[tokio::test]
async fn easy_two_pids_concurrent_ok() {
    with_test_timeout(TEST_TIMEOUT, async {
        let (registry, sessions, peer_a, peer_b) = setup_two_pids(1003, 1004).await;
        let _da = peer_a.spawn_auto_pong();
        let _db = peer_b.spawn_auto_pong();
        let catalog = Catalog::fallback();

        let a = call_exec(
            registry.clone(),
            catalog.clone(),
            sessions.clone(),
            1003,
            "a",
            false,
        );
        let b = call_exec(
            registry.clone(),
            catalog.clone(),
            sessions.clone(),
            1004,
            "b",
            false,
        );

        assert_eq!(a.await.unwrap().expect("a")["ok"], true);
        assert_eq!(b.await.unwrap().expect("b")["ok"], true);

        let reg = registry.lock().await;
        assert_eq!(reg.get(1003).unwrap().bridge, BridgeStatus::Connected);
        assert_eq!(reg.get(1004).unwrap().bridge, BridgeStatus::Connected);
    })
    .await;
}

// ---------------------------------------------------------------------------
// Medium
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn med_shared_storm_fifo_echo() {
    with_test_timeout(TEST_TIMEOUT, async {
        let (registry, sessions, peer) = setup_pid(2001).await;
        let _driver = spawn_echo_n(peer, K_STORM);
        let catalog = Catalog::fallback();

        let handles: Vec<_> = (0..K_STORM)
            .map(|i| {
                call_exec(
                    registry.clone(),
                    catalog.clone(),
                    sessions.clone(),
                    2001,
                    marker_script(i),
                    false,
                )
            })
            .collect();

        for (i, h) in handles.into_iter().enumerate() {
            let v = h.await.unwrap().unwrap_or_else(|e| panic!("caller {i}: {e:?}"));
            assert_eq!(v["ok"], true, "caller {i}");
            assert_eq!(
                v["result"],
                marker_script(i),
                "crossed or wrong marker for caller {i}"
            );
        }
        wait_until_async(|| queue_empty(&registry, 2001), POLL_BUDGET, "queue empty").await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn med_exclusive_storm_while_held() {
    with_test_timeout(TEST_TIMEOUT, async {
        let (registry, sessions, peer) = setup_pid(2002).await;
        let held = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        // 1 held + 2 shared after release
        let _driver = spawn_hold_then_echo(peer, held.clone(), release.clone(), 2);
        let catalog = Catalog::fallback();

        let held_wait = held.notified();
        tokio::pin!(held_wait);
        let held_call = call_exec(
            registry.clone(),
            catalog.clone(),
            sessions.clone(),
            2002,
            "held",
            false,
        );
        held_wait.await;

        let mut exclusive_errs = Vec::new();
        for i in 0..4 {
            let err = dispatch_tool(
                &registry,
                &catalog,
                &sessions,
                "execute_python",
                json!({"pid": 2002, "script": format!("excl{i}"), "exclusive": true}),
            )
            .await
            .expect_err("exclusive must fail");
            exclusive_errs.push(err);
        }
        assert_eq!(exclusive_errs.len(), 4);
        for err in &exclusive_errs {
            assert!(is_queue_busy(err), "expected queue_busy, got {err:?}");
        }

        let s1 = call_exec(
            registry.clone(),
            catalog.clone(),
            sessions.clone(),
            2002,
            "s1",
            false,
        );
        let s2 = call_exec(
            registry.clone(),
            catalog.clone(),
            sessions.clone(),
            2002,
            "s2",
            false,
        );

        release.notify_waiters();

        assert_eq!(held_call.await.unwrap().expect("held")["result"], "held");
        assert_eq!(s1.await.unwrap().expect("s1")["result"], "s1");
        assert_eq!(s2.await.unwrap().expect("s2")["result"], "s2");
        wait_until_async(|| queue_empty(&registry, 2002), POLL_BUDGET, "queue empty").await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn med_pid_loss_isolates_peer() {
    with_test_timeout(TEST_TIMEOUT, async {
        let (registry, sessions, peer_a, peer_b) = setup_two_pids(2003, 2004).await;
        let held = Arc::new(Notify::new());
        let hold_a = spawn_hold_forever(peer_a, held.clone());
        let _drive_b = peer_b.spawn_auto_pong();
        let catalog = Catalog::fallback();

        let held_wait = held.notified();
        tokio::pin!(held_wait);
        let a_call = call_exec_timed(
            registry.clone(),
            catalog.clone(),
            sessions.clone(),
            2003,
            "a-held",
            false,
        );
        held_wait.await;

        // Abort hold task → drops FakeTdPeer → IPC loss on A.
        hold_a.abort();
        let _ = a_call.await;

        wait_until_async(
            || async {
                let reg = registry.lock().await;
                reg.get(2003)
                    .is_some_and(|e| e.bridge == BridgeStatus::Disconnected)
            },
            POLL_BUDGET,
            "A disconnected",
        )
        .await;

        {
            let reg = registry.lock().await;
            let entry = reg.get(2003).expect("A entry");
            assert!(
                !entry.resurrection.cancelled_tasks.is_empty(),
                "A should stack cancelled tasks on loss"
            );
            assert_eq!(
                reg.get(2004).expect("B").bridge,
                BridgeStatus::Connected,
                "B must stay connected"
            );
        }

        let b_excl = dispatch_tool(
            &registry,
            &catalog,
            &sessions,
            "execute_python",
            json!({"pid": 2004, "script": "b-excl", "exclusive": true}),
        )
        .await
        .expect("B exclusive must succeed while A is down");
        assert_eq!(b_excl["ok"], true);
    })
    .await;
}

// ---------------------------------------------------------------------------
// Hard
// ---------------------------------------------------------------------------

/// `job_queue_depth` counts jobs waiting in the actor mpsc (not domain TaskQueue).
/// While one job is in-flight on the wire it has already been taken from the
/// channel, so filling the channel to capacity requires CAPACITY additional
/// senders behind the held head.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hard_saturate_job_channel_then_drain() {
    with_test_timeout(TEST_TIMEOUT, async {
        let (registry, sessions, peer) = setup_pid(3001).await;
        let held = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let fill = JOB_CHANNEL_CAPACITY;
        let _driver = spawn_hold_then_echo(peer, held.clone(), release.clone(), fill);
        let catalog = Catalog::fallback();

        // 1 in-flight + CAPACITY queued in mpsc (+ maybe one more blocked).
        let total = 1 + fill;
        let held_wait = held.notified();
        tokio::pin!(held_wait);
        let mut handles = Vec::with_capacity(total);
        for i in 0..total {
            handles.push(call_exec_timed(
                registry.clone(),
                catalog.clone(),
                sessions.clone(),
                3001,
                marker_script(i),
                false,
            ));
        }

        held_wait.await;

        wait_until_async(
            || {
                let sessions = sessions.clone();
                async move {
                    sessions
                        .job_queue_depth(3001)
                        .await
                        .is_some_and(|d| d >= JOB_CHANNEL_CAPACITY.saturating_sub(1))
                }
            },
            POLL_BUDGET,
            "job_queue_depth near capacity",
        )
        .await;

        let depth = sessions.job_queue_depth(3001).await.expect("depth");
        assert!(
            depth >= JOB_CHANNEL_CAPACITY.saturating_sub(1),
            "expected mpsc depth near {JOB_CHANNEL_CAPACITY}, got {depth}"
        );

        release.notify_waiters();

        for (i, h) in handles.into_iter().enumerate() {
            let timed = h.await.unwrap();
            let v = timed
                .unwrap_or_else(|_| panic!("caller {i} timed out"))
                .unwrap_or_else(|e| panic!("caller {i} err {e:?}"));
            assert_eq!(v["ok"], true, "caller {i}");
            assert_eq!(v["result"], marker_script(i), "caller {i} marker");
        }
        wait_until_async(|| queue_empty(&registry, 3001), POLL_BUDGET, "queue empty").await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hard_saturate_then_disconnect_flushes() {
    with_test_timeout(TEST_TIMEOUT, async {
        let (registry, sessions, peer) = setup_pid(3002).await;
        let held = Arc::new(Notify::new());
        let fill = JOB_CHANNEL_CAPACITY;
        let hold_task = spawn_hold_forever(peer, held.clone());
        let catalog = Catalog::fallback();

        let total = 1 + fill;
        let held_wait = held.notified();
        tokio::pin!(held_wait);
        let mut handles = Vec::with_capacity(total);
        for i in 0..total {
            handles.push(call_exec_timed(
                registry.clone(),
                catalog.clone(),
                sessions.clone(),
                3002,
                marker_script(i),
                false,
            ));
        }

        held_wait.await;
        wait_until_async(
            || {
                let sessions = sessions.clone();
                async move {
                    sessions
                        .job_queue_depth(3002)
                        .await
                        .is_some_and(|d| d >= JOB_CHANNEL_CAPACITY.saturating_sub(1))
                }
            },
            POLL_BUDGET,
            "saturated",
        )
        .await;

        // Drop peer (abort hold task) → disconnect.
        hold_task.abort();

        let mut any_ok = 0usize;
        let mut any_err = 0usize;
        let mut any_timeout = 0usize;
        for h in handles {
            match h.await.unwrap() {
                Ok(Ok(_)) => any_ok += 1,
                Ok(Err(_)) => any_err += 1,
                Err(_) => any_timeout += 1,
            }
        }
        assert_eq!(
            any_ok, 0,
            "no waiter should succeed after disconnect (ok={any_ok} err={any_err} timeout={any_timeout})"
        );
        assert!(
            any_err + any_timeout == total,
            "every waiter must end; err={any_err} timeout={any_timeout}"
        );

        wait_until_async(
            || async {
                let reg = registry.lock().await;
                match reg.get(3002) {
                    None => true,
                    Some(e) => e.bridge == BridgeStatus::Disconnected && e.queue.is_empty(),
                }
            },
            POLL_BUDGET,
            "settled after disconnect",
        )
        .await;

        let peer2 = rehandshake_and_spawn(&registry, &sessions, 3002).await;
        let _d = spawn_echo_n(peer2, 1);
        let v = dispatch_tool(
            &registry,
            &catalog,
            &sessions,
            "execute_python",
            json!({"pid": 3002, "script": "recovered"}),
        )
        .await
        .expect("post re-handshake call");
        assert_eq!(v["ok"], true);
        assert_eq!(v["result"], "recovered");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hard_supersede_while_inflight_held() {
    with_test_timeout(TEST_TIMEOUT, async {
        let (registry, sessions, peer_old) = setup_pid(3003).await;
        let held = Arc::new(Notify::new());
        let hold_old = spawn_hold_forever(peer_old, held.clone());
        let catalog = Catalog::fallback();

        let held_wait = held.notified();
        tokio::pin!(held_wait);
        let old_call = call_exec_timed(
            registry.clone(),
            catalog.clone(),
            sessions.clone(),
            3003,
            "old",
            false,
        );
        held_wait.await;

        let peer_new = rehandshake_and_spawn(&registry, &sessions, 3003).await;
        let _drive_new = spawn_echo_n(peer_new, 1);

        // Old generation must not report success.
        match old_call.await.unwrap() {
            Ok(Ok(v)) => panic!("old generation must not succeed, got {v}"),
            Ok(Err(_)) | Err(_) => {}
        }
        hold_old.abort();

        let v = dispatch_tool(
            &registry,
            &catalog,
            &sessions,
            "execute_python",
            json!({"pid": 3003, "script": "new"}),
        )
        .await
        .expect("new session serves");
        assert_eq!(v["ok"], true);
        assert_eq!(v["result"], "new");

        let reg = registry.lock().await;
        assert_eq!(reg.get(3003).unwrap().bridge, BridgeStatus::Connected);
    })
    .await;
}

// ---------------------------------------------------------------------------
// Extreme
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn x_saturate_then_supersede() {
    with_test_timeout(EXTREME_TIMEOUT, async {
        let (registry, sessions, peer_old) = setup_pid(4001).await;
        let held = Arc::new(Notify::new());
        let hold_old = spawn_hold_forever(peer_old, held.clone());
        let catalog = Catalog::fallback();

        let total = 1 + JOB_CHANNEL_CAPACITY;
        let held_wait = held.notified();
        tokio::pin!(held_wait);
        let mut handles = Vec::with_capacity(total);
        for i in 0..total {
            handles.push(call_exec_timed(
                registry.clone(),
                catalog.clone(),
                sessions.clone(),
                4001,
                marker_script(i),
                false,
            ));
        }

        held_wait.await;
        wait_until_async(
            || {
                let sessions = sessions.clone();
                async move {
                    sessions
                        .job_queue_depth(4001)
                        .await
                        .is_some_and(|d| d >= JOB_CHANNEL_CAPACITY.saturating_sub(1))
                }
            },
            POLL_BUDGET,
            "near capacity before supersede",
        )
        .await;

        let peer_new = rehandshake_and_spawn(&registry, &sessions, 4001).await;
        let _drive_new = peer_new.spawn_auto_pong();
        hold_old.abort();

        let mut any_ok = 0usize;
        for h in handles {
            if matches!(h.await.unwrap(), Ok(Ok(_))) {
                any_ok += 1;
            }
        }
        assert_eq!(
            any_ok, 0,
            "old waiters must not succeed across supersede (ok={any_ok})"
        );

        let v = dispatch_tool(
            &registry,
            &catalog,
            &sessions,
            "execute_python",
            json!({"pid": 4001, "script": "after"}),
        )
        .await
        .expect("new session");
        assert_eq!(v["ok"], true);

        let reg = registry.lock().await;
        assert_eq!(reg.get(4001).unwrap().bridge, BridgeStatus::Connected);
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn x_asymmetric_storm_two_pids() {
    with_test_timeout(EXTREME_TIMEOUT, async {
        let (registry, sessions, peer_a, peer_b) = setup_two_pids(4002, 4003).await;
        let held = Arc::new(Notify::new());
        let hold_a = spawn_hold_forever(peer_a, held.clone());
        let _drive_b = spawn_echo_n(peer_b, K_STORM);
        let catalog = Catalog::fallback();

        // Saturate A
        let fill = JOB_CHANNEL_CAPACITY;
        let held_wait = held.notified();
        tokio::pin!(held_wait);
        let mut a_handles = Vec::with_capacity(1 + fill);
        for i in 0..(1 + fill) {
            a_handles.push(call_exec_timed(
                registry.clone(),
                catalog.clone(),
                sessions.clone(),
                4002,
                format!("a{i}"),
                false,
            ));
        }
        held_wait.await;

        // Storm B concurrently
        let b_handles: Vec<_> = (0..K_STORM)
            .map(|i| {
                call_exec(
                    registry.clone(),
                    catalog.clone(),
                    sessions.clone(),
                    4003,
                    marker_script(i),
                    false,
                )
            })
            .collect();

        for (i, h) in b_handles.into_iter().enumerate() {
            let v = h.await.unwrap().unwrap_or_else(|e| panic!("B caller {i}: {e:?}"));
            assert_eq!(v["result"], marker_script(i), "B marker {i}");
        }

        {
            let reg = registry.lock().await;
            assert_eq!(
                reg.get(4003).unwrap().bridge,
                BridgeStatus::Connected,
                "B must never disconnect"
            );
        }

        // Drop A — waiters should fail; B remains healthy.
        hold_a.abort();
        for h in a_handles {
            let _ = h.await;
        }

        {
            let reg = registry.lock().await;
            assert_eq!(reg.get(4003).unwrap().bridge, BridgeStatus::Connected);
        }
    })
    .await;
}

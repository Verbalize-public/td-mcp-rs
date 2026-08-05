//! End-to-end: real BridgeSessions actor + memory IPC + FakeTdPeer.
//!
//! Exercises the full path the live named-pipe/UDS transport would:
//! handshake → registry → actor → framed request → peer response →
//! registry completion → diagnostics. Disconnect/resurrection is covered
//! via a dropped peer followed by a same-pid re-handshake. Idle heartbeat
//! detects a dropped peer without an intervening MCP call.

#![allow(clippy::unwrap_used, reason = "test setup/assertions may panic")]
#![allow(clippy::expect_used, reason = "test setup/assertions may panic")]
#![allow(clippy::panic, reason = "test assertions may panic")]

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tdmcp_core::{PidRegistry, ProcessAttrs, ProcessFingerprint};
use tdmcp_daemon::bridge::{BridgeSessions, BridgeTimeouts, HeartbeatConfig};
use tdmcp_diagnostics::Catalog;
use tdmcp_ipc::{IpcStream, Message};
use tdmcp_mcp::dispatch_tool;
use tdmcp_test_support::FakeTdPeer;
use tokio::sync::Mutex;

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

async fn setup_with(
    pid: u32,
    heartbeat: HeartbeatConfig,
) -> (Arc<Mutex<PidRegistry>>, BridgeSessions, FakeTdPeer) {
    setup_with_ttl(pid, heartbeat, Duration::from_secs(15)).await
}

async fn setup_with_ttl(
    pid: u32,
    heartbeat: HeartbeatConfig,
    disconnected_ttl: Duration,
) -> (Arc<Mutex<PidRegistry>>, BridgeSessions, FakeTdPeer) {
    let registry = Arc::new(Mutex::new(PidRegistry::new()));
    let sessions = BridgeSessions::new(registry.clone())
        .with_heartbeat(heartbeat)
        .with_disconnected_ttl(disconnected_ttl);
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

/// Default setup: heartbeat disabled so tool-call-only peer drivers stay simple.
async fn setup(pid: u32) -> (Arc<Mutex<PidRegistry>>, BridgeSessions, FakeTdPeer) {
    setup_with(pid, HeartbeatConfig::disabled()).await
}

async fn wait_until_disconnected(registry: &Arc<Mutex<PidRegistry>>, pid: u32, budget: Duration) {
    let deadline = tokio::time::Instant::now() + budget;
    loop {
        {
            let reg = registry.lock().await;
            if let Some(entry) = reg.get(pid) {
                if entry.bridge == tdmcp_core::BridgeStatus::Disconnected {
                    return;
                }
            }
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("expected pid {pid} disconnected within {budget:?}");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
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

/// Drive the fake peer: answer `n` requests with canned results.
async fn drive_peer(mut peer: FakeTdPeer, n: usize) {
    for _ in 0..n {
        let msg = peer.recv_message().await.expect("recv request");
        let Message::Request { id, method, .. } = msg else {
            panic!("expected request, got {msg:?}");
        };
        let result: Value = match method.as_str() {
            "execute_python" => json!({"ok": true, "result": 1}),
            "capture" => json!({
                "ok": true,
                "bytes": 1024,
                "path": "/project1/out1",
                "mimeType": "image/png",
                "imageBase64": "iVBORw0KGgo=",
            }),
            "inspect" => json!({"ok": true, "nodes": [{"ok": true, "path": "/project1"}]}),
            "api_help" => json!({
                "ok": true,
                "results": [{
                    "ok": true,
                    "kind": "class",
                    "name": "noiseTOP",
                    "doc": "Noise TOP",
                    "opType": "noiseTOP",
                    "family": "TOP",
                    "members": ["cook", "par"],
                    "memberCount": 2
                }],
                "queriesTruncated": false
            }),
            "ping" => json!({"ok": true, "pong": true}),
            _ => json!({"ok": true}),
        };
        peer.send_response(id, result).await.expect("send response");
    }
}

#[tokio::test]
async fn execute_python_round_trip() {
    let (registry, sessions, peer) = setup(42).await;
    let driver = tokio::spawn(drive_peer(peer, 1));
    let catalog = Catalog::fallback();

    let v = dispatch_tool(
        &registry,
        &catalog,
        &sessions,
        "execute_python",
        json!({"pid": 42, "script": "result=1"}),
    )
    .await
    .expect("ok");

    assert_eq!(v["ok"], true);
    assert_eq!(v["result"], 1);
    let _ = driver.await;

    // Task completed successfully → queue empty, no resurrection.
    let reg = registry.lock().await;
    let entry = reg.get(42).expect("entry");
    assert!(entry.queue.is_empty());
    assert!(!entry.resurrection.resurrected);
}

#[tokio::test]
async fn capture_round_trip() {
    let (registry, sessions, peer) = setup(43).await;
    let driver = tokio::spawn(drive_peer(peer, 1));
    let catalog = Catalog::fallback();

    let v = dispatch_tool(
        &registry,
        &catalog,
        &sessions,
        "capture",
        json!({"pid": 43, "path": "/project1/out1", "mode": "top"}),
    )
    .await
    .expect("ok");

    assert_eq!(v["ok"], true);
    assert_eq!(v["path"], "/project1/out1");
    let _ = driver.await;
}

#[tokio::test]
async fn api_help_round_trip() {
    let (registry, sessions, peer) = setup(143).await;
    let driver = tokio::spawn(drive_peer(peer, 1));
    let catalog = Catalog::fallback();

    let v = dispatch_tool(
        &registry,
        &catalog,
        &sessions,
        "api_help",
        json!({
            "pid": 143,
            "queries": [{"kind": "class", "name": "noiseTOP"}]
        }),
    )
    .await
    .expect("ok");

    assert_eq!(v["ok"], true);
    assert_eq!(v["results"][0]["name"], "noiseTOP");
    assert_eq!(v["results"][0]["ok"], true);
    let _ = driver.await;
}

#[tokio::test]
async fn api_help_partial_entry_failure_still_ok() {
    let (registry, sessions, peer) = setup(144).await;
    let driver = tokio::spawn(async move {
        let mut peer = peer;
        let msg = peer.recv_message().await.expect("recv request");
        let Message::Request { id, method, .. } = msg else {
            panic!("expected request, got {msg:?}");
        };
        assert_eq!(method, "api_help");
        peer.send_response(
            id,
            json!({
                "ok": true,
                "results": [
                    {
                        "ok": true,
                        "kind": "class",
                        "name": "noiseTOP",
                        "members": [],
                        "memberCount": 0
                    },
                    {
                        "ok": false,
                        "kind": "class",
                        "name": "missingTOP",
                        "code": "tdmcp.api_help.not_found",
                        "message": "name not found on td: missingTOP"
                    }
                ]
            }),
        )
        .await
        .expect("send response");
    });
    let catalog = Catalog::fallback();

    let v = dispatch_tool(
        &registry,
        &catalog,
        &sessions,
        "api_help",
        json!({
            "pid": 144,
            "queries": [
                {"kind": "class", "name": "noiseTOP"},
                {"kind": "class", "name": "missingTOP"}
            ]
        }),
    )
    .await
    .expect("top-level ok with partial entry failure");

    assert_eq!(v["ok"], true);
    assert_eq!(v["results"][0]["ok"], true);
    assert_eq!(v["results"][1]["ok"], false);
    assert_eq!(v["results"][1]["code"], "tdmcp.api_help.not_found");
    let _ = driver.await;
}

#[tokio::test]
async fn exclusive_fails_while_shared_in_flight() {
    let (registry, sessions, peer) = setup(44).await;
    // Peer holds the shared request in-flight (delays its response) so the
    // exclusive call must hit queue_busy at enqueue time.
    let driver = tokio::spawn(async move {
        let mut peer = peer;
        let msg = peer.recv_message().await.expect("recv request");
        let Message::Request { id, .. } = msg else {
            panic!("expected request, got {msg:?}");
        };
        // Hold the shared call in-flight.
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        peer.send_response(id, json!({"ok": true, "result": 1}))
            .await
            .expect("send response");
    });
    let catalog = Catalog::fallback();

    let reg_a = registry.clone();
    let sess_a = sessions.clone();
    let catalog_a = catalog.clone();
    let first = tokio::spawn(async move {
        dispatch_tool(
            &reg_a,
            &catalog_a,
            &sess_a,
            "execute_python",
            json!({"pid": 44, "script": "result=1", "exclusive": false}),
        )
        .await
    });

    // Let the shared call enqueue + reach the actor (peer is holding it).
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;

    let err = dispatch_tool(
        &registry,
        &catalog,
        &sessions,
        "execute_python",
        json!({"pid": 44, "script": "result=2", "exclusive": true}),
    )
    .await
    .expect_err("exclusive must fail");
    match err {
        tdmcp_mcp::ToolCallError::Failed(fail) => {
            assert_eq!(fail.diagnostics.items[0].code, "tdmcp.bridge.queue_busy");
        }
        other => panic!("expected Failed, got {other:?}"),
    }

    let _ = first.await.expect("first completes");
    let _ = driver.await;
}

#[tokio::test]
async fn disconnect_then_resurrection() {
    let (registry, sessions, _peer) = setup(45).await;
    // Drop the peer without driving it → the actor's next send/recv fails.
    drop(_peer);

    // A call after the peer is gone must fail (transport/disconnected), and the
    // actor tears down → on_bridge_lost stacks a cancelled task.
    let catalog = Catalog::fallback();
    let _ = dispatch_tool(
        &registry,
        &catalog,
        &sessions,
        "execute_python",
        json!({"pid": 45, "script": "result=1"}),
    )
    .await;

    // Give the actor a moment to tear down.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    {
        let reg = registry.lock().await;
        let entry = reg.get(45).expect("entry");
        assert_eq!(entry.bridge, tdmcp_core::BridgeStatus::Disconnected);
        assert!(!entry.resurrection.cancelled_tasks.is_empty());
    }

    // Re-handshake the same pid → resurrected.
    let peer2 = rehandshake_and_spawn(&registry, &sessions, 45).await;

    {
        let reg = registry.lock().await;
        let entry = reg.get(45).expect("entry");
        assert!(entry.resurrection.resurrected);
    }

    // First successful task after resurrection clears the stack.
    let driver = tokio::spawn(drive_peer(peer2, 1));
    let v = dispatch_tool(
        &registry,
        &catalog,
        &sessions,
        "execute_python",
        json!({"pid": 45, "script": "result=1"}),
    )
    .await
    .expect("ok");
    assert_eq!(v["ok"], true);
    let _ = driver.await;

    {
        let reg = registry.lock().await;
        let entry = reg.get(45).expect("entry");
        assert!(!entry.resurrection.resurrected);
        assert!(entry.resurrection.cancelled_tasks.is_empty());
    }
}

#[tokio::test]
async fn idle_drop_disconnects_without_mcp_call() {
    let hb = HeartbeatConfig {
        enabled: true,
        interval: Duration::from_millis(40),
        pong_timeout: Duration::from_millis(80),
        idle_dead: Duration::from_millis(200),
    };
    let (registry, _sessions, peer) = setup_with(46, hb).await;
    drop(peer);

    let deadline = tokio::time::Instant::now() + Duration::from_millis(500);
    loop {
        {
            let reg = registry.lock().await;
            let entry = reg.get(46).expect("entry");
            if entry.bridge == tdmcp_core::BridgeStatus::Disconnected {
                break;
            }
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("expected disconnected via idle heartbeat without MCP call");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn idle_peer_with_auto_pong_stays_connected() {
    let hb = HeartbeatConfig {
        enabled: true,
        interval: Duration::from_millis(40),
        pong_timeout: Duration::from_millis(80),
        idle_dead: Duration::from_millis(250),
    };
    let (registry, _sessions, peer) = setup_with(47, hb).await;
    let _driver = peer.spawn_auto_pong();

    // Several heartbeat intervals with no tool calls.
    tokio::time::sleep(Duration::from_millis(220)).await;

    let reg = registry.lock().await;
    let entry = reg.get(47).expect("entry");
    assert_eq!(
        entry.bridge,
        tdmcp_core::BridgeStatus::Connected,
        "auto-pong peer must stay connected across idle heartbeats"
    );
}

#[tokio::test]
async fn disconnected_pid_evicted_after_ttl() {
    let ttl = Duration::from_millis(80);
    let (registry, sessions, peer) = setup_with_ttl(48, HeartbeatConfig::disabled(), ttl).await;
    drop(peer);

    let catalog = Catalog::fallback();
    let _ = dispatch_tool(
        &registry,
        &catalog,
        &sessions,
        "execute_python",
        json!({"pid": 48, "script": "result=1"}),
    )
    .await;

    wait_until_disconnected(&registry, 48, Duration::from_millis(500)).await;

    tokio::time::sleep(ttl + Duration::from_millis(80)).await;

    let reg = registry.lock().await;
    assert!(
        reg.get(48).is_none(),
        "disconnected pid must leave fleet after TTL"
    );
}

#[tokio::test]
async fn any_handshake_evicts_other_disconnected() {
    let (registry, sessions, peer_a) = setup(49).await;
    drop(peer_a);

    let catalog = Catalog::fallback();
    let _ = dispatch_tool(
        &registry,
        &catalog,
        &sessions,
        "execute_python",
        json!({"pid": 49, "script": "result=1"}),
    )
    .await;
    wait_until_disconnected(&registry, 49, Duration::from_millis(500)).await;

    let peer_b = rehandshake_and_spawn(&registry, &sessions, 50).await;
    let _driver = peer_b.spawn_auto_pong();

    {
        let reg = registry.lock().await;
        assert!(reg.get(49).is_none(), "ghost pid 49 must be purged");
        assert_eq!(
            reg.get(50).expect("pid 50").bridge,
            tdmcp_core::BridgeStatus::Connected
        );
    }
}

#[tokio::test]
async fn superseding_while_in_flight_clears_queue_for_exclusive() {
    // Peer receives the tool request but never replies — leaves in-flight.
    let (registry, sessions, mut peer_old) = setup(71).await;
    let catalog = Catalog::fallback();

    let call = {
        let registry = registry.clone();
        let sessions = sessions.clone();
        let catalog = catalog.clone();
        tokio::spawn(async move {
            dispatch_tool(
                &registry,
                &catalog,
                &sessions,
                "execute_python",
                json!({"pid": 71, "script": "result=1"}),
            )
            .await
        })
    };

    // Drain the request so the actor promotes the task to in-flight.
    let msg = peer_old.recv_message().await.expect("recv request");
    assert!(matches!(msg, Message::Request { .. }));

    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        {
            let reg = registry.lock().await;
            let entry = reg.get(71).expect("entry");
            if !entry.queue.is_empty() {
                break;
            }
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("expected non-empty queue while tool wait is blocked");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let peer_new = rehandshake_and_spawn(&registry, &sessions, 71).await;
    let _driver = peer_new.spawn_auto_pong();

    // Superseded teardown must clear the zombie slot.
    tokio::time::sleep(Duration::from_millis(150)).await;
    {
        let mut reg = registry.lock().await;
        let entry = reg.get(71).expect("entry");
        assert_eq!(entry.bridge, tdmcp_core::BridgeStatus::Connected);
        assert!(
            entry.queue.is_empty(),
            "supersede must cancel in-flight queue slots"
        );
        // Exclusive enqueue must succeed now (would Busy if slot lingered).
        reg.enqueue(71, "ExclusiveProbe", tdmcp_core::TaskMode::Exclusive)
            .expect("exclusive after supersede queue clear");
        let _ = reg.cancel_queue_keep_connected(71);
    }

    let _ = call.await;
    let v = dispatch_tool(
        &registry,
        &catalog,
        &sessions,
        "execute_python",
        json!({"pid": 71, "script": "result=1", "exclusive": true}),
    )
    .await
    .expect("exclusive call after supersede must not queue_busy");
    assert_eq!(v["ok"], true);
}

#[tokio::test]
async fn superseding_spawn_does_not_mark_new_session_disconnected() {
    let (registry, sessions, _peer_old) = setup(51).await;

    // Replacing the session drops the old job_tx → old actor teardowns with a
    // stale generation and must not flip the new connection to disconnected.
    let peer_new = rehandshake_and_spawn(&registry, &sessions, 51).await;
    let _driver = peer_new.spawn_auto_pong();

    tokio::time::sleep(Duration::from_millis(100)).await;

    let reg = registry.lock().await;
    let entry = reg.get(51).expect("entry");
    assert_eq!(
        entry.bridge,
        tdmcp_core::BridgeStatus::Connected,
        "stale teardown must not clobber a newer session"
    );
}

#[tokio::test]
async fn superseding_spawn_aborts_old_actor_while_stream_still_open() {
    // Keep the old peer stream open (simulates Python disconnect join failure).
    // The new session must own the pid; the old actor must exit via cancel.
    let (registry, sessions, peer_old) = setup(61).await;
    assert_eq!(sessions.connected_count().await, 1);

    let peer_new = rehandshake_and_spawn(&registry, &sessions, 61).await;
    let _driver = peer_new.spawn_auto_pong();

    // Allow cancel + teardown of the superseded actor.
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        sessions.connected_count().await,
        1,
        "exactly one live session handle for the pid"
    );

    let catalog = Catalog::fallback();
    let v = dispatch_tool(
        &registry,
        &catalog,
        &sessions,
        "execute_python",
        json!({"pid": 61, "script": "result=1"}),
    )
    .await
    .expect("new session must serve calls");
    assert_eq!(v["ok"], true);

    // Dropping the old peer after supersede must not flip the new session.
    drop(peer_old);
    tokio::time::sleep(Duration::from_millis(100)).await;
    {
        let reg = registry.lock().await;
        assert_eq!(
            reg.get(61).expect("entry").bridge,
            tdmcp_core::BridgeStatus::Connected
        );
    }
}

async fn setup_with_timeouts(
    pid: u32,
    timeouts: BridgeTimeouts,
) -> (Arc<Mutex<PidRegistry>>, BridgeSessions, FakeTdPeer) {
    setup_with_heartbeat_and_timeouts(pid, HeartbeatConfig::disabled(), timeouts).await
}

async fn setup_with_heartbeat_and_timeouts(
    pid: u32,
    heartbeat: HeartbeatConfig,
    timeouts: BridgeTimeouts,
) -> (Arc<Mutex<PidRegistry>>, BridgeSessions, FakeTdPeer) {
    let registry = Arc::new(Mutex::new(PidRegistry::new()));
    let sessions = BridgeSessions::new(registry.clone())
        .with_heartbeat(heartbeat)
        .with_timeouts(timeouts);
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

#[tokio::test]
async fn timeout_does_not_desync_next_call() {
    // Short call budget so the first response arrives after timeout.
    let timeouts = BridgeTimeouts {
        call: Duration::from_millis(80),
        script: Duration::from_millis(80),
    };
    let (registry, sessions, peer) = setup_with_timeouts(52, timeouts).await;
    let catalog = Catalog::fallback();

    let driver = tokio::spawn(async move {
        let mut peer = peer;
        // First request: respond late (after call timeout).
        let msg = peer.recv_message().await.expect("recv request 1");
        let Message::Request { id: id1, .. } = msg else {
            panic!("expected request, got {msg:?}");
        };
        tokio::time::sleep(Duration::from_millis(200)).await;
        peer.send_response(id1, json!({"ok": true, "result": "late"}))
            .await
            .expect("send late response");

        // Second request: respond promptly with a distinct payload.
        let msg = peer.recv_message().await.expect("recv request 2");
        let Message::Request { id: id2, .. } = msg else {
            panic!("expected request, got {msg:?}");
        };
        peer.send_response(id2, json!({"ok": true, "result": "fresh"}))
            .await
            .expect("send fresh response");
    });

    let first = dispatch_tool(
        &registry,
        &catalog,
        &sessions,
        "execute_python",
        json!({"pid": 52, "script": "result=1"}),
    )
    .await;
    match first {
        Err(tdmcp_mcp::ToolCallError::Failed(fail)) => {
            assert_eq!(fail.diagnostics.items[0].code, "tdmcp.bridge.timeout");
        }
        other => panic!("expected timeout Failed, got {other:?}"),
    }

    // Give the late response time to land on the wire before the next call.
    tokio::time::sleep(Duration::from_millis(150)).await;

    let second = dispatch_tool(
        &registry,
        &catalog,
        &sessions,
        "execute_python",
        json!({"pid": 52, "script": "result=2"}),
    )
    .await
    .expect("second call must succeed after draining stale response");
    assert_eq!(second["ok"], true);
    assert_eq!(second["result"], "fresh");

    {
        let reg = registry.lock().await;
        let entry = reg.get(52).expect("entry");
        assert_eq!(
            entry.bridge,
            tdmcp_core::BridgeStatus::Connected,
            "timeout must not tear down the session"
        );
    }

    let _ = driver.await;
}

#[tokio::test]
async fn script_method_gets_longer_timeout() {
    // Delay sits between call and script budgets.
    let timeouts = BridgeTimeouts {
        call: Duration::from_millis(80),
        script: Duration::from_millis(400),
    };
    let delay = Duration::from_millis(200);

    // execute_python must succeed under the script budget.
    {
        let (registry, sessions, peer) = setup_with_timeouts(53, timeouts).await;
        let catalog = Catalog::fallback();
        let driver = tokio::spawn(async move {
            let mut peer = peer;
            let msg = peer.recv_message().await.expect("recv");
            let Message::Request { id, method, .. } = msg else {
                panic!("expected request, got {msg:?}");
            };
            assert_eq!(method, "execute_python");
            tokio::time::sleep(delay).await;
            peer.send_response(id, json!({"ok": true, "result": 1}))
                .await
                .expect("send");
        });
        let v = dispatch_tool(
            &registry,
            &catalog,
            &sessions,
            "execute_python",
            json!({"pid": 53, "script": "result=1"}),
        )
        .await
        .expect("script method should wait longer");
        assert_eq!(v["result"], 1);
        let _ = driver.await;
    }

    // inspect must time out under the shorter call budget with the same delay.
    {
        let (registry, sessions, peer) = setup_with_timeouts(54, timeouts).await;
        let catalog = Catalog::fallback();
        let driver = tokio::spawn(async move {
            let mut peer = peer;
            let msg = peer.recv_message().await.expect("recv");
            let Message::Request { id, method, .. } = msg else {
                panic!("expected request, got {msg:?}");
            };
            assert_eq!(method, "inspect");
            tokio::time::sleep(delay).await;
            // Late response — discarded / ignored after timeout.
            let _ = peer
                .send_response(
                    id,
                    json!({"ok": true, "nodes": [{"ok": true, "path": "/project1"}]}),
                )
                .await;
        });
        let err = dispatch_tool(
            &registry,
            &catalog,
            &sessions,
            "inspect",
            json!({"pid": 54, "paths": ["/project1"]}),
        )
        .await
        .expect_err("inspect should use the shorter call timeout");
        match err {
            tdmcp_mcp::ToolCallError::Failed(fail) => {
                assert_eq!(fail.diagnostics.items[0].code, "tdmcp.bridge.timeout");
            }
            other => panic!("expected timeout Failed, got {other:?}"),
        }
        let _ = driver.await;
    }
}

/// Call timeout longer than idle_dead: without refreshing last_activity on
/// Timeout, the actor would idle-dead immediately after the timed-out wait
/// (the dual Cursor+OpenCode failure mode).
#[tokio::test]
async fn call_timeout_does_not_idle_dead_session() {
    let timeouts = BridgeTimeouts {
        call: Duration::from_millis(150),
        script: Duration::from_millis(150),
    };
    let heartbeat = HeartbeatConfig {
        enabled: true,
        // Faster than idle_dead so post-timeout silence is kept alive by pings.
        interval: Duration::from_millis(40),
        pong_timeout: Duration::from_millis(100),
        // Shorter than the call budget — the bug fires when last_activity is
        // left stale across a TimedOut wait longer than idle_dead.
        idle_dead: Duration::from_millis(80),
    };
    let (registry, sessions, peer) =
        setup_with_heartbeat_and_timeouts(55, heartbeat, timeouts).await;
    let catalog = Catalog::fallback();

    let driver = tokio::spawn(async move {
        let mut peer = peer;
        let msg = peer.recv_message().await.expect("recv request 1");
        let Message::Request { id: id1, .. } = msg else {
            panic!("expected request, got {msg:?}");
        };
        // Hold past call timeout; late response is discarded under later budgets.
        tokio::time::sleep(Duration::from_millis(250)).await;
        let _ = peer
            .send_response(id1, json!({"ok": true, "result": "late"}))
            .await;

        // Answer heartbeats and the follow-up tool call until we see a non-ping.
        loop {
            let msg = peer.recv_message().await.expect("recv follow-up");
            let Message::Request { id, method, .. } = msg else {
                panic!("expected request, got {msg:?}");
            };
            if method == "ping" {
                peer.send_response(id, json!({"ok": true}))
                    .await
                    .expect("pong");
                continue;
            }
            peer.send_response(id, json!({"ok": true, "result": "alive"}))
                .await
                .expect("send alive");
            break;
        }
    });

    let first = dispatch_tool(
        &registry,
        &catalog,
        &sessions,
        "execute_python",
        json!({"pid": 55, "script": "result=1"}),
    )
    .await;
    match first {
        Err(tdmcp_mcp::ToolCallError::Failed(fail)) => {
            assert_eq!(fail.diagnostics.items[0].code, "tdmcp.bridge.timeout");
        }
        other => panic!("expected timeout Failed, got {other:?}"),
    }

    // Wait longer than idle_dead so a stale last_activity would have torn down.
    // Heartbeats keep the session alive when activity was refreshed on Timeout.
    tokio::time::sleep(Duration::from_millis(200)).await;

    {
        let reg = registry.lock().await;
        let entry = reg.get(55).expect("entry");
        assert_eq!(
            entry.bridge,
            tdmcp_core::BridgeStatus::Connected,
            "call timeout must not idle-dead the session"
        );
    }

    let second = dispatch_tool(
        &registry,
        &catalog,
        &sessions,
        "execute_python",
        json!({"pid": 55, "script": "result=2"}),
    )
    .await
    .expect("session must still serve after post-timeout idle window");
    assert_eq!(second["ok"], true);
    assert_eq!(second["result"], "alive");

    let _ = driver.await;
}

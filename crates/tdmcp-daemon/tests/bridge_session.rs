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
use tdmcp_daemon::bridge::{BridgeSessions, HeartbeatConfig};
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
    let registry = Arc::new(Mutex::new(PidRegistry::new()));
    let sessions = BridgeSessions::new(registry.clone()).with_heartbeat(heartbeat);
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
                "mimeType": "image/jpeg",
                "jpegBase64": "/9j/4AAQ",
            }),
            "inspect" => json!({"ok": true, "node": {"path": "/project1"}}),
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
    assert_eq!(v["capture"]["path"], "/project1/out1");
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
        tdmcp_mcp::ToolCallError::Failed { diagnostics, .. } => {
            assert_eq!(diagnostics.items[0].code, "tdmcp.bridge.queue_busy");
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
    let (peer2, server2) = FakeTdPeer::pair(45);
    let server_task = tokio::spawn(async move {
        IpcStream::accept_memory_handshake(server2, "/bridge", "0.1.0")
            .await
            .expect("server handshake 2")
    });
    let mut peer2 = peer2;
    peer2.handshake("proj").await.expect("client handshake 2");
    let ipc_stream2 = server_task.await.expect("join server 2");
    {
        let mut reg = registry.lock().await;
        reg.handshake(45, attrs(), Some("1".into()), chrono::Utc::now());
    }
    sessions.spawn(45, ipc_stream2).await;

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

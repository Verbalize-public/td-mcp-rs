//! W6 acceptance matrix: the operation journal against a real bridge transport.
//!
//! These are the five cases the plan names — slow/frozen TouchDesigner,
//! disconnect, restart-after-dispatch, cancellation, retention — driven
//! through the same `BridgeSessions` actor, IPC stream, and framing that the
//! live named-pipe/UDS transport uses. The point is not that the adapter
//! behaves; it is that the *daemon's real failure modes* produce honest
//! receipts, and that none of them silently claims an effect.
//!
//! Unix uses the on-disk backend, including restart durability. Windows uses
//! in-memory SQLite for transport semantics; its disk backend explicitly reports
//! unsupported permissions, so restart durability is tested only on Unix.

#![allow(clippy::unwrap_used, reason = "test setup/assertions may panic")]
#![allow(clippy::expect_used, reason = "test setup/assertions may panic")]
#![allow(clippy::panic, reason = "test assertions may panic")]

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tdmcp_core::EffectState;
use tdmcp_daemon::bridge::{BridgeSessions, BridgeTimeouts, HeartbeatConfig};
use tdmcp_daemon::opdispatch::{call_and_record, read_mutate_effect, OperationRecorder};
use tdmcp_daemon::opjournal::{
    JournalConfig, OperationJournal, ReceiptState, SqliteOperationJournal,
};
use tdmcp_ipc::IpcStream;
use tdmcp_mcp::evidence::OwnerId;
use tdmcp_test_support::FakeTdPeer;
use tokio::sync::Mutex;

/// Owner for the matrix. In production this is the stable per-client identity
/// the transport authenticates as, never a request argument.
fn owner() -> OwnerId {
    OwnerId::parse("matrix-client").unwrap()
}

fn config() -> JournalConfig {
    JournalConfig {
        max_rows_per_owner: 16,
        max_total_rows: 64,
        max_in_flight_per_owner: 8,
        max_in_flight_total: 16,
        max_detail_bytes: 256,
        retention: Duration::from_secs(60),
    }
}

/// A task-owned private journal root. Every test gets its own, and the ones
/// that need a restart hand the same path to a second `open`.
fn journal_root(name: &str) -> std::path::PathBuf {
    let unique = format!(
        "tdmcp-opmatrix-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    );
    let path = std::env::temp_dir().join(unique.replace(['(', ')', ' '], ""));
    std::fs::create_dir_all(&path).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    path
}

fn open_journal(root: &std::path::Path, config: JournalConfig) -> SqliteOperationJournal {
    #[cfg(unix)]
    {
        SqliteOperationJournal::open(root, config).unwrap()
    }
    #[cfg(windows)]
    {
        let _ = root;
        SqliteOperationJournal::open_memory(config).unwrap()
    }
}

fn recorder(root: &std::path::Path) -> Arc<OperationRecorder> {
    let journal = open_journal(root, config());
    Arc::new(OperationRecorder::new(Arc::new(journal), owner()))
}

/// A connected session with a real actor behind a real framed stream.
async fn session(
    pid: u32,
    call_budget: Duration,
) -> (
    Arc<Mutex<tdmcp_core::PidRegistry>>,
    BridgeSessions,
    FakeTdPeer,
) {
    let registry = Arc::new(Mutex::new(tdmcp_core::PidRegistry::new()));
    let timeouts = BridgeTimeouts {
        call: call_budget,
        script: call_budget,
    };
    let sessions = BridgeSessions::new(registry.clone())
        .with_heartbeat(HeartbeatConfig::disabled())
        .with_timeouts(timeouts)
        .with_disconnected_ttl(Duration::from_secs(15));
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
        use tdmcp_core::{ProcessAttrs, ProcessFingerprint};
        let mut reg = registry.lock().await;
        reg.handshake(
            pid,
            ProcessAttrs {
                title: Some("proj".into()),
                fingerprint: ProcessFingerprint {
                    title: Some("proj".into()),
                    image: Some("TouchDesigner.exe".into()),
                    start_time: Some("t0".into()),
                },
                ..Default::default()
            },
            Some("1".into()),
        );
    }
    sessions.spawn(pid, ipc_stream).await;
    (registry, sessions, peer)
}

fn receipt(
    journal: &Arc<OperationRecorder>,
    id: &str,
) -> tdmcp_daemon::opjournal::OperationReceipt {
    journal
        .journal()
        .get(&owner(), id)
        .unwrap()
        .expect("receipt for the owner")
}

/// A slow or frozen TouchDesigner: the request reaches the executor and no
/// response ever comes back.
///
/// This is the case that matters most. The mutation may well have been applied
/// inside TD before it stopped answering, so the honest receipt is unsettled
/// with an unknown effect — never `failed`, which would tell the caller the
/// write did not happen.
#[tokio::test]
async fn a_frozen_touchdesigner_leaves_the_dispatch_unsettled_rather_than_failed() {
    let root = journal_root("frozen");
    let recorder = recorder(&root);
    let (_registry, sessions, mut peer) = session(4242, Duration::from_millis(120)).await;

    // The peer receives the request and then says nothing at all.
    let frozen = tokio::spawn(async move {
        let msg = peer.recv_message().await.expect("request arrived");
        assert!(matches!(msg, tdmcp_ipc::Message::Request { .. }));
        // Hold the response forever.
        std::future::pending::<()>().await;
    });

    let recorded = call_and_record(
        &sessions,
        &recorder,
        4242,
        "mutate_nodes",
        json!({"steps": []}),
        &read_mutate_effect,
    )
    .await
    .expect("the call is recorded even though it times out");

    let err = recorded.result.expect_err("a frozen peer cannot answer");
    assert!(
        matches!(err, tdmcp_mcp::BridgeRpcError::Timeout { .. }),
        "expected a timeout, got {err:?}"
    );
    assert!(
        recorded.receipt_error.is_none(),
        "the receipt settled cleanly"
    );

    let receipt = receipt(&recorder, &recorded.receipt_id);
    assert_eq!(
        receipt.state,
        ReceiptState::WaitExpired,
        "a frozen peer must not be reported as a failure"
    );
    assert_eq!(
        receipt.effect,
        EffectState::Unknown,
        "nobody observed whether the scene changed"
    );
    assert!(
        !receipt.state.is_terminal(),
        "the receipt stays recoverable"
    );

    frozen.abort();
    std::fs::remove_dir_all(&root).ok();
}

/// The peer vanishes mid-call: the request was sent, and the result is lost
/// because the transport went away rather than because the work finished.
#[tokio::test]
async fn a_bridge_that_drops_mid_call_reports_a_lost_result_not_a_failure() {
    let root = journal_root("disconnect");
    let recorder = recorder(&root);
    let (_registry, sessions, mut peer) = session(4243, Duration::from_secs(30)).await;

    let dropper = tokio::spawn(async move {
        let msg = peer.recv_message().await.expect("request arrived");
        assert!(matches!(msg, tdmcp_ipc::Message::Request { .. }));
        // Drop the peer: the stream closes with the request unanswered.
        drop(peer);
    });

    let recorded = call_and_record(
        &sessions,
        &recorder,
        4243,
        "mutate_nodes",
        json!({"steps": []}),
        &read_mutate_effect,
    )
    .await
    .expect("recorded");

    assert!(
        recorded.result.is_err(),
        "a dropped peer cannot produce a result"
    );
    let receipt = receipt(&recorder, &recorded.receipt_id);
    assert!(
        matches!(receipt.state, ReceiptState::WaitExpired),
        "expected an unsettled receipt, got {:?}",
        receipt.state
    );
    assert_eq!(receipt.effect, EffectState::Unknown);

    dropper.await.ok();
    std::fs::remove_dir_all(&root).ok();
}

/// Restart after a dispatch that was never observed.
///
/// This is the case the durable journal exists for: the daemon is gone, so it
/// cannot know what the mutation did, and the next process must report that
/// rather than presenting the operation as never having run.
#[cfg(unix)]
#[tokio::test]
async fn a_restart_reports_an_unobserved_dispatch_as_unknown_rather_than_replaying_it() {
    let root = journal_root("restart");
    let receipt_id = {
        let recorder = recorder(&root);
        let (_registry, sessions, mut peer) = session(4244, Duration::from_millis(120)).await;
        let frozen = tokio::spawn(async move {
            let _ = peer.recv_message().await;
            std::future::pending::<()>().await;
        });
        let recorded = call_and_record(
            &sessions,
            &recorder,
            4244,
            "mutate_nodes",
            json!({"steps": []}),
            &read_mutate_effect,
        )
        .await
        .expect("recorded");
        assert!(recorded.result.is_err());
        frozen.abort();
        recorded.receipt_id
    };

    // A second process opens the same journal. Recovery runs inside `open`.
    let reopened = SqliteOperationJournal::open(&root, config()).unwrap();
    let recovered = reopened.get(&owner(), &receipt_id).unwrap().expect("row");
    assert_eq!(
        recovered.state,
        ReceiptState::UnknownEffect,
        "after a restart the outcome is unknown, not absent and not applied"
    );
    assert_eq!(recovered.effect, EffectState::Unknown);
    // And it is reported, not acted on: recovery is idempotent and never sends.
    assert!(reopened.recover_uncertain().unwrap().is_empty());

    std::fs::remove_dir_all(&root).ok();
}

/// A cancelled caller: the future awaiting the bridge result is dropped.
///
/// The tool layer applies its own timeout around the bridge call, so this is
/// the ordinary path, not an exotic one. The receipt must still be written,
/// because the caller is gone and will never ask.
#[tokio::test]
async fn a_cancelled_caller_still_leaves_a_receipt_behind() {
    let root = journal_root("cancelled");
    let recorder = recorder(&root);
    let (_registry, sessions, mut peer) = session(4245, Duration::from_secs(30)).await;

    let frozen = tokio::spawn(async move {
        let _ = peer.recv_message().await;
        std::future::pending::<()>().await;
    });

    let call = call_and_record(
        &sessions,
        &recorder,
        4245,
        "mutate_nodes",
        json!({"steps": []}),
        &read_mutate_effect,
    );
    // Drop the in-flight future the way a client disconnect or a tool-layer
    // timeout does. `timeout` drops it synchronously before returning, so the
    // receipt is written by the time this returns — no sleep needed.
    let elapsed = tokio::time::timeout(Duration::from_millis(60), call).await;
    assert!(elapsed.is_err(), "the call must still be in flight");

    // Exactly one dispatch was admitted, so exactly one receipt exists.
    let rows = recorder.journal().list(&owner(), 16).unwrap();
    assert_eq!(rows.len(), 1, "one dispatch means one receipt");
    assert_eq!(
        rows[0].state,
        ReceiptState::WaitExpired,
        "a cancelled caller leaves an unsettled receipt, not a silent gap"
    );
    assert_eq!(rows[0].effect, EffectState::Unknown);
    assert!(!rows[0].state.is_terminal());

    frozen.abort();
    std::fs::remove_dir_all(&root).ok();
}

/// Repeated cancellations free slots while preserving unknown effects.
#[tokio::test]
async fn repeated_caller_cancellations_do_not_exhaust_receipt_admission() {
    let root = journal_root("repeated-cancellation");
    let journal = Arc::new(open_journal(&root, config()));
    let recorder = Arc::new(OperationRecorder::new(journal.clone(), owner()));
    let (_registry, sessions, mut peer) = session(4247, Duration::from_secs(30)).await;
    let frozen = tokio::spawn(async move { while peer.recv_message().await.is_ok() {} });

    // Exceed both configured admission bounds, through real dispatch futures.
    for _ in 0..20 {
        let call = call_and_record(
            &sessions,
            &recorder,
            4247,
            "mutate_nodes",
            json!({"steps": []}),
            &read_mutate_effect,
        );
        assert!(tokio::time::timeout(Duration::from_millis(5), call)
            .await
            .is_err());
        assert_eq!(journal.counts(&owner()).unwrap().in_flight, 0);
    }
    assert!(journal.list(&owner(), 16).unwrap().iter().all(|receipt| {
        receipt.state == ReceiptState::WaitExpired
            && receipt.effect == EffectState::Unknown
            && receipt.expires_at_ms.is_some()
    }));
    frozen.abort();
    drop(recorder);
    drop(journal);
    std::fs::remove_dir_all(&root).ok();
}

/// Retention bounds history after the result arrives or the caller stops
/// waiting. Active dispatches keep their admission slots and never expire.
///
/// Driven with a zero-length retention so the settled row is already past its
/// expiry when the next admission prunes. This is a property of the journal,
/// not of the transport, so it is asserted directly rather than through a
/// bridge that would only add noise.
#[tokio::test]
async fn retention_bounds_ended_waits_but_never_discards_active_dispatches() {
    let root = journal_root("retention");
    let mut expiring = config();
    // A receipt settled under this retention is expired the instant it is
    // written; `prune` runs on the next admission.
    expiring.retention = Duration::ZERO;
    let journal = open_journal(&root, expiring);

    // Settled: an observer recorded its answer, so dropping it loses nothing.
    let settled = journal.admit(&owner(), "mutate_nodes", None).unwrap();
    journal.mark_dispatched(&owner(), &settled).unwrap();
    journal
        .finish(
            &owner(),
            &settled,
            ReceiptState::Completed,
            EffectState::Applied,
            None,
        )
        .unwrap();

    // The wait ended without a result: uncertainty is now retained history.
    let unsettled = journal.admit(&owner(), "mutate_nodes", None).unwrap();
    journal.mark_dispatched(&owner(), &unsettled).unwrap();
    journal
        .mark_wait_expired(&owner(), &unsettled, "caller stopped waiting")
        .unwrap();

    let active = journal.admit(&owner(), "mutate_nodes", None).unwrap();
    journal.mark_dispatched(&owner(), &active).unwrap();
    // A fresh admission is what triggers the prune.
    let keeper = journal.admit(&owner(), "mutate_nodes", None).unwrap();

    assert!(
        journal.get(&owner(), &settled).unwrap().is_none(),
        "a settled receipt past its retention is pruned"
    );
    assert!(
        journal.get(&owner(), &unsettled).unwrap().is_none(),
        "an ended wait is pruned after the configured history retention"
    );
    assert_eq!(
        journal
            .get(&owner(), &active)
            .unwrap()
            .unwrap()
            .expires_at_ms,
        None
    );
    assert!(journal.get(&owner(), &keeper).unwrap().is_some());

    std::fs::remove_dir_all(&root).ok();
}

/// A bridge that is not connected at all, so nothing was ever sent.
///
/// Reported by the transport as `Unknown` or `NotConnected` depending on
/// whether the pid was ever registered. Either way nothing reached TouchDesigner,
/// and the receipt must say exactly that.
#[tokio::test]
async fn a_call_to_an_unregistered_target_records_that_nothing_was_sent() {
    let root = journal_root("never-sent");
    let recorder = recorder(&root);
    let registry = Arc::new(Mutex::new(tdmcp_core::PidRegistry::new()));
    let sessions =
        BridgeSessions::new(registry.clone()).with_heartbeat(HeartbeatConfig::disabled());

    let recorded = call_and_record(
        &sessions,
        &recorder,
        9999,
        "mutate_nodes",
        json!({}),
        &read_mutate_effect,
    )
    .await
    .expect("recorded");

    let receipt = receipt(&recorder, &recorded.receipt_id);
    assert_eq!(
        receipt.state,
        ReceiptState::Cancelled,
        "nothing was dispatched, so the receipt is settled, not unsettled"
    );
    assert_eq!(receipt.effect, EffectState::PreventedBeforeDispatch);
    // Settled means the caller may retry immediately.
    assert!(receipt.expires_at_ms.is_some());

    std::fs::remove_dir_all(&root).ok();
}

/// The happy path, so the matrix is not only failure cases: a returned payload
/// establishes its own effect and the receipt says so.
#[tokio::test]
async fn a_returned_mutation_records_the_effect_the_payload_established() {
    let root = journal_root("returned");
    let recorder = recorder(&root);
    let (_registry, sessions, mut peer) = session(4246, Duration::from_secs(30)).await;

    let driver = tokio::spawn(async move {
        let msg = peer.recv_message().await.expect("request arrived");
        let tdmcp_ipc::Message::Request { id, .. } = msg else {
            panic!("expected a request, got {msg:?}");
        };
        peer.send_response(
            id,
            json!({"ok": true, "applied": 2, "failedAt": 0, "steps": []}),
        )
        .await
        .expect("response");
    });

    let recorded = call_and_record(
        &sessions,
        &recorder,
        4246,
        "mutate_nodes",
        json!({"steps": []}),
        &read_mutate_effect,
    )
    .await
    .expect("recorded");

    assert!(recorded.result.is_ok());
    let receipt = receipt(&recorder, &recorded.receipt_id);
    assert_eq!(receipt.state, ReceiptState::Completed);
    assert_eq!(receipt.effect, EffectState::Applied);
    assert!(receipt.state.is_terminal());

    driver.await.ok();
    std::fs::remove_dir_all(&root).ok();
}

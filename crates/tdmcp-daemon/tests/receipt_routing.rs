//! Integration: a real HTTP tool call produces a real, owner-scoped receipt.
//!
//! The unit tests pin the adapter and the journal in isolation. This file is
//! the claim that they compose: a request arrives over HTTP with an identity
//! claim, travels the real router and dispatch path, and leaves a receipt that
//! the same client can find afterwards — and that another client cannot.

#![allow(clippy::unwrap_used, reason = "test setup/assertions may panic")]
#![allow(clippy::expect_used, reason = "test setup/assertions may panic")]
#![allow(clippy::panic, reason = "test assertions may panic")]

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::json;
use tdmcp_core::{PidRegistry, ProcessAttrs, ProcessFingerprint, TaskResult};
use tdmcp_diagnostics::Catalog;
use tdmcp_mcp::evidence::OwnerId;
use tdmcp_mcp::testing::test_resource_provider;
use tdmcp_mcp::{build_mcp_router, AppState, BridgeRpc};
use tower::ServiceExt;

use tdmcp_daemon::opjournal::{
    JournalConfig, MemoryOperationJournal, OperationJournal, ReceiptState,
};
use tdmcp_daemon::receipt_port::ClientReceiptFactory;

/// The journal every test shares, plus the router that writes to it.
struct Harness {
    journal: Arc<dyn OperationJournal>,
    router: axum::Router,
}

fn config() -> JournalConfig {
    JournalConfig {
        max_rows_per_owner: 16,
        max_total_rows: 64,
        max_in_flight_per_owner: 8,
        max_in_flight_total: 16,
        max_detail_bytes: 128,
        retention: Duration::from_secs(60),
    }
}

/// A router wired to an in-memory journal, in `auth.mode = "none"` so the
/// claim is the only identity input. A memory journal is deliberate: the disk
/// backend's platform rules are not what this file is about, and the
/// persistence path is already covered by the disk tests in opjournal.
fn harness(canned: serde_json::Value) -> Harness {
    let journal: Arc<dyn OperationJournal> = Arc::new(MemoryOperationJournal::new(config()));
    let registry = Arc::new(Mutex::new(registry()));
    let bridge: Arc<dyn BridgeRpc> = Arc::new(ActorBridge {
        registry: Arc::clone(&registry),
        canned,
    });
    let state = AppState::new_shared(
        Arc::clone(&registry),
        Catalog::fallback(),
        bridge,
        test_resource_provider().expect("resource provider"),
    )
    .with_receipts(Arc::new(ClientReceiptFactory::new(
        Arc::clone(&journal),
        "none",
        "",
    )));
    Harness {
        journal,
        router: build_mcp_router(state),
    }
}

fn registry() -> PidRegistry {
    let mut registry = PidRegistry::new();
    registry.handshake(
        34,
        ProcessAttrs {
            title: Some("test".into()),
            fingerprint: ProcessFingerprint {
                title: Some("test".into()),
                ..Default::default()
            },
            ..Default::default()
        },
        Some("1".into()),
    );
    registry
}

/// One tool call, optionally with an identity claim.
async fn call(harness: &Harness, claim: Option<&str>) -> StatusCode {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/mcp/tools/call")
        .header("content-type", "application/json");
    if let Some(claim) = claim {
        builder = builder.header("x-tdmcp-client", claim);
    }
    let request = builder
        .body(Body::from(
            json!({"name": "execute_python", "arguments": {"pid": 34, "script": "result = 1"}})
                .to_string(),
        ))
        .unwrap();
    harness
        .router
        .clone()
        .oneshot(request)
        .await
        .unwrap()
        .status()
}

/// A bridge that also plays the daemon's per-pid actor.
///
/// Queue progression belongs to the actor, not to the tool layer: it starts the
/// enqueued task before sending and completes it after the reply. A fake that
/// only returned a value would leave every task queued forever, so the second
/// call in a test would be rejected as queue-busy before it was ever admitted —
/// testing the queue rather than the receipts. Emulating the actor around the
/// send is the faithful version.
struct ActorBridge {
    registry: Arc<Mutex<PidRegistry>>,
    canned: serde_json::Value,
}

#[async_trait::async_trait]
impl BridgeRpc for ActorBridge {
    async fn call(
        &self,
        pid: u32,
        _method: &str,
        _params: serde_json::Value,
    ) -> Result<serde_json::Value, tdmcp_mcp::BridgeRpcError> {
        self.registry
            .lock()
            .await
            .start_next(pid)
            .expect("the tool layer enqueued this task before calling the bridge");
        let reply = self.canned.clone();
        self.registry
            .lock()
            .await
            .complete_task(pid, TaskResult::Success)
            .expect("complete the task this call started");
        Ok(reply)
    }
}

/// The owner id an unauthenticated daemon derives for `label`.
fn owner_for(label: &str) -> OwnerId {
    OwnerId::parse(format!("local:{label}")).expect("valid owner id")
}

fn receipts(harness: &Harness, label: &str) -> Vec<tdmcp_daemon::opjournal::OperationReceipt> {
    harness
        .journal
        .list(&owner_for(label), 16)
        .expect("list succeeds")
}

/// `applied` is what lets the receipt say what changed rather than shrugging.
fn applying_payload() -> serde_json::Value {
    json!({
        "ok": true,
        "result": 1,
        "applied": 1,
        "failedAt": 0,
    })
}

#[tokio::test]
async fn a_tool_call_from_a_named_client_leaves_that_clients_receipt() {
    let harness = harness(applying_payload());
    assert_eq!(call(&harness, Some("ide-1")).await, StatusCode::OK);

    let rows = receipts(&harness, "ide-1");
    assert_eq!(rows.len(), 1, "one call means one receipt");
    let row = &rows[0];
    assert_eq!(row.state, ReceiptState::Completed);
    assert_eq!(row.effect, tdmcp_core::EffectState::Applied);
    assert_eq!(row.subject, "execute_python");
}

#[tokio::test]
async fn the_same_client_finds_its_receipt_again_on_a_later_call() {
    // This is what the stable identity is for: the second call's receipt has to
    // be attributable to the same principal as the first, or "what did my last
    // call do" is unanswerable after a reconnect.
    let harness = harness(applying_payload());
    call(&harness, Some("ide-1")).await;
    call(&harness, Some("ide-1")).await;

    let rows = receipts(&harness, "ide-1");
    assert_eq!(rows.len(), 2, "both calls are recorded under one client");
    assert!(rows.iter().all(|r| r.state == ReceiptState::Completed));
}

#[tokio::test]
async fn two_clients_never_share_a_receipt() {
    let harness = harness(applying_payload());
    call(&harness, Some("ide-1")).await;
    call(&harness, Some("ide-2")).await;

    assert_eq!(receipts(&harness, "ide-1").len(), 1);
    assert_eq!(receipts(&harness, "ide-2").len(), 1);
    // And neither can see the other's work.
    assert!(harness
        .journal
        .list(&owner_for("ide-3"), 16)
        .expect("list succeeds")
        .is_empty());
}

#[tokio::test]
async fn a_call_without_a_claim_is_recorded_under_the_anonymous_principal() {
    // An older client sends no header. It must keep working, and its receipts
    // must be findable — under a name that admits several clients share it.
    let harness = harness(applying_payload());
    assert_eq!(call(&harness, None).await, StatusCode::OK);

    let rows = receipts(&harness, "anonymous");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].state, ReceiptState::Completed);
}

#[tokio::test]
async fn a_malformed_claim_runs_the_call_but_records_no_receipt() {
    // The call itself must not break: refusing a tools/call over a bad header
    // would break clients for a bookkeeping problem. But the missing receipt
    // has to be missing rather than misattributed — folding it into the
    // anonymous principal would merge two different clients' history.
    let harness = harness(applying_payload());
    assert_eq!(
        call(&harness, Some("not a valid label!")).await,
        StatusCode::OK
    );

    assert!(
        receipts(&harness, "anonymous").is_empty(),
        "a refused claim must not be recorded as anonymous"
    );
    assert!(receipts(&harness, "not a valid label!").is_empty());
}

#[tokio::test]
async fn a_call_whose_payload_says_nothing_about_the_scene_is_still_recorded_honestly() {
    // No `applied` count: the call returned, but nothing established what it
    // changed. The receipt must say exactly that rather than implying success.
    let harness = harness(json!({"ok": true}));
    assert_eq!(call(&harness, Some("ide-1")).await, StatusCode::OK);

    let rows = receipts(&harness, "ide-1");
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].effect,
        tdmcp_core::EffectState::Unknown,
        "an unobserved effect must not be rounded to applied or none"
    );
    assert_eq!(rows[0].state, ReceiptState::UnknownEffect);
}

#[tokio::test]
async fn a_daemon_without_a_journal_still_answers_tool_calls() {
    // A daemon that could not open its journal degrades to "no receipts", not
    // "no service". Tool semantics must not depend on bookkeeping.
    // One registry, shared: the bridge advances the same queue the router
    // enqueues into, exactly as the real per-pid actor does.
    let registry = Arc::new(Mutex::new(registry()));
    let bridge: Arc<dyn BridgeRpc> = Arc::new(ActorBridge {
        registry: Arc::clone(&registry),
        canned: applying_payload(),
    });
    let state = AppState::new_shared(
        registry,
        Catalog::fallback(),
        bridge,
        test_resource_provider().expect("resource provider"),
    );
    let router = build_mcp_router(state);
    let request = Request::builder()
        .method("POST")
        .uri("/mcp/tools/call")
        .header("content-type", "application/json")
        .header("x-tdmcp-client", "ide-1")
        .body(Body::from(
            json!({"name": "execute_python", "arguments": {"pid": 34, "script": "result = 1"}})
                .to_string(),
        ))
        .unwrap();
    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

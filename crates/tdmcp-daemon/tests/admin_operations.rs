//! The read-back route: a client can find its own receipts and account for the
//! dispatches nobody observed.
//!
//! The point of a receipt is that somebody can read it. These tests cover the
//! two claims that make that safe to rely on: a client sees only its own
//! history, and its account of an unobserved dispatch is recorded *beside* the
//! state rather than becoming one.

#![allow(clippy::unwrap_used, reason = "test setup/assertions may panic")]
#![allow(clippy::expect_used, reason = "test setup/assertions may panic")]
#![allow(clippy::panic, reason = "test assertions may panic")]

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tdmcp_core::{DaemonId, PidRegistry, SlaveRegistry};
use tdmcp_daemon::admin::{build_admin_router, RestartArgs};
use tdmcp_daemon::federation::FederationRuntime;
use tdmcp_daemon::opjournal::{
    JournalConfig, MemoryOperationJournal, OperationJournal, ReceiptState,
};
use tdmcp_daemon::receipt_port::ClientReceiptFactory;
use tdmcp_daemon::LogSink;
use tdmcp_diagnostics::Catalog;
use tdmcp_mcp::evidence::OwnerId;
use tdmcp_mcp::testing::{test_resource_provider, FakeBridgeRpc};
use tdmcp_mcp::AppState;
use tower::ServiceExt;

const CLIENT_HEADER: &str = "x-tdmcp-client";

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

/// The router, plus the journal so a test can stage receipts directly rather
/// than driving a whole dispatch through the transport.
fn admin_router(
    journal: Option<Arc<dyn OperationJournal>>,
) -> (axum::Router, Option<Arc<dyn OperationJournal>>) {
    let (sink, _ring) = test_log_sink();
    let receipts = journal
        .as_ref()
        .map(|journal| Arc::new(ClientReceiptFactory::new(Arc::clone(journal), "none", "")));
    let state = AppState::new(
        PidRegistry::new(),
        Catalog::fallback(),
        Arc::new(FakeBridgeRpc::responding(json!({}))),
        test_resource_provider().expect("resource provider"),
    );
    let router = build_admin_router(
        state,
        restart_args(),
        tokio_util::sync::CancellationToken::new(),
        Arc::new(AtomicBool::new(false)),
        test_federation(),
        sink,
        PathBuf::from("/tmp/tdmcp-test-logs"),
        receipts,
    );
    (router, journal)
}

fn journal() -> Arc<dyn OperationJournal> {
    Arc::new(MemoryOperationJournal::new(config()))
}

fn restart_args() -> RestartArgs {
    RestartArgs {
        exe: PathBuf::from("tdmcp-daemon"),
        port: 9860,
        bind_address: "127.0.0.1".into(),
        data_dir: PathBuf::from("."),
        bridge_dir: PathBuf::from("."),
        catalog_path: PathBuf::from("."),
        no_gui: true,
    }
}

fn test_federation() -> FederationRuntime {
    FederationRuntime {
        link_status: tokio::sync::watch::channel("Local only".to_owned()).0,
        settings: tdmcp_daemon::settings::Settings::new(
            PathBuf::from("."),
            tdmcp_config::ConfigFile::default(),
        ),
        role: "standalone".into(),
        daemon_id: DaemonId::new("test-daemon"),
        hostname: "localhost".into(),
        port: 9860,
        bind_address: "127.0.0.1".into(),
        config_path: PathBuf::from("."),
        slaves: Arc::new(Mutex::new(SlaveRegistry::new())),
        local_auth_token: String::new(),
        master_url: String::new(),
        master_psk: String::new(),
        version: "0.0.0".into(),
    }
}

use tokio::sync::Mutex;

fn test_log_sink() -> (LogSink, Arc<tdmcp_daemon::LogRing>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let appender = tracing_appender::rolling::never(dir.path(), "test.log");
    let (writer, guard) = tracing_appender::non_blocking(appender);
    std::mem::forget(guard);
    std::mem::forget(dir);
    let ring = Arc::new(tdmcp_daemon::LogRing::new(1024));
    let sink = LogSink::new(Arc::clone(&ring), writer);
    (sink, ring)
}

fn owner_of(label: &str) -> OwnerId {
    OwnerId::parse(format!("local:{label}")).expect("valid owner id")
}

/// Stage a dispatch that reached TouchDesigner and was never observed — the
/// exact shape a restart leaves behind.
fn stage_unobserved(journal: &Arc<dyn OperationJournal>, label: &str) -> String {
    let owner = owner_of(label);
    let id = journal.admit(&owner, "mutate_nodes", None).expect("admit");
    journal.mark_dispatched(&owner, &id).expect("dispatch");
    journal
        .mark_wait_expired(&owner, &id, "caller stopped waiting")
        .expect("wait expired");
    journal.recover_uncertain().expect("recover");
    id
}

async fn get(router: axum::Router, claim: Option<&str>, uri: &str) -> (StatusCode, Value) {
    let mut builder = Request::builder().method("GET").uri(uri);
    if let Some(claim) = claim {
        builder = builder.header(CLIENT_HEADER, claim);
    }
    let response = router
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .expect("router responds");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body reads");
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

async fn resolve(router: axum::Router, claim: &str, body: Value) -> (StatusCode, Value) {
    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/operations/resolve")
                .header(CLIENT_HEADER, claim)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .expect("router responds");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body reads");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn a_client_can_see_that_one_of_its_dispatches_was_never_observed() {
    let journal = journal();
    let id = stage_unobserved(&journal, "ide-1");
    let (router, _journal) = admin_router(Some(Arc::clone(&journal)));

    let (status, body) = get(router, Some("ide-1"), "/admin/operations").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], json!(true));
    assert_eq!(body["unresolved"], json!(1));
    let receipt = &body["receipts"][0];
    assert_eq!(receipt["id"], json!(id));
    assert_eq!(receipt["state"], json!("unknown_effect"));
    assert_eq!(receipt["effect"], json!("unknown"));
}

#[tokio::test]
async fn a_client_only_ever_sees_its_own_receipts() {
    let journal = journal();
    stage_unobserved(&journal, "ide-1");
    stage_unobserved(&journal, "ide-2");
    let (router, _journal) = admin_router(Some(journal));

    let (status, body) = get(router, Some("ide-1"), "/admin/operations").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["count"], json!(1));
    assert_eq!(body["receipts"][0]["owner"], json!("local:ide-1"));
}

#[tokio::test]
async fn an_observed_operation_is_not_reported_as_unresolved() {
    let journal = journal();
    let owner = owner_of("ide-1");
    let id = journal.admit(&owner, "mutate_nodes", None).expect("admit");
    journal.mark_dispatched(&owner, &id).expect("dispatch");
    journal
        .finish(
            &owner,
            &id,
            ReceiptState::Completed,
            tdmcp_core::EffectState::Applied,
            None,
        )
        .expect("finish");
    let (router, _journal) = admin_router(Some(journal));

    let (_status, body) = get(router, Some("ide-1"), "/admin/operations").await;
    assert_eq!(
        body["unresolved"],
        json!(0),
        "a completed call is not an outstanding question"
    );
}

#[tokio::test]
async fn accounting_for_a_dispatch_records_it_without_changing_the_record() {
    let journal = journal();
    let id = stage_unobserved(&journal, "ide-1");
    let (router, _journal) = admin_router(Some(Arc::clone(&journal)));

    let (status, body) = resolve(
        router.clone(),
        "ide-1",
        json!({"id": id, "observedEffect": "applied", "note": "the node is there"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // The response says outright that this was the caller's account and that
    // nothing about the daemon's own record changed.
    assert_eq!(body["observedBy"], json!("caller"));
    assert_eq!(body["stateUnchanged"], json!(true));
    let receipt = &body["receipt"];
    assert_eq!(
        receipt["state"],
        json!("unknown_effect"),
        "a caller's account must not become an observation"
    );
    assert_eq!(receipt["effect"], json!("unknown"));
    let resolution = &receipt["resolution"];
    assert_eq!(resolution["observedEffect"], json!("applied"));
    assert_eq!(resolution["note"], json!("the node is there"));

    // And the read-back route shows the same thing.
    let (_status, listed) = get(router.clone(), Some("ide-1"), "/admin/operations").await;
    assert_eq!(listed["receipts"][0]["state"], json!("unknown_effect"));
    assert!(listed["receipts"][0]["resolution"]["observedEffect"] == json!("applied"));
}

#[tokio::test]
async fn one_client_cannot_account_for_another_clients_dispatch() {
    let journal = journal();
    let id = stage_unobserved(&journal, "ide-1");
    let (router, _journal) = admin_router(Some(Arc::clone(&journal)));

    let (status, _body) = resolve(
        router,
        "ide-2",
        json!({"id": id, "observedEffect": "applied"}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // The owner's own record is untouched.
    let receipt = journal
        .get(&owner_of("ide-1"), &id)
        .expect("read")
        .expect("row");
    assert!(receipt.resolution.is_none());
}

#[tokio::test]
async fn a_malformed_claim_reads_nobody_s_receipts() {
    let journal = journal();
    stage_unobserved(&journal, "ide-1");
    let (router, _journal) = admin_router(Some(journal));

    let (status, body) = get(router, Some("not a valid label!"), "/admin/operations").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], json!(false));
    assert!(body.get("receipts").is_none(), "no rows, not an empty list");
}

#[tokio::test]
async fn a_daemon_without_a_journal_says_so_rather_than_reporting_no_problems() {
    // The distinction that matters: "you have nothing unresolved" and "I cannot
    // tell" are different answers, and only the second is true here.
    let (router, _journal) = admin_router(None);

    let (status, body) = get(router.clone(), Some("ide-1"), "/admin/operations").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], json!(false));
    assert_eq!(body["available"], json!(false));
    assert!(body.get("unresolved").is_none());

    let (status, _body) = resolve(
        router,
        "ide-1",
        json!({"id": "op_whatever", "observedEffect": "applied"}),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn the_row_limit_is_clamped_rather_than_trusted() {
    let journal = journal();
    for _ in 0..5 {
        stage_unobserved(&journal, "ide-1");
    }
    let (router, _journal) = admin_router(Some(journal));

    let (_status, body) = get(router, Some("ide-1"), "/admin/operations?limit=99999").await;
    assert_eq!(body["count"], json!(5));
    assert!(body["count"].as_u64().unwrap() <= 64);
}

#[tokio::test]
async fn the_cost_report_is_measured_bytes_and_never_a_token_claim() {
    // The ledger is process-wide, so this test only asserts the shape and the
    // unit, not a particular total. The accumulation behaviour is pinned by the
    // `cost::` unit tests against an isolated ledger.
    let (router, _journal) = admin_router(Some(journal()));

    let (status, body) = get(router, Some("ide-1"), "/admin/output-cost").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], json!(true));
    let report = &body["report"];
    assert_eq!(
        report["unit"],
        json!("serialized_bytes"),
        "a byte figure must never be readable as a token count"
    );
    // The ledger is process-wide, so a sibling test in this binary may already
    // have fed it. Assert the shape, not a particular total: the
    // "no measurement is not a measured zero" property is pinned by
    // `cost::tests::an_empty_ledger_reports_no_measurement_rather_than_no_saving`
    // against an isolated ledger, which is the only place it can be stated
    // without depending on test order.
    // Matched by reference: indexing a `Value` yields an owned `Value`, and the
    // panic arm needs the same binding afterwards.
    match &body["avoidedFraction"] {
        Value::Null => {}
        Value::Number(n) => {
            let fraction = n.as_f64().expect("a finite fraction");
            assert!(
                (0.0..=1.0).contains(&fraction),
                "avoided fraction must be a proportion, got {fraction}"
            );
        }
        other => panic!("avoidedFraction must be null or a number, got {other}"),
    }
    assert!(
        body["bytesAvoided"].as_u64().is_some(),
        "bytes avoided is numeric"
    );
    let rendered = body.to_string();
    assert!(!rendered.contains("tokenSav"), "{rendered}");
    assert!(
        rendered.contains("not tokens"),
        "the caveat travels with the data"
    );
}

#[tokio::test]
async fn real_dispatches_are_measured_into_the_shared_ledger() {
    // Proves the wiring rather than the arithmetic: a v2-wrapped inspect must
    // land in the ledger the route reads.
    let before = tdmcp_mcp::cost::global_report().calls;

    let bridge: Arc<dyn tdmcp_mcp::BridgeRpc> = Arc::new(FakeBridgeRpc::responding(json!({
        "ok": true,
        "nodes": []
    })));
    let state = AppState::new(
        registry_with_pid(),
        Catalog::fallback(),
        bridge,
        test_resource_provider().expect("resource provider"),
    );
    let router = tdmcp_mcp::build_mcp_router(state);
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/mcp/tools/call")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "name": "inspect",
                "arguments": {
                    "pid": 34,
                    "paths": ["/project1"],
                    "output": {"contract": "v2", "view": "compact"}
                }
            })
            .to_string(),
        ))
        .unwrap();
    let response = router.oneshot(request).await.expect("router responds");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body reads");
    let body: Value = serde_json::from_slice(&bytes).expect("json body");
    let data = &body["data"];
    assert_eq!(
        data["contract"],
        json!("v2"),
        "the v2 envelope must be what was measured"
    );
    assert_eq!(data["presentation"]["requested"], json!("compact"));

    let after = tdmcp_mcp::cost::global_report();
    assert!(
        after.calls > before,
        "a v2 response must be counted, got {} calls",
        after.calls
    );
    assert_eq!(after.unit, "serialized_bytes");
}

fn registry_with_pid() -> tdmcp_core::PidRegistry {
    let mut registry = tdmcp_core::PidRegistry::new();
    registry.handshake(
        34,
        tdmcp_core::ProcessAttrs {
            title: Some("test".into()),
            fingerprint: tdmcp_core::ProcessFingerprint {
                title: Some("test".into()),
                ..Default::default()
            },
            ..Default::default()
        },
        Some("1".into()),
    );
    registry
}

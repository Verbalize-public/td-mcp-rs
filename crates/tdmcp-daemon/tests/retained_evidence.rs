//! Integration: a retained observation survives the whole round trip, and only
//! the principal that asked for it can read it back.
//!
//! The unit tests in `tdmcp-mcp` prove the capture helper's protocol against a
//! fake, and `receipt_routing.rs` proves the claim→owner resolution. This file
//! is the claim that the two compose: an `inspect` over HTTP with
//! `output.retain: true` really commits bytes into a real store, the response
//! really names the id that commit returned, and the retained page really
//! decodes to the value the caller saw. A test of either half alone would pass
//! with the seam between them missing.

#![allow(clippy::unwrap_used, reason = "test setup/assertions may panic")]
#![allow(clippy::expect_used, reason = "test setup/assertions may panic")]
#![allow(clippy::panic, reason = "test assertions may panic")]

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tdmcp_core::{PidRegistry, ProcessAttrs, ProcessFingerprint, TaskResult};
use tdmcp_diagnostics::Catalog;
use tdmcp_mcp::evidence::{EvidenceId, EvidenceStore, OwnerId, ReadEncoding, ReadRequest};
use tdmcp_mcp::testing::test_resource_provider;
use tdmcp_mcp::{build_mcp_router, AppState, BridgeRpc};
use tower::ServiceExt;

use tdmcp_daemon::evidence_store::{EvidenceStoreConfig, MemoryEvidenceStore};
use tdmcp_daemon::opjournal::{JournalConfig, MemoryOperationJournal, OperationJournal};
use tdmcp_daemon::receipt_port::ClientReceiptFactory;

/// The reply the fake bridge returns for an `inspect`.
///
/// Deliberately not minimal: a retained observation that only round-tripped an
/// empty object would pass every assertion below while proving nothing about
/// the bytes actually preserved.
fn inspect_reply() -> Value {
    json!({
        "ok": true,
        "pid": 34,
        "nodes": [
            {
                "path": "/project1/box1",
                "exists": true,
                "opType": "boxCOMP",
                "params": {"const0value": 8, "enabled": true, "label": null}
            },
            {"path": "/project1/never", "exists": false}
        ],
        "timing": {"elapsedMs": 3}
    })
}

/// A store plus the router that writes into it.
struct Harness {
    store: Arc<MemoryEvidenceStore>,
    router: axum::Router,
}

fn journal_config() -> JournalConfig {
    JournalConfig {
        max_rows_per_owner: 16,
        max_total_rows: 64,
        max_in_flight_per_owner: 8,
        max_in_flight_total: 16,
        max_detail_bytes: 128,
        retention: Duration::from_secs(60),
    }
}

/// A router wired to a real in-memory evidence store and a real receipt
/// factory in `auth.mode = "none"`, so the claim is the only identity input and
/// the owner is derived exactly as a real unauthenticated daemon derives it.
fn harness() -> Harness {
    let store = Arc::new(
        MemoryEvidenceStore::new(EvidenceStoreConfig::default()).expect("an in-memory store"),
    );
    let journal: Arc<dyn OperationJournal> =
        Arc::new(MemoryOperationJournal::new(journal_config()));
    let registry = Arc::new(tokio::sync::Mutex::new(registry()));
    let bridge: Arc<dyn BridgeRpc> = Arc::new(ActorBridge {
        registry: Arc::clone(&registry),
        canned: inspect_reply(),
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
    )))
    .with_evidence(store.clone());
    Harness {
        store,
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

/// The bridge also plays the daemon's per-pid actor.
///
/// Queue progression belongs to the actor, not the tool layer: it starts the
/// enqueued task before sending and completes it after the reply. A fake that
/// only returned a value would leave the task queued forever, so a second call
/// in a test would be rejected as queue-busy before it was ever admitted.
struct ActorBridge {
    registry: Arc<tokio::sync::Mutex<PidRegistry>>,
    canned: Value,
}

#[async_trait::async_trait]
impl BridgeRpc for ActorBridge {
    async fn call(
        &self,
        pid: u32,
        _method: &str,
        _params: Value,
    ) -> Result<Value, tdmcp_mcp::BridgeRpcError> {
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

/// One `inspect` call, optionally with a claim and optional output options.
///
/// Returns the decoded `data` payload on success, or the error envelope.
async fn call(
    harness: &Harness,
    claim: Option<&str>,
    output: Option<Value>,
) -> Result<Value, Value> {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/mcp/tools/call")
        .header("content-type", "application/json");
    if let Some(claim) = claim {
        builder = builder.header("x-tdmcp-client", claim);
    }
    let mut arguments = json!({"pid": 34, "paths": ["/project1/box1", "/project1/never"]});
    if let Some(output) = output {
        arguments["output"] = output;
    }
    let request = builder
        .body(Body::from(
            json!({"name": "inspect", "arguments": arguments}).to_string(),
        ))
        .expect("request body");
    let response = harness
        .router
        .clone()
        .oneshot(request)
        .await
        .expect("router answers");
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "the tool route reports refusals in the body, never as a transport status"
    );
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let payload: Value = serde_json::from_slice(&bytes).expect("json body");
    if payload["ok"] == json!(true) {
        Ok(payload["data"].clone())
    } else {
        Err(payload)
    }
}

/// The owner a real unauthenticated daemon derives for `label`.
fn owner_for(label: &str) -> OwnerId {
    OwnerId::parse(format!("local:{label}")).expect("valid owner id")
}

/// Read a whole observation back in SMALL pages, following the cursor.
///
/// The page size is deliberately tiny relative to the payload, so this walks
/// several pages. That is the point: a retained observation is only useful if
/// paging over immutable committed bytes is STABLE, so the test reassembles
/// the document from the cursor rather than asking for it in one read.
fn read_back(harness: &Harness, label: &str, evidence_id: &str) -> Result<String, String> {
    let owner = owner_for(label);
    let id = EvidenceId::parse(evidence_id).map_err(|error| error.to_string())?;
    let mut text = String::new();
    let mut cursor: Option<tdmcp_mcp::evidence::EvidenceCursor> = None;
    // Bounded so a store that never stops handing out cursors fails here
    // instead of looping until the test times out.
    for _ in 0..64 {
        let page = harness
            .store
            .read(
                &owner,
                ReadRequest {
                    evidence_id: id.clone(),
                    part: "observation.json".to_owned(),
                    cursor: cursor.clone(),
                    max_bytes: 64,
                    encoding: ReadEncoding::Utf8,
                },
            )
            .map_err(|error| error.to_string())?;
        let tdmcp_mcp::evidence::PageContent::Utf8(fragment) = page.content else {
            return Err("a page was not returned as utf-8 text".to_owned());
        };
        text.push_str(&fragment);
        if page.complete {
            return Ok(text);
        }
        cursor = page.next_cursor;
        if cursor.is_none() {
            return Err("an incomplete page carried no cursor to continue from".to_owned());
        }
    }
    Err("paging never reported a complete page".to_owned())
}

#[tokio::test]
async fn a_retained_inspect_commits_bytes_the_same_client_can_read_back_verbatim() {
    let harness = harness();
    let data = call(
        &harness,
        Some("ide-1"),
        Some(json!({"contract": "v2", "view": "full", "retain": true})),
    )
    .await
    .expect("a retained call succeeds");

    // The block is present on EVERY v2 response, so "not retained" can never be
    // the same reply as "retained but unmentioned".
    let block = data
        .get("retention")
        .expect("a v2 document carries a retention block");
    assert_eq!(block["retained"], json!(true), "not retained: {block}");
    let evidence_id = block["evidenceId"]
        .as_str()
        .expect("a retained block names the id it committed")
        .to_owned();
    assert!(
        block["expiresAtMs"].is_i64(),
        "no lease was reported: {block}"
    );
    // A committed capture carries NO reason and no failure flag. Reporting a
    // reason next to a durable id would invite a caller to treat a committed
    // observation as something that went wrong.
    assert!(
        block.get("reason").is_none() && block.get("retentionFailed").is_none(),
        "a committed capture also reported a failure: {block}"
    );

    // The id is real: the store knows it, and it belongs to the caller.
    let record = harness
        .store
        .inspect(
            &owner_for("ide-1"),
            &EvidenceId::parse(&evidence_id).expect("parses"),
        )
        .expect("the store knows the id the response named");
    assert_eq!(record.owner, owner_for("ide-1"));
    assert!(
        record.complete,
        "a record returned before publication is not evidence"
    );
    assert_eq!(
        record.bytes,
        block["bytes"].as_u64().expect("byte count reported"),
        "the response claimed a byte count the record disagrees with"
    );

    // The bytes are what the caller saw: a retained page answers the same
    // question `payloadHash` claims it answers, including the distinctions the
    // whole contract is about — absent is not false, null is not missing.
    let text = read_back(&harness, "ide-1", &evidence_id).expect("the owner can read it back");
    let stored: Value = serde_json::from_str(&text).expect("the retained bytes are json");
    assert_eq!(
        stored,
        inspect_reply(),
        "the retained bytes are not the legacy value"
    );
    assert_eq!(stored["nodes"][0]["params"]["label"], Value::Null);
    assert_eq!(stored["nodes"][0]["params"]["enabled"], json!(true));
    assert_eq!(stored["nodes"][1]["exists"], json!(false));
    assert_eq!(
        data["data"], stored,
        "the inline value and the retained page disagree"
    );
}

#[tokio::test]
async fn another_client_cannot_read_evidence_the_first_one_retained() {
    let harness = harness();
    let data = call(
        &harness,
        Some("ide-1"),
        Some(json!({"contract": "v2", "view": "full", "retain": true})),
    )
    .await
    .expect("a retained call succeeds");
    let evidence_id = data["retention"]["evidenceId"]
        .as_str()
        .expect("an id")
        .to_owned();

    // The owner CAN read it. Asserted first, because a store that refuses
    // everybody would make every refusal below pass for the wrong reason.
    assert!(
        read_back(&harness, "ide-1", &evidence_id).is_ok(),
        "the client that retained it cannot read it back, so the refusals below \
         would prove nothing"
    );

    // The id is opaque, not a capability: knowing it is not authorisation.
    let refusal = read_back(&harness, "ide-2", &evidence_id)
        .expect_err("a second client must not read the first client's evidence");
    assert!(
        refusal.contains("denied") || refusal.contains("not found"),
        "the refusal does not say why: {refusal}"
    );

    // And the anonymous principal is no exception to that.
    let anonymous = read_back(&harness, "anonymous", &evidence_id)
        .expect_err("an unauthenticated caller must not read it either");
    assert!(anonymous.contains("denied") || anonymous.contains("not found"));
}

#[tokio::test]
async fn a_call_that_did_not_ask_for_retention_writes_nothing_to_the_store() {
    let harness = harness();
    let data = call(
        &harness,
        Some("ide-1"),
        Some(json!({"contract": "v2", "view": "full"})),
    )
    .await
    .expect("an ordinary call succeeds");

    // A store being bound is not consent. Retention is as explicit as a compact
    // view, and the reply says so rather than being silent about it.
    let block = &data["retention"];
    assert_eq!(block["retained"], json!(false), "{block}");
    // The block reports WHY in words rather than as a flag, and a decline
    // carries no failure flag — "I was not asked to" is not a failure.
    assert!(
        block["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("not requested"),
        "{block}"
    );
    assert!(
        block.get("retentionFailed").is_none(),
        "an unrequested capture was reported as a failure: {block}"
    );
    assert!(
        !data["data"]
            .as_object()
            .expect("object")
            .contains_key("retention"),
        "retention leaked into the document body"
    );

    // Nothing was committed: the owner's own listing is empty.
    let request = ReadRequest {
        evidence_id: EvidenceId::parse("nonexistent").expect("parses"),
        part: "observation.json".to_owned(),
        cursor: None,
        max_bytes: 64,
        encoding: ReadEncoding::Utf8,
    };
    assert!(harness.store.read(&owner_for("ide-1"), request).is_err());
}

#[tokio::test]
async fn a_refused_claim_is_refused_for_retention_rather_than_defaulted_to_a_principal() {
    let harness = harness();
    // A claim the daemon refuses: `:` is excluded from client labels precisely
    // so a label cannot be crafted to look like a different scope. This is the
    // case where there IS no principal, and the tempting shortcut — fall back
    // to a shared default owner — would let any caller who cannot authenticate
    // write into (and later read) one principal's retained evidence.
    let refusal = call(
        &harness,
        Some("local:someone-else"),
        Some(json!({"contract": "v2", "view": "full", "retain": true})),
    )
    .await
    .expect_err("a refused claim cannot be given a principal to retain under");
    let summary = refusal["summary"].as_str().unwrap_or_default().to_owned();
    assert!(
        summary.contains("retain") || summary.contains("client identity"),
        "the refusal does not say why retention was impossible: {refusal:#}"
    );

    // And nothing was written under ANY principal — not the forged label, not a
    // default. Probed through a list of every principal the daemon could have
    // chosen, because a store that quietly kept bytes somewhere would still
    // have handed a caller a durable handle it has no principal for.
    for label in [
        "someone-else",
        "local:someone-else",
        "anonymous",
        "defaulted",
        "shared",
    ] {
        let probe = harness.store.inspect(
            &owner_for(label),
            &EvidenceId::parse("nonexistent").expect("parses"),
        );
        assert!(
            probe.is_err(),
            "a refused claim left evidence under {label}"
        );
    }
}

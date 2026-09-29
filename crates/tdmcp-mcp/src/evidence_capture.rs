//! Producer and reader wiring for retained evidence.
//!
//! The store in `tdmcp-daemon` and the port in [`crate::evidence`] exist and
//! are tested, but until something WRITES an observation and something else
//! READS it back, they are a library and not a capability. This module is the
//! seam between the two: it turns a value a tool already produced into a
//! committed observation, and it is deliberately the only place that knows
//! how to do so.
//!
//! # Why the producer is here and not in `tools.rs`
//!
//! Because the write must be a DECISION made where the observation is already
//! in hand. A producer that ran for every call would retain every payload
//! nobody asked to keep; a producer bolted onto the argument decoder would be
//! reachable with an `output.retain` the caller never set. Retention is an
//! explicit request, and an explicit request gets an explicit outcome.
//!
//! # Honesty rules this module holds to
//!
//! * **A capture is never reported as durable when it failed to commit.**
//!   `capture` reports the store's own error; it does not retry, because a
//!   blind retry could leave two observations claiming the same bytes.
//! * **The bytes are exactly what the caller saw.** No re-serialization to a
//!   different shape, no normalization, no key reordering. The stored part is
//!   the same [`serde_json::Value`] the response was built from.
//! * **A capture never claims to be a fresh observation.** The timestamp is
//!   the one the record already carries, and the coverage block is the
//!   record's own coverage, not a fresh claim about the scene.
//! * **Retention failure never becomes a silent success.** A caller that asked
//!   to retain gets a durable id or a failure naming why — never the inline
//!   value with the id quietly dropped.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::evidence::{
    AppendChunk, EvidenceError, EvidenceId, EvidencePage, EvidenceStore, OwnerId, PartKind,
    PartManifest, ReadRequest, Reservation, ReserveObservation,
};
use crate::output_v2::ObservationRecord;

/// What a capture actually did, in the words a caller needs.
///
/// The three arms are deliberately distinct because a caller that asked to
/// retain has to be able to tell them apart: the observation is durable and
/// readable, it was not retained, or retention was requested and could not be
/// made durable. Collapsing the last two into "here is the value" is the exact
/// substitution the contract forbids.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CaptureOutcome {
    /// Committed and readable. The id is the caller's handle.
    Retained {
        /// Opaque observation id, valid for reads within its lease.
        evidence_id: EvidenceId,
        /// Lease expiry, so a caller can decide when to re-pin.
        expires_at_ms: i64,
        /// Exact bytes committed across all parts.
        bytes: u64,
    },
    /// Not retained. The value is still complete and was not trimmed.
    NotRetained {
        /// Why the observation was not kept, in plain words.
        reason: String,
    },
    /// Retention was asked for and could not be made durable.
    ///
    /// This is deliberately NOT folded into [`CaptureOutcome::NotRetained`]:
    /// a caller holding a request to retain needs to know the difference
    /// between "I declined" and "I tried and lost it", because the second may
    /// have written a partial object.
    RetentionFailed {
        /// The store's own error, verbatim inside a short lead.
        reason: String,
    },
}

impl CaptureOutcome {
    /// The retained id, when there is one.
    #[must_use]
    pub fn evidence_id(&self) -> Option<&EvidenceId> {
        match self {
            Self::Retained { evidence_id, .. } => Some(evidence_id),
            Self::NotRetained { .. } | Self::RetentionFailed { .. } => None,
        }
    }

    /// Whether a durable observation exists for this call.
    #[must_use]
    pub fn is_retained(&self) -> bool {
        self.evidence_id().is_some()
    }
}

/// The name of the single part a JSON capture is stored under.
///
/// It is not a filesystem path and never becomes one: the store treats a part
/// name as an opaque label, so a caller cannot address anything outside the
/// observation it was given.
const JSON_PART: &str = "observation.json";

/// Content type recorded for a JSON capture.
const JSON_CONTENT_TYPE: &str = "application/json";

/// Lowercase hexadecimal SHA-256 of the exact bytes a capture stores.
///
/// Written out here so the producer's integrity contract is visible at the
/// call site: the manifest declares a digest and the store re-verifies it at
/// finalize. A capture that declared a wrong digest would fail to commit,
/// which is the correct outcome — a refusal beats an unreadable observation.
fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// The default retention window a producer uses when the caller sets none.
///
/// Long enough that a capture is useful across a task, short enough that a
/// forgotten capture does not accumulate forever. Stated as one constant so it
/// is a number a reader can find rather than a per-call-site guess.
pub const DEFAULT_RETENTION: Duration = Duration::from_secs(3600);

/// A capture request built from a value a tool already produced.
///
/// Every field is derived from the observation the caller is about to see, so
/// the stored document and the returned document cannot diverge.
#[derive(Debug, Clone)]
pub struct CaptureRequest<'a> {
    /// The principal the observation belongs to. Never inferred, never taken
    /// from a request argument.
    pub owner: &'a OwnerId,
    /// The exact value the caller receives inline.
    pub value: &'a Value,
    /// The record's own start time, in Unix epoch milliseconds.
    pub observed_at_ms: i64,
    /// The record's own coverage, retained verbatim.
    pub coverage: Value,
    /// Inert metadata describing what produced this capture.
    pub metadata: Value,
    /// Requested lease lifetime.
    pub ttl: Duration,
}

/// Commit one observation to the store and return what happened.
///
/// # Errors
///
/// Returns `Err` only when the store could not produce an outcome at all. A
/// store error that is itself an answer (quota, conflict) is folded into
/// [`CaptureOutcome::RetentionFailed`] so the caller can report it without
/// unwinding. There is no retry: a reservation or commit that reports an
/// uncertain outcome is surfaced with the store's own text, because a blind
/// retry could leave two observations claiming the same bytes.
pub fn capture(
    store: &dyn EvidenceStore,
    request: CaptureRequest<'_>,
) -> Result<CaptureOutcome, EvidenceError> {
    // The bytes are the exact serialization of the value the caller will see.
    // A `Value` built by a tool always serializes; a failure here would mean
    // the caller's own reply could not be serialized either.
    let bytes = match serde_json::to_vec(request.value) {
        Ok(bytes) => bytes,
        Err(error) => {
            return Ok(CaptureOutcome::RetentionFailed {
                reason: format!("the observation could not be serialized for retention: {error}"),
            })
        }
    };
    let length = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    let manifest = PartManifest {
        name: JSON_PART.to_owned(),
        content_type: JSON_CONTENT_TYPE.to_owned(),
        content_encoding: None,
        kind: PartKind::Utf8,
        bytes: length,
        sha256: sha256_hex(&bytes),
        binary_layout: None,
        metadata: json!({"producer": "tdmcp-mcp.evidence_capture", "part": JSON_PART}),
        coverage: request.coverage.clone(),
    };
    let reservation: Reservation = match store.reserve(
        request.owner,
        ReserveObservation {
            ttl: request.ttl,
            metadata: request.metadata.clone(),
            coverage: request.coverage.clone(),
            parts: vec![manifest],
        },
    ) {
        Ok(reservation) => reservation,
        Err(error) => {
            return Ok(CaptureOutcome::RetentionFailed {
                reason: format!("the observation was not admitted: {error}"),
            })
        }
    };

    // The reservation states the store's real chunk bound. `append_chunk`
    // refuses an oversized chunk rather than truncating it, so a value larger
    // than one chunk is sent as several in order.
    let max_chunk = usize::try_from(reservation.max_chunk_bytes)
        .unwrap_or(usize::MAX)
        .max(1);
    let mut offset = 0usize;
    while offset < bytes.len() {
        let end = offset.saturating_add(max_chunk).min(bytes.len());
        let chunk = &bytes[offset..end];
        if let Err(error) = store.append_chunk(
            request.owner,
            AppendChunk {
                upload_id: &reservation.upload_id,
                part: JSON_PART,
                offset: u64::try_from(offset).unwrap_or(u64::MAX),
                bytes: chunk,
            },
        ) {
            // A chunk that did not land leaves an incomplete upload. Aborting
            // releases the reservation; the reported failure is what stops a
            // caller treating a partial observation as durable.
            let _ = store.abort(request.owner, &reservation.upload_id);
            return Ok(CaptureOutcome::RetentionFailed {
                reason: format!("the observation could not be written: {error}"),
            });
        }
        offset = end;
    }

    if let Err(error) = store.finalize_part(request.owner, &reservation.upload_id, JSON_PART) {
        let _ = store.abort(request.owner, &reservation.upload_id);
        return Ok(CaptureOutcome::RetentionFailed {
            reason: format!("the observation could not be finalized: {error}"),
        });
    }

    match store.commit(request.owner, &reservation.upload_id) {
        Ok(record) => Ok(CaptureOutcome::Retained {
            evidence_id: record.evidence_id,
            expires_at_ms: record.expires_at_ms,
            bytes: record.bytes,
        }),
        Err(error) => {
            // A commit may have succeeded and reported `CommitUncertain`; the
            // store owns the recovery story for that, so the caller gets the
            // store's text rather than a guess.
            let _ = store.abort(request.owner, &reservation.upload_id);
            Ok(CaptureOutcome::RetentionFailed {
                reason: format!("the observation was not committed: {error}"),
            })
        }
    }
}

/// Refuse a capture above `store_max_bytes` before any upload is started.
///
/// A producer that stored whatever it was handed would let one call exhaust
/// an owner's quota and then refuse every later capture for that owner, which
/// reads as the store being broken. The bound is checked before the
/// reservation so an oversized value costs nothing.
pub fn ensure_capturable(store_max_bytes: u64, value: &Value) -> Result<(), String> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| format!("the observation could not be measured: {error}"))?;
    let length =
        u64::try_from(bytes.len()).map_err(|_| "the observation is too large".to_owned())?;
    if length > store_max_bytes {
        return Err(format!(
            "the observation is {length} bytes, above the {store_max_bytes}-byte retention bound"
        ));
    }
    Ok(())
}

/// A stable, paginated read of a retained observation.
///
/// The wrapper adds only the honest naming of "there is no next page", which a
/// caller otherwise has to infer from a missing cursor.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadOutcome {
    /// The page exactly as the store returned it.
    pub page: EvidencePage,
    /// Whether the caller may ask for another page.
    pub has_more: bool,
}

/// Read one bounded page of a retained observation.
///
/// # Errors
///
/// Propagates the store's error verbatim, including [`EvidenceError::Released`]
/// and [`EvidenceError::Expired`], which a caller must be able to tell apart
/// from [`EvidenceError::NotFound`]: the first two mean the observation existed
/// and is gone on purpose, the last means it never did.
pub fn read_page(
    store: &dyn EvidenceStore,
    owner: &OwnerId,
    request: ReadRequest,
) -> Result<ReadOutcome, EvidenceError> {
    let page = store.read(owner, request)?;
    let has_more = page.next_cursor.is_some();
    Ok(ReadOutcome { page, has_more })
}

/// Build the retention block a caller receives alongside a retained value.
///
/// A retained capture returns its id AND keeps the value inline, so a caller
/// that did not ask to page can ignore the block entirely. The block is
/// present in all three outcomes so "not retained" and "retained but
/// unmentioned" can never be the same reply.
#[must_use]
pub fn retention_block(outcome: &CaptureOutcome) -> Value {
    match outcome {
        CaptureOutcome::Retained {
            evidence_id,
            expires_at_ms,
            bytes,
        } => json!({
            "retained": true,
            "evidenceId": evidence_id.as_str(),
            "expiresAtMs": expires_at_ms,
            "bytes": bytes,
        }),
        CaptureOutcome::NotRetained { reason } => json!({
            "retained": false,
            "reason": reason,
        }),
        CaptureOutcome::RetentionFailed { reason } => json!({
            "retained": false,
            "reason": reason,
            "retentionRequested": true,
            "retentionFailed": true,
        }),
    }
}

/// The retention block for a capture whose task did not come back at all.
///
/// A blocking task that was cancelled or panicked is neither a decline nor a
/// reported store error: the commit may or may not have landed. Calling that
/// "not retained" would invite the caller to retry and risk two observations
/// claiming the same bytes, and calling it retained would hand back a handle
/// nobody confirmed. So the uncertainty is named, and it is the one case a
/// caller must resolve by inspecting before retrying.
#[must_use]
pub fn uncertain_block(reason: String) -> Value {
    json!({
        "retained": false,
        "reason": reason,
        "retentionRequested": true,
        "retentionFailed": true,
        // Distinct from every other outcome on purpose.
        "retentionUncertain": true,
    })
}

/// The observation timestamp a record carries, in Unix epoch milliseconds.
///
/// A producer stamps a capture with the time the READ happened, never a fresh
/// `now()` at retention time: a capture taken thirty seconds after the read
/// must not claim to describe the scene at retention time.
#[must_use]
pub fn observed_at_ms(record: &ObservationRecord) -> i64 {
    record.started_at.timestamp_millis()
}

/// A capture's coverage, taken from the record's own accounting.
///
/// The record already distinguishes `missing` from `unavailable` and carries a
/// truncation flag, and those are exactly the distinctions a retained
/// observation must not lose, so the whole coverage object is stored verbatim
/// rather than summarised to a count.
#[must_use]
pub fn coverage_of(record: &ObservationRecord) -> Value {
    serde_json::to_value(&record.coverage).unwrap_or(Value::Null)
}

/// Metadata stored beside a capture, naming the observation it came from.
///
/// The identity block is the record's own: two captures of the same payload
/// have different `observationId` values on purpose, because a retained
/// capture is an EVENT and not a content address.
#[must_use]
pub fn metadata_of(record: &ObservationRecord, tool: &str) -> Value {
    let identity = record.identity();
    json!({
        "tool": tool,
        "observedAtMs": observed_at_ms(record),
        "observationId": identity.observation_id,
        "contract": identity.contract,
        "daemonInstance": identity.daemon_instance,
        "payloadHash": {
            "algorithm": record.payload_hash.algorithm,
            "encoding": record.payload_hash.encoding,
            "value": record.payload_hash.value,
        },
        "queryHash": {
            "algorithm": record.query_hash.algorithm,
            "encoding": record.query_hash.encoding,
            "value": record.query_hash.value,
        },
        "target": serde_json::to_value(&record.target).unwrap_or(Value::Null),
        "bridgeGeneration": record.bridge_generation,
    })
}

/// Commit a record's legacy value as one observation.
///
/// This is what a tool calls at the point it has a record in hand; keeping it
/// here means every tool's capture path has one shape instead of one per call
/// site.
#[must_use]
pub fn capture_record(
    store: &dyn EvidenceStore,
    owner: &OwnerId,
    record: &ObservationRecord,
    tool: &str,
    legacy: &Value,
    ttl: Duration,
) -> CaptureOutcome {
    match capture(
        store,
        CaptureRequest {
            owner,
            value: legacy,
            observed_at_ms: observed_at_ms(record),
            coverage: coverage_of(record),
            metadata: metadata_of(record, tool),
            ttl,
        },
    ) {
        Ok(outcome) => outcome,
        Err(error) => CaptureOutcome::RetentionFailed {
            reason: format!("the observation could not be retained: {error}"),
        },
    }
}

/// True when a store error means "the observation is gone on purpose".
///
/// A caller reading a stale handle must not be told the observation never
/// existed: [`EvidenceError::Released`] and [`EvidenceError::Expired`] are both
/// answers, and neither is [`EvidenceError::NotFound`].
#[must_use]
pub fn is_gone_deliberately(error: &EvidenceError) -> bool {
    matches!(error, EvidenceError::Released | EvidenceError::Expired)
}

/// A store and the OWNER one authenticated call resolved to.
///
/// The two travel together on purpose: a store without an owner has no
/// principal to scope a read or a write to, and an owner without a store has
/// nowhere to put anything. Bundling them means no call site can pass a store
/// and invent an owner beside it.
#[derive(Clone, Copy)]
pub struct EvidenceContext<'a> {
    /// The store this call may use.
    pub store: &'a Arc<dyn EvidenceStore>,
    /// The principal bound to this call by the transport layer.
    pub owner: &'a OwnerId,
}

/// One transport's resolved store-plus-owner, borrowed as a context.
///
/// Built where the request and the authenticated principal are both in hand
/// (the transport handler), then handed to dispatch. The store is a trait
/// object with no `Debug` bound and the trait is **synchronous** — it performs
/// blocking disk and database work — so a call must never reach it from a
/// future straight off the executor. Holding the pieces behind this one type
/// gives the producer a single place to enforce that, and gives the transport
/// a value it can own for the rest of the call.
pub struct EvidenceBinding {
    store: Arc<dyn EvidenceStore>,
    owner: OwnerId,
}

impl std::fmt::Debug for EvidenceBinding {
    /// Print the principal's SHAPE, never the principal.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EvidenceBinding")
            .field("owner_len", &self.owner.as_str().len())
            .field("store", &"<bound evidence store>")
            .finish()
    }
}

impl EvidenceBinding {
    /// Bind a store to the principal this request authenticated as.
    pub fn new(store: Arc<dyn EvidenceStore>, owner: OwnerId) -> Self {
        Self { store, owner }
    }

    /// Borrow as a dispatch context.
    #[must_use]
    pub fn context(&self) -> EvidenceContext<'_> {
        EvidenceContext {
            store: &self.store,
            owner: &self.owner,
        }
    }

    /// The bound store, for a call that already owns this binding.
    #[must_use]
    pub fn store(&self) -> &Arc<dyn EvidenceStore> {
        &self.store
    }

    /// The principal, for a call that already owns this binding.
    #[must_use]
    pub fn owner(&self) -> &OwnerId {
        &self.owner
    }
}

impl std::fmt::Debug for EvidenceContext<'_> {
    /// Print the principal's SHAPE, never the principal.
    ///
    /// An owner id is derived from a transport credential. A `Debug` line is
    /// routinely captured in logs, so printing the value would scatter
    /// principal identifiers through them; the length is enough to catch a
    /// truncated or mis-bound owner without disclosing one.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EvidenceContext")
            .field("owner_len", &self.owner.as_str().len())
            .field("store", &"<bound evidence store>")
            .finish()
    }
}

/// A producer bound to one store, for repeated captures by the same caller.
#[derive(Clone)]
pub struct EvidenceCapture {
    store: Arc<dyn EvidenceStore>,
}

impl std::fmt::Debug for EvidenceCapture {
    /// Report only that a store is BOUND, never anything about its state.
    ///
    /// `dyn EvidenceStore` has no `Debug`, and inventing one over its internals
    /// would print a database handle or a byte total that says nothing about
    /// whether any capture is durable. A log line claiming a retained
    /// observation from a debug-printed producer would be exactly the kind of
    /// claim nothing here can support.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EvidenceCapture")
            .field("store", &"<bound evidence store>")
            .finish()
    }
}

impl EvidenceCapture {
    /// Bind a producer to a store.
    #[must_use]
    pub fn new(store: Arc<dyn EvidenceStore>) -> Self {
        Self { store }
    }

    /// The store behind this producer.
    #[must_use]
    pub fn store(&self) -> &Arc<dyn EvidenceStore> {
        &self.store
    }

    /// Commit one observation, returning what happened.
    ///
    /// # Errors
    ///
    /// As [`capture`].
    pub fn capture(&self, request: CaptureRequest<'_>) -> Result<CaptureOutcome, EvidenceError> {
        capture(self.store.as_ref(), request)
    }

    /// Read one page back, as a reader would.
    ///
    /// # Errors
    ///
    /// As [`read_page`].
    pub fn read(
        &self,
        owner: &OwnerId,
        request: ReadRequest,
    ) -> Result<ReadOutcome, EvidenceError> {
        read_page(self.store.as_ref(), owner, request)
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "unit tests"
)]
mod tests {
    use super::*;
    use crate::evidence::{
        EvidenceCursor, EvidenceGrant, EvidenceRecord, ExportReceipt, GrantRequest, ReadEncoding,
        TransferProgress, UploadId,
    };
    use std::sync::Mutex;

    /// A store that records what it was asked to do, so a test can assert the
    /// producer's call shape without a real backend.
    #[derive(Default)]
    struct RecordingStore {
        calls: Mutex<Vec<String>>,
        fail_reserve: Option<EvidenceError>,
        fail_append: Option<EvidenceError>,
        fail_finalize: Option<EvidenceError>,
        fail_commit: Option<EvidenceError>,
        page: Mutex<Option<EvidencePage>>,
    }

    impl RecordingStore {
        fn record(&self, call: &str) {
            self.calls.lock().unwrap().push(call.to_owned());
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }

        fn count(&self, prefix: &str) -> usize {
            self.calls()
                .iter()
                .filter(|c| c.starts_with(prefix))
                .count()
        }
    }

    fn owner() -> OwnerId {
        OwnerId::parse("client-a").unwrap()
    }

    fn id() -> EvidenceId {
        EvidenceId::parse("ev-1").unwrap()
    }

    fn record(bytes: u64) -> EvidenceRecord {
        EvidenceRecord {
            evidence_id: id(),
            owner: owner(),
            created_at_ms: 1,
            expires_at_ms: 2,
            pinned: false,
            bytes,
            metadata: Value::Null,
            coverage: Value::Null,
            parts: vec![PartManifest {
                name: JSON_PART.to_owned(),
                content_type: JSON_CONTENT_TYPE.to_owned(),
                content_encoding: None,
                kind: PartKind::Utf8,
                bytes,
                sha256: "0".repeat(64),
                binary_layout: None,
                metadata: Value::Null,
                coverage: Value::Null,
            }],
            complete: true,
        }
    }

    fn page(complete: bool) -> EvidencePage {
        EvidencePage {
            evidence_id: id(),
            part: JSON_PART.to_owned(),
            content_type: JSON_CONTENT_TYPE.to_owned(),
            content_encoding: None,
            byte_start: 0,
            byte_end: 4,
            total_bytes: 8,
            returned_bytes: 4,
            content: crate::evidence::PageContent::Utf8("abcd".to_owned()),
            next_cursor: if complete {
                None
            } else {
                Some(EvidenceCursor::parse("cur-1").unwrap())
            },
            complete,
            binary_layout: None,
        }
    }

    /// A store that fails one named step, and only that one.
    fn failing(step: &str, error: EvidenceError) -> RecordingStore {
        let mut store = RecordingStore::default();
        match step {
            "reserve" => store.fail_reserve = Some(error),
            "append" => store.fail_append = Some(error),
            "finalize" => store.fail_finalize = Some(error),
            _ => store.fail_commit = Some(error),
        }
        store
    }

    impl EvidenceStore for RecordingStore {
        fn reserve(
            &self,
            _owner: &OwnerId,
            request: ReserveObservation,
        ) -> Result<Reservation, EvidenceError> {
            self.record("reserve");
            if let Some(error) = &self.fail_reserve {
                return Err(error.clone());
            }
            let bytes = request.parts.iter().map(|p| p.bytes).sum();
            Ok(Reservation {
                upload_id: UploadId::parse("up-1").unwrap(),
                reserved_bytes: bytes,
                // Deliberately small so the chunk-splitting path is exercised.
                max_chunk_bytes: 4,
            })
        }

        fn append_chunk(
            &self,
            _owner: &OwnerId,
            request: AppendChunk<'_>,
        ) -> Result<TransferProgress, EvidenceError> {
            self.record(&format!("append@{}", request.offset));
            if let Some(error) = &self.fail_append {
                return Err(error.clone());
            }
            Ok(TransferProgress {
                next_offset: request.offset + u64::try_from(request.bytes.len()).unwrap_or(0),
                total_bytes: 64,
            })
        }

        fn finalize_part(
            &self,
            _owner: &OwnerId,
            _upload: &UploadId,
            part: &str,
        ) -> Result<(), EvidenceError> {
            self.record(&format!("finalize:{part}"));
            if let Some(error) = &self.fail_finalize {
                return Err(error.clone());
            }
            Ok(())
        }

        fn commit(
            &self,
            _owner: &OwnerId,
            _upload: &UploadId,
        ) -> Result<EvidenceRecord, EvidenceError> {
            self.record("commit");
            if let Some(error) = &self.fail_commit {
                return Err(error.clone());
            }
            Ok(record(10))
        }

        fn abort(&self, _owner: &OwnerId, _upload: &UploadId) -> Result<(), EvidenceError> {
            self.record("abort");
            Ok(())
        }

        fn inspect(
            &self,
            _owner: &OwnerId,
            _id: &EvidenceId,
        ) -> Result<EvidenceRecord, EvidenceError> {
            self.record("inspect");
            Ok(record(10))
        }

        fn read(
            &self,
            _owner: &OwnerId,
            _request: ReadRequest,
        ) -> Result<EvidencePage, EvidenceError> {
            self.record("read");
            if let Some(page) = self.page.lock().unwrap().clone() {
                return Ok(page);
            }
            Ok(page(false))
        }

        fn pin(
            &self,
            _owner: &OwnerId,
            _id: &EvidenceId,
            _ttl: Duration,
        ) -> Result<EvidenceRecord, EvidenceError> {
            self.record("pin");
            Ok(record(10))
        }

        fn release(&self, _owner: &OwnerId, _id: &EvidenceId) -> Result<(), EvidenceError> {
            self.record("release");
            Ok(())
        }

        fn grant(
            &self,
            _owner: &OwnerId,
            _request: GrantRequest,
        ) -> Result<EvidenceGrant, EvidenceError> {
            self.record("grant");
            Err(EvidenceError::Backend("no grants in this fake".to_owned()))
        }

        fn revoke_grant(
            &self,
            _owner: &OwnerId,
            _grant: &crate::evidence::GrantId,
        ) -> Result<(), EvidenceError> {
            self.record("revoke_grant");
            Ok(())
        }

        fn export(
            &self,
            _owner: &OwnerId,
            _request: crate::evidence::ExportRequest,
        ) -> Result<ExportReceipt, EvidenceError> {
            self.record("export");
            Err(EvidenceError::Backend("no exports in this fake".to_owned()))
        }
    }

    /// A record with the coverage a real inspect produces: some paths
    /// returned, one missing, one present-but-unreadable, none truncated.
    ///
    /// Built here rather than borrowed from `output_v2`'s tests so this
    /// module's tests state their own preconditions.
    fn sample_record() -> ObservationRecord {
        let legacy = json!({
            "ok": true,
            "nodes": [
                {"ok": true, "path": "/project1/a", "params": []},
                {"ok": false, "path": "/project1/missing", "code": "tdmcp.op.not_found"},
                {"ok": false, "path": "/project1/broken", "code": "tdmcp.op.inspect_failed"},
                {"ok": true, "path": "/project1/b"},
            ],
        });
        crate::output_v2::record_observation(
            &json!({"paths": ["/project1"]}),
            &legacy,
            chrono::Utc::now(),
            chrono::Utc::now(),
            Some((10, 20)),
            crate::output_v2::target_for(None, tdmcp_core::Pid(42), None),
            Some(7),
        )
    }

    /// An owner whose borrow outlives the request built from it.
    ///
    /// A request borrows its owner, so a caller passing a fresh `OwnerId`
    /// would hand back a request referencing a dropped temporary. Binding the
    /// owner to a `let` first is the caller's half of that contract, and the
    /// signature here is what forces it.
    fn request<'a>(owner: &'a OwnerId, value: &'a Value) -> CaptureRequest<'a> {
        CaptureRequest {
            owner,
            value,
            observed_at_ms: 1,
            coverage: json!({"captured": "all_requested"}),
            metadata: json!({"source": "test"}),
            ttl: DEFAULT_RETENTION,
        }
    }

    fn value() -> Value {
        json!({"ok": true, "nodes": [{"path": "/project1/a"}]})
    }

    #[test]
    fn a_capture_commits_and_returns_the_durable_handle() {
        let store = RecordingStore::default();
        let owner = owner();
        let value = value();
        let outcome = capture(&store, request(&owner, &value)).unwrap();
        match &outcome {
            CaptureOutcome::Retained {
                evidence_id, bytes, ..
            } => {
                assert_eq!(evidence_id, &id());
                assert_eq!(*bytes, 10);
            }
            other => panic!("expected a retained outcome, got {other:?}"),
        }
        assert!(outcome.is_retained());
        let calls = store.calls();
        // The order matters: reserve before writing, commit only after every
        // part is finalized. A commit before finalize would publish an
        // observation whose bytes are not yet durable.
        assert_eq!(calls.first().map(String::as_str), Some("reserve"));
        assert!(calls.contains(&"finalize:observation.json".to_owned()));
        assert_eq!(calls.last().map(String::as_str), Some("commit"));
    }

    #[test]
    fn a_value_larger_than_the_chunk_bound_is_split_not_refused() {
        let store = RecordingStore::default();
        // The fake advertises a 4-byte chunk bound and this value serializes
        // to far more, so several appends are required. `append_chunk` refuses
        // an oversized chunk, so sending it whole would fail the capture.
        let owner = owner();
        let value = json!({"payload": "a".repeat(200)});
        let outcome = capture(&store, request(&owner, &value)).unwrap();
        assert!(outcome.is_retained());
        let appends = store.count("append@");
        assert!(appends > 1, "a value over the chunk bound was sent whole");
    }

    #[test]
    fn chunks_are_sent_in_strict_offset_order() {
        let store = RecordingStore::default();
        let owner = owner();
        let value = json!({"payload": "abcdefghij"});
        assert!(capture(&store, request(&owner, &value))
            .unwrap()
            .is_retained());
        // The store records `"append@<offset>"`, so the offset is everything
        // after the `@`; parsing the whole label is the test's own bug, not
        // the producer's.
        let offsets: Vec<u64> = store
            .calls()
            .into_iter()
            .filter_map(|call| call.strip_prefix("append@")?.parse().ok())
            .collect();
        assert!(
            offsets.len() > 1,
            "expected several chunks, got {offsets:?}"
        );
        assert!(
            offsets.windows(2).all(|pair| pair[0] < pair[1]),
            "offsets were not strictly increasing: {offsets:?}"
        );
        // Strictly ordered and starting at zero: a gap would leave a hole the
        // store refuses, and starting above zero would skip the first bytes.
        assert_eq!(offsets.first().copied(), Some(0));
    }

    #[test]
    fn a_reserve_refusal_is_reported_and_nothing_is_written() {
        let store = failing(
            "reserve",
            EvidenceError::QuotaExceeded("owner is full".to_owned()),
        );
        let owner = owner();
        let value = value();
        let outcome = capture(&store, request(&owner, &value)).unwrap();
        match &outcome {
            CaptureOutcome::RetentionFailed { reason } => {
                assert!(reason.contains("not admitted"), "got: {reason}");
                // The store's own words must survive: a quota refusal
                // described only as "retention failed" is unactionable.
                assert!(reason.contains("owner is full"), "got: {reason}");
            }
            other => panic!("expected a retention failure, got {other:?}"),
        }
        // A refusal that wrote bytes would leave an upload nobody finishes.
        assert_eq!(store.calls(), vec!["reserve".to_owned()]);
    }

    #[test]
    fn a_write_refusal_aborts_the_upload_rather_than_leaving_it_half_written() {
        let store = failing("append", EvidenceError::Io("disk is full".to_owned()));
        let owner = owner();
        let value = value();
        let outcome = capture(&store, request(&owner, &value)).unwrap();
        assert!(matches!(outcome, CaptureOutcome::RetentionFailed { .. }));
        let calls = store.calls();
        assert!(
            calls.contains(&"abort".to_owned()),
            "an incomplete upload must be released, calls were {calls:?}"
        );
        assert!(
            !calls.contains(&"commit".to_owned()),
            "nothing may be committed after a failed write"
        );
    }

    #[test]
    fn a_finalize_refusal_also_releases_the_upload() {
        let store = failing("finalize", EvidenceError::HashMismatch);
        let owner = owner();
        let value = value();
        assert!(matches!(
            capture(&store, request(&owner, &value)).unwrap(),
            CaptureOutcome::RetentionFailed { .. }
        ));
        let calls = store.calls();
        assert!(calls.contains(&"abort".to_owned()), "calls were {calls:?}");
        assert!(!calls.contains(&"commit".to_owned()));
    }

    #[test]
    fn an_uncertain_commit_is_reported_and_never_retried() {
        let store = failing(
            "commit",
            EvidenceError::CommitUncertain("the acknowledgement was lost".to_owned()),
        );
        let owner = owner();
        let value = value();
        let outcome = capture(&store, request(&owner, &value)).unwrap();
        match &outcome {
            CaptureOutcome::RetentionFailed { reason } => {
                assert!(reason.contains("uncertain"), "got: {reason}");
            }
            other => panic!("expected the uncertainty surfaced, got {other:?}"),
        }
        // Exactly one commit attempt: a blind retry could leave two
        // observations claiming the same bytes.
        assert_eq!(
            store.count("commit"),
            1,
            "an uncertain commit must not be retried"
        );
    }

    #[test]
    fn the_declared_digest_and_length_match_the_exact_bytes() {
        let store = RecordingStore::default();
        let owner = owner();
        let value = json!({"ok": true, "zero": 0, "false": false, "null": null});
        let bytes = serde_json::to_vec(&value).unwrap();
        // The zero / false / null distinctions must survive serialisation
        // intact, or a retained observation would answer a different question
        // than the inline value did.
        let round: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(round, value);
        assert_eq!(round["zero"], json!(0));
        assert_eq!(round["false"], json!(false));
        assert!(round["null"].is_null());
        assert_eq!(sha256_hex(&bytes).len(), 64);
        assert!(capture(&store, request(&owner, &value))
            .unwrap()
            .is_retained());
    }

    #[test]
    fn an_oversized_capture_is_refused_before_any_upload_starts() {
        let store = RecordingStore::default();
        let value = json!({"payload": "a".repeat(4096)});
        let error = ensure_capturable(16, &value).unwrap_err();
        assert!(error.contains("above"), "got: {error}");
        // Refusing on size must not touch the store at all.
        assert!(store.calls().is_empty());
    }

    #[test]
    fn a_capture_within_the_bound_is_accepted() {
        assert!(ensure_capturable(4096, &value()).is_ok());
    }

    #[test]
    fn a_page_with_a_cursor_reports_more_and_a_final_page_does_not() {
        let store = RecordingStore::default();
        let partial = read_page(
            &store,
            &owner(),
            ReadRequest {
                evidence_id: id(),
                part: JSON_PART.to_owned(),
                cursor: None,
                max_bytes: 8,
                encoding: ReadEncoding::Utf8,
            },
        )
        .unwrap();
        assert!(partial.has_more);
        assert!(partial.page.next_cursor.is_some());

        let complete = RecordingStore::default();
        *complete.page.lock().unwrap() = Some(page(true));
        let last = read_page(
            &complete,
            &owner(),
            ReadRequest {
                evidence_id: id(),
                part: JSON_PART.to_owned(),
                cursor: Some(EvidenceCursor::parse("cur-1").unwrap()),
                max_bytes: 8,
                encoding: ReadEncoding::Utf8,
            },
        )
        .unwrap();
        assert!(!last.has_more, "a complete page must not advertise more");
        assert!(last.page.next_cursor.is_none());
        assert!(last.page.complete);
    }

    #[test]
    fn a_released_observation_is_not_reported_as_one_that_never_existed() {
        // The distinction a caller depends on: released and expired are
        // answers, and neither is NotFound.
        assert!(is_gone_deliberately(&EvidenceError::Released));
        assert!(is_gone_deliberately(&EvidenceError::Expired));
        assert!(!is_gone_deliberately(&EvidenceError::NotFound));
        assert!(!is_gone_deliberately(&EvidenceError::AccessDenied));
    }

    #[test]
    fn the_retention_block_distinguishes_declined_from_failed() {
        let retained = retention_block(&CaptureOutcome::Retained {
            evidence_id: id(),
            expires_at_ms: 7,
            bytes: 3,
        });
        assert_eq!(retained["retained"], json!(true));
        assert_eq!(retained["evidenceId"], json!("ev-1"));
        assert_eq!(retained["expiresAtMs"], json!(7));

        let declined = retention_block(&CaptureOutcome::NotRetained {
            reason: "the caller did not ask".to_owned(),
        });
        assert_eq!(declined["retained"], json!(false));
        assert!(declined.get("evidenceId").is_none());
        assert!(declined.get("retentionFailed").is_none());

        let failed = retention_block(&CaptureOutcome::RetentionFailed {
            reason: "the store refused".to_owned(),
        });
        assert_eq!(failed["retained"], json!(false));
        // A caller that asked to retain must be able to see that it failed,
        // not read it as a quiet decline.
        assert_eq!(failed["retentionRequested"], json!(true));
        assert_eq!(failed["retentionFailed"], json!(true));
    }

    #[test]
    fn a_capture_carries_the_records_own_coverage_and_identity() {
        // Coverage is stored verbatim so `missing` / `unavailable` /
        // `truncated` survive retention rather than collapsing to a count.
        let record = sample_record();
        let coverage = coverage_of(&record);
        assert!(coverage.get("missing").is_some(), "got: {coverage}");
        assert!(coverage.get("unavailable").is_some(), "got: {coverage}");
        assert!(coverage.get("truncated").is_some(), "got: {coverage}");

        let metadata = metadata_of(&record, "inspect");
        assert_eq!(metadata["tool"], json!("inspect"));
        // The stamp is the READ's time, not a fresh now() at retention time.
        assert_eq!(metadata["observedAtMs"], json!(observed_at_ms(&record)));
        assert_eq!(
            metadata["observedAtMs"],
            json!(record.started_at.timestamp_millis())
        );
        assert!(metadata["observationId"]
            .as_str()
            .is_some_and(|v| !v.is_empty()));
        assert_eq!(metadata["payloadHash"]["algorithm"], json!("sha256"));
    }
}

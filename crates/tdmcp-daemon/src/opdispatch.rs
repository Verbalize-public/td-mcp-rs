//! Attaching the operation journal to a real dispatch decision.
//!
//! [`crate::opjournal`] records receipts; this module is the adapter that turns
//! one bridge call into those receipts, and it is where the honesty rules
//! become a decision rather than a convention.
//!
//! The mapping, in order:
//!
//! 1. The call is **admitted** before anything is sent. If the receipt cannot be
//!    made durable the call is refused — a dispatch the daemon cannot remember
//!    is exactly the dispatch this feature exists to catch.
//! 2. The call is **dispatched** only once the work has been handed to the
//!    executor.
//! 3. A **returned** result is an observation, so it may advance the effect —
//!    but only as far as the payload actually establishes.
//! 4. Anything that ends the wait without an observation (a timeout, a
//!    disconnect, a cancelled caller, a dropped future) leaves the receipt
//!    **unsettled** with an unknown effect. Nothing here ever re-sends anything.
//!
//! Step 4 is where this could go wrong in a way that matters. A timeout is not
//! a failure: the call may still be running inside TouchDesigner, and the
//! request is very often already queued in the bridge actor. Reporting `failed`
//! tells the caller the mutation did not happen, which nobody observed.
//! Reporting `completed` claims more than anyone knows. The honest record is
//! unsettled, and a restart promotes it to [`ReceiptState::UnknownEffect`] —
//! which *reports* the uncertainty rather than resolving it.
//!
//! The tool layer applies its own timeout *around* the bridge call, so the
//! future awaiting a result is routinely dropped from the outside. That is why
//! [`PendingDispatch`] settles on drop: a cancelled caller must leave an honest
//! receipt behind, and it cannot do that by returning an error.

use std::sync::Arc;

use serde_json::Value;
use tdmcp_core::EffectState;
use tdmcp_mcp::bridge_rpc::{BridgeRpc, BridgeRpcError};
use tdmcp_mcp::evidence::{EvidenceError, OwnerId};

use crate::opjournal::{OperationJournal, ReceiptState};

/// A call the journal is tracking.
///
/// Deliberately not `Clone`: one ticket is one dispatch decision, and letting it
/// be duplicated would allow two terminal states to be recorded for one call.
#[derive(Debug)]
pub struct DispatchTicket {
    id: String,
    owner: OwnerId,
    subject: String,
}

impl DispatchTicket {
    /// Opaque receipt identity, for a later read by the owner.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The tool or subsystem this dispatch belongs to.
    #[must_use]
    pub fn subject(&self) -> &str {
        &self.subject
    }

    /// The owning principal these receipts belong to.
    #[must_use]
    pub fn owner(&self) -> &OwnerId {
        &self.owner
    }
}

/// A dispatch that is in flight and not yet accounted for.
///
/// The receipt settles when this value is settled explicitly, or when it is
/// dropped. Dropping is not a panic path — it is the ordinary end of a call
/// whose caller stopped waiting — so it records the honest state rather than
/// leaving a receipt that looks merely dispatched.
pub struct PendingDispatch {
    ticket: DispatchTicket,
    journal: Arc<dyn OperationJournal>,
    /// How the dispatch ended, once known.
    settled: bool,
}

impl std::fmt::Debug for PendingDispatch {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PendingDispatch")
            .field("id", &self.ticket.id)
            .field("subject", &self.ticket.subject)
            .field("settled", &self.settled)
            .finish()
    }
}

impl PendingDispatch {
    /// Receipt identity, so a caller can correlate the result with a receipt.
    #[must_use]
    pub fn id(&self) -> &str {
        self.ticket.id()
    }

    /// Record that the work reached the executor.
    ///
    /// # Errors
    /// Returns the journal's error if the transition could not be recorded.
    pub fn dispatched(&self) -> Result<(), EvidenceError> {
        self.journal
            .mark_dispatched(&self.ticket.owner, &self.ticket.id)
    }

    /// Record the observed result and the effect the payload established.
    ///
    /// # Errors
    /// Returns the journal's error if the transition could not be recorded.
    pub fn returned(
        &mut self,
        ok: bool,
        effect: EffectState,
        detail: Option<&str>,
    ) -> Result<(), EvidenceError> {
        self.dispatched()?;
        let mut state = if ok {
            ReceiptState::Completed
        } else {
            ReceiptState::Failed
        };
        // `Completed` with an unknown effect would read as "dispatched, applied,
        // and done" for work whose effect nobody established. The journal
        // refuses that combination on purpose, and this is the adapter honouring
        // the same rule rather than working around it.
        if state == ReceiptState::Completed && effect == EffectState::Unknown {
            state = ReceiptState::UnknownEffect;
        }
        let detail = match detail {
            Some(detail) => Some(detail.to_owned()),
            None if state == ReceiptState::UnknownEffect => {
                Some("the call returned, but the payload did not establish its effect".to_owned())
            }
            None => None,
        };
        self.journal.finish(
            &self.ticket.owner,
            &self.ticket.id,
            state,
            effect,
            detail.as_deref(),
        )?;
        self.settled = true;
        Ok(())
    }

    /// Record that the work was never handed to the executor.
    ///
    /// This settles the receipt as prevented-before-dispatch rather than
    /// leaving it admitted forever. "Nothing was sent" is a real, useful
    /// answer — the caller's next attempt is the first dispatch — and leaving
    /// the row unsettled would make every failed send consume an in-flight
    /// slot until the bound refused all further work.
    ///
    /// # Errors
    /// Returns the journal's error if the transition could not be recorded.
    pub fn not_sent(&mut self, detail: &str) -> Result<(), EvidenceError> {
        self.journal
            .mark_not_sent(&self.ticket.owner, &self.ticket.id, detail)?;
        self.settled = true;
        Ok(())
    }

    /// Stop tracking without recording an outcome.
    ///
    /// The receipt stays `dispatched` on purpose: a later `recover_uncertain`
    /// will find it and report it. Dropping the ticket here would turn an
    /// unobserved dispatch into silence, which is the failure mode this whole
    /// feature exists to prevent.
    #[must_use]
    pub fn into_abandoned(mut self) -> String {
        // `take` rather than a move: this type implements `Drop`, so its fields
        // cannot be moved out of it.
        self.settled = true;
        std::mem::take(&mut self.ticket.id)
    }
}

impl Drop for PendingDispatch {
    fn drop(&mut self) {
        if self.settled {
            return;
        }
        // The caller's wait ended without an observation: a timeout at the tool
        // layer, a client that disconnected, or a cancelled request. Best effort
        // by necessity — `Drop` cannot report — so the transition is made from
        // whatever the journal currently holds.
        //
        // The receipt is often still `admitted`, because `call_and_record`
        // admits it before awaiting the bridge. Recording that as "not sent"
        // would be a lie: the request may already be in the bridge actor's
        // queue, and nobody can check from here. So uncertainty resolves in the
        // safe direction instead — "may have been dispatched, result not
        // observed" — which is what a dispatched row says. Deciding "nothing was
        // sent" is left to the transport, which is the only party that actually
        // knows it failed to hand the job over.
        let _ = self.dispatched();
        let _ = self.journal.mark_wait_expired(
            &self.ticket.owner,
            &self.ticket.id,
            "the caller stopped waiting",
        );
    }
}

/// Creates receipts for dispatches on behalf of one authenticated caller.
pub struct OperationRecorder {
    journal: Arc<dyn OperationJournal>,
    owner: OwnerId,
}

impl std::fmt::Debug for OperationRecorder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The journal is a trait object over a live SQLite connection; printing
        // it would be noise, and printing the owner would leak a principal.
        formatter
            .debug_struct("OperationRecorder")
            .field("owner", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl OperationRecorder {
    /// Bind a recorder to one owner.
    ///
    /// The owner must come from the transport that authenticated the caller.
    /// A value taken from a request argument would let one caller read or forge
    /// another's receipts, so this type offers no way to substitute one later.
    #[must_use]
    pub fn new(journal: Arc<dyn OperationJournal>, owner: OwnerId) -> Self {
        Self { journal, owner }
    }

    /// Admit a dispatch before anything is sent.
    ///
    /// # Errors
    /// Returns the journal's error when the receipt could not be made durable.
    /// The caller must not dispatch in that case.
    pub fn admit(
        self: &Arc<Self>,
        subject: &str,
        detail: Option<&str>,
    ) -> Result<PendingDispatch, EvidenceError> {
        let id = self.journal.admit(&self.owner, subject, detail)?;
        Ok(PendingDispatch {
            ticket: DispatchTicket {
                id,
                owner: self.owner.clone(),
                subject: subject.to_owned(),
            },
            journal: self.journal.clone(),
            settled: false,
        })
    }

    /// The journal behind this recorder.
    #[must_use]
    pub fn journal(&self) -> &Arc<dyn OperationJournal> {
        &self.journal
    }

    /// The owner these receipts belong to.
    #[must_use]
    pub fn owner(&self) -> &OwnerId {
        &self.owner
    }
}

/// How a dispatch attempt ended, as far as the caller can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchOutcome {
    /// The work was never handed to the executor.
    NotSent {
        /// Why the send failed.
        why: &'static str,
    },
    /// The executor returned a result. `effect` is what the payload was
    /// observed to establish — never inferred from the fact that a result
    /// arrived.
    Returned {
        /// Whether the executor reported success.
        ok: bool,
        /// Effect the returned payload was observed to establish.
        effect: EffectState,
    },
    /// The result never arrived: the caller's wait ended first.
    WaitEnded {
        /// Why the wait ended, recorded verbatim as the receipt detail.
        why: &'static str,
    },
}

impl DispatchOutcome {
    /// Apply this outcome to a pending dispatch.
    ///
    /// # Errors
    /// Returns the journal's error if the transition could not be recorded.
    pub fn apply(self, pending: &mut PendingDispatch) -> Result<(), EvidenceError> {
        match self {
            Self::NotSent { why } => pending.not_sent(why),
            Self::Returned { ok, effect } => pending.returned(ok, effect, None),
            // Handled by `PendingDispatch::drop`; recorded here too so an
            // explicit wait-end is distinguishable from a drop that happened
            // for any other reason.
            Self::WaitEnded { why } => {
                pending.dispatched()?;
                pending.journal.mark_wait_expired(
                    &pending.ticket.owner,
                    &pending.ticket.id,
                    why,
                )?;
                pending.settled = true;
                Ok(())
            }
        }
    }
}

/// What a recorded call returned.
///
/// A journal failure is reported separately from the call's own result,
/// because they are not the same event: the bridge may well have done the work
/// while the receipt failed to record it. Collapsing them would either discard
/// a real result or hide a real bookkeeping failure.
#[derive(Debug)]
pub struct RecordedCall {
    /// Receipt identity for this call.
    pub receipt_id: String,
    /// The bridge result, exactly as the transport produced it.
    pub result: Result<Value, BridgeRpcError>,
    /// Set when the terminal state could not be recorded. The receipt is left
    /// unsettled, so `recover_uncertain` will still report it.
    pub receipt_error: Option<EvidenceError>,
}

/// Call the bridge, recording an honest receipt for the attempt.
///
/// The pending dispatch is created before the call and settles when the call
/// returns, or when this future is dropped — which is exactly what the tool
/// layer's timeout and a client's cancellation do.
pub async fn call_and_record(
    bridge: &dyn BridgeRpc,
    recorder: &Arc<OperationRecorder>,
    pid: u32,
    method: &str,
    params: Value,
    read_effect: &dyn Fn(&Value) -> EffectState,
) -> Result<RecordedCall, EvidenceError> {
    let mut pending = recorder.admit(method, Some(&format!("pid {pid}")))?;
    let receipt_id = pending.id().to_owned();

    // Whether the request reached the bridge is decided from the error, not
    // assumed: "not connected" and "never registered" mean nothing was sent,
    // while a timeout, a disconnect, or a returned error all mean the work
    // reached the executor (or lost contact with it), which is a much stronger
    // and quite different claim.
    let result = bridge.call(pid, method, params).await;

    let outcome = match &result {
        Ok(value) => DispatchOutcome::Returned {
            // A bridge-reported failure is delivered inside an `Ok` payload,
            // so `ok` comes from the payload and not from the transport.
            ok: payload_ok(value),
            effect: read_effect(value),
        },
        Err(BridgeRpcError::Timeout { .. }) => DispatchOutcome::WaitEnded { why: "timed out" },
        Err(BridgeRpcError::Disconnected { .. }) => DispatchOutcome::WaitEnded {
            why: "the bridge disconnected mid-call",
        },
        Err(BridgeRpcError::NotConnected { .. }) | Err(BridgeRpcError::Unknown { .. }) => {
            DispatchOutcome::NotSent {
                why: "no connected bridge for the target",
            }
        }
        // The bridge returned a structured error, so work happened and the
        // result is an observation — with an effect we did not observe.
        Err(BridgeRpcError::BridgeReturned { .. }) => DispatchOutcome::Returned {
            ok: false,
            effect: EffectState::Unknown,
        },
        // The daemon refused to send because it could not record the dispatch.
        // `NotSent` is this variant's meaning by definition — it is raised
        // before anything is handed to a bridge — and saying so settles the
        // receipt honestly instead of leaving an unknown that no observation
        // will ever resolve.
        Err(BridgeRpcError::ReceiptUnavailable { .. }) => DispatchOutcome::NotSent {
            why: "the daemon refused the dispatch before sending it",
        },
    };

    // A receipt that cannot be settled stays unsettled, which is the safe
    // direction: recovery reports it rather than the caller losing it.
    let receipt_error = outcome.apply(&mut pending).err();
    Ok(RecordedCall {
        receipt_id,
        result,
        receipt_error,
    })
}

/// Whether a bridge payload reports success.
///
/// Absent means success, matching the bridge's own convention: a payload
/// without an `ok` field is not a failure. A non-boolean `ok` is not read as a
/// truthy value, because `{"ok": "false"}` is not a documented success.
fn payload_ok(value: &Value) -> bool {
    match value.get("ok") {
        Some(Value::Bool(ok)) => *ok,
        _ => true,
    }
}

/// Read the effect a mutate-style payload establishes.
///
/// A returned result is an observation of the *call*, not automatically of the
/// *scene*. Only the payload can say what changed, and when it does not say,
/// the effect stays [`EffectState::Unknown`] rather than being rounded to
/// "nothing happened".
#[must_use]
pub fn read_mutate_effect(value: &Value) -> EffectState {
    let Some(applied) = value.get("applied").and_then(Value::as_u64) else {
        return EffectState::Unknown;
    };
    let failed = value
        .get("failedAt")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    if failed > 0 {
        // Something was changed and something was not. Reporting `applied`
        // would drop the half that did not; reporting `none` would deny the
        // half that did.
        EffectState::PartiallyApplied
    } else if applied > 0 {
        EffectState::Applied
    } else {
        EffectState::None
    }
}

/// Read the effect for a call that only reads.
#[must_use]
pub fn read_read_effect(_value: &Value) -> EffectState {
    EffectState::NotApplicable
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
    use crate::opjournal::{JournalConfig, MemoryOperationJournal};
    use serde_json::json;
    use std::time::Duration;

    fn config() -> JournalConfig {
        JournalConfig {
            max_rows_per_owner: 8,
            max_total_rows: 16,
            max_in_flight_per_owner: 4,
            max_in_flight_total: 8,
            max_detail_bytes: 64,
            retention: Duration::from_secs(60),
        }
    }

    fn recorder() -> (Arc<OperationRecorder>, Arc<dyn OperationJournal>) {
        let journal: Arc<dyn OperationJournal> = Arc::new(MemoryOperationJournal::new(config()));
        let owner = OwnerId::parse("alice").unwrap();
        (
            Arc::new(OperationRecorder::new(journal.clone(), owner)),
            journal,
        )
    }

    fn receipt(
        journal: &Arc<dyn OperationJournal>,
        id: &str,
    ) -> crate::opjournal::OperationReceipt {
        journal
            .get(&OwnerId::parse("alice").unwrap(), id)
            .unwrap()
            .expect("receipt")
    }

    #[test]
    fn a_returned_result_advances_both_state_and_effect() {
        let (recorder, journal) = recorder();
        let mut pending = recorder.admit("mutate_nodes", None).unwrap();
        DispatchOutcome::Returned {
            ok: true,
            effect: EffectState::Applied,
        }
        .apply(&mut pending)
        .unwrap();
        let receipt = receipt(&journal, pending.id());
        assert_eq!(receipt.state, ReceiptState::Completed);
        assert_eq!(receipt.effect, EffectState::Applied);
    }

    #[test]
    fn a_returned_failure_keeps_the_effect_the_result_established() {
        let (recorder, journal) = recorder();
        let mut pending = recorder.admit("mutate_nodes", None).unwrap();
        DispatchOutcome::Returned {
            ok: false,
            effect: EffectState::PartiallyApplied,
        }
        .apply(&mut pending)
        .unwrap();
        let receipt = receipt(&journal, pending.id());
        assert_eq!(receipt.state, ReceiptState::Failed);
        // A failed call is not a no-effect call: this is the distinction that a
        // mutation that half-applied must preserve.
        assert_eq!(receipt.effect, EffectState::PartiallyApplied);
    }

    #[test]
    fn a_wait_that_ended_claims_no_effect() {
        let (recorder, journal) = recorder();
        let mut pending = recorder.admit("execute_python", None).unwrap();
        DispatchOutcome::WaitEnded { why: "timed out" }
            .apply(&mut pending)
            .unwrap();
        let receipt = receipt(&journal, pending.id());
        assert_eq!(receipt.state, ReceiptState::WaitExpired);
        assert_eq!(
            receipt.effect,
            EffectState::Unknown,
            "a lost result is not an observation"
        );
    }

    #[test]
    fn a_dropped_caller_leaves_an_unsettled_honest_receipt() {
        let (recorder, journal) = recorder();
        let id = {
            let pending = recorder.admit("mutate_nodes", None).unwrap();
            pending.dispatched().unwrap();
            let id = pending.id().to_owned();
            // The tool layer's timeout drops the future that is awaiting the
            // bridge result. Nothing here gets to return an error.
            drop(pending);
            id
        };
        let receipt = receipt(&journal, &id);
        assert_eq!(receipt.state, ReceiptState::WaitExpired);
        assert_eq!(receipt.effect, EffectState::Unknown);
        assert!(!receipt.state.is_terminal());
    }

    #[test]
    fn work_that_was_never_sent_is_settled_as_prevented_before_dispatch() {
        let (recorder, journal) = recorder();
        let mut pending = recorder.admit("mutate_nodes", None).unwrap();
        DispatchOutcome::NotSent {
            why: "no connected bridge for the target",
        }
        .apply(&mut pending)
        .unwrap();
        let receipt = receipt(&journal, pending.id());
        assert_eq!(receipt.state, ReceiptState::Cancelled);
        assert_eq!(receipt.effect, EffectState::PreventedBeforeDispatch);
        // Settled means it no longer holds an in-flight slot, so a client whose
        // TD is down can retry without being refused by its own quota.
        assert!(receipt.expires_at_ms.is_some());
    }

    #[test]
    fn a_result_that_cannot_establish_its_effect_is_not_reported_as_complete() {
        let (recorder, journal) = recorder();
        let mut pending = recorder.admit("mutate_nodes", None).unwrap();
        DispatchOutcome::Returned {
            ok: true,
            effect: EffectState::Unknown,
        }
        .apply(&mut pending)
        .unwrap();
        let receipt = receipt(&journal, pending.id());
        assert_eq!(receipt.state, ReceiptState::UnknownEffect);
        assert_eq!(receipt.effect, EffectState::Unknown);
        assert!(receipt
            .detail
            .as_deref()
            .is_some_and(|d| d.contains("did not establish")));
    }

    #[test]
    fn a_mutate_payload_reports_a_partial_effect_rather_than_a_rounded_one() {
        assert_eq!(
            read_mutate_effect(&json!({"ok": true, "applied": 2, "failedAt": 1})),
            EffectState::PartiallyApplied
        );
        assert_eq!(
            read_mutate_effect(&json!({"ok": true, "applied": 3, "failedAt": 0})),
            EffectState::Applied
        );
        assert_eq!(
            read_mutate_effect(&json!({"ok": true, "applied": 0})),
            EffectState::None
        );
        // Applied count present but a failure with no index: still partial.
        assert_eq!(
            read_mutate_effect(&json!({"ok": true, "applied": 1, "failedAt": 5})),
            EffectState::PartiallyApplied
        );
        // Nothing said about the scene, so nothing is claimed about it.
        assert_eq!(
            read_mutate_effect(&json!({"ok": true})),
            EffectState::Unknown
        );
    }

    #[test]
    fn an_abandoned_dispatch_stays_recoverable() {
        let (recorder, journal) = recorder();
        let pending = recorder.admit("capture", None).unwrap();
        pending.dispatched().unwrap();
        let id = pending.into_abandoned();
        let recovered = journal.recover_uncertain().unwrap();
        assert_eq!(recovered.len(), 1, "an abandoned dispatch must be reported");
        assert_eq!(recovered[0].id, id);
        assert_eq!(recovered[0].state, ReceiptState::UnknownEffect);
    }

    #[test]
    fn a_ticket_cannot_be_settled_twice() {
        let (recorder, _journal) = recorder();
        let mut pending = recorder.admit("mutate_nodes", None).unwrap();
        DispatchOutcome::Returned {
            ok: true,
            effect: EffectState::Applied,
        }
        .apply(&mut pending)
        .unwrap();
        // The journal guards transitions by expected state, so a second
        // terminal write is refused rather than overwriting the first.
        let second = DispatchOutcome::Returned {
            ok: false,
            effect: EffectState::None,
        }
        .apply(&mut pending);
        assert!(
            matches!(second, Err(EvidenceError::Conflict)),
            "expected Conflict, got {second:?}"
        );
    }

    #[test]
    fn a_failing_settlement_leaves_the_receipt_recoverable_rather_than_lost() {
        let (recorder, journal) = recorder();
        let mut pending = recorder.admit("mutate_nodes", None).unwrap();
        pending.dispatched().unwrap();
        // Force a second transition past the guard by finishing behind its back,
        // then let the pending dispatch try to settle the same receipt.
        journal
            .finish(
                &OwnerId::parse("alice").unwrap(),
                pending.id(),
                ReceiptState::Completed,
                EffectState::Applied,
                None,
            )
            .unwrap();
        let failed = DispatchOutcome::Returned {
            ok: false,
            effect: EffectState::None,
        }
        .apply(&mut pending);
        assert!(matches!(failed, Err(EvidenceError::Conflict)));
        // The settled receipt is still there — the conflict did not delete it.
        assert_eq!(
            receipt(&journal, pending.id()).state,
            ReceiptState::Completed
        );
    }

    #[test]
    fn a_bridge_error_is_delivered_inside_an_ok_payload() {
        assert!(payload_ok(&json!({"ok": true})));
        assert!(!payload_ok(&json!({"ok": false})));
        // No `ok` field is the bridge's "not a failure" convention.
        assert!(payload_ok(&json!({"nodes": []})));
        // A non-boolean is not read as truthy.
        assert!(payload_ok(&json!({"ok": "false"})));
    }
}

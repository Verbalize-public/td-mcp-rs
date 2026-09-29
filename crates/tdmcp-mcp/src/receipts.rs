//! Recording what a dispatch actually did.
//!
//! The tool layer calls the bridge and gets back either a payload or a
//! transport error. What it cannot tell is whether the *scene* changed, because
//! a call that never returns is not a call that failed. This module is the port
//! between that gap and a durable receipt.
//!
//! # Why a port rather than a direct call
//!
//! The journal lives in `tdmcp-daemon`, which depends on this crate. Depending
//! the other way would be a cycle, so the daemon implements [`DispatchJournal`]
//! and the tool layer only ever sees this trait. That also keeps the honesty
//! rules on this side of the boundary: a caller cannot accidentally skip the
//! settle step, because [`DispatchGuard`] settles on drop.

use serde_json::Value;
use tdmcp_core::EffectState;

/// Wire methods that only read. A read cannot change the scene, so its effect
/// is [`EffectState::NotApplicable`] rather than an unknown — the difference
/// between "this could not have changed anything" and "we did not find out".
pub const READ_ONLY_METHODS: &[&str] = &[
    "api_help",
    "capture",
    "dialog_state",
    "editor_context",
    "inspect",
    "palette_index",
    "palette_probe",
    "project_lint",
    "td_installs",
];

/// Wire methods whose payload states what it applied.
pub const APPLYING_METHODS: &[&str] = &["execute_python", "mutate_nodes"];

/// Why a receipt could not be written.
///
/// A string rather than a typed error: the tool layer must not fail a call
/// because bookkeeping failed, and it must not pretend the bookkeeping
/// succeeded either. It reports the detail alongside the call's own result.
pub type JournalError = String;

/// One admitted dispatch whose outcome is not yet known.
pub trait ReceiptHandle: Send {
    /// Record a terminal state and the effect the payload established.
    fn settled(
        &mut self,
        ok: bool,
        effect: EffectState,
        detail: Option<&str>,
    ) -> Result<(), JournalError>;

    /// Opaque receipt identity, for the caller to correlate.
    fn id(&self) -> &str;

    /// Record that the dispatch's outcome was never observed.
    ///
    /// Called from [`DispatchGuard`]'s destructor. The requirement is that this
    /// leaves the receipt *unsettled* — recoverable, and honest about the fact
    /// that nobody saw what happened — and never records a failure. The request
    /// may already be inside TouchDesigner.
    fn abandoned(&mut self);
}

/// Admit dispatches and hand back a guard that settles them.
///
/// # Honesty notes
///
/// Implementations must not re-dispatch anything. A receipt exists to make an
/// unobserved operation *visible*, and a journal that "helpfully" retried would
/// convert a reported unknown into an unrequested mutation.
pub trait DispatchJournal: Send + Sync {
    /// Admit one dispatch before it is sent.
    ///
    /// Returning an error means the receipt is not durable, so the caller must
    /// not proceed: a dispatch the daemon cannot remember is the one case this
    /// whole feature exists to catch.
    /// The implementor supplies the owner, never the caller. A per-call owner
    /// argument would be a request-supplied identity, which is the one thing
    /// owner scoping exists to prevent.
    fn begin(&self, subject: &str) -> Result<DispatchGuard<'_>, JournalError>;
}

/// Keeps a dispatch honest until somebody says otherwise.
///
/// Dropping without settling records an *unsettled* receipt, never a failure.
/// That is the ordinary end of a call whose caller went away — a tool-layer
/// timeout, a client disconnect, a cancelled request — and the work may well
/// have been applied inside TouchDesigner.
pub struct DispatchGuard<'a> {
    handle: Option<Box<dyn ReceiptHandle + 'a>>,
}

impl<'a> DispatchGuard<'a> {
    /// Wrap a handle. Public so a [`DispatchJournal`] in another crate can
    /// build one; callers on the dispatch path never construct these directly.
    #[must_use]
    pub fn new(handle: Box<dyn ReceiptHandle + 'a>) -> Self {
        Self {
            handle: Some(handle),
        }
    }

    /// Receipt identity, if the journal produced one.
    #[must_use]
    pub fn id(&self) -> Option<&str> {
        self.handle.as_ref().map(|handle| handle.id())
    }

    /// Record the observed outcome.
    ///
    /// A returned error is reported to the caller rather than swallowed: the
    /// work may have happened while the receipt failed to record it, and
    /// collapsing the two would hide either the result or the bookkeeping
    /// failure.
    pub fn settled(
        &mut self,
        ok: bool,
        effect: EffectState,
        detail: Option<&str>,
    ) -> Result<(), JournalError> {
        let Some(mut handle) = self.handle.take() else {
            // Already settled. Settling twice is not an error worth failing a
            // call over, but it must not overwrite the first record either.
            return Ok(());
        };
        let result = handle.settled(ok, effect, detail);
        // Forget the handle without letting `Drop` re-record it, whatever the
        // journal's answer was: a failed settle is reported, not compounded.
        self.handle = None;
        result
    }
}

impl Drop for DispatchGuard<'_> {
    fn drop(&mut self) {
        // A caller that went away — a tool-layer timeout, a disconnected
        // client, a cancelled request — is the ordinary end of a dispatch, not
        // an error. Recording a failure here would tell the next reader the
        // write did not happen, which nobody observed either way.
        if let Some(mut handle) = self.handle.take() {
            handle.abandoned();
        }
    }
}

impl std::fmt::Debug for DispatchGuard<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DispatchGuard")
            .field("id", &self.id())
            .field("settled", &self.handle.is_none())
            .finish()
    }
}

/// Read the effect a mutating payload establishes.
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
        return EffectState::PartiallyApplied;
    }
    if applied > 0 {
        return EffectState::Applied;
    }
    EffectState::None
}

/// Read the effect for a wire method.
///
/// The method decides the *class* of operation; the payload decides what
/// happened within that class. A read that returned cannot have changed
/// anything, so its effect is not merely unknown — it is not applicable.
#[must_use]
pub fn read_effect(method: &str, value: &Value) -> EffectState {
    if READ_ONLY_METHODS.contains(&method) {
        return EffectState::NotApplicable;
    }
    if APPLYING_METHODS.contains(&method) {
        return read_mutate_effect(value);
    }
    // An unrecognised method could be anything. Claiming a scene change would
    // be worse than admitting ignorance.
    EffectState::Unknown
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    /// What a handle was last told, as `(ok, effect, detail)`.
    type Seen = Option<(bool, EffectState, Option<String>)>;

    #[test]
    fn a_payload_that_applied_everything_reports_applied() {
        assert_eq!(
            read_mutate_effect(&json!({"applied": 2, "failedAt": 0})),
            EffectState::Applied
        );
    }

    #[test]
    fn a_partially_applied_payload_is_not_rounded_to_either_half() {
        // This is the case the whole distinction exists for: 2 nodes changed
        // and 1 did not. Reporting `applied` loses the failure; `none` denies
        // the work that did happen.
        assert_eq!(
            read_mutate_effect(&json!({"applied": 2, "failedAt": 1})),
            EffectState::PartiallyApplied
        );
    }

    #[test]
    fn a_payload_that_applied_nothing_reports_none() {
        assert_eq!(
            read_mutate_effect(&json!({"applied": 0, "failedAt": 0})),
            EffectState::None
        );
    }

    #[test]
    fn a_payload_that_says_nothing_about_the_scene_stays_unknown() {
        // Absent is not zero. A result with no applied count establishes
        // nothing, and defaulting it to 0 would tell the caller the write did
        // not happen.
        for payload in [
            json!({}),
            json!({"ok": true}),
            json!({"steps": []}),
            json!({"applied": null}),
            json!({"applied": "2"}),
        ] {
            assert_eq!(
                read_mutate_effect(&payload),
                EffectState::Unknown,
                "payload {payload} establishes no effect"
            );
        }
    }

    #[test]
    fn a_read_that_returned_is_not_merely_unknown() {
        // The distinction a caller actually needs: "this could not have changed
        // anything" versus "we did not find out".
        assert_eq!(
            read_effect("inspect", &json!({})),
            EffectState::NotApplicable
        );
        assert_eq!(
            read_effect("capture", &json!({"width": 1})),
            EffectState::NotApplicable
        );
    }

    #[test]
    fn a_mutating_method_still_depends_on_its_payload() {
        assert_eq!(
            read_effect("mutate_nodes", &json!({"applied": 1, "failedAt": 0})),
            EffectState::Applied
        );
        assert_eq!(
            read_effect("execute_python", &json!({})),
            EffectState::Unknown
        );
    }

    #[test]
    fn an_unrecognised_method_claims_no_effect() {
        assert_eq!(
            read_effect("something_new", &json!({"applied": 5})),
            EffectState::Unknown
        );
    }

    /// A handle that records what it was told, so a guard's own behaviour can
    /// be observed without a journal.
    struct Recorder {
        id: String,
        seen: Arc<Mutex<Seen>>,
        abandoned: Arc<Mutex<bool>>,
    }

    impl ReceiptHandle for Recorder {
        fn settled(
            &mut self,
            ok: bool,
            effect: EffectState,
            detail: Option<&str>,
        ) -> Result<(), JournalError> {
            *self.seen.lock().expect("recorder lock") =
                Some((ok, effect, detail.map(str::to_owned)));
            Ok(())
        }

        fn id(&self) -> &str {
            &self.id
        }

        fn abandoned(&mut self) {
            *self.abandoned.lock().expect("recorder lock") = true;
        }
    }

    fn guard_over(seen: Arc<Mutex<Seen>>) -> DispatchGuard<'static> {
        guard_with_abandon_flag(Arc::clone(&seen), Arc::new(Mutex::new(false)))
    }

    fn guard_with_abandon_flag(
        seen: Arc<Mutex<Seen>>,
        abandoned: Arc<Mutex<bool>>,
    ) -> DispatchGuard<'static> {
        DispatchGuard::new(Box::new(Recorder {
            id: "receipt-1".to_owned(),
            seen,
            abandoned,
        }))
    }

    #[test]
    fn settling_records_the_observed_outcome() {
        let seen = Arc::new(Mutex::new(None));
        let mut guard = guard_over(Arc::clone(&seen));
        assert_eq!(guard.id(), Some("receipt-1"));
        guard
            .settled(true, EffectState::Applied, Some("2 nodes"))
            .unwrap();
        assert_eq!(
            *seen.lock().expect("recorder lock"),
            Some((true, EffectState::Applied, Some("2 nodes".to_owned())))
        );
    }

    #[test]
    fn settling_twice_does_not_overwrite_the_first_record() {
        let seen = Arc::new(Mutex::new(None));
        let mut guard = guard_over(Arc::clone(&seen));
        guard.settled(true, EffectState::Applied, None).unwrap();
        // A second settle is a no-op, not a rewrite: the first observation is
        // the one that actually happened.
        guard
            .settled(false, EffectState::None, Some("later"))
            .unwrap();
        assert_eq!(
            *seen.lock().expect("recorder lock"),
            Some((true, EffectState::Applied, None)),
            "the first record must survive"
        );
    }

    #[test]
    fn a_dropped_guard_records_that_the_outcome_was_never_observed() {
        // The safety-critical half of the contract. A caller that goes away is
        // the ordinary end of a dispatch, and nothing observed the effect — the
        // receipt has to say so rather than staying silently admitted.
        let seen = Arc::new(Mutex::new(None));
        let abandoned = Arc::new(Mutex::new(false));
        {
            let _guard = guard_with_abandon_flag(Arc::clone(&seen), Arc::clone(&abandoned));
        }
        assert!(*abandoned.lock().expect("recorder lock"));
        assert!(
            seen.lock().expect("recorder lock").is_none(),
            "an abandoned dispatch must not be recorded as having an outcome"
        );
    }

    #[test]
    fn a_settled_guard_is_not_also_abandoned() {
        // Otherwise every successful call would leave a second, contradictory
        // "outcome never observed" record behind.
        let seen = Arc::new(Mutex::new(None));
        let abandoned = Arc::new(Mutex::new(false));
        {
            let mut guard = guard_with_abandon_flag(Arc::clone(&seen), Arc::clone(&abandoned));
            guard.settled(true, EffectState::Applied, None).unwrap();
        }
        assert!(!*abandoned.lock().expect("recorder lock"));
    }

    #[test]
    fn a_failing_settle_is_reported_and_not_retried_on_drop() {
        struct Failing;
        impl ReceiptHandle for Failing {
            fn settled(
                &mut self,
                _ok: bool,
                _effect: EffectState,
                _detail: Option<&str>,
            ) -> Result<(), JournalError> {
                Err("disk full".to_owned())
            }
            fn id(&self) -> &str {
                "receipt-fail"
            }
            fn abandoned(&mut self) {}
        }
        let mut guard = DispatchGuard::new(Box::new(Failing));
        let error = guard
            .settled(true, EffectState::Applied, None)
            .expect_err("the journal failure must be reported");
        assert_eq!(error, "disk full");
        // Dropping must not re-record: the caller was already told, and a
        // second attempt against a full disk is noise.
    }
}

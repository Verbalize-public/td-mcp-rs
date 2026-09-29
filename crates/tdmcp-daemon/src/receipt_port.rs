//! The daemon's implementation of the tool layer's receipt port.
//!
//! [`crate::opdispatch`] knows how to record a dispatch honestly. It lives here
//! in the daemon because that is where the durable journal lives, and the tool
//! layer is in `tdmcp-mcp`, which this crate already depends on — not the other
//! way round. So the tool layer declares the port
//! ([`tdmcp_mcp::receipts::DispatchJournal`]) and this module satisfies it.
//!
//! # Who owns a receipt
//!
//! The owner is bound when the port is constructed, from the transport's
//! authenticated client identity. It is never a parameter of [`begin`]: an owner
//! supplied per call would be a request-supplied identity, which is precisely
//! what owner scoping exists to prevent. A client can therefore read back its
//! own receipts after a reconnect, and cannot reach anyone else's.
//!
//! [`begin`]: DispatchJournalPort::begin

use std::sync::Arc;

use tdmcp_core::EffectState;
use tdmcp_mcp::receipts::{DispatchGuard, DispatchJournal, JournalError, ReceiptHandle};

use crate::opdispatch::{OperationRecorder, PendingDispatch};

/// A receipt port bound to one authenticated client.
pub struct DispatchJournalPort {
    recorder: Arc<OperationRecorder>,
}

impl DispatchJournalPort {
    /// Bind the port to one client's recorder.
    #[must_use]
    pub fn new(recorder: Arc<OperationRecorder>) -> Self {
        Self { recorder }
    }

    /// The recorder behind this port, for direct receipt reads.
    #[must_use]
    pub fn recorder(&self) -> &Arc<OperationRecorder> {
        &self.recorder
    }
}

impl std::fmt::Debug for DispatchJournalPort {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The owner is deliberately not printed: a principal in a log line is
        // a tracking surface, and this type is `Debug` because the tool layer
        // holds it, not because its contents need inspecting.
        formatter
            .debug_struct("DispatchJournalPort")
            .finish_non_exhaustive()
    }
}

impl DispatchJournal for DispatchJournalPort {
    fn begin(&self, subject: &str) -> Result<DispatchGuard<'_>, JournalError> {
        let pending = self
            .recorder
            .admit(subject, None)
            .map_err(|error| format!("could not record the dispatch: {error}"))?;
        Ok(DispatchGuard::new(Box::new(JournalHandle { pending })))
    }
}

/// One in-flight dispatch, as the tool layer sees it.
struct JournalHandle {
    pending: PendingDispatch,
}

impl ReceiptHandle for JournalHandle {
    fn settled(
        &mut self,
        ok: bool,
        effect: EffectState,
        detail: Option<&str>,
    ) -> Result<(), JournalError> {
        self.pending
            .returned(ok, effect, detail)
            .map_err(|error| format!("the work finished but its receipt was not recorded: {error}"))
    }

    fn id(&self) -> &str {
        self.pending.id()
    }

    fn abandoned(&mut self) {
        // No transition here on purpose. `PendingDispatch` records the
        // unsettled state from its own destructor, and this handle is dropped
        // immediately after this returns — so doing it here would run the same
        // transition twice. The doc on the trait's `abandoned` describes the
        // outcome, and the outcome is exactly what that drop produces.
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
    use crate::opjournal::{JournalConfig, MemoryOperationJournal, OperationJournal, ReceiptState};
    use std::time::Duration;
    use tdmcp_mcp::evidence::OwnerId;

    fn port() -> (
        DispatchJournalPort,
        Arc<OperationRecorder>,
        Arc<dyn OperationJournal>,
    ) {
        let config = JournalConfig {
            max_rows_per_owner: 8,
            max_total_rows: 16,
            max_in_flight_per_owner: 4,
            max_in_flight_total: 8,
            max_detail_bytes: 128,
            retention: Duration::from_secs(60),
        };
        let journal: Arc<dyn OperationJournal> = Arc::new(MemoryOperationJournal::new(config));
        let recorder = Arc::new(OperationRecorder::new(
            Arc::clone(&journal),
            OwnerId::parse("client-a").unwrap(),
        ));
        (
            DispatchJournalPort::new(Arc::clone(&recorder)),
            recorder,
            journal,
        )
    }

    fn receipt(
        journal: &Arc<dyn OperationJournal>,
        id: &str,
    ) -> crate::opjournal::OperationReceipt {
        journal
            .get(&OwnerId::parse("client-a").unwrap(), id)
            .unwrap()
            .expect("receipt")
    }

    #[test]
    fn a_settled_dispatch_is_recorded_under_the_transports_owner() {
        let (port, _recorder, journal) = port();
        let mut guard = port.begin("mutate_nodes").expect("admitted");
        let id = guard.id().expect("an id").to_owned();
        guard
            .settled(true, EffectState::Applied, None)
            .expect("settled");

        let row = receipt(&journal, &id);
        assert_eq!(row.state, ReceiptState::Completed);
        assert_eq!(row.effect, EffectState::Applied);
    }

    #[test]
    fn a_dropped_dispatch_leaves_an_unsettled_receipt() {
        // The reason this adapter exists as its own layer: the tool layer drops
        // its future on its own timeout, and the request has already reached the
        // bridge. The receipt must not read as a failure.
        let (port, _recorder, journal) = port();
        let id = {
            let guard = port.begin("mutate_nodes").expect("admitted");
            guard.id().expect("an id").to_owned()
        };

        let row = receipt(&journal, &id);
        assert_eq!(
            row.state,
            ReceiptState::WaitExpired,
            "an abandoned dispatch is unobserved, not failed"
        );
        assert_eq!(row.effect, EffectState::Unknown);
        assert!(!row.state.is_terminal());
    }

    #[test]
    fn a_completed_result_with_an_unknown_effect_is_not_recorded_as_applied() {
        // The tool layer settles with whatever the payload established. If that
        // is "nothing", the receipt must not claim the work is done.
        let (port, _recorder, journal) = port();
        let mut guard = port.begin("mutate_nodes").expect("admitted");
        let id = guard.id().expect("an id").to_owned();
        guard
            .settled(true, EffectState::Unknown, None)
            .expect("settled");

        assert_eq!(receipt(&journal, &id).state, ReceiptState::UnknownEffect);
    }

    #[test]
    fn one_client_cannot_read_another_clients_receipt() {
        // The port is bound to an owner precisely so this is structural rather
        // than a check each call site has to remember.
        let (_port, _recorder, journal) = port();
        assert!(journal
            .get(&OwnerId::parse("client-b").unwrap(), "anything")
            .unwrap()
            .is_none());
    }

    #[test]
    fn the_port_does_not_print_its_owner() {
        // A principal in a `Debug` line is a tracking surface; the type is
        // Debug only because the tool layer holds it.
        let (port, _recorder, _journal) = port();
        let rendered = format!("{port:?}");
        assert!(!rendered.contains("client-a"), "owner leaked: {rendered}");
    }
}

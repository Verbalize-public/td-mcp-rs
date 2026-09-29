//! Fixed cost instrumentation for tool responses.
//!
//! # The unit is serialized bytes. It is not tokens.
//!
//! Every number here is a count of bytes in the JSON that was actually
//! written. That is a real, reproducible measurement. It is *not* a token
//! count, and nothing computed in this module may be presented as one.
//!
//! The distinction is not pedantry. Tokens are a property of a specific
//! tokenizer applied to a specific document mix, and the ratio between bytes and
//! tokens varies with the content: a payload of short parameter names and a
//! payload of long script excerpts tokenize very differently at the same byte
//! count. Reporting "34% fewer tokens" from a byte measurement would be an
//! invented number, and the whole point of this work is to make output cost
//! measurable rather than estimated.
//!
//! Converting these figures into tokens requires a tokenizer this build cannot
//! currently obtain (see the ledger's blocker note), and even then the
//! conversion would have to be stated with the tokenizer name and version,
//! because a different tokenizer gives a different answer.
//!
//! # What "avoided" means
//!
//! [`CostReport::bytes_avoided`] compares the bytes the caller would have
//! received from the full representation against the bytes they actually
//! received. It is an exact subtraction of two measured values, and it is
//! attributed only to calls where both were counted. It says nothing about
//! tokens, and nothing about cost in any currency.

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

/// A tally of what the output contract actually cost, in serialized bytes.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostReport {
    /// Number of responses recorded.
    pub calls: u64,
    /// Responses that returned a compact document.
    pub compact_calls: u64,
    /// Responses that returned the full value.
    pub full_calls: u64,
    /// Bytes served, counted from the representation actually returned.
    pub bytes_served: u64,
    /// Bytes the full representation would have cost, where that was counted.
    ///
    /// A compact response counts its full counterpart too, because the planner
    /// built and measured both. A full response counts itself. A response whose
    /// size could not be counted contributes nothing here, so this can be
    /// smaller than `calls` accounts for.
    pub bytes_full_equivalent: u64,
    /// Responses refused because they exceeded the caller's byte budget.
    pub budget_refusals: u64,
    /// Why the full value was kept, when a compact request did not get one.
    ///
    /// Sorted by reason so a report is stable between reads. An empty map is
    /// the good case: it means every compact request was honoured.
    pub full_by_reason: BTreeMap<String, u64>,
    /// The unit every byte figure above is measured in.
    ///
    /// Serialized as a constant so a consumer cannot mistake these fields for
    /// tokens, and so a future change of unit cannot pass unnoticed.
    pub unit: String,
}

impl CostReport {
    /// Bytes the compact representation saved, where both sides were counted.
    ///
    /// Saturating: a response can only be "worse" than full if the accounting
    /// is wrong, and reporting a negative saving as zero is the safer way to be
    /// wrong here.
    #[must_use]
    pub fn bytes_avoided(&self) -> u64 {
        self.bytes_full_equivalent.saturating_sub(self.bytes_served)
    }

    /// Avoided bytes as a fraction of the full equivalent, when that is
    /// computable.
    ///
    /// `None` rather than `0.0` when nothing was counted, so "no measurement"
    /// and "measured, no saving" stay distinguishable.
    #[must_use]
    pub fn avoided_fraction(&self) -> Option<f64> {
        if self.bytes_full_equivalent == 0 {
            return None;
        }
        Some(self.bytes_avoided() as f64 / self.bytes_full_equivalent as f64)
    }
}

/// One recorded response.
#[derive(Debug, Clone, Copy)]
pub struct Observation<'a> {
    /// Representation the caller asked for.
    pub requested_compact: bool,
    /// Representation actually returned.
    pub applied_compact: bool,
    /// Counted bytes of the representation returned, if within budget.
    pub served_bytes: Option<usize>,
    /// Counted bytes of the full representation, if it was measured.
    pub full_bytes: Option<usize>,
    /// Why full was kept, when compact was requested and not applied.
    pub full_reason: Option<&'a str>,
}

#[derive(Debug, Default)]
struct LedgerState {
    calls: u64,
    compact_calls: u64,
    full_calls: u64,
    bytes_served: u64,
    bytes_full_equivalent: u64,
    budget_refusals: u64,
    full_by_reason: BTreeMap<String, u64>,
}

/// Process-wide cost tally.
///
/// One shared instance exists because the question "is compact output
/// actually reducing what agents pull over the wire?" is only answerable
/// across calls. A per-call figure already travels in each response's
/// `presentation.bytes` block; this aggregates so the effect can be seen at
/// all.
#[derive(Debug, Default)]
pub struct CostLedger {
    state: Mutex<LedgerState>,
}

impl CostLedger {
    /// Create an empty ledger.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one response.
    ///
    /// Never fails and never blocks on anything but this ledger's own mutex, so
    /// a measurement problem can never affect a tool call. Counted sizes that
    /// could not be determined are recorded as absent rather than as zero,
    /// which would silently improve the avoided-bytes figure.
    pub fn record(&self, observation: Observation<'_>) {
        let Ok(mut state) = self.state.lock() else {
            // A poisoned ledger must not take the daemon with it. Losing a
            // measurement is strictly better than failing a dispatch.
            return;
        };
        state.calls += 1;
        if observation.applied_compact {
            state.compact_calls += 1;
        } else {
            state.full_calls += 1;
        }
        if let Some(bytes) = observation.served_bytes {
            state.bytes_served += u64::try_from(bytes).unwrap_or(u64::MAX);
        }
        if let Some(bytes) = observation.full_bytes {
            state.bytes_full_equivalent += u64::try_from(bytes).unwrap_or(u64::MAX);
        }
        // A reason is only meaningful when compact was asked for and refused.
        if observation.requested_compact && !observation.applied_compact {
            if let Some(reason) = observation.full_reason {
                *state.full_by_reason.entry(reason.to_owned()).or_insert(0) += 1;
            }
        }
    }

    /// Record a response refused for exceeding the caller's byte budget.
    pub fn record_budget_refusal(&self) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        state.calls += 1;
        state.budget_refusals += 1;
    }

    /// A snapshot of the tally.
    #[must_use]
    pub fn report(&self) -> CostReport {
        let Ok(state) = self.state.lock() else {
            return CostReport {
                unit: UNIT.to_owned(),
                ..CostReport::default()
            };
        };
        CostReport {
            calls: state.calls,
            compact_calls: state.compact_calls,
            full_calls: state.full_calls,
            bytes_served: state.bytes_served,
            bytes_full_equivalent: state.bytes_full_equivalent,
            budget_refusals: state.budget_refusals,
            full_by_reason: state.full_by_reason.clone(),
            unit: UNIT.to_owned(),
        }
    }
}

/// The unit string every byte figure is labelled with.
pub const UNIT: &str = "serialized_bytes";

/// The process-wide ledger.
///
/// A `OnceLock` rather than a `static` with a constructor so the type has no
/// interior initialisation order concerns; `CostLedger::new` is still public for
/// a caller that wants an isolated tally.
static LEDGER: OnceLock<CostLedger> = OnceLock::new();

/// The shared process-wide ledger.
#[must_use]
pub fn global_ledger() -> &'static CostLedger {
    LEDGER.get_or_init(CostLedger::new)
}

/// A snapshot of the process-wide tally.
#[must_use]
pub fn global_report() -> CostReport {
    global_ledger().report()
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

    fn compact(bytes: usize, full: usize) -> Observation<'static> {
        Observation {
            requested_compact: true,
            applied_compact: true,
            served_bytes: Some(bytes),
            full_bytes: Some(full),
            full_reason: None,
        }
    }

    #[test]
    fn an_empty_ledger_reports_no_measurement_rather_than_no_saving() {
        let report = CostLedger::new().report();
        assert_eq!(report.calls, 0);
        assert_eq!(report.bytes_avoided(), 0);
        assert_eq!(
            report.avoided_fraction(),
            None,
            "no measurement is not a measured zero"
        );
    }

    #[test]
    fn the_avoided_figure_is_the_exact_difference_between_two_measurements() {
        let ledger = CostLedger::new();
        ledger.record(compact(60, 100));
        let report = ledger.report();
        assert_eq!(report.calls, 1);
        assert_eq!(report.compact_calls, 1);
        assert_eq!(report.bytes_served, 60);
        assert_eq!(report.bytes_full_equivalent, 100);
        assert_eq!(report.bytes_avoided(), 40);
        assert!((report.avoided_fraction().unwrap() - 0.40).abs() < 1e-9);
    }

    #[test]
    fn an_unmeasured_response_contributes_nothing_rather_than_zero() {
        // Defaulting an unknown size to zero would inflate the avoided figure
        // on both sides and quietly improve the number this module exists to
        // keep honest.
        let ledger = CostLedger::new();
        ledger.record(Observation {
            requested_compact: true,
            applied_compact: true,
            served_bytes: None,
            full_bytes: None,
            full_reason: None,
        });
        let report = ledger.report();
        assert_eq!(report.calls, 1);
        assert_eq!(report.bytes_served, 0);
        assert_eq!(report.bytes_full_equivalent, 0);
        assert_eq!(report.avoided_fraction(), None);
    }

    #[test]
    fn a_full_response_counts_itself_as_the_full_equivalent() {
        let ledger = CostLedger::new();
        ledger.record(Observation {
            requested_compact: false,
            applied_compact: false,
            served_bytes: Some(100),
            full_bytes: Some(100),
            full_reason: None,
        });
        let report = ledger.report();
        assert_eq!(report.full_calls, 1);
        assert_eq!(report.bytes_avoided(), 0);
        assert_eq!(report.avoided_fraction(), Some(0.0));
    }

    #[test]
    fn a_refused_compact_request_is_counted_under_its_reason() {
        // "Compact got worse" is a question, and the per-reason breakdown is
        // the only way to answer it. An empty map is the good case.
        let ledger = CostLedger::new();
        ledger.record(Observation {
            requested_compact: true,
            applied_compact: false,
            served_bytes: Some(100),
            full_bytes: Some(100),
            full_reason: Some("not_smaller"),
        });
        ledger.record(Observation {
            requested_compact: true,
            applied_compact: false,
            served_bytes: Some(50),
            full_bytes: Some(50),
            full_reason: Some("no_transforms"),
        });
        let report = ledger.report();
        assert_eq!(report.compact_calls, 0);
        assert_eq!(report.full_calls, 2);
        assert_eq!(report.full_by_reason.get("not_smaller"), Some(&1));
        assert_eq!(report.full_by_reason.get("no_transforms"), Some(&1));
    }

    #[test]
    fn a_reason_on_a_honoured_request_is_not_counted_as_a_refusal() {
        // A payload can be compact AND still report a planning reason; recording
        // that as a refusal would misstate how often compact actually worked.
        let ledger = CostLedger::new();
        ledger.record(Observation {
            requested_compact: true,
            applied_compact: true,
            served_bytes: Some(60),
            full_bytes: Some(100),
            full_reason: Some("not_smaller"),
        });
        assert!(
            ledger.report().full_by_reason.is_empty(),
            "an honoured compact request is not a refusal"
        );
    }

    #[test]
    fn a_budget_refusal_is_visible_as_its_own_count() {
        let ledger = CostLedger::new();
        ledger.record_budget_refusal();
        ledger.record_budget_refusal();
        let report = ledger.report();
        assert_eq!(report.budget_refusals, 2);
        assert_eq!(report.calls, 2);
        assert_eq!(report.bytes_served, 0, "a refused call served nothing");
    }

    #[test]
    fn every_byte_figure_is_labelled_as_serialized_bytes() {
        // A consumer reading the JSON must not be able to mistake these for
        // tokens, which is the one interpretation this data cannot support.
        let ledger = CostLedger::new();
        ledger.record(compact(60, 100));
        let report = ledger.report();
        assert_eq!(report.unit, "serialized_bytes");
        let json = serde_json::to_string(&report).expect("report serializes");
        assert!(json.contains(r#""unit":"serialized_bytes""#), "{json}");
        assert!(!json.contains("token"), "no token claim may appear: {json}");
    }

    #[test]
    fn the_ledger_accumulates_across_calls() {
        // The reason the ledger exists at all: a single response cannot say
        // whether compact output is worth having.
        let ledger = CostLedger::new();
        for _ in 0..3 {
            ledger.record(compact(60, 100));
        }
        let report = ledger.report();
        assert_eq!(report.calls, 3);
        assert_eq!(report.bytes_avoided(), 120);
    }

    #[test]
    fn a_poisoned_lock_costs_a_measurement_rather_than_a_dispatch() {
        use std::sync::Arc;
        let ledger = Arc::new(CostLedger::new());
        let poisoner = Arc::clone(&ledger);
        let _ = std::thread::spawn(move || {
            let _guard = poisoner.state.lock().expect("first lock");
            panic!("poison it");
        })
        .join();
        // A measurement that cannot be taken must never fail a tool call.
        ledger.record(compact(60, 100));
        assert_eq!(ledger.report().calls, 0, "the ledger is empty, not broken");
    }
}

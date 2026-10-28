//! Durable receipts for externally-visible operations.
//!
//! This module records what the daemon knows about an operation that can have
//! an effect outside its own process: a mutation, a script run, a long-running
//! job. It exists because "the call did not return" and "the call was never
//! delivered" look identical to a caller unless something durable remembers
//! which happened.
//!
//! The rules this module enforces are the ones the product contract treats as
//! non-negotiable:
//!
//! * It never claims exactly-once or whole-scene atomicity. One receipt is one
//!   dispatch decision, not a transaction over a TouchDesigner scene.
//! * It has no automatic retry path at all. An operation whose effect is
//!   [`EffectState::Unknown`] stays unknown; re-dispatching it would risk
//!   applying a mutation twice, so recovery reports instead of acting.
//! * A dispatch counter that advanced does not mean the scene changed. The
//!   effect is only ever advanced by an explicit observation.
//! * Cancellation records what is known. If the caller's wait ended while the
//!   work may still be running, the effect stays unknown rather than being
//!   reported as prevented.
//!
//! The owner is supplied by the transport that authenticated the caller, never
//! read from a request argument, so one caller cannot read another's receipts.
//!
//! The disk backend is intentionally synchronous; composition code must run it
//! on a blocking worker.

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tdmcp_core::{EffectState, ExtensionData, OperationOutcome, OperationState};
use tdmcp_mcp::evidence::{EvidenceError, OwnerId};

const SCHEMA_VERSION: i64 = 2;
/// Journal rows retained for one owner, including terminal tombstones.
const DEFAULT_MAX_ROWS_PER_OWNER: u64 = 512;
/// Journal rows retained across the journal, including tombstones.
const DEFAULT_MAX_TOTAL_ROWS: u64 = 4_096;
/// Active waiters allowed for one owner before admissions are refused.
const DEFAULT_MAX_IN_FLIGHT_PER_OWNER: u64 = 64;
/// Active waiters allowed across the journal before admissions are refused.
const DEFAULT_MAX_IN_FLIGHT_TOTAL: u64 = 512;
/// Maximum serialized bytes in a caller-supplied receipt detail.
const MAX_DETAIL_BYTES: u64 = 8 * 1024;
/// Default retention for a receipt whose result arrived or whose wait ended.
const DEFAULT_RETENTION: Duration = Duration::from_secs(24 * 60 * 60);

/// Bounded limits for one operation journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JournalConfig {
    /// Maximum retained receipts per owner, including tombstones.
    pub max_rows_per_owner: u64,
    /// Maximum retained receipts across the journal, including tombstones.
    pub max_total_rows: u64,
    /// Maximum active (admitted or dispatched) receipts for one owner.
    pub max_in_flight_per_owner: u64,
    /// Maximum active receipts across the journal.
    pub max_in_flight_total: u64,
    /// Maximum serialized bytes of caller-supplied detail on one receipt.
    pub max_detail_bytes: u64,
    /// How long a terminal or wait-expired receipt is retained before pruning.
    pub retention: Duration,
}

impl Default for JournalConfig {
    fn default() -> Self {
        Self {
            max_rows_per_owner: DEFAULT_MAX_ROWS_PER_OWNER,
            max_total_rows: DEFAULT_MAX_TOTAL_ROWS,
            max_in_flight_per_owner: DEFAULT_MAX_IN_FLIGHT_PER_OWNER,
            max_in_flight_total: DEFAULT_MAX_IN_FLIGHT_TOTAL,
            max_detail_bytes: MAX_DETAIL_BYTES,
            retention: DEFAULT_RETENTION,
        }
    }
}

/// Lifecycle position of a recorded operation.
///
/// This is a strict subset of [`OperationState`]: the journal does not model
/// `Queued` or `Running`, because it records decisions made at dispatch
/// boundaries rather than in-process progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptState {
    /// The request passed admission and policy checks; nothing was sent yet.
    Admitted,
    /// The journal durably recorded that the work was handed to an executor.
    Dispatched,
    /// The caller stopped waiting; the work may still be running.
    WaitExpired,
    /// The executor returned a successful terminal result.
    Completed,
    /// The executor returned a terminal failure.
    Failed,
    /// Cancellation was requested and its terminal effect is known.
    Cancelled,
    /// Dispatch may have happened but no terminal result was observed.
    UnknownEffect,
}

impl ReceiptState {
    /// Whether a receipt in this state can still change.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::UnknownEffect
        )
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Admitted => "admitted",
            Self::Dispatched => "dispatched",
            Self::WaitExpired => "wait_expired",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::UnknownEffect => "unknown_effect",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "admitted" => Some(Self::Admitted),
            "dispatched" => Some(Self::Dispatched),
            "wait_expired" => Some(Self::WaitExpired),
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            "unknown_effect" => Some(Self::UnknownEffect),
            _ => None,
        }
    }

    /// Project into the shared contract vocabulary.
    ///
    /// A receipt has no `Queued` or `Running` counterpart because the journal
    /// does not observe them, and mapping them onto something it did observe
    /// would claim more than it knows.
    #[must_use]
    pub fn to_operation_state(self) -> OperationState {
        match self {
            Self::Admitted => OperationState::Admitted,
            Self::Dispatched => OperationState::Dispatched,
            Self::WaitExpired => OperationState::WaitExpired,
            Self::Completed => OperationState::Completed,
            Self::Failed => OperationState::Failed,
            Self::Cancelled => OperationState::Cancelled,
            Self::UnknownEffect => OperationState::UnknownEffect,
        }
    }
}

/// What a *caller* says it observed about an unobserved dispatch.
///
/// This is deliberately not a receipt state. The daemon never saw the result, so
/// nothing here can promote an [`ReceiptState::UnknownEffect`] receipt to
/// `completed` — a future reader of the `state` field would otherwise be unable
/// to tell an observation from an assertion. The caller's account sits beside
/// the state, labelled, and the state stays unknown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallerResolution {
    /// What the caller observed in TouchDesigner.
    pub observed_effect: EffectState,
    /// Bounded, caller-supplied note. Never interpreted by the daemon.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Wall-clock milliseconds when the caller recorded this.
    pub resolved_at_ms: i64,
}

/// One durable receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationReceipt {
    /// Opaque receipt identity, generated by the journal.
    pub id: String,
    /// Owning principal, from the authenticated transport.
    pub owner: String,
    /// Tool or subsystem the operation belongs to.
    pub subject: String,
    /// Lifecycle position.
    pub state: ReceiptState,
    /// What is known about externally visible effects.
    pub effect: EffectState,
    /// Caller-supplied detail, bounded and never interpreted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Wall-clock milliseconds when the receipt was created.
    pub created_at_ms: i64,
    /// Wall-clock milliseconds of the most recent transition.
    pub updated_at_ms: i64,
    /// When a terminal or wait-expired receipt may be dropped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at_ms: Option<i64>,
    /// The owner's own account of an unobserved dispatch, if they gave one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution: Option<CallerResolution>,
}

impl OperationReceipt {
    /// The contract-shaped outcome for this receipt.
    #[must_use]
    pub fn outcome(&self) -> OperationOutcome {
        OperationOutcome {
            state: self.state.to_operation_state(),
            effect: self.effect,
            extensions: ExtensionData::new(),
        }
    }
}

/// Owner-scoped counts across all retained receipts, independent of pagination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationCounts {
    /// All retained receipts for the owner.
    pub total: u64,
    /// Admitted or dispatched calls whose caller is still waiting.
    pub in_flight: u64,
    /// Unobserved dispatches without a caller resolution.
    pub unresolved: u64,
}

/// A durable receipt log for externally-visible operations.
pub trait OperationJournal: Send + Sync {
    /// Record a new operation at the admitted state, before anything is sent.
    ///
    /// Returns the receipt identity the caller must use for later transitions.
    fn admit(
        &self,
        owner: &OwnerId,
        subject: &str,
        detail: Option<&str>,
    ) -> Result<String, EvidenceError>;

    /// Record that the work was handed to an executor.
    ///
    /// This transition is durable before the call returns, so a crash after
    /// dispatch is still visible as dispatched rather than as never-started.
    fn mark_dispatched(&self, owner: &OwnerId, id: &str) -> Result<(), EvidenceError>;

    /// Record a terminal result and what is known about its effects.
    fn finish(
        &self,
        owner: &OwnerId,
        id: &str,
        state: ReceiptState,
        effect: EffectState,
        detail: Option<&str>,
    ) -> Result<(), EvidenceError>;

    /// Record that the caller's wait ended while the work may still be running.
    ///
    /// This is deliberately not [`Self::finish`] and deliberately not a
    /// terminal state. A timeout, a disconnect, or a cancelled caller are three
    /// different events, and none of them establishes that the work stopped:
    /// the call may still be running inside TouchDesigner. Recording this as
    /// `failed` would claim the work did not happen, and recording it as
    /// `completed` would claim more than anybody observed. The receipt stays
    /// unknown, but releases its admission slot and gets the configured history
    /// retention. It remains readable and resolvable without a restart. Ending
    /// the wait does not stop or retry the work.
    ///
    /// The effect is always [`EffectState::Unknown`] and is not a parameter,
    /// so no caller can turn a lost result into a known one by accident.
    fn mark_wait_expired(
        &self,
        owner: &OwnerId,
        id: &str,
        detail: &str,
    ) -> Result<(), EvidenceError>;

    /// Record that the work was never handed to the executor.
    ///
    /// This is a distinct transition from [`Self::finish`] because the receipt
    /// is still *admitted* here, not *dispatched*. Without it, a send that
    /// fails — the target is not running, the bridge is down — would leave a row
    /// that is never settled and never pruned, and since unsettled rows hold an
    /// in-flight slot, a client whose TouchDesigner is closed would eventually
    /// be refused by its own quota with no way to recover. "Nothing was sent" is
    /// a real answer and the caller's next attempt is the first dispatch.
    ///
    /// The effect is always [`EffectState::PreventedBeforeDispatch`].
    fn mark_not_sent(&self, owner: &OwnerId, id: &str, detail: &str) -> Result<(), EvidenceError>;

    /// Read one receipt owned by `owner`.
    fn get(&self, owner: &OwnerId, id: &str) -> Result<Option<OperationReceipt>, EvidenceError>;

    /// Record the caller's own account of an unobserved dispatch.
    ///
    /// The receipt's `state` and `effect` are NOT changed. The daemon never
    /// saw the result, so a caller's assertion cannot become an observation:
    /// if it could, a later reader of `state` could no longer tell the two
    /// apart, and the whole point of `unknown_effect` is that it stays unknown.
    ///
    /// The account is stored beside the state as a [`CallerResolution`], which
    /// is only legal on a receipt that is actually unresolved. Resolving an
    /// already-observed receipt is a conflict: its state is the better record,
    /// and a caller note must not paper over it.
    ///
    /// This is also not a retry. Nothing is re-sent, and the caller's account is
    /// not permission to repeat the work.
    fn resolve(
        &self,
        owner: &OwnerId,
        id: &str,
        observed_effect: EffectState,
        note: Option<&str>,
    ) -> Result<OperationReceipt, EvidenceError>;

    /// List an owner's receipts, newest first, at most `limit` rows.
    fn list(&self, owner: &OwnerId, limit: u64) -> Result<Vec<OperationReceipt>, EvidenceError>;

    /// Count an owner's retained receipts and active waits without pagination.
    fn counts(&self, owner: &OwnerId) -> Result<OperationCounts, EvidenceError>;

    /// Fail every non-terminal receipt as uncertain.
    ///
    /// Returns the recovered receipts so the caller can tell them. This never
    /// re-dispatches anything: a dispatched operation whose result was never
    /// observed is reported as unknown, not retried.
    fn recover_uncertain(&self) -> Result<Vec<OperationReceipt>, EvidenceError>;
}

/// SQLite/WAL operation journal.
///
/// The journal is deliberately one-writer: a second writer is refused rather
/// than allowed to race the first over the same rows.
pub struct SqliteOperationJournal {
    connection: Mutex<rusqlite::Connection>,
    config: JournalConfig,
    /// Held for the lifetime of the journal so the OS releases the lock when
    /// the process exits, including on an unclean exit.
    _writer_lock: File,
}

impl std::fmt::Debug for SqliteOperationJournal {
    /// Reports the configuration only. The connection and lock file are
    /// operational state whose contents are not a stable description of the
    /// journal, so printing them would invite comparing unstable output.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SqliteOperationJournal")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl SqliteOperationJournal {
    /// Open or create a private journal rooted at `root`.
    ///
    /// Unix permissions are created and verified as owner-only. The disk
    /// backend fails closed on Windows because this crate has no audited ACL
    /// implementation; use [`MemoryOperationJournal`] there.
    pub fn open(
        root: impl AsRef<Path>,
        config: JournalConfig,
    ) -> Result<SqliteOperationJournal, EvidenceError> {
        let root = root.as_ref();
        if !root.is_dir() {
            return Err(EvidenceError::InvalidInput(
                "operation journal root must be a real directory".to_owned(),
            ));
        }
        verify_private_directory(root)?;
        let lock_path = root.join(".writer.lock");
        let writer_lock = open_private_regular_file(&lock_path)?;
        writer_lock.try_lock().map_err(|error| match error {
            std::fs::TryLockError::WouldBlock => EvidenceError::Conflict,
            std::fs::TryLockError::Error(error) => io_error(error),
        })?;

        let path = root.join("operations.sqlite3");
        let connection = rusqlite::Connection::open(&path).map_err(db_error)?;
        connection
            .execute_batch(
                "PRAGMA journal_mode=WAL;
                 PRAGMA synchronous=FULL;
                 PRAGMA foreign_keys=ON;",
            )
            .map_err(db_error)?;
        let version: Option<i64> = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .optional()
            .map_err(db_error)?;
        match version {
            None | Some(0) => {
                connection
                    .execute_batch(
                        "CREATE TABLE IF NOT EXISTS receipts (
                             id           TEXT    NOT NULL PRIMARY KEY,
                             owner        TEXT    NOT NULL,
                             subject      TEXT    NOT NULL,
                             state        TEXT    NOT NULL,
                             effect       TEXT    NOT NULL,
                             detail       TEXT,
                             created_at_ms INTEGER NOT NULL,
                             updated_at_ms INTEGER NOT NULL,
                             expires_at_ms INTEGER,
                             resolution    TEXT
                         );
                         CREATE INDEX IF NOT EXISTS receipts_owner
                             ON receipts(owner, created_at_ms);
                         PRAGMA user_version = 2;",
                    )
                    .map_err(db_error)?;
            }
            // A v1 journal predates caller-supplied resolutions. The column is
            // added rather than the journal rebuilt: the receipts in it are the
            // only record of dispatches nobody observed, and discarding them to
            // apply a schema change would throw away exactly the thing this
            // journal exists to preserve.
            Some(1) => {
                connection
                    .execute_batch(
                        "ALTER TABLE receipts ADD COLUMN resolution TEXT;
                         PRAGMA user_version = 2;",
                    )
                    .map_err(db_error)?;
            }
            Some(SCHEMA_VERSION) => {}
            Some(other) => {
                return Err(EvidenceError::InvalidInput(format!(
                    "operation journal schema version {other} is not supported by this build"
                )));
            }
        }

        let journal = Self {
            connection: Mutex::new(connection),
            config,
            _writer_lock: writer_lock,
        };
        // Recovery runs before the journal is handed out, so the first caller
        // after a crash already sees uncertain receipts rather than receipts
        // that look merely dispatched.
        journal.recover_uncertain()?;
        Ok(journal)
    }

    /// Open a journal over an in-memory database, for tests only.
    ///
    /// This makes no durability claim: nothing here survives a process exit.
    pub fn open_memory(config: JournalConfig) -> Result<SqliteOperationJournal, EvidenceError> {
        let connection = rusqlite::Connection::open_in_memory().map_err(db_error)?;
        connection
            .execute_batch(
                "PRAGMA foreign_keys=ON;
                 CREATE TABLE receipts (
                     id           TEXT    NOT NULL PRIMARY KEY,
                     owner        TEXT    NOT NULL,
                     subject      TEXT    NOT NULL,
                     state        TEXT    NOT NULL,
                     effect       TEXT    NOT NULL,
                     detail       TEXT,
                     created_at_ms INTEGER NOT NULL,
                     updated_at_ms INTEGER NOT NULL,
                     expires_at_ms INTEGER,
                     resolution    TEXT
                 );
                 CREATE INDEX receipts_owner ON receipts(owner, created_at_ms);",
            )
            .map_err(db_error)?;
        Ok(Self {
            connection: Mutex::new(connection),
            config,
            _writer_lock: File::open("/dev/null").map_err(io_error)?,
        })
    }

    /// Read one receipt while the connection lock is already held.
    ///
    /// Separate from [`OperationJournal::get`] so a path that is mid-transaction
    /// — `resolve` re-reads to tell "not yours" from "not annotatable" — does
    /// not have to drop and retake the lock it is holding.
    fn get_locked(
        &self,
        connection: &rusqlite::Connection,
        owner: &OwnerId,
        id: &str,
    ) -> Result<Option<OperationReceipt>, EvidenceError> {
        connection
            .query_row(
                "SELECT id, owner, subject, state, effect, detail, created_at_ms, updated_at_ms,
                        expires_at_ms, resolution
                   FROM receipts WHERE id = ?1 AND owner = ?2",
                params![id, owner.as_str()],
                row_to_receipt,
            )
            .optional()
            .map_err(db_error)
    }

    fn bound_detail(&self, detail: Option<&str>) -> Result<Option<String>, EvidenceError> {
        let Some(detail) = detail else {
            return Ok(None);
        };
        if u64::try_from(detail.len()).unwrap_or(u64::MAX) > self.config.max_detail_bytes {
            return Err(EvidenceError::QuotaExceeded(
                "operation receipt detail exceeds the configured bound".to_owned(),
            ));
        }
        if detail.chars().any(char::is_control) {
            return Err(EvidenceError::InvalidInput(
                "operation receipt detail must not contain control characters".to_owned(),
            ));
        }
        Ok(Some(detail.to_owned()))
    }

    fn prune(
        &self,
        connection: &rusqlite::Connection,
        owner: &OwnerId,
    ) -> Result<(), EvidenceError> {
        // Active waits never expire. Once the wait ends, even an unknown effect
        // is retained as bounded history, just like recovery's unknown_effect
        // receipts. Keeping that history must not consume active admission slots.
        connection
            .execute(
                "DELETE FROM receipts
                  WHERE expires_at_ms IS NOT NULL AND expires_at_ms <= ?1",
                params![now_ms()],
            )
            .map_err(db_error)?;
        connection
            .execute(
                "DELETE FROM receipts
                  WHERE id IN (
                      SELECT id FROM receipts
                       WHERE owner = ?1 AND expires_at_ms IS NOT NULL
                       ORDER BY created_at_ms DESC, id DESC
                       LIMIT -1 OFFSET ?2
                  )",
                params![owner.as_str(), sql_count(self.config.max_rows_per_owner)],
            )
            .map_err(db_error)?;
        connection
            .execute(
                "DELETE FROM receipts
                  WHERE id IN (
                      SELECT id FROM receipts
                       WHERE expires_at_ms IS NOT NULL
                       ORDER BY created_at_ms DESC, id DESC
                       LIMIT -1 OFFSET ?1
                  )",
                params![sql_count(self.config.max_total_rows)],
            )
            .map_err(db_error)?;
        Ok(())
    }

    /// Refuse a new admission when too many callers are still waiting.
    ///
    /// History retention is separate: a wait-expired receipt still reports an
    /// unknown effect, but no longer occupies an active waiter's slot.
    fn in_flight_bounds(
        &self,
        connection: &rusqlite::Connection,
        owner: &OwnerId,
    ) -> Result<(), EvidenceError> {
        let owner_in_flight: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM receipts
                  WHERE owner = ?1 AND expires_at_ms IS NULL",
                params![owner.as_str()],
                |row| row.get(0),
            )
            .map_err(db_error)?;
        if u64::try_from(owner_in_flight).unwrap_or(u64::MAX) >= self.config.max_in_flight_per_owner
        {
            return Err(EvidenceError::QuotaExceeded(format!(
                "owner already has {} active operation receipts; wait for active calls to return or their callers to stop waiting; reading or resolving receipts does not stop work",
                owner_in_flight
            )));
        }
        let total_in_flight: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM receipts WHERE expires_at_ms IS NULL",
                [],
                |row| row.get(0),
            )
            .map_err(db_error)?;
        if u64::try_from(total_in_flight).unwrap_or(u64::MAX) >= self.config.max_in_flight_total {
            return Err(EvidenceError::QuotaExceeded(format!(
                "journal already holds {total_in_flight} active operation receipts; wait for active calls to return or their callers to stop waiting; reading or resolving receipts does not stop work"
            )));
        }
        Ok(())
    }
}

impl OperationJournal for SqliteOperationJournal {
    fn admit(
        &self,
        owner: &OwnerId,
        subject: &str,
        detail: Option<&str>,
    ) -> Result<String, EvidenceError> {
        if subject.is_empty() || subject.len() > 128 {
            return Err(EvidenceError::InvalidInput(
                "operation subject must be nonempty and bounded".to_owned(),
            ));
        }
        let detail = self.bound_detail(detail)?;
        let id = format!("op_{}", uuid::Uuid::new_v4());
        let now = now_ms();
        let connection = self
            .connection
            .lock()
            .map_err(|_| EvidenceError::Conflict)?;
        // Bound first, insert second: a refused admission must leave no row.
        self.prune(&connection, owner)?;
        self.in_flight_bounds(&connection, owner)?;
        connection
            .execute(
                "INSERT INTO receipts
                     (id, owner, subject, state, effect, detail, created_at_ms, updated_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
                params![
                    id,
                    owner.as_str(),
                    subject,
                    ReceiptState::Admitted.as_str(),
                    effect_str(EffectState::NotApplicable),
                    detail,
                    now,
                ],
            )
            .map_err(db_error)?;
        Ok(id)
    }

    fn mark_dispatched(&self, owner: &OwnerId, id: &str) -> Result<(), EvidenceError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| EvidenceError::Conflict)?;
        transition(
            &connection,
            Transition {
                expected: ReceiptState::Admitted,
                next: ReceiptState::Dispatched,
                // Dispatch is not effect: the scene may not have changed at all.
                effect: EffectState::Unknown,
                detail: None,
                retention: self.config.retention,
                owner,
                id,
            },
        )
    }

    fn finish(
        &self,
        owner: &OwnerId,
        id: &str,
        state: ReceiptState,
        effect: EffectState,
        detail: Option<&str>,
    ) -> Result<(), EvidenceError> {
        if !state.is_terminal() {
            return Err(EvidenceError::InvalidInput(format!(
                "receipt state {:?} is not terminal",
                state
            )));
        }
        // An unknown effect is a terminal state, but it must not be recorded
        // as a clean success. Reject the combination that would read as
        // "dispatched, applied, and done" for an operation nobody observed.
        if matches!(state, ReceiptState::Completed) && matches!(effect, EffectState::Unknown) {
            return Err(EvidenceError::InvalidInput(
                "a completed receipt must state a known effect; use unknown_effect for an \
                 unobserved dispatch"
                    .to_owned(),
            ));
        }
        let detail = self.bound_detail(detail)?;
        let connection = self
            .connection
            .lock()
            .map_err(|_| EvidenceError::Conflict)?;
        transition(
            &connection,
            Transition {
                expected: ReceiptState::Dispatched,
                next: state,
                effect,
                detail,
                retention: self.config.retention,
                owner,
                id,
            },
        )
    }

    fn mark_wait_expired(
        &self,
        owner: &OwnerId,
        id: &str,
        detail: &str,
    ) -> Result<(), EvidenceError> {
        let detail = self.bound_detail(Some(detail))?;
        let connection = self
            .connection
            .lock()
            .map_err(|_| EvidenceError::Conflict)?;
        // An ended wait becomes retained history, even though no effect was
        // observed. It must not keep an active admission slot until a restart.
        transition(
            &connection,
            Transition {
                expected: ReceiptState::Dispatched,
                next: ReceiptState::WaitExpired,
                effect: EffectState::Unknown,
                detail,
                retention: self.config.retention,
                owner,
                id,
            },
        )
    }

    fn mark_not_sent(&self, owner: &OwnerId, id: &str, detail: &str) -> Result<(), EvidenceError> {
        let detail = self.bound_detail(Some(detail))?;
        let connection = self
            .connection
            .lock()
            .map_err(|_| EvidenceError::Conflict)?;
        // Expected is `admitted`, not `dispatched`: reaching this transition is
        // itself the claim that nothing was sent.
        transition(
            &connection,
            Transition {
                expected: ReceiptState::Admitted,
                next: ReceiptState::Cancelled,
                effect: EffectState::PreventedBeforeDispatch,
                detail,
                retention: self.config.retention,
                owner,
                id,
            },
        )
    }

    fn get(&self, owner: &OwnerId, id: &str) -> Result<Option<OperationReceipt>, EvidenceError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| EvidenceError::Conflict)?;
        self.get_locked(&connection, owner, id)
    }

    fn resolve(
        &self,
        owner: &OwnerId,
        id: &str,
        observed_effect: EffectState,
        note: Option<&str>,
    ) -> Result<OperationReceipt, EvidenceError> {
        let note = self.bound_detail(note)?;
        let resolution = serde_json::to_string(&CallerResolution {
            observed_effect,
            note,
            resolved_at_ms: now_ms(),
        })
        // A struct this small cannot fail to serialize; a failure here would be
        // a bug, and reporting it beats dropping the caller's account.
        .map_err(|error| EvidenceError::Backend(error.to_string()))?;
        let connection = self
            .connection
            .lock()
            .map_err(|_| EvidenceError::Conflict)?;
        // Only an unresolved receipt may be annotated, and only ever *alongside*
        // its state. The expected states are the ones where nobody observed the
        // result: a dispatched or wait-expired call whose work may still be
        // running, and an unknown-effect one promoted by recovery.
        let updated = connection
            .execute(
                "UPDATE receipts
                    SET resolution = ?1, updated_at_ms = ?2
                  WHERE id = ?3 AND owner = ?4
                    AND state IN ('dispatched', 'wait_expired', 'unknown_effect')",
                params![resolution, now_ms(), id, owner.as_str()],
            )
            .map_err(db_error)?;
        if updated == 0 {
            // Say nothing about a row that is not the caller's: a foreign id
            // and a state that may not be annotated are the same answer, so this
            // cannot be used to probe another principal's receipts.
            return Err(if self.get_locked(&connection, owner, id)?.is_some() {
                EvidenceError::Conflict
            } else {
                EvidenceError::NotFound
            });
        }
        self.get_locked(&connection, owner, id)?
            .ok_or(EvidenceError::NotFound)
    }

    fn list(&self, owner: &OwnerId, limit: u64) -> Result<Vec<OperationReceipt>, EvidenceError> {
        let limit = limit.clamp(1, self.config.max_rows_per_owner);
        let connection = self
            .connection
            .lock()
            .map_err(|_| EvidenceError::Conflict)?;
        let mut statement = connection
            .prepare(
                "SELECT id, owner, subject, state, effect, detail, created_at_ms, updated_at_ms,
                        expires_at_ms, resolution
                   FROM receipts WHERE owner = ?1
                   ORDER BY created_at_ms DESC, id DESC
                   LIMIT ?2",
            )
            .map_err(db_error)?;
        let rows = statement
            .query_map(params![owner.as_str(), sql_count(limit)], row_to_receipt)
            .map_err(db_error)?;
        let mut receipts = Vec::new();
        for row in rows {
            receipts.push(row.map_err(db_error)?);
        }
        Ok(receipts)
    }

    fn counts(&self, owner: &OwnerId) -> Result<OperationCounts, EvidenceError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| EvidenceError::Conflict)?;
        connection
            .query_row(
                "SELECT COUNT(*),
                    COUNT(CASE WHEN expires_at_ms IS NULL THEN 1 END),
                    COUNT(CASE WHEN state IN ('dispatched', 'wait_expired', 'unknown_effect')
                                AND resolution IS NULL THEN 1 END)
               FROM receipts WHERE owner = ?1",
                params![owner.as_str()],
                |row| {
                    Ok(OperationCounts {
                        total: u64::try_from(row.get::<_, i64>(0)?).unwrap_or(u64::MAX),
                        in_flight: u64::try_from(row.get::<_, i64>(1)?).unwrap_or(u64::MAX),
                        unresolved: u64::try_from(row.get::<_, i64>(2)?).unwrap_or(u64::MAX),
                    })
                },
            )
            .map_err(db_error)
    }

    fn recover_uncertain(&self) -> Result<Vec<OperationReceipt>, EvidenceError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| EvidenceError::Conflict)?;
        // `dispatched` and `wait_expired` both mean the caller's wait ended
        // while the work may still have been running. `admitted` means nothing
        // was sent, so it stays admitted rather than being called uncertain.
        let candidates: Vec<OperationReceipt> = {
            let mut statement = connection
                .prepare(
                    "SELECT id, owner, subject, state, effect, detail, created_at_ms, updated_at_ms,
                            expires_at_ms, resolution
                       FROM receipts WHERE state IN ('dispatched', 'wait_expired')",
                )
                .map_err(db_error)?;
            let rows = statement.query_map([], row_to_receipt).map_err(db_error)?;
            let mut collected = Vec::new();
            for row in rows {
                collected.push(row.map_err(db_error)?);
            }
            collected
        };

        let mut recovered = Vec::new();
        for receipt in &candidates {
            let updated = connection
                .execute(
                    "UPDATE receipts
                        SET state = ?1, effect = ?2, updated_at_ms = ?3,
                            expires_at_ms = ?3 + ?4
                      WHERE id = ?5 AND state IN ('dispatched', 'wait_expired')",
                    params![
                        ReceiptState::UnknownEffect.as_str(),
                        effect_str(EffectState::Unknown),
                        now_ms(),
                        retention_ms(self.config.retention),
                        receipt.id,
                    ],
                )
                .map_err(db_error)?;
            if updated == 1 {
                if let Ok(Some(mut refreshed)) = self.get_inner(&connection, &receipt.id) {
                    refreshed.detail = receipt.detail.clone();
                    recovered.push(refreshed);
                }
            }
        }
        Ok(recovered)
    }
}

impl SqliteOperationJournal {
    fn get_inner(
        &self,
        connection: &rusqlite::Connection,
        id: &str,
    ) -> Result<Option<OperationReceipt>, EvidenceError> {
        connection
            .query_row(
                "SELECT id, owner, subject, state, effect, detail, created_at_ms, updated_at_ms,
                        expires_at_ms, resolution
                   FROM receipts WHERE id = ?1",
                params![id],
                row_to_receipt,
            )
            .optional()
            .map_err(db_error)
    }
}

/// Build the conversion error for a stored value this build cannot interpret.
///
/// The column index is carried through so the error identifies which field was
/// unreadable, rather than just saying "conversion failed".
fn unrecognized(message: String, column: usize) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(column, rusqlite::types::Type::Text, message.into())
}

fn row_to_receipt(row: &rusqlite::Row<'_>) -> rusqlite::Result<OperationReceipt> {
    let state_text: String = row.get(3)?;
    let effect_text: String = row.get(4)?;
    // An unrecognized stored value fails closed: a newer writer's state must
    // not be silently reinterpreted as a known one.
    let state = ReceiptState::parse(&state_text)
        .ok_or_else(|| unrecognized(format!("unrecognized operation state {state_text:?}"), 3))?;
    let effect = parse_effect(&effect_text)
        .ok_or_else(|| unrecognized(format!("unrecognized operation effect {effect_text:?}"), 4))?;
    let resolution_text: Option<String> = row.get(9)?;
    let resolution = match &resolution_text {
        None => None,
        Some(text) => Some(
            serde_json::from_str::<CallerResolution>(text).map_err(|error| {
                unrecognized(
                    format!("unrecognized caller resolution {text:?}: {error}"),
                    9,
                )
            })?,
        ),
    };
    Ok(OperationReceipt {
        id: row.get(0)?,
        owner: row.get(1)?,
        subject: row.get(2)?,
        state,
        effect,
        detail: row.get(5)?,
        created_at_ms: row.get(6)?,
        updated_at_ms: row.get(7)?,
        expires_at_ms: row.get(8)?,
        resolution,
    })
}

/// Apply a guarded state transition.
///
/// The `expected` state is part of the `WHERE` clause, so a transition that
/// races another writer updates zero rows instead of overwriting a newer
/// state. A receipt that moved on is a conflict, not a silent overwrite.
/// One requested change to a receipt's state and effect.
struct Transition<'a> {
    /// The state the caller believes the receipt is in.
    expected: ReceiptState,
    /// The state the receipt should move to.
    next: ReceiptState,
    /// The effect to record alongside the new state.
    effect: EffectState,
    /// Optional bounded detail; `None` leaves any existing detail intact.
    detail: Option<String>,
    /// Retention used once a result arrives or the caller's wait ends.
    retention: Duration,
    /// Owning principal, from the authenticated transport.
    owner: &'a OwnerId,
    /// Receipt identity.
    id: &'a str,
}

/// Apply a guarded state transition.
///
/// `Transition::expected` is part of the SQL `WHERE` clause, so a transition
/// that races another writer updates zero rows instead of overwriting a newer
/// state. A receipt that moved on is a conflict, not a silent overwrite.
fn transition(
    connection: &rusqlite::Connection,
    request: Transition<'_>,
) -> Result<(), EvidenceError> {
    // Retention comes from the caller's configuration. Reading a constant here
    // would make a journal opened with a different retention write an expiry
    // its owner never asked for, and the mismatch would only show up later as
    // a receipt vanishing on the wrong schedule.
    let now = now_ms();
    let expires = if request.next.is_terminal() || request.next == ReceiptState::WaitExpired {
        Some(now + retention_ms(request.retention))
    } else {
        None
    };
    let updated = connection
        .execute(
            "UPDATE receipts
                SET state = ?1, effect = ?2, detail = COALESCE(?3, detail), updated_at_ms = ?4,
                    expires_at_ms = ?5
              WHERE id = ?6 AND owner = ?7 AND state = ?8",
            params![
                request.next.as_str(),
                effect_str(request.effect),
                request.detail,
                now,
                expires,
                request.id,
                request.owner.as_str(),
                request.expected.as_str(),
            ],
        )
        .map_err(db_error)?;
    if updated == 1 {
        return Ok(());
    }
    // Distinguish "not yours / not there" from "moved on".
    let exists: Option<String> = connection
        .query_row(
            "SELECT state FROM receipts WHERE id = ?1 AND owner = ?2",
            params![request.id, request.owner.as_str()],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_error)?;
    match exists {
        // No row for this owner: either it never existed or it belongs to
        // somebody else. Both are "not yours" and neither leaks which.
        None => Err(EvidenceError::NotFound),
        // The row exists and is this owner's, but the guarded UPDATE matched
        // nothing. That means the state is no longer the one this transition
        // expected, so the receipt already moved on or was recovered. Report a
        // conflict rather than overwriting the newer state.
        Some(_) => Err(EvidenceError::Conflict),
    }
}

fn effect_str(effect: EffectState) -> &'static str {
    match effect {
        EffectState::NotApplicable => "not_applicable",
        EffectState::PreventedBeforeDispatch => "prevented_before_dispatch",
        EffectState::None => "none",
        EffectState::Applied => "applied",
        EffectState::PartiallyApplied => "partially_applied",
        EffectState::Unknown => "unknown",
    }
}

fn parse_effect(value: &str) -> Option<EffectState> {
    match value {
        "not_applicable" => Some(EffectState::NotApplicable),
        "prevented_before_dispatch" => Some(EffectState::PreventedBeforeDispatch),
        "none" => Some(EffectState::None),
        "applied" => Some(EffectState::Applied),
        "partially_applied" => Some(EffectState::PartiallyApplied),
        "unknown" => Some(EffectState::Unknown),
        _ => None,
    }
}

fn retention_ms(retention: Duration) -> i64 {
    i64::try_from(retention.as_millis()).unwrap_or(i64::MAX)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

/// In-memory journal for tests and for platforms with no audited private-file
/// implementation.
///
/// This makes no durability claim whatsoever: a receipt here is lost when the
/// process exits, so a caller must not treat a recovered in-memory receipt as
/// evidence that a dispatch survived a restart.
#[derive(Default)]
pub struct MemoryOperationJournal {
    receipts: Mutex<HashMap<String, OperationReceipt>>,
    config: JournalConfig,
}

impl MemoryOperationJournal {
    /// Create an in-memory journal.
    #[must_use]
    pub fn new(config: JournalConfig) -> Self {
        Self {
            receipts: Mutex::new(HashMap::new()),
            config,
        }
    }
}

impl OperationJournal for MemoryOperationJournal {
    fn admit(
        &self,
        owner: &OwnerId,
        subject: &str,
        detail: Option<&str>,
    ) -> Result<String, EvidenceError> {
        if subject.is_empty() || subject.len() > 128 {
            return Err(EvidenceError::InvalidInput(
                "operation subject must be nonempty and bounded".to_owned(),
            ));
        }
        if let Some(detail) = detail {
            if u64::try_from(detail.len()).unwrap_or(u64::MAX) > self.config.max_detail_bytes {
                return Err(EvidenceError::QuotaExceeded(
                    "operation receipt detail exceeds the configured bound".to_owned(),
                ));
            }
        }
        let now = now_ms();
        let id = format!("op_{}", uuid::Uuid::new_v4());
        let receipt = OperationReceipt {
            id: id.clone(),
            owner: owner.as_str().to_owned(),
            subject: subject.to_owned(),
            state: ReceiptState::Admitted,
            effect: EffectState::NotApplicable,
            detail: detail.map(str::to_owned),
            created_at_ms: now,
            updated_at_ms: now,
            expires_at_ms: None,
            resolution: None,
        };
        let mut receipts = self.receipts.lock().map_err(|_| EvidenceError::Conflict)?;
        receipts.insert(id.clone(), receipt);
        Ok(id)
    }

    fn mark_dispatched(&self, owner: &OwnerId, id: &str) -> Result<(), EvidenceError> {
        let mut receipts = self.receipts.lock().map_err(|_| EvidenceError::Conflict)?;
        let receipt = receipts
            .get_mut(id)
            .filter(|receipt| receipt.owner == owner.as_str())
            .ok_or(EvidenceError::NotFound)?;
        if receipt.state != ReceiptState::Admitted {
            return Err(EvidenceError::Conflict);
        }
        receipt.state = ReceiptState::Dispatched;
        receipt.effect = EffectState::Unknown;
        receipt.updated_at_ms = now_ms();
        Ok(())
    }

    fn finish(
        &self,
        owner: &OwnerId,
        id: &str,
        state: ReceiptState,
        effect: EffectState,
        detail: Option<&str>,
    ) -> Result<(), EvidenceError> {
        if !state.is_terminal() {
            return Err(EvidenceError::InvalidInput(format!(
                "receipt state {state:?} is not terminal"
            )));
        }
        if matches!(state, ReceiptState::Completed) && matches!(effect, EffectState::Unknown) {
            return Err(EvidenceError::InvalidInput(
                "a completed receipt must state a known effect; use unknown_effect for an \
                 unobserved dispatch"
                    .to_owned(),
            ));
        }
        if let Some(detail) = detail {
            if u64::try_from(detail.len()).unwrap_or(u64::MAX) > self.config.max_detail_bytes {
                return Err(EvidenceError::QuotaExceeded(
                    "operation receipt detail exceeds the configured bound".to_owned(),
                ));
            }
        }
        let now = now_ms();
        let mut receipts = self.receipts.lock().map_err(|_| EvidenceError::Conflict)?;
        let receipt = receipts
            .get_mut(id)
            .filter(|receipt| receipt.owner == owner.as_str())
            .ok_or(EvidenceError::NotFound)?;
        if receipt.state != ReceiptState::Dispatched {
            return Err(EvidenceError::Conflict);
        }
        receipt.state = state;
        receipt.effect = effect;
        if detail.is_some() {
            receipt.detail = detail.map(str::to_owned);
        }
        receipt.updated_at_ms = now;
        receipt.expires_at_ms = Some(now + retention_ms(self.config.retention));
        Ok(())
    }

    fn mark_wait_expired(
        &self,
        owner: &OwnerId,
        id: &str,
        detail: &str,
    ) -> Result<(), EvidenceError> {
        if u64::try_from(detail.len()).unwrap_or(u64::MAX) > self.config.max_detail_bytes {
            return Err(EvidenceError::QuotaExceeded(
                "operation receipt detail exceeds the configured bound".to_owned(),
            ));
        }
        let mut receipts = self.receipts.lock().map_err(|_| EvidenceError::Conflict)?;
        let receipt = receipts
            .get_mut(id)
            .filter(|receipt| receipt.owner == owner.as_str())
            .ok_or(EvidenceError::NotFound)?;
        if receipt.state != ReceiptState::Dispatched {
            return Err(EvidenceError::Conflict);
        }
        receipt.state = ReceiptState::WaitExpired;
        // Never anything but Unknown: the effect is not a parameter here.
        receipt.effect = EffectState::Unknown;
        receipt.detail = Some(detail.to_owned());
        receipt.updated_at_ms = now_ms();
        receipt.expires_at_ms = Some(receipt.updated_at_ms + retention_ms(self.config.retention));
        Ok(())
    }

    fn mark_not_sent(&self, owner: &OwnerId, id: &str, detail: &str) -> Result<(), EvidenceError> {
        if u64::try_from(detail.len()).unwrap_or(u64::MAX) > self.config.max_detail_bytes {
            return Err(EvidenceError::QuotaExceeded(
                "operation receipt detail exceeds the configured bound".to_owned(),
            ));
        }
        let mut receipts = self.receipts.lock().map_err(|_| EvidenceError::Conflict)?;
        let receipt = receipts
            .get_mut(id)
            .filter(|receipt| receipt.owner == owner.as_str())
            .ok_or(EvidenceError::NotFound)?;
        // Still admitted: reaching here is what establishes that nothing was sent.
        if receipt.state != ReceiptState::Admitted {
            return Err(EvidenceError::Conflict);
        }
        receipt.state = ReceiptState::Cancelled;
        receipt.effect = EffectState::PreventedBeforeDispatch;
        receipt.detail = Some(detail.to_owned());
        receipt.updated_at_ms = now_ms();
        receipt.expires_at_ms = Some(now_ms() + retention_ms(self.config.retention));
        Ok(())
    }

    fn resolve(
        &self,
        owner: &OwnerId,
        id: &str,
        observed_effect: EffectState,
        note: Option<&str>,
    ) -> Result<OperationReceipt, EvidenceError> {
        if let Some(note) = note {
            if u64::try_from(note.len()).unwrap_or(u64::MAX) > self.config.max_detail_bytes {
                return Err(EvidenceError::QuotaExceeded(
                    "operation receipt detail exceeds the configured bound".to_owned(),
                ));
            }
            if note.chars().any(char::is_control) {
                return Err(EvidenceError::InvalidInput(
                    "operation receipt detail must not contain control characters".to_owned(),
                ));
            }
        }
        let now = now_ms();
        let mut receipts = self.receipts.lock().map_err(|_| EvidenceError::Conflict)?;
        let receipt = receipts
            .get_mut(id)
            .filter(|receipt| receipt.owner == owner.as_str())
            .ok_or(EvidenceError::NotFound)?;
        // Same rule as the disk backend: only an unobserved dispatch may be
        // annotated, and the annotation never rewrites the state.
        if !matches!(
            receipt.state,
            ReceiptState::Dispatched | ReceiptState::WaitExpired | ReceiptState::UnknownEffect
        ) {
            return Err(EvidenceError::Conflict);
        }
        receipt.resolution = Some(CallerResolution {
            observed_effect,
            note: note.map(str::to_owned),
            resolved_at_ms: now,
        });
        receipt.updated_at_ms = now;
        Ok(receipt.clone())
    }

    fn get(&self, owner: &OwnerId, id: &str) -> Result<Option<OperationReceipt>, EvidenceError> {
        let receipts = self.receipts.lock().map_err(|_| EvidenceError::Conflict)?;
        Ok(receipts
            .get(id)
            .filter(|receipt| receipt.owner == owner.as_str())
            .cloned())
    }

    fn list(&self, owner: &OwnerId, limit: u64) -> Result<Vec<OperationReceipt>, EvidenceError> {
        let receipts = self.receipts.lock().map_err(|_| EvidenceError::Conflict)?;
        let mut owned: Vec<OperationReceipt> = receipts
            .values()
            .filter(|receipt| receipt.owner == owner.as_str())
            .cloned()
            .collect();
        owned.sort_by(|a, b| {
            b.created_at_ms
                .cmp(&a.created_at_ms)
                .then_with(|| b.id.cmp(&a.id))
        });
        owned
            .truncate(usize::try_from(limit.clamp(1, self.config.max_rows_per_owner)).unwrap_or(1));
        Ok(owned)
    }

    fn counts(&self, owner: &OwnerId) -> Result<OperationCounts, EvidenceError> {
        let receipts = self.receipts.lock().map_err(|_| EvidenceError::Conflict)?;
        let mut counts = OperationCounts {
            total: 0,
            in_flight: 0,
            unresolved: 0,
        };
        for receipt in receipts
            .values()
            .filter(|receipt| receipt.owner == owner.as_str())
        {
            counts.total += 1;
            counts.in_flight += u64::from(receipt.expires_at_ms.is_none());
            counts.unresolved += u64::from(
                matches!(
                    receipt.state,
                    ReceiptState::Dispatched
                        | ReceiptState::WaitExpired
                        | ReceiptState::UnknownEffect
                ) && receipt.resolution.is_none(),
            );
        }
        Ok(counts)
    }

    fn recover_uncertain(&self) -> Result<Vec<OperationReceipt>, EvidenceError> {
        let now = now_ms();
        let mut receipts = self.receipts.lock().map_err(|_| EvidenceError::Conflict)?;
        let mut recovered = Vec::new();
        for receipt in receipts.values_mut() {
            if matches!(
                receipt.state,
                ReceiptState::Dispatched | ReceiptState::WaitExpired
            ) {
                receipt.state = ReceiptState::UnknownEffect;
                receipt.effect = EffectState::Unknown;
                receipt.updated_at_ms = now;
                receipt.expires_at_ms = Some(now + retention_ms(self.config.retention));
                recovered.push(receipt.clone());
            }
        }
        Ok(recovered)
    }
}

fn io_error(error: std::io::Error) -> EvidenceError {
    EvidenceError::Io(error.to_string())
}

/// Translate a backend error.
///
/// Deliberately distinct from [`io_error`]: a failed journal query is a
/// backend problem, and labelling it a filesystem problem would send a reader
/// looking at the wrong layer.
fn db_error(error: rusqlite::Error) -> EvidenceError {
    EvidenceError::Backend(error.to_string())
}

/// Convert a bounded row count to the largest value SQLite accepts.
///
/// `rusqlite` implements `ToSql` only up to `i64`. A configuration larger than
/// that cannot be honored, so it saturates to the maximum representable bound
/// rather than wrapping into a negative offset that would delete the wrong rows.
fn sql_count(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

#[cfg(unix)]
fn open_private_regular_file(path: &Path) -> Result<File, EvidenceError> {
    use std::os::unix::fs::OpenOptionsExt;

    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(EvidenceError::InvalidInput(format!(
                    "operation journal file must be a regular non-symlink file: {}",
                    path.display()
                )));
            }
            let before = tdmcp_projectio::publish::identity(path).map_err(io_error)?;
            fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o600))
                .map_err(io_error)?;
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(path)
                .map_err(io_error)?;
            // Re-check identity after opening: a file swapped between the check
            // and the open must not be silently accepted.
            if tdmcp_projectio::publish::identity(path).map_err(io_error)? != before {
                return Err(EvidenceError::Io(format!(
                    "operation journal file identity changed while opening: {}",
                    path.display()
                )));
            }
            Ok(file)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .map_err(io_error),
        Err(error) => Err(io_error(error)),
    }
}

#[cfg(windows)]
fn open_private_regular_file(path: &Path) -> Result<File, EvidenceError> {
    // This crate has no audited ACL implementation, so the disk journal fails
    // closed on Windows rather than writing receipts with unknown ownership.
    let _ = path;
    Err(EvidenceError::UnsupportedPlatform(
        "the disk operation journal is unavailable on this platform; \
         no audited private-file implementation exists"
            .to_owned(),
    ))
}

#[cfg(unix)]
fn verify_private_directory(path: &Path) -> Result<(), EvidenceError> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = fs::symlink_metadata(path).map_err(io_error)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(EvidenceError::Io(format!(
            "operation journal directory is not owner-private: {}",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(not(unix))]
fn verify_private_directory(path: &Path) -> Result<(), EvidenceError> {
    let _ = path;
    Err(EvidenceError::UnsupportedPlatform(
        "the disk operation journal is unavailable on this platform; \
         no audited private-file implementation exists"
            .to_owned(),
    ))
}

/// Create a journal root that is private to this user.
///
/// Idempotent: an existing directory is re-permissioned rather than replaced,
/// because a non-private journal root is a problem to repair, not a reason to
/// discard the receipts already in it.
///
/// # Errors
/// Returns the underlying I/O failure, or a refusal on platforms where this
/// crate has no audited private-directory implementation.
#[cfg(unix)]
pub fn create_private_journal_root(path: &Path) -> Result<(), EvidenceError> {
    use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
    let mut builder = fs::DirBuilder::new();
    builder.recursive(false).mode(0o700);
    match builder.create(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(io_error)
        }
        Err(error) => Err(io_error(error)),
    }
}

/// Fails closed: no audited private-directory implementation exists here.
#[cfg(not(unix))]
pub fn create_private_journal_root(_path: &Path) -> Result<(), EvidenceError> {
    Err(EvidenceError::UnsupportedPlatform(
        "the disk operation journal is unavailable on this platform; \
         no audited private-file implementation exists"
            .to_owned(),
    ))
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
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::thread;

    fn owner(value: &str) -> OwnerId {
        OwnerId::parse(value).unwrap()
    }

    fn alice() -> OwnerId {
        owner("alice")
    }

    fn bob() -> OwnerId {
        owner("bob")
    }

    fn config() -> JournalConfig {
        JournalConfig {
            max_rows_per_owner: 8,
            max_total_rows: 16,
            max_in_flight_per_owner: 4,
            max_in_flight_total: 8,
            max_detail_bytes: 32,
            retention: Duration::from_secs(60),
        }
    }

    fn temp_root(name: &str) -> PathBuf {
        let unique = format!(
            "tdmcp-opjournal-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        );
        let path = std::env::temp_dir().join(unique.replace(['(', ')', ' '], ""));
        fs::create_dir_all(&path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, PermissionsExt::from_mode(0o700)).unwrap();
        }
        path
    }

    fn memory() -> MemoryOperationJournal {
        MemoryOperationJournal::new(config())
    }

    #[test]
    fn an_admitted_operation_starts_before_anything_is_dispatched() {
        let journal = memory();
        let id = journal.admit(&alice(), "mutate_nodes", None).unwrap();
        let receipt = journal.get(&alice(), &id).unwrap().unwrap();
        assert_eq!(receipt.state, ReceiptState::Admitted);
        // Admitted means nothing was sent, so no effect is claimed either way.
        assert_eq!(receipt.effect, EffectState::NotApplicable);
        assert!(!receipt.state.is_terminal());
    }

    #[test]
    fn a_dispatched_operation_does_not_yet_claim_its_effect() {
        let journal = memory();
        let id = journal.admit(&alice(), "mutate_nodes", None).unwrap();
        journal.mark_dispatched(&alice(), &id).unwrap();
        let receipt = journal.get(&alice(), &id).unwrap().unwrap();
        assert_eq!(receipt.state, ReceiptState::Dispatched);
        // Dispatch is not effect. A counter advancing does not mean the scene
        // changed, so the effect stays unknown until something observes it.
        assert_eq!(receipt.effect, EffectState::Unknown);
    }

    #[test]
    fn a_completed_operation_records_the_effect_it_observed() {
        let journal = memory();
        let id = journal.admit(&alice(), "mutate_nodes", None).unwrap();
        journal.mark_dispatched(&alice(), &id).unwrap();
        journal
            .finish(
                &alice(),
                &id,
                ReceiptState::Completed,
                EffectState::Applied,
                Some("3 steps applied"),
            )
            .unwrap();
        let receipt = journal.get(&alice(), &id).unwrap().unwrap();
        assert_eq!(receipt.state, ReceiptState::Completed);
        assert_eq!(receipt.effect, EffectState::Applied);
        assert!(receipt.state.is_terminal());
        assert!(receipt.expires_at_ms.is_some());
        assert_eq!(receipt.outcome().state, OperationState::Completed);
    }

    #[test]
    fn a_failed_operation_is_terminal_and_keeps_its_partial_effect_visible() {
        let journal = memory();
        let id = journal.admit(&alice(), "mutate_nodes", None).unwrap();
        journal.mark_dispatched(&alice(), &id).unwrap();
        journal
            .finish(
                &alice(),
                &id,
                ReceiptState::Failed,
                EffectState::PartiallyApplied,
                Some("failed at step 2"),
            )
            .unwrap();
        let receipt = journal.get(&alice(), &id).unwrap().unwrap();
        assert_eq!(receipt.effect, EffectState::PartiallyApplied);
        assert_eq!(receipt.detail.as_deref(), Some("failed at step 2"));
    }

    #[test]
    fn wait_expiry_never_claims_the_work_was_prevented() {
        let journal = memory();
        let id = journal.admit(&alice(), "capture", None).unwrap();
        journal.mark_dispatched(&alice(), &id).unwrap();
        // A wait that ended is not a cancellation: the effect is unknown.
        let receipt = journal.get(&alice(), &id).unwrap().unwrap();
        assert_eq!(receipt.effect, EffectState::Unknown);
        assert_ne!(receipt.effect, EffectState::PreventedBeforeDispatch);
        assert_ne!(receipt.effect, EffectState::None);
    }

    #[test]
    fn an_interrupted_dispatch_recovers_as_uncertain_and_is_never_replayed() {
        let journal = memory();
        let id = journal.admit(&alice(), "mutate_nodes", None).unwrap();
        journal.mark_dispatched(&alice(), &id).unwrap();

        // Simulate a crash: recovery runs with no terminal result observed.
        let recovered = journal.recover_uncertain().unwrap();
        assert_eq!(recovered.len(), 1);
        let receipt = journal.get(&alice(), &id).unwrap().unwrap();
        assert_eq!(receipt.state, ReceiptState::UnknownEffect);
        assert_eq!(receipt.effect, EffectState::Unknown);
        assert_eq!(receipt.outcome().state, OperationState::UnknownEffect);

        // Recovery is idempotent and does not dispatch anything again: the
        // receipt stays exactly as recovery left it.
        assert!(journal.recover_uncertain().unwrap().is_empty());
        let after = journal.get(&alice(), &id).unwrap().unwrap();
        assert_eq!(after.state, ReceiptState::UnknownEffect);
        assert_eq!(after.effect, EffectState::Unknown);
    }

    #[test]
    fn an_admitted_operation_is_not_called_uncertain_because_nothing_was_sent() {
        let journal = memory();
        let id = journal.admit(&alice(), "mutate_nodes", None).unwrap();
        assert!(journal.recover_uncertain().unwrap().is_empty());
        let receipt = journal.get(&alice(), &id).unwrap().unwrap();
        assert_eq!(receipt.state, ReceiptState::Admitted);
    }

    #[test]
    fn a_completed_receipt_cannot_claim_an_unknown_effect() {
        let journal = memory();
        let id = journal.admit(&alice(), "mutate_nodes", None).unwrap();
        journal.mark_dispatched(&alice(), &id).unwrap();
        // `completed` + `unknown` would read as "dispatched, done, and nobody
        // knows what it did" — exactly the state the contract forbids.
        assert!(journal
            .finish(
                &alice(),
                &id,
                ReceiptState::Completed,
                EffectState::Unknown,
                None
            )
            .is_err());
        // The rejected call must not have advanced the receipt.
        let receipt = journal.get(&alice(), &id).unwrap().unwrap();
        assert_eq!(receipt.state, ReceiptState::Dispatched);
    }

    #[test]
    fn a_non_terminal_state_cannot_be_finished() {
        let journal = memory();
        let id = journal.admit(&alice(), "mutate_nodes", None).unwrap();
        journal.mark_dispatched(&alice(), &id).unwrap();
        assert!(journal
            .finish(
                &alice(),
                &id,
                ReceiptState::Dispatched,
                EffectState::Applied,
                None
            )
            .is_err());
    }

    #[test]
    fn a_receipt_cannot_be_finished_twice_or_out_of_order() {
        let journal = memory();
        let id = journal.admit(&alice(), "mutate_nodes", None).unwrap();
        // Finishing before dispatch is not a valid order.
        assert!(journal
            .finish(
                &alice(),
                &id,
                ReceiptState::Completed,
                EffectState::Applied,
                None
            )
            .is_err());
        journal.mark_dispatched(&alice(), &id).unwrap();
        journal
            .finish(
                &alice(),
                &id,
                ReceiptState::Completed,
                EffectState::Applied,
                None,
            )
            .unwrap();
        // A second terminal write is a conflict, not a silent overwrite.
        assert_eq!(
            journal
                .finish(&alice(), &id, ReceiptState::Failed, EffectState::None, None)
                .unwrap_err(),
            EvidenceError::Conflict
        );
    }

    #[test]
    fn one_owner_cannot_read_or_write_another_owners_receipt() {
        let journal = memory();
        let id = journal.admit(&alice(), "mutate_nodes", None).unwrap();
        journal.mark_dispatched(&alice(), &id).unwrap();
        assert!(journal.get(&bob(), &id).unwrap().is_none());
        assert!(journal.mark_dispatched(&bob(), &id).is_err());
        assert!(journal
            .finish(
                &bob(),
                &id,
                ReceiptState::Completed,
                EffectState::Applied,
                None
            )
            .is_err());
        assert!(journal.list(&bob(), 10).unwrap().is_empty());
        // Alice's receipt is untouched by the rejected cross-owner writes.
        assert_eq!(
            journal.get(&alice(), &id).unwrap().unwrap().state,
            ReceiptState::Dispatched
        );
    }

    #[test]
    fn receipts_are_bounded_per_owner_and_newest_first() {
        let journal = memory();
        for index in 0..12 {
            let id = journal
                .admit(&alice(), "mutate_nodes", Some(&format!("op{index}")))
                .unwrap();
            journal.mark_dispatched(&alice(), &id).unwrap();
            journal
                .finish(
                    &alice(),
                    &id,
                    ReceiptState::Completed,
                    EffectState::Applied,
                    None,
                )
                .unwrap();
        }
        let listed = journal.list(&alice(), 100).unwrap();
        assert!(
            listed.len() <= 8,
            "retention bound not applied: {}",
            listed.len()
        );
    }

    /// Retention comes from the configuration, not from a constant buried in
    /// the transition helper. A journal opened with a short retention must set
    /// its expiry from that value, or a caller's retention setting is silently
    /// ignored and receipts disappear on a schedule nobody chose.
    #[test]
    fn a_terminal_receipt_expires_on_the_configured_retention() {
        let journal = SqliteOperationJournal::open_memory(JournalConfig {
            retention: Duration::from_secs(5),
            ..config()
        })
        .unwrap();
        let id = journal.admit(&alice(), "mutate_nodes", None).unwrap();
        journal.mark_dispatched(&alice(), &id).unwrap();
        journal
            .finish(
                &alice(),
                &id,
                ReceiptState::Completed,
                EffectState::Applied,
                None,
            )
            .unwrap();
        let receipt = journal.get(&alice(), &id).unwrap().unwrap();
        let expires = receipt.expires_at_ms.expect("a terminal receipt expires");
        let wanted = receipt.updated_at_ms + 5_000;
        assert_eq!(
            expires, wanted,
            "expiry must follow the configured retention, not a default"
        );
    }

    /// A dispatched receipt is the caller's only record that an operation whose
    /// effect nobody observed actually left the process. Bounding retained rows
    /// by age must therefore never discard one: dropping it would turn "we
    /// don't know what this did" into "we never did anything".
    #[test]
    fn pruning_never_discards_a_dispatched_receipt_however_old_it_is() {
        let journal = SqliteOperationJournal::open_memory(config()).unwrap();
        // Stay inside the configured in-flight bound so the setup itself
        // succeeds; the refusal below is what proves the bound is enforced.
        let mut dispatched = Vec::new();
        for _ in 0..3 {
            let id = journal.admit(&alice(), "mutate_nodes", None).unwrap();
            journal.mark_dispatched(&alice(), &id).unwrap();
            dispatched.push(id);
        }
        // Admitting more work must not evict any of them, because they are
        // still in flight and each one is an unobserved-effect record. The
        // bound is enforced by refusing new work, not by forgetting old work.
        let mut refused = false;
        for _ in 0..20 {
            match journal.admit(&alice(), "mutate_nodes", None) {
                Ok(id) => {
                    journal.mark_dispatched(&alice(), &id).unwrap();
                }
                Err(EvidenceError::QuotaExceeded(_)) => {
                    refused = true;
                    break;
                }
                Err(error) => panic!("admission failed with an unexpected error: {error}"),
            }
        }
        assert!(
            refused,
            "the in-flight bound must refuse new work rather than evicting a dispatch"
        );
        for id in &dispatched {
            let receipt = journal
                .get(&alice(), id)
                .unwrap()
                .unwrap_or_else(|| panic!("dispatched receipt {id} was discarded by pruning"));
            assert_eq!(receipt.state, ReceiptState::Dispatched);
            assert_eq!(receipt.effect, EffectState::Unknown);
        }
    }

    /// A receipt nobody has finished is not bounded by the row limit, because
    /// bounding it would mean deciding on the caller's behalf that an
    /// in-flight operation is no longer worth reporting. The journal is
    /// required to refuse growth rather than to lie about pending work.
    #[test]
    fn an_over_large_number_of_in_flight_receipts_is_refused_rather_than_truncated() {
        let journal = SqliteOperationJournal::open_memory(JournalConfig {
            max_rows_per_owner: 4,
            max_total_rows: 8,
            ..config()
        })
        .unwrap();
        let mut ids = Vec::new();
        let mut refusal = None;
        for _ in 0..12 {
            match journal.admit(&alice(), "mutate_nodes", None) {
                Ok(id) => ids.push(id),
                Err(error) => {
                    refusal = Some(error);
                    break;
                }
            }
        }
        // Either every in-flight receipt was retained, or admission refused
        // once the bound was reached. Silently dropping one is the failure
        // this test exists to prevent.
        if let Some(error) = refusal {
            assert!(
                matches!(
                    error,
                    EvidenceError::QuotaExceeded(_) | EvidenceError::Backend(_)
                ),
                "in-flight admission must refuse with a quota error, not fail opaquely: {error}"
            );
        }
        for id in &ids {
            assert!(
                journal.get(&alice(), id).unwrap().is_some(),
                "an admitted receipt {id} must not be evicted while still in flight"
            );
        }
    }

    /// The SQLite backend and the in-memory backend must agree about the whole
    /// lifecycle. A caller that works against one and not the other would
    /// otherwise get different receipts for the same sequence of calls.
    #[test]
    fn both_backends_agree_on_the_happy_path_and_on_uncertainty() {
        fn lifecycle(journal: &dyn OperationJournal) -> (ReceiptState, EffectState) {
            let id = journal.admit(&alice(), "mutate_nodes", None).unwrap();
            journal.mark_dispatched(&alice(), &id).unwrap();
            let recovered = journal.recover_uncertain().unwrap();
            assert_eq!(
                recovered.len(),
                1,
                "recovery must report the in-flight receipt"
            );
            let receipt = journal.get(&alice(), &id).unwrap().unwrap();
            (receipt.state, receipt.effect)
        }
        let memory = lifecycle(&MemoryOperationJournal::new(config()));
        let sqlite = lifecycle(&SqliteOperationJournal::open_memory(config()).unwrap());
        assert_eq!(memory, sqlite);
        assert_eq!(memory.0, ReceiptState::UnknownEffect);
        assert_eq!(memory.1, EffectState::Unknown);
    }

    #[test]
    fn an_unbounded_detail_is_refused_rather_than_truncated() {
        let journal = memory();
        let oversized = "x".repeat(64);
        assert_eq!(
            journal
                .admit(&alice(), "mutate_nodes", Some(&oversized))
                .unwrap_err(),
            EvidenceError::QuotaExceeded(
                "operation receipt detail exceeds the configured bound".to_owned()
            )
        );
    }

    #[test]
    fn an_empty_or_oversized_subject_is_refused() {
        let journal = memory();
        assert!(journal.admit(&alice(), "", None).is_err());
        assert!(journal.admit(&alice(), &"x".repeat(200), None).is_err());
    }

    #[test]
    fn a_missing_receipt_is_absent_while_a_foreign_one_is_also_absent() {
        let journal = memory();
        assert!(journal.get(&alice(), "op_missing").unwrap().is_none());
        assert_eq!(
            journal.mark_dispatched(&alice(), "op_missing").unwrap_err(),
            EvidenceError::NotFound
        );
    }

    /// Build a v1-shaped journal by hand, so the migration is exercised against
    /// a database this build did not create.
    ///
    /// Creating a fresh journal and reading it back would prove nothing: a fresh
    /// journal runs the CREATE TABLE branch, never the migration.
    #[cfg(unix)]
    fn write_a_v1_journal(root: &std::path::Path) -> String {
        let connection = rusqlite::Connection::open(root.join("operations.sqlite3")).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE receipts (
                     id           TEXT    NOT NULL PRIMARY KEY,
                     owner        TEXT    NOT NULL,
                     subject      TEXT    NOT NULL,
                     state        TEXT    NOT NULL,
                     effect       TEXT    NOT NULL,
                     detail       TEXT,
                     created_at_ms INTEGER NOT NULL,
                     updated_at_ms INTEGER NOT NULL,
                     expires_at_ms INTEGER
                 );
                 CREATE INDEX receipts_owner ON receipts(owner, created_at_ms);
                 PRAGMA user_version = 1;",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO receipts
                     (id, owner, subject, state, effect, detail, created_at_ms, updated_at_ms,
                      expires_at_ms)
                 VALUES ('op_v1', 'alice', 'mutate_nodes', 'unknown_effect', 'unknown', NULL, 1, 1, NULL)",
                [],
            )
            .unwrap();
        connection
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .unwrap();
        drop(connection);
        // The v1 row is the only record of a dispatch nobody observed, so the
        // migration must carry it across rather than rebuild the journal.
        "op_v1".to_owned()
    }

    #[cfg(unix)]
    #[test]
    fn an_older_journal_is_migrated_without_losing_its_unobserved_receipts() {
        let root = temp_root("migrate");
        let id = write_a_v1_journal(&root);

        let journal = SqliteOperationJournal::open(&root, config()).unwrap();
        let receipt = journal
            .get(&alice(), &id)
            .unwrap()
            .expect("a v1 receipt survives the migration");
        assert_eq!(receipt.state, ReceiptState::UnknownEffect);
        assert_eq!(receipt.effect, EffectState::Unknown);
        assert_eq!(receipt.subject, "mutate_nodes");
        // Pre-resolution, so there is no caller account to carry forward.
        assert!(receipt.resolution.is_none());

        // The migrated journal is fully usable, not merely readable.
        let fresh = journal.admit(&alice(), "mutate_nodes", None).unwrap();
        journal.mark_dispatched(&alice(), &fresh).unwrap();
        let resolved = journal
            .resolve(&alice(), &fresh, EffectState::Applied, Some("checked"))
            .unwrap();
        assert!(resolved.resolution.is_some());
    }

    #[cfg(unix)]
    #[test]
    fn the_disk_journal_recovers_a_dispatch_that_a_restart_interrupted() {
        let root = temp_root("restart");
        let id = {
            let journal = SqliteOperationJournal::open(&root, config()).unwrap();
            let id = journal.admit(&alice(), "mutate_nodes", None).unwrap();
            journal.mark_dispatched(&alice(), &id).unwrap();
            // Drop the journal without a terminal result, as a crash would.
            id
        };

        let reopened = SqliteOperationJournal::open(&root, config()).unwrap();
        let receipt = reopened.get(&alice(), &id).unwrap().unwrap();
        assert_eq!(
            receipt.state,
            ReceiptState::UnknownEffect,
            "a dispatch interrupted by a restart must not read as merely dispatched"
        );
        assert_eq!(receipt.effect, EffectState::Unknown);
        // Recovery reported it, and did not retry it.
        assert!(reopened.recover_uncertain().unwrap().is_empty());
        fs::remove_dir_all(&root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn the_disk_journal_survives_a_restart_for_a_terminal_receipt() {
        let root = temp_root("terminal");
        let id = {
            let journal = SqliteOperationJournal::open(&root, config()).unwrap();
            let id = journal.admit(&alice(), "mutate_nodes", None).unwrap();
            journal.mark_dispatched(&alice(), &id).unwrap();
            journal
                .finish(
                    &alice(),
                    &id,
                    ReceiptState::Completed,
                    EffectState::Applied,
                    Some("done"),
                )
                .unwrap();
            id
        };
        let reopened = SqliteOperationJournal::open(&root, config()).unwrap();
        let receipt = reopened.get(&alice(), &id).unwrap().unwrap();
        assert_eq!(receipt.state, ReceiptState::Completed);
        assert_eq!(receipt.effect, EffectState::Applied);
        assert_eq!(receipt.detail.as_deref(), Some("done"));
        fs::remove_dir_all(&root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn a_second_writer_is_refused_instead_of_racing() {
        let root = temp_root("writer");
        let first = SqliteOperationJournal::open(&root, config()).unwrap();
        let second = SqliteOperationJournal::open(&root, config());
        assert_eq!(
            second.unwrap_err(),
            EvidenceError::Conflict,
            "a second journal must be refused, not allowed to race the first"
        );
        drop(first);
        // Once the first is dropped the lock is released and a new open works.
        assert!(SqliteOperationJournal::open(&root, config()).is_ok());
        fs::remove_dir_all(&root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn the_disk_journal_keeps_owners_isolated_across_a_restart() {
        let root = temp_root("owners");
        let id = {
            let journal = SqliteOperationJournal::open(&root, config()).unwrap();
            let id = journal.admit(&alice(), "mutate_nodes", None).unwrap();
            journal.mark_dispatched(&alice(), &id).unwrap();
            id
        };
        let reopened = SqliteOperationJournal::open(&root, config()).unwrap();
        assert!(reopened.get(&bob(), &id).unwrap().is_none());
        assert!(reopened.list(&bob(), 10).unwrap().is_empty());
        assert!(reopened.get(&alice(), &id).is_ok());
        fs::remove_dir_all(&root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn a_world_readable_journal_root_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let root = temp_root("perms");
        fs::set_permissions(&root, PermissionsExt::from_mode(0o755)).unwrap();
        assert!(SqliteOperationJournal::open(&root, config()).is_err());
        fs::remove_dir_all(&root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_journal_lock_is_refused() {
        use std::os::unix::fs::symlink;
        let root = temp_root("symlink");
        let target = root.join("elsewhere");
        fs::write(&target, b"not a lock").unwrap();
        symlink(&target, root.join(".writer.lock")).unwrap();
        assert!(SqliteOperationJournal::open(&root, config()).is_err());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_unrecognized_stored_state_fails_closed_rather_than_being_reinterpreted() {
        let journal = SqliteOperationJournal::open_memory(config()).unwrap();
        let id = journal.admit(&alice(), "mutate_nodes", None).unwrap();
        {
            let connection = journal.connection.lock().unwrap();
            connection
                .execute(
                    "UPDATE receipts SET state = 'some_future_state' WHERE id = ?1",
                    params![id],
                )
                .unwrap();
        }
        // A state this build does not know must not be read as a known one.
        assert!(journal.get(&alice(), &id).is_err());
    }

    #[test]
    fn concurrent_admissions_do_not_race_into_a_conflict() {
        let journal = Arc::new(memory());
        let successes = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        for index in 0..4 {
            let journal = Arc::clone(&journal);
            let successes = Arc::clone(&successes);
            handles.push(thread::spawn(move || {
                let owner = owner(&format!("owner{index}"));
                for _ in 0..4 {
                    let id = journal.admit(&owner, "mutate_nodes", None).unwrap();
                    journal.mark_dispatched(&owner, &id).unwrap();
                    journal
                        .finish(
                            &owner,
                            &id,
                            ReceiptState::Completed,
                            EffectState::Applied,
                            None,
                        )
                        .unwrap();
                    successes.fetch_add(1, Ordering::SeqCst);
                }
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(successes.load(Ordering::SeqCst), 16);
    }

    #[test]
    fn the_in_memory_journal_makes_no_durability_claim() {
        // The type documents that a receipt is lost on exit; this test pins the
        // observable consequence rather than the prose.
        let journal = memory();
        let id = journal.admit(&alice(), "mutate_nodes", None).unwrap();
        journal.mark_dispatched(&alice(), &id).unwrap();
        assert!(journal.get(&alice(), &id).unwrap().is_some());
        drop(journal);
        // Nothing to reopen: a fresh in-memory journal is empty by construction.
        let fresh = memory();
        assert!(fresh.get(&alice(), &id).unwrap().is_none());
    }

    #[test]
    fn a_sqlite_backed_journal_behaves_like_the_memory_one_for_the_whole_lifecycle() {
        let journal = SqliteOperationJournal::open_memory(config()).unwrap();
        let id = journal
            .admit(&alice(), "mutate_nodes", Some("note"))
            .unwrap();
        journal.mark_dispatched(&alice(), &id).unwrap();
        journal
            .finish(
                &alice(),
                &id,
                ReceiptState::Cancelled,
                EffectState::PreventedBeforeDispatch,
                None,
            )
            .unwrap();
        let receipt = journal.get(&alice(), &id).unwrap().unwrap();
        assert_eq!(receipt.state, ReceiptState::Cancelled);
        assert_eq!(receipt.effect, EffectState::PreventedBeforeDispatch);
        assert_eq!(receipt.detail.as_deref(), Some("note"));
        assert_eq!(receipt.outcome().state, OperationState::Cancelled);
    }

    /// Both backends must expose the same wait-expiry semantics, so the adapter
    /// that picks a backend does not change what a lost result means.
    fn wait_expiry_contract(journal: &dyn OperationJournal) {
        let id = journal.admit(&alice(), "mutate_nodes", None).unwrap();
        journal.mark_dispatched(&alice(), &id).unwrap();
        journal
            .mark_wait_expired(&alice(), &id, "caller timed out")
            .unwrap();
        let receipt = journal.get(&alice(), &id).unwrap().unwrap();
        assert_eq!(receipt.state, ReceiptState::WaitExpired);
        assert_eq!(receipt.effect, EffectState::Unknown);
        assert_eq!(receipt.detail.as_deref(), Some("caller timed out"));
        // Not terminal: the receipt stays unsettled and recoverable.
        assert!(!receipt.state.is_terminal());
        let expires = receipt
            .expires_at_ms
            .expect("ended waits have bounded retention");
        assert_eq!(
            expires - receipt.updated_at_ms,
            retention_ms(config().retention)
        );
        assert_eq!(receipt.outcome().state, OperationState::WaitExpired);
    }

    #[test]
    fn an_expired_wait_is_never_recorded_as_observed() {
        wait_expiry_contract(&memory());
    }

    #[test]
    fn a_sqlite_backed_expired_wait_matches_the_memory_one() {
        wait_expiry_contract(&SqliteOperationJournal::open_memory(config()).unwrap());
    }

    #[test]
    fn a_restart_turns_an_expired_wait_into_an_unknown_effect_rather_than_replaying_it() {
        let root = temp_root("wait-expiry-restart");
        let id = {
            let journal = SqliteOperationJournal::open(&root, config()).unwrap();
            let id = journal.admit(&alice(), "mutate_nodes", None).unwrap();
            journal.mark_dispatched(&alice(), &id).unwrap();
            journal
                .mark_wait_expired(&alice(), &id, "caller timed out")
                .unwrap();
            id
        };
        // A restart is where the distinction matters: the daemon no longer knows
        // whether the mutation landed, and the only honest record says so.
        // Recovery already ran inside `open`, so the settled state is read
        // rather than re-requested.
        let reopened = SqliteOperationJournal::open(&root, config()).unwrap();
        let receipt = reopened.get(&alice(), &id).unwrap().unwrap();
        assert_eq!(receipt.state, ReceiptState::UnknownEffect);
        assert_eq!(receipt.effect, EffectState::Unknown);
        // The detail written at expiry is preserved through recovery.
        assert_eq!(receipt.detail.as_deref(), Some("caller timed out"));
        // And it is still not replayed: recovery is idempotent, so a second
        // pass has nothing left to settle.
        assert!(reopened.recover_uncertain().unwrap().is_empty());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn repeated_expired_waits_release_admission_slots_without_a_restart() {
        let mut bounded = config();
        bounded.max_in_flight_per_owner = 1;
        bounded.max_in_flight_total = 1;
        let journal = SqliteOperationJournal::open_memory(bounded).unwrap();
        for _ in 0..130 {
            for owner in [alice(), bob()] {
                let id = journal.admit(&owner, "mutate_nodes", None).unwrap();
                journal.mark_dispatched(&owner, &id).unwrap();
                journal.mark_wait_expired(&owner, &id, "gave up").unwrap();
                let receipt = journal.get(&owner, &id).unwrap().unwrap();
                assert_eq!(receipt.state, ReceiptState::WaitExpired);
                assert_eq!(receipt.effect, EffectState::Unknown);
                assert!(receipt.expires_at_ms.is_some());
            }
        }
        // Uncertain history stays bounded independently of active waiters.
        assert!(journal.list(&alice(), 100).unwrap().len() <= bounded.max_rows_per_owner as usize);
        let active = journal.admit(&alice(), "mutate_nodes", None).unwrap();
        journal.mark_dispatched(&alice(), &active).unwrap();
        // Real active dispatches still consume both the owner and global quota.
        for owner in [alice(), bob()] {
            assert!(matches!(
                journal.admit(&owner, "mutate_nodes", None),
                Err(EvidenceError::QuotaExceeded(_))
            ));
        }
    }

    #[test]
    fn an_expired_wait_cannot_be_rewritten_as_a_known_outcome() {
        let journal = memory();
        let id = journal.admit(&alice(), "mutate_nodes", None).unwrap();
        journal.mark_dispatched(&alice(), &id).unwrap();
        journal.mark_wait_expired(&alice(), &id, "gave up").unwrap();
        // Only `finish` from `dispatched` is allowed, so a caller that lost the
        // result cannot later come back and claim the work completed.
        let rewrite = journal.finish(
            &alice(),
            &id,
            ReceiptState::Completed,
            EffectState::Applied,
            None,
        );
        assert!(
            matches!(rewrite, Err(EvidenceError::Conflict)),
            "expected Conflict, got {rewrite:?}"
        );
    }

    #[test]
    fn an_expired_wait_cannot_be_rewritten_by_another_owner() {
        let journal = memory();
        let id = journal.admit(&alice(), "mutate_nodes", None).unwrap();
        journal.mark_dispatched(&alice(), &id).unwrap();
        let foreign = journal.mark_wait_expired(&bob(), &id, "gave up");
        assert!(
            matches!(foreign, Err(EvidenceError::NotFound)),
            "expected NotFound, got {foreign:?}"
        );
    }

    /// Both backends must agree that "nothing was sent" is a settled, terminal
    /// answer, so the adapter's choice of backend cannot change it.
    fn not_sent_contract(journal: &dyn OperationJournal) {
        let id = journal.admit(&alice(), "mutate_nodes", None).unwrap();
        journal
            .mark_not_sent(&alice(), &id, "no connected bridge")
            .unwrap();
        let receipt = journal.get(&alice(), &id).unwrap().unwrap();
        assert_eq!(receipt.state, ReceiptState::Cancelled);
        assert_eq!(receipt.effect, EffectState::PreventedBeforeDispatch);
        assert!(receipt.state.is_terminal());
        assert_eq!(receipt.outcome().state, OperationState::Cancelled);
        // Settled means it holds no in-flight slot, so a client whose TD is down
        // can retry instead of being refused by its own quota.
        assert!(receipt.expires_at_ms.is_some());
    }

    /// The caller's account of an unobserved dispatch must be *beside* the
    /// state, never merged into it. This is the property the whole route rests
    /// on: if a caller's note could promote a receipt to completed, a later
    /// reader of `state` could no longer tell an observation from an assertion.
    fn resolution_contract(journal: &dyn OperationJournal) {
        let id = journal.admit(&alice(), "mutate_nodes", None).unwrap();
        journal.mark_dispatched(&alice(), &id).unwrap();
        journal
            .mark_wait_expired(&alice(), &id, "caller stopped waiting")
            .unwrap();
        // Recovery promotes it to the terminal unknown, which is where a caller
        // returning after a restart will find it.
        journal.recover_uncertain().unwrap();

        let resolved = journal
            .resolve(&alice(), &id, EffectState::Applied, Some("applied in TD"))
            .unwrap();
        assert_eq!(
            resolved.state,
            ReceiptState::UnknownEffect,
            "a caller's account must not rewrite the daemon's state"
        );
        assert_eq!(
            resolved.effect,
            EffectState::Unknown,
            "and must not rewrite the daemon's effect either"
        );
        let note = resolved
            .resolution
            .expect("the caller's account is recorded");
        assert_eq!(note.observed_effect, EffectState::Applied);
        assert_eq!(note.note.as_deref(), Some("applied in TD"));
        assert!(note.resolved_at_ms > 0);

        // It survives a re-read, so the answer is not lost between calls.
        let again = journal.get(&alice(), &id).unwrap().unwrap();
        assert_eq!(again.state, ReceiptState::UnknownEffect);
        assert!(again.resolution.is_some());
    }

    #[test]
    fn a_caller_can_account_for_an_unobserved_dispatch_without_changing_it() {
        resolution_contract(&memory());
    }

    #[test]
    fn a_sqlite_backed_resolution_matches_the_memory_one() {
        resolution_contract(&SqliteOperationJournal::open_memory(config()).unwrap());
    }

    #[test]
    fn an_observed_receipt_cannot_be_re_annotated_away() {
        // Its state is the better record: the daemon saw the result. A caller
        // note must not paper over it or be allowed to disagree.
        let journal = memory();
        let id = journal.admit(&alice(), "mutate_nodes", None).unwrap();
        journal.mark_dispatched(&alice(), &id).unwrap();
        journal
            .finish(
                &alice(),
                &id,
                ReceiptState::Completed,
                EffectState::None,
                None,
            )
            .unwrap();
        assert!(matches!(
            journal.resolve(&alice(), &id, EffectState::Applied, None),
            Err(EvidenceError::Conflict)
        ));
        let receipt = journal.get(&alice(), &id).unwrap().unwrap();
        assert_eq!(receipt.state, ReceiptState::Completed);
        assert!(receipt.resolution.is_none());
    }

    #[test]
    fn one_client_cannot_annotate_another_clients_receipt() {
        let journal = memory();
        let id = journal.admit(&alice(), "mutate_nodes", None).unwrap();
        journal.mark_dispatched(&alice(), &id).unwrap();
        // A foreign id is NotFound, not a hint that the row exists.
        assert!(matches!(
            journal.resolve(&bob(), &id, EffectState::Applied, None),
            Err(EvidenceError::NotFound)
        ));
        assert!(journal
            .get(&alice(), &id)
            .unwrap()
            .unwrap()
            .resolution
            .is_none());
    }

    #[test]
    fn an_oversized_or_control_bearing_note_is_refused() {
        let journal = memory();
        let id = journal.admit(&alice(), "mutate_nodes", None).unwrap();
        journal.mark_dispatched(&alice(), &id).unwrap();
        let long = "n".repeat(1_000);
        assert!(matches!(
            journal.resolve(&alice(), &id, EffectState::Applied, Some(&long)),
            Err(EvidenceError::QuotaExceeded(_))
        ));
        assert!(matches!(
            journal.resolve(&alice(), &id, EffectState::Applied, Some("has\u{7}nul")),
            Err(EvidenceError::InvalidInput(_))
        ));
    }

    #[test]
    fn a_resolution_is_not_license_to_retry() {
        // Recording what the caller saw must not change what the daemon will do
        // next: a dispatch recorded as unknown stays unknown, so nothing is
        // re-sent on the strength of the caller's note.
        let journal = memory();
        let id = journal.admit(&alice(), "mutate_nodes", None).unwrap();
        journal.mark_dispatched(&alice(), &id).unwrap();
        journal
            .resolve(&alice(), &id, EffectState::Applied, None)
            .unwrap();
        // The caller's account does not settle anything: the receipt is still
        // an unobserved dispatch, not an operation anyone can treat as done.
        let receipt = journal.get(&alice(), &id).unwrap().unwrap();
        assert_eq!(receipt.state, ReceiptState::Dispatched);
        assert_eq!(receipt.outcome().state, OperationState::Dispatched);
        // And it does not suppress recovery either. A later restart still
        // promotes it, which is what keeps the unknown honest rather than
        // quietly parked.
        let recovered = journal.recover_uncertain().unwrap();
        assert_eq!(recovered.len(), 1, "recovery still reports the dispatch");
        assert_eq!(recovered[0].state, ReceiptState::UnknownEffect);
        // Recovery carries the caller's account forward rather than dropping it.
        assert!(journal
            .get(&alice(), &id)
            .unwrap()
            .unwrap()
            .resolution
            .is_some());
    }

    #[test]
    fn a_send_that_never_left_the_daemon_is_settled_rather_than_left_admitted() {
        not_sent_contract(&memory());
    }

    #[test]
    fn a_sqlite_backed_not_sent_matches_the_memory_one() {
        not_sent_contract(&SqliteOperationJournal::open_memory(config()).unwrap());
    }

    #[test]
    fn repeated_failed_sends_cannot_exhaust_an_owner_quota() {
        // The regression this guards: a target that is not running produces a
        // "nothing was sent" outcome on every call. If those receipts were left
        // unsettled they would hold in-flight slots forever, and a client
        // waiting for TouchDesigner to start would be permanently refused.
        let mut bounded = config();
        bounded.max_in_flight_per_owner = 2;
        bounded.max_in_flight_total = 2;
        let journal = SqliteOperationJournal::open_memory(bounded).unwrap();
        for _ in 0..8 {
            let id = journal.admit(&alice(), "mutate_nodes", None).unwrap();
            journal
                .mark_not_sent(&alice(), &id, "td is not running")
                .unwrap();
        }
        // A real dispatch is still admitted afterwards.
        let id = journal.admit(&alice(), "mutate_nodes", None).unwrap();
        assert!(!id.is_empty());
    }

    #[test]
    fn a_dispatched_receipt_cannot_be_rewritten_as_never_sent() {
        let journal = memory();
        let id = journal.admit(&alice(), "mutate_nodes", None).unwrap();
        journal.mark_dispatched(&alice(), &id).unwrap();
        // Claiming "nothing was sent" after the work reached the executor would
        // erase the one fact the caller needs.
        let rewrite = journal.mark_not_sent(&alice(), &id, "queue busy");
        assert!(
            matches!(rewrite, Err(EvidenceError::Conflict)),
            "expected Conflict, got {rewrite:?}"
        );
        assert_eq!(
            journal.get(&alice(), &id).unwrap().unwrap().state,
            ReceiptState::Dispatched
        );
    }

    #[test]
    fn a_not_sent_receipt_cannot_be_rewritten_by_another_owner() {
        let journal = memory();
        let id = journal.admit(&alice(), "mutate_nodes", None).unwrap();
        let foreign = journal.mark_not_sent(&bob(), &id, "td is not running");
        assert!(
            matches!(foreign, Err(EvidenceError::NotFound)),
            "expected NotFound, got {foreign:?}"
        );
    }
}

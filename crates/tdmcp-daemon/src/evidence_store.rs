//! SQLite/WAL and in-memory implementations of the immutable evidence port.
//!
//! The disk backend is intentionally synchronous. Composition code must run it
//! on a blocking worker, never on TouchDesigner's main thread.

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, ValueRef};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tdmcp_mcp::evidence::{
    AppendChunk, BinaryLayout, EvidenceCursor, EvidenceError, EvidenceGrant, EvidenceId,
    EvidencePage, EvidenceRecord, EvidenceStore, ExportPolicy, ExportReceipt, ExportRequest,
    GrantId, GrantRequest, GrantScope, OwnerId, PageContent, PartKind, PartManifest, ReadEncoding,
    ReadRequest, Reservation, ReserveObservation, TransferProgress, UploadId,
};
use tdmcp_projectio::publish::{identity, rename_noreplace, rename_replace};
use uuid::Uuid;

const SCHEMA_VERSION: i64 = 2;
const SQLITE_U64_MAX: u64 = 9_223_372_036_854_775_807;
const STATUS_UPLOADING: &str = "uploading";
const STATUS_COMPLETE: &str = "complete";
const STATUS_EXPIRED: &str = "expired";
const STATUS_RELEASED: &str = "released";

#[derive(Debug, Clone, Copy)]
struct SqlU64(u64);

// The cursor row keeps `page_bytes` as its own slot instead of folding it into
// the trailing group: a cursor is only replayable under the byte budget that
// produced it, so a positional mismatch here must be a type error rather than a
// silent reordering. The order is (reader, evidence_id, part, encoding,
// next_offset, projection, page_bytes, grant_id).
type CursorDbRow = (
    String,
    String,
    String,
    String,
    SqlU64,
    String,
    SqlU64,
    Option<String>,
);
type PartDbRow = (
    String,
    Option<String>,
    String,
    SqlU64,
    String,
    Option<String>,
    String,
    String,
);

impl ToSql for SqlU64 {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        let value = i64::try_from(self.0)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        Ok(ToSqlOutput::from(value))
    }
}

impl FromSql for SqlU64 {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let value = value.as_i64()?;
        u64::try_from(value)
            .map(Self)
            .map_err(|_| FromSqlError::OutOfRange(value))
    }
}

/// Configurable logical and transfer bounds for an evidence store.
#[derive(Debug, Clone)]
pub struct EvidenceStoreConfig {
    /// Maximum active logical bytes owned by one principal, including reservations.
    pub max_owner_bytes: u64,
    /// Maximum active logical bytes across the store, including reservations.
    pub max_total_bytes: u64,
    /// Maximum logical bytes in one observation.
    pub max_observation_bytes: u64,
    /// Maximum bytes in one part.
    pub max_part_bytes: u64,
    /// Maximum bytes accepted by one producer transfer.
    pub max_chunk_bytes: u64,
    /// Maximum chunk rows retained for one part, independent of logical bytes.
    pub max_chunks_per_part: u64,
    /// Maximum decoded bytes returned by one page.
    pub max_page_bytes: u64,
    /// Maximum pinned logical bytes per owner.
    pub max_pinned_bytes_per_owner: u64,
    /// Maximum retained observation rows per owner, including bounded tombstones.
    pub max_owner_observations: u64,
    /// Maximum retained observation rows across the store, including tombstones.
    pub max_total_observations: u64,
    /// Maximum number of named parts in one observation.
    pub max_parts_per_observation: usize,
    /// Maximum serialized bytes in each metadata, coverage, or layout value.
    pub max_metadata_bytes: u64,
    /// Maximum combined serialized metadata/coverage/layout bytes per observation.
    pub max_observation_metadata_bytes: u64,
    /// Maximum active grants on one observation.
    pub max_grants_per_evidence: u64,
    /// Maximum retained paging cursors per reader.
    pub max_cursors_per_owner: u64,
    /// How long expired/released tombstones retain explicit terminal errors.
    pub tombstone_ttl: Duration,
    /// Maximum observation, pin, or grant lifetime.
    pub max_ttl: Duration,
}

impl Default for EvidenceStoreConfig {
    fn default() -> Self {
        Self {
            max_owner_bytes: 1024 * 1024 * 1024,
            max_total_bytes: 4 * 1024 * 1024 * 1024,
            max_observation_bytes: 512 * 1024 * 1024,
            max_part_bytes: 512 * 1024 * 1024,
            max_chunk_bytes: 1024 * 1024,
            max_chunks_per_part: 4_096,
            max_page_bytes: 1024 * 1024,
            max_pinned_bytes_per_owner: 1024 * 1024 * 1024,
            max_owner_observations: 1_024,
            max_total_observations: 4_096,
            max_parts_per_observation: 64,
            max_metadata_bytes: 256 * 1024,
            max_observation_metadata_bytes: 1024 * 1024,
            max_grants_per_evidence: 64,
            max_cursors_per_owner: 4_096,
            tombstone_ttl: Duration::from_secs(60 * 60),
            max_ttl: Duration::from_secs(7 * 24 * 60 * 60),
        }
    }
}

impl EvidenceStoreConfig {
    fn validate(&self) -> Result<(), EvidenceError> {
        if self.max_owner_bytes == 0
            || self.max_total_bytes == 0
            || self.max_observation_bytes == 0
            || self.max_part_bytes == 0
            || self.max_chunk_bytes == 0
            || self.max_chunks_per_part == 0
            || self.max_page_bytes == 0
            || self.max_pinned_bytes_per_owner == 0
            || self.max_owner_observations == 0
            || self.max_total_observations == 0
            || self.max_parts_per_observation == 0
            || self.max_metadata_bytes == 0
            || self.max_observation_metadata_bytes == 0
            || self.max_grants_per_evidence == 0
            || self.max_cursors_per_owner == 0
            || self.tombstone_ttl.is_zero()
            || self.max_ttl.is_zero()
        {
            return Err(EvidenceError::InvalidInput(
                "all evidence limits must be nonzero".to_owned(),
            ));
        }
        if self.max_owner_bytes > SQLITE_U64_MAX
            || self.max_total_bytes > SQLITE_U64_MAX
            || self.max_observation_bytes > SQLITE_U64_MAX
            || self.max_part_bytes > SQLITE_U64_MAX
            || self.max_chunk_bytes > SQLITE_U64_MAX
            || self.max_chunks_per_part > SQLITE_U64_MAX
            || self.max_page_bytes > SQLITE_U64_MAX
            || self.max_pinned_bytes_per_owner > SQLITE_U64_MAX
            || self.max_owner_observations > SQLITE_U64_MAX
            || self.max_total_observations > SQLITE_U64_MAX
            || self.max_metadata_bytes > SQLITE_U64_MAX
            || self.max_observation_metadata_bytes > SQLITE_U64_MAX
            || self.max_grants_per_evidence > SQLITE_U64_MAX
            || self.max_cursors_per_owner > SQLITE_U64_MAX
        {
            return Err(EvidenceError::InvalidInput(
                "SQLite-backed evidence limits may not exceed i64::MAX".to_owned(),
            ));
        }
        Ok(())
    }
}

/// Named durability boundaries available to deterministic failure tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FailurePoint {
    /// Before writing a new staged chunk blob.
    BlobWrite,
    /// Before atomically publishing a staged chunk blob.
    BlobPublish,
    /// Before inserting durable chunk metadata.
    ChunkMetadata,
    /// Immediately before committing an observation metadata transaction.
    DatabaseFinalize,
    /// Before publishing an assembled export.
    ExportPublish,
    /// After an export destination has been moved aside to a backup, but
    /// before the assembled artifact is published in its place.
    ///
    /// A crash here leaves the destination missing with the backup present, so
    /// startup reconciliation must repair it rather than report success.
    ExportReplaceWindow,
    /// After a publish rename, before the containing directory is fsynced.
    DirectorySync,
}

/// Injectable failure policy. Production uses [`NoFailures`].
pub trait EvidenceFailureInjector: Send + Sync {
    /// Fail or allow one named boundary.
    fn check(&self, point: FailurePoint) -> Result<(), EvidenceError>;
}

/// Production failure policy which permits every operation.
#[derive(Debug, Default)]
pub struct NoFailures;

impl EvidenceFailureInjector for NoFailures {
    fn check(&self, _point: FailurePoint) -> Result<(), EvidenceError> {
        Ok(())
    }
}

/// Private SQLite/WAL metadata plus immutable content-addressed chunk files.
pub struct SqliteEvidenceStore {
    inner: Store<DiskBlobs>,
}

/// Fully in-memory implementation with the same SQLite metadata/state machine.
pub struct MemoryEvidenceStore {
    inner: Store<MemoryBlobs>,
}

struct Store<B> {
    connection: Mutex<Connection>,
    filesystem_gate: Mutex<()>,
    blobs: B,
    config: EvidenceStoreConfig,
    failures: Arc<dyn EvidenceFailureInjector>,
}

trait BlobStore: Send + Sync {
    fn put(
        &self,
        hash: &str,
        bytes: &[u8],
        failures: &dyn EvidenceFailureInjector,
    ) -> Result<(), EvidenceError>;
    fn get(&self, hash: &str, expected_len: u64) -> Result<Vec<u8>, EvidenceError>;
    fn remove_unreferenced(&self, referenced: &HashSet<String>) -> Result<(), EvidenceError>;
    fn cleanup_staging(&self) -> Result<(), EvidenceError>;
    fn reject_export_destination(&self, destination: &Path) -> Result<(), EvidenceError>;
    /// Record a pending export before any destination-visible rename.
    fn journal_export(&self, journal: &ExportJournal) -> Result<(), EvidenceError>;
    /// Forget a resolved export journal.
    fn clear_export_journal(&self, id: &str) -> Result<(), EvidenceError>;
    /// Every journal entry that has not been resolved yet.
    fn list_export_journals(&self) -> Result<Vec<ExportJournal>, EvidenceError>;
    /// Repair or fail closed on journals left by an interrupted export.
    fn reconcile_exports(&self) -> Result<(), EvidenceError> {
        for journal in self.list_export_journals()? {
            self.resolve_export_journal(&journal)?;
        }
        Ok(())
    }
    /// Bring one interrupted export to a definite end state.
    fn resolve_export_journal(&self, journal: &ExportJournal) -> Result<(), EvidenceError>;
}

/// Durable intent record for an export that is about to touch a caller-visible
/// destination.
///
/// `BackupAndReplace` is a two-rename sequence, so a crash can leave the
/// destination missing with the displaced original at `backup`. Without this
/// record the next open cannot tell "never started" from "half done" and would
/// report a clean store over a lost user file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ExportJournalState {
    /// Artifact staged; destination not yet displaced.
    Prepared,
    /// Original destination moved to `backup`; artifact not yet published.
    DestinationBackedUp,
    /// Artifact published at the destination; journal not yet cleared.
    Published,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExportJournal {
    /// Opaque journal identity, also the journal file name.
    id: String,
    /// How far the sequence progressed before the interruption.
    state: ExportJournalState,
    /// Staged artifact path.
    temp: PathBuf,
    /// Caller-visible destination.
    destination: PathBuf,
    /// Displaced original, when the destination previously existed.
    backup: Option<PathBuf>,
    /// Expected content hash of the staged artifact.
    sha256: String,
    /// Expected artifact length in bytes.
    bytes: u64,
}

struct DiskBlobs {
    root: PathBuf,
    blobs: PathBuf,
    staging: PathBuf,
    journals: PathBuf,
    _writer_lock: File,
}

#[derive(Default)]
struct MemoryBlobs {
    values: Mutex<HashMap<String, Vec<u8>>>,
}

impl SqliteEvidenceStore {
    /// Open or create a private store rooted at `root`, then reconcile incomplete,
    /// expired, staged, and orphaned state before returning.
    ///
    /// Unix permissions are created/tightened and verified as owner-only. The
    /// disk backend currently fails closed on Windows because this crate has no
    /// audited ACL implementation; the in-memory backend remains available.
    pub fn open(
        root: impl AsRef<Path>,
        config: EvidenceStoreConfig,
    ) -> Result<Self, EvidenceError> {
        Self::open_with_failures(root, config, Arc::new(NoFailures))
    }

    /// Open with a deterministic failure injector.
    pub fn open_with_failures(
        root: impl AsRef<Path>,
        config: EvidenceStoreConfig,
        failures: Arc<dyn EvidenceFailureInjector>,
    ) -> Result<Self, EvidenceError> {
        config.validate()?;
        #[cfg(windows)]
        {
            let _ = (root.as_ref(), config, failures);
            return Err(EvidenceError::UnsupportedPlatform(
                "private Windows ACL creation/verification is not implemented".to_owned(),
            ));
        }
        #[cfg(not(windows))]
        {
            let blobs = DiskBlobs::open(root.as_ref())?;
            let database = blobs.root.join("index.sqlite3");
            let database_file = open_private_regular_file(&database)?;
            let database_identity = identity(&database).map_err(io_error)?;
            drop(database_file);
            let connection = Connection::open(&database).map_err(db_error)?;
            if identity(&database).map_err(io_error)? != database_identity {
                return Err(EvidenceError::Io(
                    "evidence database identity changed while opening".to_owned(),
                ));
            }
            configure_connection(&connection, false)?;
            initialize_schema(&connection)?;
            secure_sqlite_files(&blobs.root)?;
            let store = Self {
                inner: Store {
                    connection: Mutex::new(connection),
                    filesystem_gate: Mutex::new(()),
                    blobs,
                    config,
                    failures,
                },
            };
            store.inner.reconcile_startup()?;
            Ok(store)
        }
    }

    #[cfg(test)]
    fn blob_path(&self, hash: &str) -> PathBuf {
        self.inner.blobs.blob_path(hash)
    }
}

impl MemoryEvidenceStore {
    /// Construct a deterministic, non-persistent implementation.
    pub fn new(config: EvidenceStoreConfig) -> Result<Self, EvidenceError> {
        Self::new_with_failures(config, Arc::new(NoFailures))
    }

    /// Construct an in-memory implementation with a failure injector.
    pub fn new_with_failures(
        config: EvidenceStoreConfig,
        failures: Arc<dyn EvidenceFailureInjector>,
    ) -> Result<Self, EvidenceError> {
        config.validate()?;
        let connection = Connection::open_in_memory().map_err(db_error)?;
        configure_connection(&connection, true)?;
        initialize_schema(&connection)?;
        Ok(Self {
            inner: Store {
                connection: Mutex::new(connection),
                filesystem_gate: Mutex::new(()),
                blobs: MemoryBlobs::default(),
                config,
                failures,
            },
        })
    }
}

macro_rules! delegate_store {
    ($type:ty) => {
        impl EvidenceStore for $type {
            fn reserve(
                &self,
                owner: &OwnerId,
                request: ReserveObservation,
            ) -> Result<Reservation, EvidenceError> {
                self.inner.reserve(owner, request)
            }
            fn append_chunk(
                &self,
                owner: &OwnerId,
                request: AppendChunk<'_>,
            ) -> Result<TransferProgress, EvidenceError> {
                self.inner.append_chunk(owner, request)
            }
            fn finalize_part(
                &self,
                owner: &OwnerId,
                upload_id: &UploadId,
                part: &str,
            ) -> Result<(), EvidenceError> {
                self.inner.finalize_part(owner, upload_id, part)
            }
            fn commit(
                &self,
                owner: &OwnerId,
                upload_id: &UploadId,
            ) -> Result<EvidenceRecord, EvidenceError> {
                self.inner.commit(owner, upload_id)
            }
            fn abort(&self, owner: &OwnerId, upload_id: &UploadId) -> Result<(), EvidenceError> {
                self.inner.abort(owner, upload_id)
            }
            fn inspect(
                &self,
                owner: &OwnerId,
                evidence_id: &EvidenceId,
            ) -> Result<EvidenceRecord, EvidenceError> {
                self.inner.inspect(owner, evidence_id)
            }
            fn read(
                &self,
                owner: &OwnerId,
                request: ReadRequest,
            ) -> Result<EvidencePage, EvidenceError> {
                self.inner.read(owner, request)
            }
            fn pin(
                &self,
                owner: &OwnerId,
                evidence_id: &EvidenceId,
                ttl: Duration,
            ) -> Result<EvidenceRecord, EvidenceError> {
                self.inner.pin(owner, evidence_id, ttl)
            }
            fn release(
                &self,
                owner: &OwnerId,
                evidence_id: &EvidenceId,
            ) -> Result<(), EvidenceError> {
                self.inner.release(owner, evidence_id)
            }
            fn grant(
                &self,
                owner: &OwnerId,
                request: GrantRequest,
            ) -> Result<EvidenceGrant, EvidenceError> {
                self.inner.grant(owner, request)
            }
            fn revoke_grant(
                &self,
                owner: &OwnerId,
                grant_id: &GrantId,
            ) -> Result<(), EvidenceError> {
                self.inner.revoke_grant(owner, grant_id)
            }
            fn export(
                &self,
                owner: &OwnerId,
                request: ExportRequest,
            ) -> Result<ExportReceipt, EvidenceError> {
                self.inner.export(owner, request)
            }
        }
    };
}

delegate_store!(SqliteEvidenceStore);
delegate_store!(MemoryEvidenceStore);

impl<B: BlobStore> Store<B> {
    fn connection(&self) -> Result<MutexGuard<'_, Connection>, EvidenceError> {
        self.connection
            .lock()
            .map_err(|_| EvidenceError::Backend("evidence database lock was poisoned".to_owned()))
    }

    fn filesystem(&self) -> Result<MutexGuard<'_, ()>, EvidenceError> {
        self.filesystem_gate
            .lock()
            .map_err(|_| EvidenceError::Backend("evidence filesystem lock was poisoned".to_owned()))
    }

    fn reserve(
        &self,
        owner: &OwnerId,
        request: ReserveObservation,
    ) -> Result<Reservation, EvidenceError> {
        let _filesystem = self.filesystem()?;
        validate_ttl(request.ttl, self.config.max_ttl)?;
        if request.parts.is_empty() {
            return Err(EvidenceError::InvalidInput(
                "an observation requires at least one part".to_owned(),
            ));
        }
        if request.parts.len() > self.config.max_parts_per_observation {
            return Err(EvidenceError::QuotaExceeded(
                "observation part-count limit exceeded".to_owned(),
            ));
        }
        let mut metadata_total = json_size_bounded(
            &request.metadata,
            self.config.max_metadata_bytes,
            "observation metadata",
        )?;
        metadata_total = metadata_total
            .checked_add(json_size_bounded(
                &request.coverage,
                self.config.max_metadata_bytes,
                "observation coverage",
            )?)
            .ok_or_else(|| {
                EvidenceError::QuotaExceeded("metadata byte count overflow".to_owned())
            })?;
        let mut names = HashSet::new();
        let mut total = 0_u64;
        for part in &request.parts {
            validate_part(part, self.config.max_part_bytes)?;
            metadata_total = metadata_total
                .checked_add(json_size_bounded(
                    &part.metadata,
                    self.config.max_metadata_bytes,
                    "part metadata",
                )?)
                .ok_or_else(|| {
                    EvidenceError::QuotaExceeded("metadata byte count overflow".to_owned())
                })?;
            metadata_total = metadata_total
                .checked_add(json_size_bounded(
                    &part.coverage,
                    self.config.max_metadata_bytes,
                    "part coverage",
                )?)
                .ok_or_else(|| {
                    EvidenceError::QuotaExceeded("metadata byte count overflow".to_owned())
                })?;
            if let Some(layout) = &part.binary_layout {
                metadata_total = metadata_total
                    .checked_add(json_size_bounded(
                        layout,
                        self.config.max_metadata_bytes,
                        "binary layout",
                    )?)
                    .ok_or_else(|| {
                        EvidenceError::QuotaExceeded("metadata byte count overflow".to_owned())
                    })?;
            }
            if !names.insert(part.name.clone()) {
                return Err(EvidenceError::InvalidInput(
                    "part names must be unique".to_owned(),
                ));
            }
            total = total.checked_add(part.bytes).ok_or_else(|| {
                EvidenceError::QuotaExceeded("observation byte count overflow".to_owned())
            })?;
        }
        if total > self.config.max_observation_bytes {
            return Err(EvidenceError::QuotaExceeded(
                "observation limit exceeded".to_owned(),
            ));
        }
        if metadata_total > self.config.max_observation_metadata_bytes {
            return Err(EvidenceError::QuotaExceeded(
                "observation metadata byte limit exceeded".to_owned(),
            ));
        }
        let now = now_ms()?;
        let expires = add_duration(now, request.ttl)?;
        let upload = UploadId::parse(format!("u_{}", Uuid::new_v4()))?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        expire_rows(
            &transaction,
            now,
            subtract_duration(now, self.config.tombstone_ttl)?,
        )?;
        let owner_used = transaction
            .query_row(
                "SELECT COALESCE(SUM(reserved_bytes),0) FROM observations WHERE owner=?1 AND status IN ('uploading','complete')",
                [owner.as_str()],
                |row| row.get::<_, SqlU64>(0),
            )
            .map_err(db_error)?.0;
        let total_used = transaction
            .query_row(
                "SELECT COALESCE(SUM(reserved_bytes),0) FROM observations WHERE status IN ('uploading','complete')",
                [],
                |row| row.get::<_, SqlU64>(0),
            )
            .map_err(db_error)?.0;
        if owner_used.saturating_add(total) > self.config.max_owner_bytes {
            return Err(EvidenceError::QuotaExceeded(
                "owner logical byte limit exceeded".to_owned(),
            ));
        }
        if total_used.saturating_add(total) > self.config.max_total_bytes {
            return Err(EvidenceError::QuotaExceeded(
                "store logical byte limit exceeded".to_owned(),
            ));
        }
        let owner_observations = transaction
            .query_row(
                "SELECT COUNT(*) FROM observations WHERE owner=?1",
                [owner.as_str()],
                |row| row.get::<_, SqlU64>(0),
            )
            .map_err(db_error)?
            .0;
        let total_observations = transaction
            .query_row("SELECT COUNT(*) FROM observations", [], |row| {
                row.get::<_, SqlU64>(0)
            })
            .map_err(db_error)?
            .0;
        if owner_observations >= self.config.max_owner_observations {
            return Err(EvidenceError::QuotaExceeded(
                "owner observation-count limit exceeded".to_owned(),
            ));
        }
        if total_observations >= self.config.max_total_observations {
            return Err(EvidenceError::QuotaExceeded(
                "store observation-count limit exceeded".to_owned(),
            ));
        }
        let metadata = json_string(&request.metadata)?;
        let coverage = json_string(&request.coverage)?;
        transaction
            .execute(
                "INSERT INTO observations(id, owner, status, created_ms, expires_ms, pinned, reserved_bytes, metadata_json, coverage_json) VALUES(?1,?2,'uploading',?3,?4,0,?5,?6,?7)",
                params![upload.as_str(), owner.as_str(), now, expires, SqlU64(total), metadata, coverage],
            )
            .map_err(db_error)?;
        for (ordinal, part) in request.parts.iter().enumerate() {
            let kind = match part.kind {
                PartKind::Utf8 => "utf8",
                PartKind::Binary => "binary",
            };
            transaction
                .execute(
                    "INSERT INTO parts(observation_id,name,ordinal,content_type,content_encoding,kind,expected_len,expected_sha,layout_json,metadata_json,coverage_json,finalized) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,0)",
                    params![
                        upload.as_str(),
                        part.name,
                        i64::try_from(ordinal).map_err(|_| EvidenceError::InvalidInput("too many parts".to_owned()))?,
                        part.content_type,
                        part.content_encoding,
                        kind,
                        SqlU64(part.bytes),
                        part.sha256,
                        optional_json_string(part.binary_layout.as_ref())?,
                        json_string(&part.metadata)?,
                        json_string(&part.coverage)?,
                    ],
                )
                .map_err(db_error)?;
        }
        transaction.commit().map_err(db_error)?;
        Ok(Reservation {
            upload_id: upload,
            reserved_bytes: total,
            max_chunk_bytes: self.config.max_chunk_bytes,
        })
    }

    fn append_chunk(
        &self,
        owner: &OwnerId,
        request: AppendChunk<'_>,
    ) -> Result<TransferProgress, EvidenceError> {
        let _filesystem = self.filesystem()?;
        validate_part_name(request.part)?;
        if request.bytes.is_empty()
            || u64::try_from(request.bytes.len()).unwrap_or(u64::MAX) > self.config.max_chunk_bytes
        {
            return Err(EvidenceError::TransferTooLarge);
        }
        let (status, actual_owner, expires) =
            self.load_observation_header(request.upload_id.as_str())?;
        require_upload_owner(owner, &actual_owner, &status, expires)?;
        let (expected_len, finalized, next_offset, chunk_count) = {
            let connection = self.connection()?;
            connection
                .query_row(
                    "SELECT p.expected_len,p.finalized,COALESCE(SUM(c.len),0),COUNT(c.ordinal) FROM parts p LEFT JOIN chunks c ON c.observation_id=p.observation_id AND c.part_name=p.name WHERE p.observation_id=?1 AND p.name=?2 GROUP BY p.expected_len,p.finalized",
                    params![request.upload_id.as_str(), request.part],
                    |row| Ok((row.get::<_, SqlU64>(0)?.0, row.get::<_, bool>(1)?, row.get::<_, SqlU64>(2)?.0, row.get::<_, SqlU64>(3)?.0)),
                )
                .optional()
                .map_err(db_error)?
                .ok_or(EvidenceError::NotFound)?
        };
        if finalized {
            return Err(EvidenceError::AlreadyFinalized);
        }
        if chunk_count >= self.config.max_chunks_per_part {
            return Err(EvidenceError::QuotaExceeded(
                "part chunk-count limit exceeded".to_owned(),
            ));
        }
        if request.offset != next_offset {
            return Err(EvidenceError::OffsetMismatch {
                expected: next_offset,
                actual: request.offset,
            });
        }
        let chunk_len =
            u64::try_from(request.bytes.len()).map_err(|_| EvidenceError::TransferTooLarge)?;
        if next_offset.saturating_add(chunk_len) > expected_len {
            return Err(EvidenceError::TransferTooLarge);
        }
        let hash = sha256_hex(request.bytes);
        self.blobs
            .put(&hash, request.bytes, self.failures.as_ref())?;
        self.failures.check(FailurePoint::ChunkMetadata)?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let current = transaction
            .query_row(
                "SELECT COALESCE(SUM(len),0) FROM chunks WHERE observation_id=?1 AND part_name=?2",
                params![request.upload_id.as_str(), request.part],
                |row| row.get::<_, SqlU64>(0),
            )
            .map_err(db_error)?
            .0;
        if current != request.offset {
            return Err(EvidenceError::OffsetMismatch {
                expected: current,
                actual: request.offset,
            });
        }
        let ordinal = transaction
            .query_row(
                "SELECT COUNT(*) FROM chunks WHERE observation_id=?1 AND part_name=?2",
                params![request.upload_id.as_str(), request.part],
                |row| row.get::<_, SqlU64>(0),
            )
            .map_err(db_error)?
            .0;
        if ordinal >= self.config.max_chunks_per_part {
            return Err(EvidenceError::QuotaExceeded(
                "part chunk-count limit exceeded".to_owned(),
            ));
        }
        transaction
            .execute(
                "INSERT INTO chunks(observation_id,part_name,ordinal,offset,len,blob_hash) VALUES(?1,?2,?3,?4,?5,?6)",
                params![request.upload_id.as_str(), request.part, SqlU64(ordinal), SqlU64(request.offset), SqlU64(chunk_len), hash],
            )
            .map_err(db_error)?;
        transaction.commit().map_err(db_error)?;
        Ok(TransferProgress {
            next_offset: request.offset + chunk_len,
            total_bytes: expected_len,
        })
    }

    fn finalize_part(
        &self,
        owner: &OwnerId,
        upload_id: &UploadId,
        part: &str,
    ) -> Result<(), EvidenceError> {
        let _filesystem = self.filesystem()?;
        validate_part_name(part)?;
        let (status, actual_owner, expires) = self.load_observation_header(upload_id.as_str())?;
        require_upload_owner(owner, &actual_owner, &status, expires)?;
        let (expected_len, expected_sha, kind, finalized) = {
            let connection = self.connection()?;
            connection
                .query_row(
                    "SELECT expected_len,expected_sha,kind,finalized FROM parts WHERE observation_id=?1 AND name=?2",
                    params![upload_id.as_str(), part],
                    |row| Ok((row.get::<_, SqlU64>(0)?.0, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, bool>(3)?)),
                )
                .optional()
                .map_err(db_error)?
                .ok_or(EvidenceError::NotFound)?
        };
        if finalized {
            return Err(EvidenceError::AlreadyFinalized);
        }
        let chunks = self.chunk_rows(upload_id.as_str(), part)?;
        let actual_len = chunks.iter().try_fold(0_u64, |sum, row| {
            sum.checked_add(row.len)
                .ok_or_else(|| EvidenceError::Backend("chunk length overflow".to_owned()))
        })?;
        if actual_len != expected_len {
            return Err(EvidenceError::Incomplete(format!(
                "part has {actual_len} of {expected_len} bytes"
            )));
        }
        let mut hasher = Sha256::new();
        let mut utf8 = Utf8Validator::default();
        for chunk in &chunks {
            let bytes = self.checked_chunk(chunk)?;
            hasher.update(&bytes);
            if kind == "utf8" {
                utf8.push(&bytes)?;
            }
        }
        if kind == "utf8" {
            utf8.finish()?;
        }
        let actual_sha = hex_digest(hasher.finalize().as_slice());
        if actual_sha != expected_sha {
            return Err(EvidenceError::HashMismatch);
        }
        let connection = self.connection()?;
        let changed = connection
            .execute(
                "UPDATE parts SET finalized=1 WHERE observation_id=?1 AND name=?2 AND finalized=0",
                params![upload_id.as_str(), part],
            )
            .map_err(db_error)?;
        if changed != 1 {
            return Err(EvidenceError::Conflict);
        }
        Ok(())
    }

    fn commit(
        &self,
        owner: &OwnerId,
        upload_id: &UploadId,
    ) -> Result<EvidenceRecord, EvidenceError> {
        let (status, actual_owner, expires) = self.load_observation_header(upload_id.as_str())?;
        require_upload_owner(owner, &actual_owner, &status, expires)?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let incomplete: i64 = transaction
            .query_row(
                "SELECT COUNT(*) FROM parts WHERE observation_id=?1 AND finalized=0",
                [upload_id.as_str()],
                |row| row.get(0),
            )
            .map_err(db_error)?;
        if incomplete != 0 {
            return Err(EvidenceError::Incomplete(format!(
                "{incomplete} parts are not finalized"
            )));
        }
        self.failures.check(FailurePoint::DatabaseFinalize)?;
        let evidence_id = EvidenceId::parse(upload_id.as_str().replacen("u_", "e_", 1))?;
        let changed = transaction
            .execute(
                "UPDATE observations SET id=?1,status='complete' WHERE id=?2 AND owner=?3 AND status='uploading'",
                params![evidence_id.as_str(), upload_id.as_str(), owner.as_str()],
            )
            .map_err(db_error)?;
        if changed != 1 {
            return Err(EvidenceError::Conflict);
        }
        if let Err(error) = transaction.commit() {
            drop(connection);
            return self.reconcile_commit(owner, upload_id, &evidence_id, error);
        }
        drop(connection);
        self.inspect(owner, &evidence_id)
    }

    fn reconcile_commit(
        &self,
        owner: &OwnerId,
        upload_id: &UploadId,
        evidence_id: &EvidenceId,
        commit_error: rusqlite::Error,
    ) -> Result<EvidenceRecord, EvidenceError> {
        let connection = self.connection()?;
        let upload_status: Option<String> = connection
            .query_row(
                "SELECT status FROM observations WHERE id=?1 AND owner=?2",
                params![upload_id.as_str(), owner.as_str()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|query_error| {
                EvidenceError::CommitUncertain(format!(
                    "commit failed ({commit_error}); reconciliation failed ({query_error})"
                ))
            })?;
        if upload_status.as_deref() == Some(STATUS_UPLOADING) {
            return Err(EvidenceError::Backend(format!(
                "observation commit failed: {commit_error}"
            )));
        }
        let committed: Option<String> = connection
            .query_row(
                "SELECT status FROM observations WHERE id=?1 AND owner=?2",
                params![evidence_id.as_str(), owner.as_str()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|query_error| {
                EvidenceError::CommitUncertain(format!(
                    "commit failed ({commit_error}); committed-state reconciliation failed ({query_error})"
                ))
            })?;
        drop(connection);
        if committed.as_deref() == Some(STATUS_COMPLETE) {
            self.inspect(owner, evidence_id)
        } else {
            Err(EvidenceError::CommitUncertain(format!(
                "commit failed and observation state is unavailable: {commit_error}"
            )))
        }
    }

    fn abort(&self, owner: &OwnerId, upload_id: &UploadId) -> Result<(), EvidenceError> {
        let _filesystem = self.filesystem()?;
        let (status, actual_owner, _) = self.load_observation_header(upload_id.as_str())?;
        if actual_owner != *owner {
            return Err(EvidenceError::AccessDenied);
        }
        if status != STATUS_UPLOADING {
            return Err(EvidenceError::AlreadyFinalized);
        }
        let connection = self.connection()?;
        connection
            .execute("DELETE FROM observations WHERE id=?1", [upload_id.as_str()])
            .map_err(db_error)?;
        drop(connection);
        self.cleanup_orphans()
    }

    fn inspect(
        &self,
        owner: &OwnerId,
        evidence_id: &EvidenceId,
    ) -> Result<EvidenceRecord, EvidenceError> {
        let now = now_ms()?;
        let connection = self.connection()?;
        let access = authorize(&connection, owner, evidence_id, None, now)?;
        load_record(&connection, evidence_id, access.parts.as_ref())
    }

    fn read(&self, owner: &OwnerId, request: ReadRequest) -> Result<EvidencePage, EvidenceError> {
        let _filesystem = self.filesystem()?;
        validate_part_name(&request.part)?;
        if request.max_bytes == 0 || request.max_bytes > self.config.max_page_bytes {
            return Err(EvidenceError::TransferTooLarge);
        }
        let now = now_ms()?;
        let (start, access_grant) = {
            let connection = self.connection()?;
            if let Some(cursor) = &request.cursor {
                let row: Option<CursorDbRow> = connection
                    .query_row(
                        "SELECT reader,evidence_id,part,encoding,next_offset,projection,page_bytes,grant_id FROM cursors WHERE token=?1",
                        [cursor.as_str()],
                        |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get::<_, SqlU64>(6)?,row.get(7)?)),
                    )
                    .optional()
                    .map_err(db_error)?;
                let (reader, evidence, part, encoding, offset, projection, page_bytes, grant_id) =
                    row.ok_or(EvidenceError::InvalidCursor)?;
                let expected_encoding = encoding_name(request.encoding);
                if reader != owner.as_str()
                    || evidence != request.evidence_id.as_str()
                    || part != request.part
                    || encoding != expected_encoding
                    || projection != "bytes"
                    || page_bytes.0 != request.max_bytes
                {
                    return Err(EvidenceError::InvalidCursor);
                }
                let access = authorize_exact_grant(
                    &connection,
                    owner,
                    &request.evidence_id,
                    Some(&request.part),
                    now,
                    grant_id.as_deref(),
                )?;
                (offset.0, access.grant_id)
            } else {
                let access = authorize(
                    &connection,
                    owner,
                    &request.evidence_id,
                    Some(&request.part),
                    now,
                )?;
                (0, access.grant_id)
            }
        };
        let manifest = self.load_part(&request.evidence_id, &request.part)?;
        if request.encoding == ReadEncoding::Utf8 && manifest.kind != PartKind::Utf8 {
            return Err(EvidenceError::InvalidInput(
                "UTF-8 representation requested for a binary part".to_owned(),
            ));
        }
        if start > manifest.bytes {
            return Err(EvidenceError::InvalidCursor);
        }
        let requested_end = start.saturating_add(request.max_bytes).min(manifest.bytes);
        let mut bytes =
            self.read_range(&request.evidence_id, &request.part, start, requested_end)?;
        if request.encoding == ReadEncoding::Utf8 && requested_end < manifest.bytes {
            while std::str::from_utf8(&bytes).is_err() && !bytes.is_empty() {
                bytes.pop();
            }
            if bytes.is_empty() && start < manifest.bytes {
                return Err(EvidenceError::InvalidInput(
                    "maxBytes is too small for the next UTF-8 code point".to_owned(),
                ));
            }
        }
        if request.encoding == ReadEncoding::Utf8 && std::str::from_utf8(&bytes).is_err() {
            return Err(EvidenceError::Corrupt);
        }
        let returned = u64::try_from(bytes.len()).map_err(|_| EvidenceError::TransferTooLarge)?;
        let end = start + returned;
        let complete = end == manifest.bytes;
        let next_cursor = if complete {
            None
        } else {
            let cursor = EvidenceCursor::parse(format!("c_{}", Uuid::new_v4()))?;
            let mut connection = self.connection()?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(db_error)?;
            let cursor_count = transaction
                .query_row(
                    "SELECT COUNT(*) FROM cursors WHERE reader=?1",
                    [owner.as_str()],
                    |row| row.get::<_, SqlU64>(0),
                )
                .map_err(db_error)?
                .0;
            if cursor_count >= self.config.max_cursors_per_owner {
                return Err(EvidenceError::QuotaExceeded(
                    "reader cursor-count limit exceeded".to_owned(),
                ));
            }
            transaction
                .execute(
                    "INSERT INTO cursors(token,reader,evidence_id,part,encoding,projection,next_offset,page_bytes,grant_id) VALUES(?1,?2,?3,?4,?5,'bytes',?6,?7,?8)",
                    params![cursor.as_str(), owner.as_str(), request.evidence_id.as_str(), request.part, encoding_name(request.encoding), SqlU64(end), SqlU64(request.max_bytes), access_grant],
                )
                .map_err(db_error)?;
            transaction.commit().map_err(db_error)?;
            Some(cursor)
        };
        let content = match request.encoding {
            ReadEncoding::Utf8 => {
                PageContent::Utf8(String::from_utf8(bytes).map_err(|_| EvidenceError::Corrupt)?)
            }
            ReadEncoding::Base64 => PageContent::Base64(BASE64_STANDARD.encode(bytes)),
        };
        Ok(EvidencePage {
            evidence_id: request.evidence_id,
            part: request.part,
            content_type: manifest.content_type,
            content_encoding: manifest.content_encoding,
            byte_start: start,
            byte_end: end,
            total_bytes: manifest.bytes,
            returned_bytes: returned,
            content,
            next_cursor,
            complete,
            binary_layout: manifest.binary_layout,
        })
    }

    fn pin(
        &self,
        owner: &OwnerId,
        evidence_id: &EvidenceId,
        ttl: Duration,
    ) -> Result<EvidenceRecord, EvidenceError> {
        validate_ttl(ttl, self.config.max_ttl)?;
        let now = now_ms()?;
        let requested_expiry = add_duration(now, ttl)?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let (actual_owner, status, current_expiry, bytes): (String, String, i64, SqlU64) =
            transaction
                .query_row(
                    "SELECT owner,status,expires_ms,reserved_bytes FROM observations WHERE id=?1",
                    [evidence_id.as_str()],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()
                .map_err(db_error)?
                .ok_or(EvidenceError::NotFound)?;
        require_owned_complete(owner, &actual_owner, &status, current_expiry, now)?;
        // Pinning extends a lease; a shorter ttl must never shorten an existing one.
        let new_expiry = current_expiry.max(requested_expiry);
        let pinned_used = transaction
            .query_row(
                "SELECT COALESCE(SUM(reserved_bytes),0) FROM observations WHERE owner=?1 AND status='complete' AND pinned=1 AND expires_ms>?2 AND id<>?3",
                params![owner.as_str(), now, evidence_id.as_str()],
                |row| row.get::<_, SqlU64>(0),
            )
            .map_err(db_error)?.0;
        if pinned_used.saturating_add(bytes.0) > self.config.max_pinned_bytes_per_owner {
            return Err(EvidenceError::QuotaExceeded(
                "owner pinned byte limit exceeded".to_owned(),
            ));
        }
        transaction
            .execute(
                "UPDATE observations SET expires_ms=?1,pinned=1 WHERE id=?2 AND status='complete'",
                params![new_expiry, evidence_id.as_str()],
            )
            .map_err(db_error)?;
        transaction.commit().map_err(db_error)?;
        drop(connection);
        self.inspect(owner, evidence_id)
    }

    fn release(&self, owner: &OwnerId, evidence_id: &EvidenceId) -> Result<(), EvidenceError> {
        let _filesystem = self.filesystem()?;
        let now = now_ms()?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let row: Option<(String, String, i64)> = transaction
            .query_row(
                "SELECT owner,status,expires_ms FROM observations WHERE id=?1",
                [evidence_id.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(db_error)?;
        let (actual_owner, status, expires) = row.ok_or(EvidenceError::NotFound)?;
        require_owned_complete(owner, &actual_owner, &status, expires, now)?;
        transaction
            .execute(
                "DELETE FROM chunks WHERE observation_id=?1",
                [evidence_id.as_str()],
            )
            .map_err(db_error)?;
        transaction
            .execute(
                "DELETE FROM grants WHERE evidence_id=?1",
                [evidence_id.as_str()],
            )
            .map_err(db_error)?;
        transaction
            .execute(
                "DELETE FROM cursors WHERE evidence_id=?1",
                [evidence_id.as_str()],
            )
            .map_err(db_error)?;
        transaction
            .execute(
                "UPDATE observations SET status='released',pinned=0,expires_ms=?1 WHERE id=?2",
                params![now, evidence_id.as_str()],
            )
            .map_err(db_error)?;
        transaction.commit().map_err(db_error)?;
        drop(connection);
        self.cleanup_orphans()
    }

    fn grant(
        &self,
        owner: &OwnerId,
        request: GrantRequest,
    ) -> Result<EvidenceGrant, EvidenceError> {
        validate_ttl(request.ttl, self.config.max_ttl)?;
        if request.grantee == *owner {
            return Err(EvidenceError::InvalidInput(
                "grant grantee must differ from owner".to_owned(),
            ));
        }
        let now = now_ms()?;
        let requested_expiry = add_duration(now, request.ttl)?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let record = load_owned_record(&transaction, owner, &request.evidence_id, now)?;
        let expires = requested_expiry.min(record.expires_at_ms);
        if expires <= now {
            return Err(EvidenceError::Expired);
        }
        validate_grant_scope(&request.scope, &record.parts)?;
        transaction
            .execute("DELETE FROM grants WHERE expires_ms<=?1", [now])
            .map_err(db_error)?;
        let grant_count = transaction
            .query_row(
                "SELECT COUNT(*) FROM grants WHERE evidence_id=?1 AND revoked=0 AND expires_ms>?2",
                params![request.evidence_id.as_str(), now],
                |row| row.get::<_, SqlU64>(0),
            )
            .map_err(db_error)?
            .0;
        if grant_count >= self.config.max_grants_per_evidence {
            return Err(EvidenceError::QuotaExceeded(
                "evidence grant-count limit exceeded".to_owned(),
            ));
        }
        let grant_id = GrantId::parse(format!("g_{}", Uuid::new_v4()))?;
        let scope_json = json_string(&request.scope)?;
        transaction
            .execute(
                "INSERT INTO grants(id,evidence_id,grantor,grantee,scope_json,expires_ms,revoked) VALUES(?1,?2,?3,?4,?5,?6,0)",
                params![grant_id.as_str(), request.evidence_id.as_str(), owner.as_str(), request.grantee.as_str(), scope_json, expires],
            )
            .map_err(db_error)?;
        transaction.commit().map_err(db_error)?;
        Ok(EvidenceGrant {
            grant_id,
            evidence_id: request.evidence_id,
            grantor: owner.clone(),
            grantee: request.grantee,
            scope: request.scope,
            expires_at_ms: expires,
        })
    }

    fn revoke_grant(&self, owner: &OwnerId, grant_id: &GrantId) -> Result<(), EvidenceError> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let grantor: Option<String> = transaction
            .query_row(
                "SELECT grantor FROM grants WHERE id=?1",
                [grant_id.as_str()],
                |row| row.get(0),
            )
            .optional()
            .map_err(db_error)?;
        let grantor = grantor.ok_or(EvidenceError::NotFound)?;
        if grantor != owner.as_str() {
            return Err(EvidenceError::AccessDenied);
        }
        transaction
            .execute("DELETE FROM grants WHERE id=?1", [grant_id.as_str()])
            .map_err(db_error)?;
        transaction.commit().map_err(db_error)?;
        Ok(())
    }

    fn export(
        &self,
        owner: &OwnerId,
        request: ExportRequest,
    ) -> Result<ExportReceipt, EvidenceError> {
        let _filesystem = self.filesystem()?;
        validate_part_name(&request.part)?;
        self.blobs.reject_export_destination(&request.destination)?;
        let now = now_ms()?;
        {
            let connection = self.connection()?;
            authorize(
                &connection,
                owner,
                &request.evidence_id,
                Some(&request.part),
                now,
            )?;
        }
        let manifest = self.load_part(&request.evidence_id, &request.part)?;
        reject_destination_symlink(&request.destination)?;
        let parent = request.destination.parent().ok_or_else(|| {
            EvidenceError::InvalidInput("export destination requires a parent directory".to_owned())
        })?;
        let parent_meta = fs::symlink_metadata(parent).map_err(io_error)?;
        if !parent_meta.is_dir() || parent_meta.file_type().is_symlink() {
            return Err(EvidenceError::InvalidInput(
                "export parent must be a real directory".to_owned(),
            ));
        }
        let temp = parent.join(format!(".tdmcp-export-{}.tmp", Uuid::new_v4()));
        let mut output = create_private_file(&temp)?;
        let chunks = self.chunk_rows(request.evidence_id.as_str(), &request.part)?;
        // Recompute the aggregate while streaming so the receipt describes the
        // bytes actually written, and reject a chunk map whose offsets or order
        // do not reconstruct the finalized part.
        let write_result = (|| {
            let mut hasher = Sha256::new();
            let mut written = 0_u64;
            for chunk in &chunks {
                if chunk.offset != written {
                    return Err(EvidenceError::Corrupt);
                }
                let bytes = self.checked_chunk(chunk)?;
                if u64::try_from(bytes.len()).ok() != Some(chunk.len) {
                    return Err(EvidenceError::Corrupt);
                }
                hasher.update(&bytes);
                written = written
                    .checked_add(chunk.len)
                    .ok_or(EvidenceError::Corrupt)?;
                output.write_all(&bytes).map_err(io_error)?;
            }
            if written != manifest.bytes {
                return Err(EvidenceError::Corrupt);
            }
            let digest = hex_digest(hasher.finalize().as_slice());
            if digest != manifest.sha256 {
                return Err(EvidenceError::HashMismatch);
            }
            output.sync_all().map_err(io_error)
        })();
        if let Err(error) = write_result {
            let _ = fs::remove_file(&temp);
            return Err(error);
        }
        drop(output);
        if let Err(error) = self.failures.check(FailurePoint::ExportPublish) {
            let _ = fs::remove_file(&temp);
            return Err(error);
        }
        // The journal is written before any destination-visible rename, so an
        // interruption at any later point is repairable instead of being
        // mistaken for "the export never ran".
        let journal = ExportJournal {
            id: Uuid::new_v4().to_string(),
            state: ExportJournalState::Prepared,
            temp: temp.clone(),
            destination: request.destination.clone(),
            backup: None,
            sha256: manifest.sha256.clone(),
            bytes: manifest.bytes,
        };
        if let Err(error) = self.blobs.journal_export(&journal) {
            let _ = fs::remove_file(&temp);
            return Err(error);
        }
        let mut journal = journal;
        let backup = match request.policy {
            ExportPolicy::NoOverwrite => {
                if let Err(error) = rename_noreplace(&temp, &request.destination) {
                    let _ = fs::remove_file(&temp);
                    let _ = self.blobs.clear_export_journal(&journal.id);
                    return Err(io_error(error));
                }
                journal.state = ExportJournalState::Published;
                self.blobs.journal_export(&journal)?;
                None
            }
            ExportPolicy::BackupAndReplace => {
                if request.destination.exists() {
                    let metadata = fs::symlink_metadata(&request.destination).map_err(io_error)?;
                    if !metadata.is_file() || metadata.file_type().is_symlink() {
                        let _ = fs::remove_file(&temp);
                        let _ = self.blobs.clear_export_journal(&journal.id);
                        return Err(EvidenceError::InvalidInput(
                            "existing export destination must be a regular non-symlink file"
                                .to_owned(),
                        ));
                    }
                    let backup = parent.join(format!(".tdmcp-backup-{}", Uuid::new_v4()));
                    rename_noreplace(&request.destination, &backup).map_err(io_error)?;
                    journal.backup = Some(backup);
                    journal.state = ExportJournalState::DestinationBackedUp;
                    if let Err(error) = self.blobs.journal_export(&journal) {
                        return Err(export_uncertain(&journal, &error.to_string()));
                    }
                }
                // A crash here leaves the destination absent with the original
                // displaced; startup reconciliation restores or republishes it.
                if let Err(error) = self.failures.check(FailurePoint::ExportReplaceWindow) {
                    return Err(export_uncertain(
                        &journal,
                        &format!("interrupted before publication ({error})"),
                    ));
                }
                if let Err(error) = rename_noreplace(&temp, &request.destination) {
                    let _ = fs::remove_file(&temp);
                    if let Some(path) = &journal.backup {
                        if let Err(rollback) = rename_noreplace(path, &request.destination) {
                            return Err(export_uncertain(
                                &journal,
                                &format!(
                                    "publication failed ({error}) and rollback failed ({rollback})"
                                ),
                            ));
                        }
                    }
                    let _ = self.blobs.clear_export_journal(&journal.id);
                    return Err(io_error(error));
                }
                journal.state = ExportJournalState::Published;
                self.blobs.journal_export(&journal)?;
                journal.backup.clone()
            }
        };
        // A directory fsync that fails leaves the rename unconfirmed. Retrying
        // the sync is the honest repair; reporting success without it would let
        // a later open see a destination that never survived a crash.
        if let Err(error) = self
            .failures
            .check(FailurePoint::DirectorySync)
            .and_then(|()| sync_directory(parent))
        {
            return Err(export_uncertain(
                &journal,
                &format!("published but the directory entry could not be synced ({error})"),
            ));
        }
        self.blobs.clear_export_journal(&journal.id)?;
        Ok(ExportReceipt {
            destination: request.destination,
            backup,
            bytes: manifest.bytes,
            sha256: manifest.sha256,
        })
    }

    fn load_observation_header(&self, id: &str) -> Result<(String, OwnerId, i64), EvidenceError> {
        let connection = self.connection()?;
        let row: Option<(String, String, i64)> = connection
            .query_row(
                "SELECT status,owner,expires_ms FROM observations WHERE id=?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(db_error)?;
        let (status, owner, expires) = row.ok_or(EvidenceError::NotFound)?;
        Ok((status, OwnerId::parse(owner)?, expires))
    }

    fn load_part(
        &self,
        evidence_id: &EvidenceId,
        part: &str,
    ) -> Result<PartManifest, EvidenceError> {
        let connection = self.connection()?;
        load_part_row(&connection, evidence_id.as_str(), part)
    }

    fn chunk_rows(&self, observation_id: &str, part: &str) -> Result<Vec<ChunkRow>, EvidenceError> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare("SELECT offset,len,blob_hash FROM chunks WHERE observation_id=?1 AND part_name=?2 ORDER BY ordinal")
            .map_err(db_error)?;
        let rows = statement
            .query_map(params![observation_id, part], |row| {
                Ok(ChunkRow {
                    offset: row.get::<_, SqlU64>(0)?.0,
                    len: row.get::<_, SqlU64>(1)?.0,
                    hash: row.get(2)?,
                })
            })
            .map_err(db_error)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db_error)
    }

    fn checked_chunk(&self, row: &ChunkRow) -> Result<Vec<u8>, EvidenceError> {
        let bytes = self.blobs.get(&row.hash, row.len)?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) != row.len
            || sha256_hex(&bytes) != row.hash
        {
            return Err(EvidenceError::Corrupt);
        }
        Ok(bytes)
    }

    fn read_range(
        &self,
        evidence_id: &EvidenceId,
        part: &str,
        start: u64,
        end: u64,
    ) -> Result<Vec<u8>, EvidenceError> {
        let capacity = usize::try_from(end.saturating_sub(start))
            .map_err(|_| EvidenceError::TransferTooLarge)?;
        let mut output = Vec::with_capacity(capacity);
        for row in self.chunk_rows(evidence_id.as_str(), part)? {
            let chunk_end = row.offset.saturating_add(row.len);
            if chunk_end <= start || row.offset >= end {
                continue;
            }
            let bytes = self.checked_chunk(&row)?;
            let local_start = usize::try_from(start.saturating_sub(row.offset))
                .map_err(|_| EvidenceError::Corrupt)?;
            let local_end = usize::try_from(end.min(chunk_end).saturating_sub(row.offset))
                .map_err(|_| EvidenceError::Corrupt)?;
            let slice = bytes
                .get(local_start..local_end)
                .ok_or(EvidenceError::Corrupt)?;
            output.extend_from_slice(slice);
        }
        if output.len() != capacity {
            return Err(EvidenceError::Corrupt);
        }
        Ok(output)
    }

    fn reconcile_startup(&self) -> Result<(), EvidenceError> {
        let _filesystem = self.filesystem()?;
        // Exports are repaired first: an unresolved journal can hold a
        // user-visible destination open, and it may need a staged artifact
        // that later stages would otherwise discard.
        self.blobs.reconcile_exports()?;
        self.blobs.cleanup_staging()?;
        let now = now_ms()?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        transaction
            .execute("DELETE FROM observations WHERE status='uploading'", [])
            .map_err(db_error)?;
        expire_rows(
            &transaction,
            now,
            subtract_duration(now, self.config.tombstone_ttl)?,
        )?;
        transaction.commit().map_err(db_error)?;
        drop(connection);
        self.cleanup_orphans()
    }

    fn cleanup_orphans(&self) -> Result<(), EvidenceError> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare("SELECT DISTINCT blob_hash FROM chunks")
            .map_err(db_error)?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(db_error)?;
        let referenced = rows.collect::<Result<HashSet<_>, _>>().map_err(db_error)?;
        drop(statement);
        drop(connection);
        self.blobs.remove_unreferenced(&referenced)
    }
}

#[derive(Debug)]
struct ChunkRow {
    offset: u64,
    len: u64,
    hash: String,
}

#[derive(Debug)]
struct Access {
    grant_id: Option<String>,
    parts: Option<HashSet<String>>,
}

fn authorize(
    connection: &Connection,
    reader: &OwnerId,
    evidence_id: &EvidenceId,
    part: Option<&str>,
    now: i64,
) -> Result<Access, EvidenceError> {
    let row: Option<(String, String, i64)> = connection
        .query_row(
            "SELECT owner,status,expires_ms FROM observations WHERE id=?1",
            [evidence_id.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(db_error)?;
    let (owner, status, expires) = row.ok_or(EvidenceError::NotFound)?;
    check_readable_status(&status, expires, now)?;
    if owner == reader.as_str() {
        return Ok(Access {
            grant_id: None,
            parts: None,
        });
    }
    let mut statement = connection
        .prepare("SELECT id,scope_json FROM grants WHERE evidence_id=?1 AND grantee=?2 AND revoked=0 AND expires_ms>?3 ORDER BY expires_ms DESC")
        .map_err(db_error)?;
    let rows = statement
        .query_map(params![evidence_id.as_str(), reader.as_str(), now], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(db_error)?;
    // A roster read (no explicit part) is authorized by the union of every
    // active grant, so disjoint or overlapping scopes do not silently hide
    // parts this reader is allowed to see. A single-part read keeps exact
    // grant binding so cursors stay reproducible.
    let mut union: Option<HashSet<String>> = None;
    let mut unbounded = false;
    for row in rows {
        let (id, scope_json) = row.map_err(db_error)?;
        let scope: GrantScope = serde_json::from_str(&scope_json).map_err(json_error)?;
        let parts = scope_parts(&scope);
        match part {
            Some(name) => {
                if parts.as_ref().is_none_or(|allowed| allowed.contains(name)) {
                    return Ok(Access {
                        grant_id: Some(id),
                        parts,
                    });
                }
            }
            None => match (&mut union, &parts) {
                (_, None) => unbounded = true,
                (None, Some(allowed)) => {
                    union = Some(allowed.clone());
                }
                (Some(accumulated), Some(allowed)) => {
                    accumulated.extend(allowed.iter().cloned());
                }
            },
        }
    }
    if unbounded {
        return Ok(Access {
            grant_id: None,
            parts: None,
        });
    }
    union.map_or(Err(EvidenceError::AccessDenied), |parts| {
        Ok(Access {
            grant_id: None,
            parts: Some(parts),
        })
    })
}

fn authorize_exact_grant(
    connection: &Connection,
    reader: &OwnerId,
    evidence_id: &EvidenceId,
    part: Option<&str>,
    now: i64,
    expected_grant: Option<&str>,
) -> Result<Access, EvidenceError> {
    if expected_grant.is_none() {
        let access = authorize(connection, reader, evidence_id, part, now)?;
        if access.grant_id.is_some() {
            return Err(EvidenceError::InvalidCursor);
        }
        return Ok(access);
    }
    let row: Option<(String, String, String, i64, bool, String, i64)> = connection
        .query_row(
            "SELECT g.grantee,g.evidence_id,g.scope_json,g.expires_ms,g.revoked,o.status,o.expires_ms FROM grants g JOIN observations o ON o.id=g.evidence_id WHERE g.id=?1",
            [expected_grant],
            |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?)),
        )
        .optional()
        .map_err(db_error)?;
    let (grantee, evidence, scope_json, grant_expiry, revoked, status, evidence_expiry) =
        row.ok_or(EvidenceError::InvalidCursor)?;
    check_readable_status(&status, evidence_expiry, now)?;
    if grantee != reader.as_str()
        || evidence != evidence_id.as_str()
        || revoked
        || grant_expiry <= now
    {
        return Err(EvidenceError::InvalidCursor);
    }
    let scope: GrantScope = serde_json::from_str(&scope_json).map_err(json_error)?;
    let parts = scope_parts(&scope);
    if part.is_some_and(|name| {
        parts
            .as_ref()
            .is_some_and(|allowed| !allowed.contains(name))
    }) {
        return Err(EvidenceError::InvalidCursor);
    }
    Ok(Access {
        grant_id: expected_grant.map(str::to_owned),
        parts,
    })
}

fn load_owned_record(
    connection: &Connection,
    owner: &OwnerId,
    evidence_id: &EvidenceId,
    now: i64,
) -> Result<EvidenceRecord, EvidenceError> {
    let access = authorize(connection, owner, evidence_id, None, now)?;
    if access.grant_id.is_some() {
        return Err(EvidenceError::AccessDenied);
    }
    load_record(connection, evidence_id, None)
}

fn load_record(
    connection: &Connection,
    evidence_id: &EvidenceId,
    allowed_parts: Option<&HashSet<String>>,
) -> Result<EvidenceRecord, EvidenceError> {
    let row: (String, i64, i64, bool, SqlU64, String, String, String) = connection
        .query_row(
            "SELECT owner,created_ms,expires_ms,pinned,reserved_bytes,metadata_json,coverage_json,status FROM observations WHERE id=?1",
            [evidence_id.as_str()],
            |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get(7)?)),
        )
        .map_err(db_error)?;
    let mut statement = connection
        .prepare("SELECT name FROM parts WHERE observation_id=?1 ORDER BY ordinal")
        .map_err(db_error)?;
    let names = statement
        .query_map([evidence_id.as_str()], |row| row.get::<_, String>(0))
        .map_err(db_error)?;
    let mut parts = Vec::new();
    for name in names {
        let name = name.map_err(db_error)?;
        if allowed_parts.is_none_or(|allowed| allowed.contains(&name)) {
            parts.push(load_part_row(connection, evidence_id.as_str(), &name)?);
        }
    }
    Ok(EvidenceRecord {
        evidence_id: evidence_id.clone(),
        owner: OwnerId::parse(row.0)?,
        created_at_ms: row.1,
        expires_at_ms: row.2,
        pinned: row.3,
        bytes: row.4 .0,
        metadata: serde_json::from_str(&row.5).map_err(json_error)?,
        coverage: serde_json::from_str(&row.6).map_err(json_error)?,
        parts,
        complete: row.7 == STATUS_COMPLETE,
    })
}

fn load_part_row(
    connection: &Connection,
    observation_id: &str,
    part: &str,
) -> Result<PartManifest, EvidenceError> {
    let row: Option<PartDbRow> = connection
        .query_row(
            "SELECT content_type,content_encoding,kind,expected_len,expected_sha,layout_json,metadata_json,coverage_json FROM parts WHERE observation_id=?1 AND name=?2",
            params![observation_id, part],
            |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get(7)?)),
        )
        .optional()
        .map_err(db_error)?;
    let (content_type, content_encoding, kind, bytes, sha256, layout, metadata, coverage) =
        row.ok_or(EvidenceError::NotFound)?;
    Ok(PartManifest {
        name: part.to_owned(),
        content_type,
        content_encoding,
        kind: match kind.as_str() {
            "utf8" => PartKind::Utf8,
            "binary" => PartKind::Binary,
            other => {
                return Err(EvidenceError::UnknownRepresentation(other.to_owned()));
            }
        },
        bytes: bytes.0,
        sha256,
        binary_layout: layout
            .map(|value| serde_json::from_str::<BinaryLayout>(&value).map_err(json_error))
            .transpose()?,
        metadata: serde_json::from_str(&metadata).map_err(json_error)?,
        coverage: serde_json::from_str(&coverage).map_err(json_error)?,
    })
}

fn require_upload_owner(
    owner: &OwnerId,
    actual_owner: &OwnerId,
    status: &str,
    expires: i64,
) -> Result<(), EvidenceError> {
    if owner != actual_owner {
        return Err(EvidenceError::AccessDenied);
    }
    if expires <= now_ms()? {
        return Err(EvidenceError::Expired);
    }
    if status != STATUS_UPLOADING {
        return Err(EvidenceError::AlreadyFinalized);
    }
    Ok(())
}

fn require_owned_complete(
    owner: &OwnerId,
    actual_owner: &str,
    status: &str,
    expires: i64,
    now: i64,
) -> Result<(), EvidenceError> {
    if owner.as_str() != actual_owner {
        return Err(EvidenceError::AccessDenied);
    }
    check_readable_status(status, expires, now)
}

fn check_readable_status(status: &str, expires: i64, now: i64) -> Result<(), EvidenceError> {
    match status {
        STATUS_RELEASED => Err(EvidenceError::Released),
        STATUS_EXPIRED => Err(EvidenceError::Expired),
        STATUS_COMPLETE if expires <= now => Err(EvidenceError::Expired),
        STATUS_COMPLETE => Ok(()),
        _ => Err(EvidenceError::Incomplete(
            "observation is not committed".to_owned(),
        )),
    }
}

fn validate_part(part: &PartManifest, max_part_bytes: u64) -> Result<(), EvidenceError> {
    validate_part_name(&part.name)?;
    if part.content_type.is_empty() || part.content_type.len() > 512 {
        return Err(EvidenceError::InvalidInput(
            "content type must be nonempty and at most 512 bytes".to_owned(),
        ));
    }
    if part
        .content_encoding
        .as_ref()
        .is_some_and(|encoding| encoding.len() > 256 || encoding.chars().any(char::is_control))
    {
        return Err(EvidenceError::InvalidInput(
            "content encoding must be at most 256 bytes with no control characters".to_owned(),
        ));
    }
    if part.bytes > max_part_bytes {
        return Err(EvidenceError::QuotaExceeded(
            "part byte limit exceeded".to_owned(),
        ));
    }
    if part.sha256.len() != 64
        || !part
            .sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(EvidenceError::InvalidInput(
            "sha256 must be 64 lowercase hexadecimal characters".to_owned(),
        ));
    }
    Ok(())
}

fn validate_part_name(name: &str) -> Result<(), EvidenceError> {
    if name.is_empty() || name.len() > 256 || name.chars().any(char::is_control) {
        return Err(EvidenceError::InvalidInput(
            "part name must be nonempty, bounded, and contain no control characters".to_owned(),
        ));
    }
    Ok(())
}

fn validate_grant_scope(
    scope: &GrantScope,
    manifests: &[PartManifest],
) -> Result<(), EvidenceError> {
    if let GrantScope::Parts(parts) = scope {
        if parts.is_empty() {
            return Err(EvidenceError::InvalidInput(
                "part-scoped grant cannot be empty".to_owned(),
            ));
        }
        let available: HashSet<_> = manifests.iter().map(|part| part.name.as_str()).collect();
        let mut distinct = HashSet::new();
        for part in parts {
            validate_part_name(part)?;
            if !available.contains(part.as_str()) || !distinct.insert(part) {
                return Err(EvidenceError::InvalidInput(
                    "grant parts must be distinct existing part names".to_owned(),
                ));
            }
        }
    }
    Ok(())
}

fn scope_parts(scope: &GrantScope) -> Option<HashSet<String>> {
    match scope {
        GrantScope::Observation => None,
        GrantScope::Parts(parts) => Some(parts.iter().cloned().collect()),
    }
}

fn configure_connection(connection: &Connection, in_memory: bool) -> Result<(), EvidenceError> {
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .map_err(db_error)?;
    connection
        .busy_timeout(Duration::from_secs(5))
        .map_err(db_error)?;
    if !in_memory {
        let mode: String = connection
            .pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))
            .map_err(db_error)?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err(EvidenceError::Backend(
                "SQLite refused WAL journal mode".to_owned(),
            ));
        }
        connection
            .pragma_update(None, "synchronous", "FULL")
            .map_err(db_error)?;
    }
    Ok(())
}

fn initialize_schema(connection: &Connection) -> Result<(), EvidenceError> {
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(db_error)?;
    if version != 0 && version != SCHEMA_VERSION {
        return Err(EvidenceError::IncompatibleSchema {
            found: version,
            supported: SCHEMA_VERSION,
        });
    }
    if version == SCHEMA_VERSION {
        return Ok(());
    }
    if version == 1 {
        // Bind each retained cursor to the byte budget that produced it so a
        // replay cannot silently return a different page.
        connection
            .execute_batch(
                "BEGIN IMMEDIATE;
                 ALTER TABLE cursors ADD COLUMN page_bytes INTEGER NOT NULL DEFAULT 0;
                 DELETE FROM cursors;
                 PRAGMA user_version=2;
                 COMMIT;",
            )
            .map_err(db_error)?;
        return Ok(());
    }
    connection
        .execute_batch(
            "BEGIN IMMEDIATE;
             CREATE TABLE observations(
               id TEXT PRIMARY KEY, owner TEXT NOT NULL, status TEXT NOT NULL,
               created_ms INTEGER NOT NULL, expires_ms INTEGER NOT NULL,
               pinned INTEGER NOT NULL, reserved_bytes INTEGER NOT NULL,
               metadata_json TEXT NOT NULL, coverage_json TEXT NOT NULL
             );
             CREATE INDEX observations_owner_status ON observations(owner,status);
             CREATE TABLE parts(
               observation_id TEXT NOT NULL REFERENCES observations(id) ON UPDATE CASCADE ON DELETE CASCADE,
               name TEXT NOT NULL, ordinal INTEGER NOT NULL,
               content_type TEXT NOT NULL, content_encoding TEXT,
               kind TEXT NOT NULL, expected_len INTEGER NOT NULL, expected_sha TEXT NOT NULL,
               layout_json TEXT, metadata_json TEXT NOT NULL, coverage_json TEXT NOT NULL,
               finalized INTEGER NOT NULL,
               PRIMARY KEY(observation_id,name), UNIQUE(observation_id,ordinal)
             );
             CREATE TABLE chunks(
               observation_id TEXT NOT NULL, part_name TEXT NOT NULL, ordinal INTEGER NOT NULL,
               offset INTEGER NOT NULL, len INTEGER NOT NULL, blob_hash TEXT NOT NULL,
               PRIMARY KEY(observation_id,part_name,ordinal),
               FOREIGN KEY(observation_id,part_name) REFERENCES parts(observation_id,name) ON UPDATE CASCADE ON DELETE CASCADE
             );
             CREATE INDEX chunks_hash ON chunks(blob_hash);
             CREATE TABLE grants(
               id TEXT PRIMARY KEY, evidence_id TEXT NOT NULL REFERENCES observations(id) ON UPDATE CASCADE ON DELETE CASCADE,
               grantor TEXT NOT NULL, grantee TEXT NOT NULL, scope_json TEXT NOT NULL,
               expires_ms INTEGER NOT NULL, revoked INTEGER NOT NULL
             );
             CREATE INDEX grants_reader ON grants(evidence_id,grantee,expires_ms);
             CREATE TABLE cursors(
               token TEXT PRIMARY KEY, reader TEXT NOT NULL,
               evidence_id TEXT NOT NULL REFERENCES observations(id) ON UPDATE CASCADE ON DELETE CASCADE,
               part TEXT NOT NULL, encoding TEXT NOT NULL, projection TEXT NOT NULL,
               next_offset INTEGER NOT NULL, page_bytes INTEGER NOT NULL,
               grant_id TEXT REFERENCES grants(id) ON DELETE CASCADE
             );
             PRAGMA user_version=2;
             COMMIT;",
        )
        .map_err(db_error)
}

fn expire_rows(
    transaction: &Transaction<'_>,
    now: i64,
    tombstone_cutoff: i64,
) -> Result<(), EvidenceError> {
    transaction
        .execute(
            "DELETE FROM observations WHERE status='uploading' AND expires_ms<=?1",
            [now],
        )
        .map_err(db_error)?;
    transaction
        .execute(
            "DELETE FROM cursors WHERE evidence_id IN (SELECT id FROM observations WHERE status='complete' AND expires_ms<=?1)",
            [now],
        )
        .map_err(db_error)?;
    transaction
        .execute(
            "DELETE FROM chunks WHERE observation_id IN (SELECT id FROM observations WHERE status='complete' AND expires_ms<=?1)",
            [now],
        )
        .map_err(db_error)?;
    transaction
        .execute(
            "UPDATE observations SET status='expired',pinned=0 WHERE status='complete' AND expires_ms<=?1",
            [now],
        )
        .map_err(db_error)?;
    transaction
        .execute("DELETE FROM grants WHERE expires_ms<=?1", [now])
        .map_err(db_error)?;
    transaction
        .execute(
            "DELETE FROM observations WHERE status IN ('expired','released') AND expires_ms<=?1",
            [tombstone_cutoff],
        )
        .map_err(db_error)?;
    Ok(())
}

impl DiskBlobs {
    #[cfg(not(windows))]
    fn open(root: &Path) -> Result<Self, EvidenceError> {
        reject_symlink_components(root)?;
        create_private_directory(root)?;
        let root = fs::canonicalize(root).map_err(io_error)?;
        let metadata = fs::symlink_metadata(&root).map_err(io_error)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(EvidenceError::InvalidInput(
                "evidence root must be a real directory".to_owned(),
            ));
        }
        verify_private_directory(&root)?;
        let lock_path = root.join(".writer.lock");
        let writer_lock = open_private_regular_file(&lock_path)?;
        writer_lock.try_lock().map_err(|error| match error {
            std::fs::TryLockError::WouldBlock => EvidenceError::Conflict,
            std::fs::TryLockError::Error(error) => io_error(error),
        })?;
        let blobs = root.join("blobs");
        let staging = root.join("staging");
        let journals = root.join("exports");
        reject_symlink(&blobs)?;
        reject_symlink(&staging)?;
        reject_symlink(&journals)?;
        create_private_directory(&blobs)?;
        create_private_directory(&staging)?;
        create_private_directory(&journals)?;
        verify_private_directory(&blobs)?;
        verify_private_directory(&staging)?;
        verify_private_directory(&journals)?;
        Ok(Self {
            root,
            blobs,
            staging,
            journals,
            _writer_lock: writer_lock,
        })
    }

    fn blob_path(&self, hash: &str) -> PathBuf {
        self.blobs.join(hash)
    }

    fn journal_path(&self, id: &str) -> PathBuf {
        self.journals.join(format!("{id}.json"))
    }
}

impl BlobStore for DiskBlobs {
    fn put(
        &self,
        hash: &str,
        bytes: &[u8],
        failures: &dyn EvidenceFailureInjector,
    ) -> Result<(), EvidenceError> {
        let target = self.blob_path(hash);
        if target.exists() {
            let expected_len =
                u64::try_from(bytes.len()).map_err(|_| EvidenceError::TransferTooLarge)?;
            let existing = self.get(hash, expected_len)?;
            if existing != bytes || sha256_hex(&existing) != hash {
                return Err(EvidenceError::Corrupt);
            }
            return Ok(());
        }
        failures.check(FailurePoint::BlobWrite)?;
        let temp = self.staging.join(format!("{}.tmp", Uuid::new_v4()));
        let mut file = create_private_file(&temp)?;
        if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
            let _ = fs::remove_file(&temp);
            return Err(io_error(error));
        }
        drop(file);
        if let Err(error) = failures.check(FailurePoint::BlobPublish) {
            let _ = fs::remove_file(&temp);
            return Err(error);
        }
        match rename_noreplace(&temp, &target) {
            Ok(()) => sync_directory(&self.blobs),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let _ = fs::remove_file(&temp);
                let expected_len =
                    u64::try_from(bytes.len()).map_err(|_| EvidenceError::TransferTooLarge)?;
                let existing = self.get(hash, expected_len)?;
                if existing == bytes && sha256_hex(&existing) == hash {
                    Ok(())
                } else {
                    Err(EvidenceError::Corrupt)
                }
            }
            Err(error) => {
                let _ = fs::remove_file(&temp);
                Err(io_error(error))
            }
        }
    }

    fn get(&self, hash: &str, expected_len: u64) -> Result<Vec<u8>, EvidenceError> {
        let path = self.blob_path(hash);
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                EvidenceError::Corrupt
            } else {
                io_error(error)
            }
        })?;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.len() != expected_len
        {
            return Err(EvidenceError::Corrupt);
        }
        let max = usize::try_from(expected_len).map_err(|_| EvidenceError::Corrupt)?;
        read_bounded_file(&path, max)
    }

    fn remove_unreferenced(&self, referenced: &HashSet<String>) -> Result<(), EvidenceError> {
        for entry in fs::read_dir(&self.blobs).map_err(io_error)? {
            let entry = entry.map_err(io_error)?;
            let file_type = entry.file_type().map_err(io_error)?;
            if file_type.is_symlink() || !file_type.is_file() {
                return Err(EvidenceError::Corrupt);
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if !referenced.contains(&name) {
                fs::remove_file(entry.path()).map_err(io_error)?;
            }
        }
        sync_directory(&self.blobs)
    }

    fn cleanup_staging(&self) -> Result<(), EvidenceError> {
        for entry in fs::read_dir(&self.staging).map_err(io_error)? {
            let entry = entry.map_err(io_error)?;
            let file_type = entry.file_type().map_err(io_error)?;
            if file_type.is_symlink() || !file_type.is_file() {
                return Err(EvidenceError::Corrupt);
            }
            fs::remove_file(entry.path()).map_err(io_error)?;
        }
        sync_directory(&self.staging)
    }

    fn reject_export_destination(&self, destination: &Path) -> Result<(), EvidenceError> {
        let parent = destination.parent().ok_or_else(|| {
            EvidenceError::InvalidInput("export destination requires a parent directory".to_owned())
        })?;
        let canonical_parent = fs::canonicalize(parent).map_err(io_error)?;
        if canonical_parent.starts_with(&self.root) {
            return Err(EvidenceError::InvalidInput(
                "export destination may not be inside the evidence store".to_owned(),
            ));
        }
        Ok(())
    }

    fn journal_export(&self, journal: &ExportJournal) -> Result<(), EvidenceError> {
        let path = self.journal_path(&journal.id);
        let body = json_string(journal)?;
        let temp = self.journals.join(format!("{}.tmp", Uuid::new_v4()));
        let mut file = create_private_file(&temp)?;
        if let Err(error) = file
            .write_all(body.as_bytes())
            .and_then(|()| file.sync_all())
        {
            let _ = fs::remove_file(&temp);
            return Err(io_error(error));
        }
        drop(file);
        // The journal is rewritten as the export advances, so this is a
        // replacing rename: a crash leaves either the previous state or the new
        // one, never a truncated file.
        if let Err(error) = rename_replace(&temp, &path) {
            let _ = fs::remove_file(&temp);
            return Err(io_error(error));
        }
        sync_directory(&self.journals)
    }

    fn clear_export_journal(&self, id: &str) -> Result<(), EvidenceError> {
        let path = self.journal_path(id);
        match fs::remove_file(&path) {
            Ok(()) => sync_directory(&self.journals),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(io_error(error)),
        }
    }

    fn list_export_journals(&self) -> Result<Vec<ExportJournal>, EvidenceError> {
        let mut journals: Vec<ExportJournal> = Vec::new();
        for entry in fs::read_dir(&self.journals).map_err(io_error)? {
            let entry = entry.map_err(io_error)?;
            let file_type = entry.file_type().map_err(io_error)?;
            if file_type.is_symlink() || !file_type.is_file() {
                return Err(EvidenceError::Corrupt);
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.ends_with(".json") {
                continue;
            }
            let body = fs::read_to_string(entry.path()).map_err(io_error)?;
            journals.push(serde_json::from_str(&body).map_err(json_error)?);
        }
        journals.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(journals)
    }

    /// Repair one interrupted export from its journal, or fail closed.
    ///
    /// This never guesses: a state it cannot prove is turned into
    /// [`EvidenceError::CommitUncertain`] naming every path involved, so the
    /// caller keeps a user file it can inspect instead of a silent overwrite.
    fn resolve_export_journal(&self, journal: &ExportJournal) -> Result<(), EvidenceError> {
        let destination_intact =
            file_has_content(&journal.destination, journal.bytes, &journal.sha256)?;
        let staged = file_has_content(&journal.temp, journal.bytes, &journal.sha256)?;
        match journal.state {
            // No user-visible rename happened yet, so discarding the staged
            // artifact is a pure rollback.
            ExportJournalState::Prepared => {
                let _ = fs::remove_file(&journal.temp);
                self.clear_export_journal(&journal.id)
            }
            ExportJournalState::Published => {
                if destination_intact {
                    let _ = fs::remove_file(&journal.temp);
                    return self.clear_export_journal(&journal.id);
                }
                if staged {
                    self.publish_staged(&journal.temp, &journal.destination)?;
                    return self.clear_export_journal(&journal.id);
                }
                Err(export_uncertain(
                    journal,
                    "the export destination neither holds the artifact nor can it be republished",
                ))
            }
            ExportJournalState::DestinationBackedUp => {
                // The original is displaced: publish the recorded artifact or
                // put the original back. Never leave the destination missing.
                if staged {
                    self.publish_staged(&journal.temp, &journal.destination)?;
                    return self.clear_export_journal(&journal.id);
                }
                match &journal.backup {
                    Some(backup) if backup.exists() => {
                        rename_noreplace(backup, &journal.destination).map_err(|error| {
                            export_uncertain(
                                journal,
                                &format!("restoring the displaced original failed ({error})"),
                            )
                        })?;
                        if let Some(parent) = journal.destination.parent() {
                            sync_directory(parent).map_err(|error| {
                                export_uncertain(
                                    journal,
                                    &format!("restored original could not be synced ({error})"),
                                )
                            })?;
                        }
                        self.clear_export_journal(&journal.id)
                    }
                    _ => Err(export_uncertain(
                        journal,
                        "the displaced original is gone and the artifact cannot be republished",
                    )),
                }
            }
        }
    }
}

impl DiskBlobs {
    fn publish_staged(&self, temp: &Path, destination: &Path) -> Result<(), EvidenceError> {
        reject_symlink_components(destination)?;
        rename_noreplace(temp, destination).map_err(io_error)?;
        if let Some(parent) = destination.parent() {
            sync_directory(parent)?;
        }
        Ok(())
    }
}

/// True only when `path` is a real regular file whose bytes hash as expected.
///
/// A missing file is `false`, not an error: every caller uses this to decide
/// whether a repair is still possible.
fn file_has_content(
    path: &Path,
    expected_len: u64,
    expected_sha256: &str,
) -> Result<bool, EvidenceError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(io_error(error)),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() != expected_len {
        return Ok(false);
    }
    let max = usize::try_from(expected_len).map_err(|_| EvidenceError::Corrupt)?;
    Ok(sha256_hex(&read_bounded_file(path, max)?) == expected_sha256)
}

fn export_uncertain(journal: &ExportJournal, detail: &str) -> EvidenceError {
    EvidenceError::CommitUncertain(format!(
        "export {} to {}: {detail}; staged artifact at {}, displaced original at {}",
        journal.id,
        journal.destination.display(),
        journal.temp.display(),
        journal
            .backup
            .as_ref()
            .map_or_else(|| "<none>".to_owned(), |path| path.display().to_string()),
    ))
}

impl BlobStore for MemoryBlobs {
    fn put(
        &self,
        hash: &str,
        bytes: &[u8],
        failures: &dyn EvidenceFailureInjector,
    ) -> Result<(), EvidenceError> {
        failures.check(FailurePoint::BlobWrite)?;
        failures.check(FailurePoint::BlobPublish)?;
        let mut values = self
            .values
            .lock()
            .map_err(|_| EvidenceError::Backend("memory blob lock was poisoned".to_owned()))?;
        if let Some(existing) = values.get(hash) {
            if existing != bytes {
                return Err(EvidenceError::Corrupt);
            }
            return Ok(());
        }
        values.insert(hash.to_owned(), bytes.to_vec());
        Ok(())
    }

    fn get(&self, hash: &str, expected_len: u64) -> Result<Vec<u8>, EvidenceError> {
        let bytes = self
            .values
            .lock()
            .map_err(|_| EvidenceError::Backend("memory blob lock was poisoned".to_owned()))?
            .get(hash)
            .cloned()
            .ok_or(EvidenceError::Corrupt)?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) != expected_len {
            return Err(EvidenceError::Corrupt);
        }
        Ok(bytes)
    }

    fn remove_unreferenced(&self, referenced: &HashSet<String>) -> Result<(), EvidenceError> {
        self.values
            .lock()
            .map_err(|_| EvidenceError::Backend("memory blob lock was poisoned".to_owned()))?
            .retain(|hash, _| referenced.contains(hash));
        Ok(())
    }

    fn cleanup_staging(&self) -> Result<(), EvidenceError> {
        Ok(())
    }

    fn reject_export_destination(&self, _destination: &Path) -> Result<(), EvidenceError> {
        Ok(())
    }

    /// The in-memory backend has no durable export to interrupt: the staging and
    /// destination lives only as long as the process, so there is nothing a
    /// later open could find. Recording nothing keeps `MemoryEvidenceStore`
    /// honest instead of pretending it can recover across a restart.
    fn journal_export(&self, _journal: &ExportJournal) -> Result<(), EvidenceError> {
        Ok(())
    }

    fn clear_export_journal(&self, _id: &str) -> Result<(), EvidenceError> {
        Ok(())
    }

    fn list_export_journals(&self) -> Result<Vec<ExportJournal>, EvidenceError> {
        Ok(Vec::new())
    }

    fn resolve_export_journal(&self, journal: &ExportJournal) -> Result<(), EvidenceError> {
        Err(export_uncertain(
            journal,
            "the in-memory evidence store has no durable export journal to reconcile",
        ))
    }
}

#[derive(Default)]
struct Utf8Validator {
    pending: Vec<u8>,
}

impl Utf8Validator {
    fn push(&mut self, bytes: &[u8]) -> Result<(), EvidenceError> {
        let mut combined = Vec::with_capacity(self.pending.len().saturating_add(bytes.len()));
        combined.extend_from_slice(&self.pending);
        combined.extend_from_slice(bytes);
        match std::str::from_utf8(&combined) {
            Ok(_) => self.pending.clear(),
            Err(error) if error.error_len().is_none() => {
                let valid = error.valid_up_to();
                self.pending = combined
                    .get(valid..)
                    .ok_or(EvidenceError::HashMismatch)?
                    .to_vec();
                if self.pending.len() > 3 {
                    return Err(EvidenceError::HashMismatch);
                }
            }
            Err(_) => return Err(EvidenceError::HashMismatch),
        }
        Ok(())
    }

    fn finish(self) -> Result<(), EvidenceError> {
        if self.pending.is_empty() {
            Ok(())
        } else {
            Err(EvidenceError::HashMismatch)
        }
    }
}

fn now_ms() -> Result<i64, EvidenceError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| {
            EvidenceError::Backend(format!("system clock is before Unix epoch: {error}"))
        })?;
    i64::try_from(duration.as_millis())
        .map_err(|_| EvidenceError::Backend("system time exceeds supported range".to_owned()))
}

fn add_duration(now: i64, duration: Duration) -> Result<i64, EvidenceError> {
    let millis = i64::try_from(duration.as_millis())
        .map_err(|_| EvidenceError::InvalidInput("TTL exceeds supported range".to_owned()))?;
    now.checked_add(millis)
        .ok_or_else(|| EvidenceError::InvalidInput("TTL expiry overflows".to_owned()))
}

fn subtract_duration(now: i64, duration: Duration) -> Result<i64, EvidenceError> {
    let millis = i64::try_from(duration.as_millis()).map_err(|_| {
        EvidenceError::InvalidInput("tombstone TTL exceeds supported range".to_owned())
    })?;
    Ok(now.saturating_sub(millis))
}

fn validate_ttl(ttl: Duration, max: Duration) -> Result<(), EvidenceError> {
    if ttl.is_zero() || ttl > max {
        return Err(EvidenceError::InvalidInput(
            "TTL must be nonzero and within the configured maximum".to_owned(),
        ));
    }
    Ok(())
}

fn encoding_name(encoding: ReadEncoding) -> &'static str {
    match encoding {
        ReadEncoding::Utf8 => "utf8",
        ReadEncoding::Base64 => "base64",
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex_digest(Sha256::digest(bytes).as_slice())
}

fn hex_digest(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

fn json_string<T: serde::Serialize>(value: &T) -> Result<String, EvidenceError> {
    serde_json::to_string(value).map_err(json_error)
}

fn optional_json_string<T: serde::Serialize>(
    value: Option<&T>,
) -> Result<Option<String>, EvidenceError> {
    value.map(json_string).transpose()
}

fn json_size_bounded<T: serde::Serialize>(
    value: &T,
    max_bytes: u64,
    field: &str,
) -> Result<u64, EvidenceError> {
    let encoded = serde_json::to_vec(value).map_err(json_error)?;
    let size = u64::try_from(encoded.len()).unwrap_or(u64::MAX);
    if size > max_bytes {
        return Err(EvidenceError::QuotaExceeded(format!(
            "{field} exceeds the configured metadata byte limit"
        )));
    }
    Ok(size)
}

fn json_error(error: serde_json::Error) -> EvidenceError {
    EvidenceError::Backend(format!("stored evidence JSON is invalid: {error}"))
}

fn db_error(error: rusqlite::Error) -> EvidenceError {
    EvidenceError::Backend(error.to_string())
}

fn io_error(error: std::io::Error) -> EvidenceError {
    EvidenceError::Io(error.to_string())
}

fn reject_destination_symlink(path: &Path) -> Result<(), EvidenceError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(EvidenceError::InvalidInput(
            "export destination may not be a symlink".to_owned(),
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error(error)),
    }
}

fn reject_symlink(path: &Path) -> Result<(), EvidenceError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(EvidenceError::InvalidInput(
            "evidence spool paths may not be symlinks".to_owned(),
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error(error)),
    }
}

fn reject_symlink_components(path: &Path) -> Result<(), EvidenceError> {
    for component in path.ancestors() {
        reject_symlink(component)?;
    }
    Ok(())
}

#[cfg(unix)]
fn create_private_directory(path: &Path) -> Result<(), EvidenceError> {
    use std::os::unix::fs::DirBuilderExt;
    let mut builder = fs::DirBuilder::new();
    builder.recursive(false).mode(0o700);
    match builder.create(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o700))
                .map_err(io_error)
        }
        Err(error) => Err(io_error(error)),
    }
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
            "evidence directory is not owner-private: {}",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(unix)]
fn secure_file(path: &Path) -> Result<(), EvidenceError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(io_error)
}

#[cfg(unix)]
fn open_private_regular_file(path: &Path) -> Result<File, EvidenceError> {
    use std::os::unix::fs::OpenOptionsExt;

    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(EvidenceError::InvalidInput(format!(
                    "private store file must be a regular non-symlink file: {}",
                    path.display()
                )));
            }
            let before = identity(path).map_err(io_error)?;
            secure_file(path)?;
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(path)
                .map_err(io_error)?;
            if identity(path).map_err(io_error)? != before {
                return Err(EvidenceError::Io(format!(
                    "private store file identity changed while opening: {}",
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

#[cfg(unix)]
fn secure_sqlite_files(root: &Path) -> Result<(), EvidenceError> {
    for name in ["index.sqlite3", "index.sqlite3-wal", "index.sqlite3-shm"] {
        let path = root.join(name);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                secure_file(&path)?;
            }
            Ok(_) => {
                return Err(EvidenceError::InvalidInput(format!(
                    "SQLite store path must be a regular non-symlink file: {}",
                    path.display()
                )));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error(error)),
        }
    }
    Ok(())
}

#[cfg(unix)]
fn create_private_file(path: &Path) -> Result<File, EvidenceError> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(io_error)
}

#[cfg(windows)]
fn create_private_file(path: &Path) -> Result<File, EvidenceError> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(io_error)
}

fn read_bounded_file(path: &Path, expected_max: usize) -> Result<Vec<u8>, EvidenceError> {
    let file = File::open(path).map_err(io_error)?;
    let limit = u64::try_from(expected_max)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    let mut bytes = Vec::with_capacity(expected_max);
    file.take(limit).read_to_end(&mut bytes).map_err(io_error)?;
    if bytes.len() > expected_max {
        return Err(EvidenceError::Corrupt);
    }
    Ok(bytes)
}

fn sync_directory(path: &Path) -> Result<(), EvidenceError> {
    #[cfg(unix)]
    {
        File::open(path)
            .and_then(|file| file.sync_all())
            .map_err(io_error)
    }
    #[cfg(windows)]
    {
        let _ = path;
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "unit tests")]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;

    fn owner(value: &str) -> OwnerId {
        OwnerId::parse(value).unwrap()
    }

    fn config() -> EvidenceStoreConfig {
        EvidenceStoreConfig {
            max_owner_bytes: 128,
            max_total_bytes: 192,
            max_observation_bytes: 96,
            max_part_bytes: 64,
            max_chunk_bytes: 5,
            max_chunks_per_part: 32,
            max_page_bytes: 16,
            max_pinned_bytes_per_owner: 96,
            max_owner_observations: 8,
            max_total_observations: 16,
            max_parts_per_observation: 4,
            max_metadata_bytes: 1_024,
            max_observation_metadata_bytes: 4_096,
            max_grants_per_evidence: 4,
            max_cursors_per_owner: 32,
            tombstone_ttl: Duration::from_secs(1),
            max_ttl: Duration::from_secs(60),
        }
    }

    fn manifest(name: &str, kind: PartKind, bytes: &[u8]) -> PartManifest {
        PartManifest {
            name: name.to_owned(),
            content_type: if kind == PartKind::Utf8 {
                "application/json; charset=utf-8".to_owned()
            } else {
                "application/octet-stream".to_owned()
            },
            content_encoding: (kind == PartKind::Utf8).then(|| "utf-8".to_owned()),
            kind,
            bytes: bytes.len().try_into().unwrap(),
            sha256: sha256_hex(bytes),
            binary_layout: (kind == PartKind::Binary).then(|| BinaryLayout {
                dtype: "float32".to_owned(),
                endianness: "little".to_owned(),
                dimensions: vec![2],
                strides: Some(vec![4]),
                layout: Some("contiguous".to_owned()),
            }),
            metadata: json!({"zero":0,"false":false,"null":null}),
            coverage: json!({"complete":true}),
        }
    }

    fn reserve_request(parts: Vec<PartManifest>) -> ReserveObservation {
        ReserveObservation {
            ttl: Duration::from_secs(30),
            metadata: json!({"source":"historical","zero":0,"false":false,"null":null}),
            coverage: json!({"captured":"all_requested"}),
            parts,
        }
    }

    fn upload(
        store: &dyn EvidenceStore,
        principal: &OwnerId,
        parts: Vec<(&str, PartKind, &[u8])>,
    ) -> EvidenceRecord {
        let manifests = parts
            .iter()
            .map(|(name, kind, bytes)| manifest(name, *kind, bytes))
            .collect();
        let reservation = store
            .reserve(principal, reserve_request(manifests))
            .unwrap();
        for (name, _, bytes) in parts {
            let mut offset = 0_usize;
            while offset < bytes.len() {
                let end = (offset + 3).min(bytes.len());
                store
                    .append_chunk(
                        principal,
                        AppendChunk {
                            upload_id: &reservation.upload_id,
                            part: name,
                            offset: offset.try_into().unwrap(),
                            bytes: &bytes[offset..end],
                        },
                    )
                    .unwrap();
                offset = end;
            }
            store
                .finalize_part(principal, &reservation.upload_id, name)
                .unwrap();
        }
        store.commit(principal, &reservation.upload_id).unwrap()
    }

    fn read_all(
        store: &dyn EvidenceStore,
        principal: &OwnerId,
        evidence_id: &EvidenceId,
        part: &str,
        encoding: ReadEncoding,
        budget: u64,
    ) -> Vec<u8> {
        let mut cursor = None;
        let mut output = Vec::new();
        loop {
            let page = store
                .read(
                    principal,
                    ReadRequest {
                        evidence_id: evidence_id.clone(),
                        part: part.to_owned(),
                        cursor,
                        max_bytes: budget,
                        encoding,
                    },
                )
                .unwrap();
            match page.content {
                PageContent::Utf8(text) => output.extend_from_slice(text.as_bytes()),
                PageContent::Base64(value) => {
                    output.extend_from_slice(&BASE64_STANDARD.decode(value).unwrap());
                }
            }
            if page.complete {
                break;
            }
            cursor = page.next_cursor;
        }
        output
    }

    #[test]
    fn memory_round_trip_preserves_unicode_binary_metadata_and_layout() {
        let store = MemoryEvidenceStore::new(config()).unwrap();
        let alice = owner("alice");
        let text = "{\"zero\":0,\"false\":false,\"null\":null,\"snow\":\"☃️\"}".as_bytes();
        let binary = [0x00, 0x00, 0xc0, 0x7f, 0x00, 0x00, 0x80, 0x7f];
        let record = upload(
            &store,
            &alice,
            vec![
                ("payload", PartKind::Utf8, text),
                ("source/pixel", PartKind::Binary, &binary),
            ],
        );

        assert!(record.complete);
        assert_eq!(record.metadata["zero"], 0);
        assert_eq!(record.metadata["false"], false);
        assert!(record.metadata["null"].is_null());
        assert_eq!(
            read_all(
                &store,
                &alice,
                &record.evidence_id,
                "payload",
                ReadEncoding::Utf8,
                7
            ),
            text
        );
        assert_eq!(
            read_all(
                &store,
                &alice,
                &record.evidence_id,
                "source/pixel",
                ReadEncoding::Base64,
                5
            ),
            binary
        );
        let inspected = store.inspect(&alice, &record.evidence_id).unwrap();
        assert_eq!(
            inspected.parts[1].binary_layout.as_ref().unwrap().dtype,
            "float32"
        );
    }

    #[test]
    fn cursors_are_stable_and_bound_to_owner_evidence_part_and_encoding() {
        let store = MemoryEvidenceStore::new(config()).unwrap();
        let alice = owner("alice");
        let bob = owner("bob");
        let first = upload(&store, &alice, vec![("a", PartKind::Utf8, b"abcdef")]);
        let second = upload(&store, &alice, vec![("b", PartKind::Utf8, b"uvwxyz")]);
        let first_page = store
            .read(
                &alice,
                ReadRequest {
                    evidence_id: first.evidence_id.clone(),
                    part: "a".to_owned(),
                    cursor: None,
                    max_bytes: 3,
                    encoding: ReadEncoding::Utf8,
                },
            )
            .unwrap();
        let cursor = first_page.next_cursor.unwrap();
        let request = ReadRequest {
            evidence_id: first.evidence_id.clone(),
            part: "a".to_owned(),
            cursor: Some(cursor.clone()),
            max_bytes: 3,
            encoding: ReadEncoding::Utf8,
        };
        assert_eq!(
            store.read(&alice, request.clone()).unwrap(),
            store.read(&alice, request).unwrap()
        );
        assert_eq!(
            store
                .read(
                    &bob,
                    ReadRequest {
                        evidence_id: first.evidence_id.clone(),
                        part: "a".to_owned(),
                        cursor: Some(cursor.clone()),
                        max_bytes: 3,
                        encoding: ReadEncoding::Utf8,
                    },
                )
                .unwrap_err(),
            EvidenceError::InvalidCursor
        );
        assert_eq!(
            store
                .read(
                    &alice,
                    ReadRequest {
                        evidence_id: second.evidence_id,
                        part: "b".to_owned(),
                        cursor: Some(cursor),
                        max_bytes: 3,
                        encoding: ReadEncoding::Utf8,
                    },
                )
                .unwrap_err(),
            EvidenceError::InvalidCursor
        );
        let forged = EvidenceCursor::parse("c_forged").unwrap();
        assert_eq!(
            store
                .read(
                    &alice,
                    ReadRequest {
                        evidence_id: first.evidence_id,
                        part: "a".to_owned(),
                        cursor: Some(forged),
                        max_bytes: 3,
                        encoding: ReadEncoding::Base64,
                    },
                )
                .unwrap_err(),
            EvidenceError::InvalidCursor
        );
    }

    /// A retained cursor is bound to the byte budget that produced it, so a
    /// replay under a different budget is rejected instead of returning a
    /// different page.
    #[test]
    fn a_cursor_replay_under_a_different_byte_budget_is_rejected() {
        let store = MemoryEvidenceStore::new(config()).unwrap();
        let alice = owner("alice");
        let record = upload(&store, &alice, vec![("a", PartKind::Utf8, b"abcdef")]);
        let first_page = store
            .read(
                &alice,
                ReadRequest {
                    evidence_id: record.evidence_id.clone(),
                    part: "a".to_owned(),
                    cursor: None,
                    max_bytes: 3,
                    encoding: ReadEncoding::Utf8,
                },
            )
            .unwrap();
        let cursor = first_page.next_cursor.unwrap();
        assert_eq!(
            store
                .read(
                    &alice,
                    ReadRequest {
                        evidence_id: record.evidence_id,
                        part: "a".to_owned(),
                        cursor: Some(cursor),
                        max_bytes: 6,
                        encoding: ReadEncoding::Utf8,
                    },
                )
                .unwrap_err(),
            EvidenceError::InvalidCursor
        );
    }

    /// Pinning with a shorter remaining time must not shorten an existing lease.
    #[test]
    fn pinning_never_shortens_an_existing_lease() {
        let store = MemoryEvidenceStore::new(config()).unwrap();
        let alice = owner("alice");
        let record = upload(&store, &alice, vec![("a", PartKind::Utf8, b"abcdef")]);
        let long = Duration::from_secs(50);
        let pinned = store.pin(&alice, &record.evidence_id, long).unwrap();
        let second = store
            .pin(&alice, &record.evidence_id, Duration::from_secs(1))
            .unwrap();
        assert!(second.expires_at_ms >= pinned.expires_at_ms);
    }

    /// A reader holding two disjoint part grants sees the union of both scopes
    /// when reading the observation roster.
    #[test]
    fn roster_reads_use_the_union_of_active_grants() {
        let store = MemoryEvidenceStore::new(config()).unwrap();
        let alice = owner("alice");
        let bob = owner("bob");
        let record = upload(
            &store,
            &alice,
            vec![
                ("left", PartKind::Utf8, b"abcdef"),
                ("right", PartKind::Utf8, b"uvwxyz"),
            ],
        );
        for part in ["left", "right"] {
            store
                .grant(
                    &alice,
                    GrantRequest {
                        evidence_id: record.evidence_id.clone(),
                        grantee: bob.clone(),
                        scope: GrantScope::Parts(vec![part.to_owned()]),
                        ttl: Duration::from_secs(10),
                    },
                )
                .unwrap();
        }
        let visible = store.inspect(&bob, &record.evidence_id).unwrap();
        let mut names: Vec<&str> = visible
            .parts
            .iter()
            .map(|part| part.name.as_str())
            .collect();
        names.sort_unstable();
        assert_eq!(names, vec!["left", "right"]);
    }

    /// A part the reader was never granted stays hidden even when another part
    /// is granted.
    #[test]
    fn roster_reads_still_exclude_ungranted_parts() {
        let store = MemoryEvidenceStore::new(config()).unwrap();
        let alice = owner("alice");
        let bob = owner("bob");
        let record = upload(
            &store,
            &alice,
            vec![
                ("left", PartKind::Utf8, b"abcdef"),
                ("secret", PartKind::Utf8, b"uvwxyz"),
            ],
        );
        store
            .grant(
                &alice,
                GrantRequest {
                    evidence_id: record.evidence_id.clone(),
                    grantee: bob.clone(),
                    scope: GrantScope::Parts(vec!["left".to_owned()]),
                    ttl: Duration::from_secs(10),
                },
            )
            .unwrap();
        let visible = store.inspect(&bob, &record.evidence_id).unwrap();
        assert_eq!(visible.parts.len(), 1);
        assert_eq!(visible.parts[0].name, "left");
    }

    #[test]
    fn wrong_owner_hash_only_release_and_grant_revocation_fail_closed() {
        let store = MemoryEvidenceStore::new(config()).unwrap();
        let alice = owner("alice");
        let bob = owner("bob");
        let record = upload(&store, &alice, vec![("payload", PartKind::Utf8, b"secret")]);
        assert_eq!(
            store.inspect(&bob, &record.evidence_id).unwrap_err(),
            EvidenceError::AccessDenied
        );
        let hash_as_id = EvidenceId::parse(record.parts[0].sha256.clone()).unwrap();
        assert_eq!(
            store.inspect(&alice, &hash_as_id).unwrap_err(),
            EvidenceError::NotFound
        );

        let grant = store
            .grant(
                &alice,
                GrantRequest {
                    evidence_id: record.evidence_id.clone(),
                    grantee: bob.clone(),
                    scope: GrantScope::Parts(vec!["payload".to_owned()]),
                    ttl: Duration::from_secs(10),
                },
            )
            .unwrap();
        let page = store
            .read(
                &bob,
                ReadRequest {
                    evidence_id: record.evidence_id.clone(),
                    part: "payload".to_owned(),
                    cursor: None,
                    max_bytes: 3,
                    encoding: ReadEncoding::Utf8,
                },
            )
            .unwrap();
        let cursor = page.next_cursor.unwrap();
        store.revoke_grant(&alice, &grant.grant_id).unwrap();
        assert!(matches!(
            store.read(
                &bob,
                ReadRequest {
                    evidence_id: record.evidence_id.clone(),
                    part: "payload".to_owned(),
                    cursor: Some(cursor),
                    max_bytes: 3,
                    encoding: ReadEncoding::Utf8,
                }
            ),
            Err(EvidenceError::InvalidCursor | EvidenceError::AccessDenied)
        ));
        store.release(&alice, &record.evidence_id).unwrap();
        assert_eq!(
            store.inspect(&alice, &record.evidence_id).unwrap_err(),
            EvidenceError::Released
        );
        assert_eq!(
            store.inspect(&bob, &record.evidence_id).unwrap_err(),
            EvidenceError::Released
        );
    }

    #[test]
    fn partial_transfer_hash_and_duplicate_finalize_are_explicit() {
        let store = MemoryEvidenceStore::new(config()).unwrap();
        let alice = owner("alice");
        let reservation = store
            .reserve(
                &alice,
                reserve_request(vec![manifest("p", PartKind::Binary, b"abcdef")]),
            )
            .unwrap();
        store
            .append_chunk(
                &alice,
                AppendChunk {
                    upload_id: &reservation.upload_id,
                    part: "p",
                    offset: 0,
                    bytes: b"abc",
                },
            )
            .unwrap();
        assert!(matches!(
            store.finalize_part(&alice, &reservation.upload_id, "p"),
            Err(EvidenceError::Incomplete(_))
        ));
        assert_eq!(
            store
                .append_chunk(
                    &alice,
                    AppendChunk {
                        upload_id: &reservation.upload_id,
                        part: "p",
                        offset: 2,
                        bytes: b"def",
                    },
                )
                .unwrap_err(),
            EvidenceError::OffsetMismatch {
                expected: 3,
                actual: 2
            }
        );
        store
            .append_chunk(
                &alice,
                AppendChunk {
                    upload_id: &reservation.upload_id,
                    part: "p",
                    offset: 3,
                    bytes: b"def",
                },
            )
            .unwrap();
        store
            .finalize_part(&alice, &reservation.upload_id, "p")
            .unwrap();
        assert_eq!(
            store
                .finalize_part(&alice, &reservation.upload_id, "p")
                .unwrap_err(),
            EvidenceError::AlreadyFinalized
        );
        store.commit(&alice, &reservation.upload_id).unwrap();
    }

    #[test]
    fn concurrent_reservations_account_for_inflight_quota() {
        let mut limits = config();
        limits.max_owner_bytes = 10;
        limits.max_total_bytes = 10;
        let store = Arc::new(MemoryEvidenceStore::new(limits).unwrap());
        let alice = owner("alice");
        let mut threads = Vec::new();
        for _ in 0..2 {
            let store = Arc::clone(&store);
            let alice = alice.clone();
            threads.push(thread::spawn(move || {
                store.reserve(
                    &alice,
                    reserve_request(vec![manifest("p", PartKind::Binary, b"12345678")]),
                )
            }));
        }
        let results: Vec<_> = threads
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, Err(EvidenceError::QuotaExceeded(_))))
                .count(),
            1
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_second_writer_is_refused_instead_of_racing_reconciliation() {
        let directory = tempfile::tempdir().unwrap();
        let _first = SqliteEvidenceStore::open(directory.path(), config()).unwrap();
        assert!(matches!(
            SqliteEvidenceStore::open(directory.path(), config()),
            Err(EvidenceError::Conflict)
        ));
    }

    /// Quota admission is serialized by IMMEDIATE transactions even when
    /// concurrent callers share a single live store.
    #[cfg(unix)]
    #[test]
    fn concurrent_reservations_serialize_quota_admission() {
        let mut limits = config();
        limits.max_owner_bytes = 10;
        limits.max_total_bytes = 10;
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(SqliteEvidenceStore::open(directory.path(), limits).unwrap());
        let alice = owner("alice");
        let request = reserve_request(vec![manifest("p", PartKind::Binary, b"12345678")]);
        let one_owner = alice.clone();
        let one = {
            let store = Arc::clone(&store);
            thread::spawn(move || store.reserve(&one_owner, request))
        };
        let two = {
            let store = Arc::clone(&store);
            thread::spawn(move || {
                store.reserve(
                    &alice,
                    reserve_request(vec![manifest("p", PartKind::Binary, b"12345678")]),
                )
            })
        };
        let results = [one.join().unwrap(), two.join().unwrap()];
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, Err(EvidenceError::QuotaExceeded(_))))
                .count(),
            1
        );
    }

    struct FailOnce {
        point: FailurePoint,
        remaining: AtomicUsize,
    }

    impl FailOnce {
        fn at(point: FailurePoint) -> Arc<Self> {
            let failure = Self::dormant(point);
            failure.arm();
            failure
        }

        fn dormant(point: FailurePoint) -> Arc<Self> {
            Arc::new(Self {
                point,
                remaining: AtomicUsize::new(0),
            })
        }

        fn arm(&self) {
            self.remaining.store(1, Ordering::SeqCst);
        }
    }

    impl EvidenceFailureInjector for FailOnce {
        fn check(&self, point: FailurePoint) -> Result<(), EvidenceError> {
            if point == self.point
                && self
                    .remaining
                    .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |value| {
                        value.checked_sub(1)
                    })
                    .is_ok()
            {
                Err(EvidenceError::InjectedFailure(format!("{point:?}")))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn injected_chunk_metadata_failure_never_creates_false_complete_handle() {
        let failure = FailOnce::at(FailurePoint::ChunkMetadata);
        let store = MemoryEvidenceStore::new_with_failures(config(), failure).unwrap();
        let alice = owner("alice");
        let reservation = store
            .reserve(
                &alice,
                reserve_request(vec![manifest("p", PartKind::Binary, b"abc")]),
            )
            .unwrap();
        assert!(matches!(
            store.append_chunk(
                &alice,
                AppendChunk {
                    upload_id: &reservation.upload_id,
                    part: "p",
                    offset: 0,
                    bytes: b"abc",
                }
            ),
            Err(EvidenceError::InjectedFailure(_))
        ));
        assert!(matches!(
            store.commit(&alice, &reservation.upload_id),
            Err(EvidenceError::Incomplete(_))
        ));
        store
            .append_chunk(
                &alice,
                AppendChunk {
                    upload_id: &reservation.upload_id,
                    part: "p",
                    offset: 0,
                    bytes: b"abc",
                },
            )
            .unwrap();
        store
            .finalize_part(&alice, &reservation.upload_id, "p")
            .unwrap();
        assert!(
            store
                .commit(&alice, &reservation.upload_id)
                .unwrap()
                .complete
        );
    }

    #[test]
    fn injected_database_finalize_preserves_unrelated_committed_evidence() {
        let failure = FailOnce::dormant(FailurePoint::DatabaseFinalize);
        let store = MemoryEvidenceStore::new_with_failures(config(), failure.clone()).unwrap();
        let alice = owner("alice");
        let baseline = upload(
            &store,
            &alice,
            vec![("baseline", PartKind::Binary, b"safe")],
        );
        let reservation = store
            .reserve(
                &alice,
                reserve_request(vec![manifest("new", PartKind::Binary, b"new")]),
            )
            .unwrap();
        store
            .append_chunk(
                &alice,
                AppendChunk {
                    upload_id: &reservation.upload_id,
                    part: "new",
                    offset: 0,
                    bytes: b"new",
                },
            )
            .unwrap();
        store
            .finalize_part(&alice, &reservation.upload_id, "new")
            .unwrap();
        failure.arm();
        assert!(matches!(
            store.commit(&alice, &reservation.upload_id),
            Err(EvidenceError::InjectedFailure(_))
        ));
        assert_eq!(
            read_all(
                &store,
                &alice,
                &baseline.evidence_id,
                "baseline",
                ReadEncoding::Base64,
                4,
            ),
            b"safe"
        );
        assert!(
            store
                .commit(&alice, &reservation.upload_id)
                .unwrap()
                .complete
        );
    }

    #[cfg(unix)]
    #[test]
    fn disk_publish_failure_cleans_staging_and_retry_succeeds() {
        let directory = tempfile::tempdir().unwrap();
        let failure = FailOnce::at(FailurePoint::BlobPublish);
        let store =
            SqliteEvidenceStore::open_with_failures(directory.path(), config(), failure).unwrap();
        let alice = owner("alice");
        let reservation = store
            .reserve(
                &alice,
                reserve_request(vec![manifest("p", PartKind::Binary, b"abc")]),
            )
            .unwrap();
        let append = || {
            store.append_chunk(
                &alice,
                AppendChunk {
                    upload_id: &reservation.upload_id,
                    part: "p",
                    offset: 0,
                    bytes: b"abc",
                },
            )
        };
        assert!(matches!(append(), Err(EvidenceError::InjectedFailure(_))));
        assert!(fs::read_dir(&store.inner.blobs.staging)
            .unwrap()
            .next()
            .is_none());
        append().unwrap();
        store
            .finalize_part(&alice, &reservation.upload_id, "p")
            .unwrap();
        assert!(
            store
                .commit(&alice, &reservation.upload_id)
                .unwrap()
                .complete
        );
    }

    /// A crash between displacing the destination and publishing the artifact
    /// must not lose the user's file. The next open republishes from the
    /// journal instead of reporting a clean store over a missing destination.
    #[cfg(unix)]
    #[test]
    fn an_interrupted_backup_export_is_repaired_on_reopen() {
        let spool = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        let destination = output.path().join("result.bin");
        fs::write(&destination, b"old").unwrap();
        let failure = FailOnce::at(FailurePoint::ExportReplaceWindow);
        let store =
            SqliteEvidenceStore::open_with_failures(spool.path(), config(), failure).unwrap();
        let alice = owner("alice");
        let record = upload(
            &store,
            &alice,
            vec![("part", PartKind::Binary, b"artifact")],
        );
        assert!(matches!(
            store.export(
                &alice,
                ExportRequest {
                    evidence_id: record.evidence_id,
                    part: "part".to_owned(),
                    destination: destination.clone(),
                    policy: ExportPolicy::BackupAndReplace,
                },
            ),
            Err(EvidenceError::CommitUncertain(_))
        ));
        // The interrupted state is the one that would lose data: the
        // destination is gone and only the displaced original survives.
        assert!(!destination.exists());
        let displaced: Vec<PathBuf> = fs::read_dir(output.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(".tdmcp-backup-"))
            })
            .collect();
        assert_eq!(displaced.len(), 1);
        assert_eq!(fs::read(&displaced[0]).unwrap(), b"old");
        drop(store);

        let reopened = SqliteEvidenceStore::open(spool.path(), config()).unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"artifact");
        assert_eq!(fs::read(&displaced[0]).unwrap(), b"old");
        assert!(reopened
            .inner
            .blobs
            .list_export_journals()
            .unwrap()
            .is_empty());
    }

    /// A crash after the publish rename but before the directory sync is
    /// confirmed: reopening accepts the destination that survived and clears the
    /// journal, rather than re-running the export.
    #[cfg(unix)]
    #[test]
    fn an_interrupted_directory_sync_is_confirmed_on_reopen() {
        let spool = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        let destination = output.path().join("result.bin");
        let failure = FailOnce::at(FailurePoint::DirectorySync);
        let store =
            SqliteEvidenceStore::open_with_failures(spool.path(), config(), failure).unwrap();
        let alice = owner("alice");
        let record = upload(
            &store,
            &alice,
            vec![("part", PartKind::Binary, b"artifact")],
        );
        assert!(matches!(
            store.export(
                &alice,
                ExportRequest {
                    evidence_id: record.evidence_id,
                    part: "part".to_owned(),
                    destination: destination.clone(),
                    policy: ExportPolicy::NoOverwrite,
                },
            ),
            Err(EvidenceError::CommitUncertain(_))
        ));
        assert_eq!(fs::read(&destination).unwrap(), b"artifact");
        assert_eq!(store.inner.blobs.list_export_journals().unwrap().len(), 1);
        drop(store);

        let reopened = SqliteEvidenceStore::open(spool.path(), config()).unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"artifact");
        assert!(reopened
            .inner
            .blobs
            .list_export_journals()
            .unwrap()
            .is_empty());
    }

    /// A journal the store cannot resolve must fail the open. Silently dropping
    /// it would hide a destination the store still owns.
    #[cfg(unix)]
    #[test]
    fn an_unresolvable_export_journal_fails_the_open() {
        let spool = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        let store = SqliteEvidenceStore::open(spool.path(), config()).unwrap();
        let journal = ExportJournal {
            id: "orphaned".to_owned(),
            state: ExportJournalState::DestinationBackedUp,
            temp: output.path().join(".tdmcp-export-gone.tmp"),
            destination: output.path().join("result.bin"),
            backup: Some(output.path().join(".tdmcp-backup-gone")),
            sha256: sha256_hex(b"artifact"),
            bytes: 8,
        };
        fs::write(
            store.inner.blobs.journal_path(&journal.id),
            serde_json::to_vec(&journal).unwrap(),
        )
        .unwrap();
        drop(store);

        assert!(matches!(
            SqliteEvidenceStore::open(spool.path(), config()),
            Err(EvidenceError::CommitUncertain(_))
        ));
    }

    /// The in-memory backend has no durable journal, so it must not pretend a
    /// restart could recover an interrupted export.
    #[test]
    fn the_in_memory_store_records_no_export_journal() {
        let store = MemoryEvidenceStore::new(config()).unwrap();
        let journal = ExportJournal {
            id: "m".to_owned(),
            state: ExportJournalState::Prepared,
            temp: PathBuf::from("/tmp/a"),
            destination: PathBuf::from("/tmp/b"),
            backup: None,
            sha256: sha256_hex(b"x"),
            bytes: 1,
        };
        store.inner.blobs.journal_export(&journal).unwrap();
        assert!(store.inner.blobs.list_export_journals().unwrap().is_empty());
        assert!(matches!(
            store.inner.blobs.resolve_export_journal(&journal),
            Err(EvidenceError::CommitUncertain(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn sqlite_rejects_symlink_roots_and_incompatible_schema_versions() {
        use std::os::unix::fs::symlink;

        let parent = tempfile::tempdir().unwrap();
        let actual = parent.path().join("actual");
        fs::create_dir(&actual).unwrap();
        let linked = parent.path().join("linked");
        symlink(&actual, &linked).unwrap();
        assert!(matches!(
            SqliteEvidenceStore::open(&linked, config()),
            Err(EvidenceError::InvalidInput(_))
        ));

        let incompatible = parent.path().join("incompatible");
        {
            let store = SqliteEvidenceStore::open(&incompatible, config()).unwrap();
            drop(store);
        }
        let connection = Connection::open(incompatible.join("index.sqlite3")).unwrap();
        connection.pragma_update(None, "user_version", 99).unwrap();
        drop(connection);
        assert!(matches!(
            SqliteEvidenceStore::open(&incompatible, config()),
            Err(EvidenceError::IncompatibleSchema {
                found: 99,
                supported: SCHEMA_VERSION
            })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn sqlite_restart_retains_committed_and_cleans_incomplete_orphans() {
        let directory = tempfile::tempdir().unwrap();
        let alice = owner("alice");
        let complete;
        let incomplete;
        let orphan_hash = sha256_hex(b"orph");
        {
            let store = SqliteEvidenceStore::open(directory.path(), config()).unwrap();
            complete = upload(&store, &alice, vec![("kept", PartKind::Binary, b"durable")]);
            incomplete = store
                .reserve(
                    &alice,
                    reserve_request(vec![manifest("lost", PartKind::Binary, b"orphan")]),
                )
                .unwrap();
            store
                .append_chunk(
                    &alice,
                    AppendChunk {
                        upload_id: &incomplete.upload_id,
                        part: "lost",
                        offset: 0,
                        bytes: b"orph",
                    },
                )
                .unwrap();
            assert!(store.blob_path(&orphan_hash).exists());
        }
        let reopened = SqliteEvidenceStore::open(directory.path(), config()).unwrap();
        assert_eq!(
            read_all(
                &reopened,
                &alice,
                &complete.evidence_id,
                "kept",
                ReadEncoding::Base64,
                4
            ),
            b"durable"
        );
        assert_eq!(
            reopened.abort(&alice, &incomplete.upload_id).unwrap_err(),
            EvidenceError::NotFound
        );
        assert!(!reopened.blob_path(&orphan_hash).exists());
    }

    #[cfg(unix)]
    #[test]
    fn sqlite_range_read_detects_corrupt_chunk_without_whole_artifact_allocation() {
        let directory = tempfile::tempdir().unwrap();
        let store = SqliteEvidenceStore::open(directory.path(), config()).unwrap();
        let alice = owner("alice");
        let record = upload(
            &store,
            &alice,
            vec![("part", PartKind::Binary, b"abcdefgh")],
        );
        let hash = sha256_hex(b"abc");
        fs::write(store.blob_path(&hash), b"BAD").unwrap();
        assert_eq!(
            store
                .read(
                    &alice,
                    ReadRequest {
                        evidence_id: record.evidence_id,
                        part: "part".to_owned(),
                        cursor: None,
                        max_bytes: 2,
                        encoding: ReadEncoding::Base64,
                    },
                )
                .unwrap_err(),
            EvidenceError::Corrupt
        );
    }

    #[cfg(unix)]
    #[test]
    fn export_is_no_overwrite_by_default_and_backup_is_explicit() {
        let spool = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        let store = SqliteEvidenceStore::open(spool.path(), config()).unwrap();
        let alice = owner("alice");
        let record = upload(
            &store,
            &alice,
            vec![("part", PartKind::Binary, b"artifact")],
        );
        let destination = output.path().join("result.bin");
        fs::write(&destination, b"old").unwrap();
        assert!(matches!(
            store.export(
                &alice,
                ExportRequest {
                    evidence_id: record.evidence_id.clone(),
                    part: "part".to_owned(),
                    destination: destination.clone(),
                    policy: ExportPolicy::NoOverwrite,
                }
            ),
            Err(EvidenceError::Io(_))
        ));
        assert_eq!(fs::read(&destination).unwrap(), b"old");
        let receipt = store
            .export(
                &alice,
                ExportRequest {
                    evidence_id: record.evidence_id,
                    part: "part".to_owned(),
                    destination: destination.clone(),
                    policy: ExportPolicy::BackupAndReplace,
                },
            )
            .unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"artifact");
        assert_eq!(fs::read(receipt.backup.unwrap()).unwrap(), b"old");
    }

    #[test]
    fn zero_byte_objects_metadata_grants_and_cursors_remain_count_bounded() {
        let mut limits = config();
        limits.max_owner_observations = 1;
        limits.max_total_observations = 1;
        limits.max_grants_per_evidence = 1;
        limits.max_cursors_per_owner = 1;
        let store = MemoryEvidenceStore::new(limits).unwrap();
        let alice = owner("alice");
        let zero = store
            .reserve(
                &alice,
                reserve_request(vec![manifest("empty", PartKind::Binary, b"")]),
            )
            .unwrap();
        assert!(matches!(
            store.reserve(
                &alice,
                reserve_request(vec![manifest("another", PartKind::Binary, b"")])
            ),
            Err(EvidenceError::QuotaExceeded(_))
        ));
        store.abort(&alice, &zero.upload_id).unwrap();

        let oversized = ReserveObservation {
            metadata: json!("x".repeat(2_000)),
            ..reserve_request(vec![manifest("p", PartKind::Binary, b"x")])
        };
        assert!(matches!(
            store.reserve(&alice, oversized),
            Err(EvidenceError::QuotaExceeded(_))
        ));

        let record = upload(&store, &alice, vec![("p", PartKind::Utf8, b"abcdef")]);
        let bob = owner("bob");
        let carol = owner("carol");
        store
            .grant(
                &alice,
                GrantRequest {
                    evidence_id: record.evidence_id.clone(),
                    grantee: bob,
                    scope: GrantScope::Observation,
                    ttl: Duration::from_secs(5),
                },
            )
            .unwrap();
        assert!(matches!(
            store.grant(
                &alice,
                GrantRequest {
                    evidence_id: record.evidence_id.clone(),
                    grantee: carol,
                    scope: GrantScope::Observation,
                    ttl: Duration::from_secs(5),
                }
            ),
            Err(EvidenceError::QuotaExceeded(_))
        ));
        store
            .read(
                &alice,
                ReadRequest {
                    evidence_id: record.evidence_id.clone(),
                    part: "p".to_owned(),
                    cursor: None,
                    max_bytes: 2,
                    encoding: ReadEncoding::Utf8,
                },
            )
            .unwrap();
        assert!(matches!(
            store.read(
                &alice,
                ReadRequest {
                    evidence_id: record.evidence_id,
                    part: "p".to_owned(),
                    cursor: None,
                    max_bytes: 2,
                    encoding: ReadEncoding::Utf8,
                }
            ),
            Err(EvidenceError::QuotaExceeded(_))
        ));
    }

    #[test]
    fn expiry_and_pin_limits_are_explicit() {
        let mut limits = config();
        limits.max_ttl = Duration::from_millis(300);
        let store = MemoryEvidenceStore::new(limits).unwrap();
        let alice = owner("alice");
        let request = ReserveObservation {
            ttl: Duration::from_millis(100),
            ..reserve_request(vec![manifest("p", PartKind::Binary, b"x")])
        };
        let reservation = store.reserve(&alice, request).unwrap();
        store
            .append_chunk(
                &alice,
                AppendChunk {
                    upload_id: &reservation.upload_id,
                    part: "p",
                    offset: 0,
                    bytes: b"x",
                },
            )
            .unwrap();
        store
            .finalize_part(&alice, &reservation.upload_id, "p")
            .unwrap();
        let record = store.commit(&alice, &reservation.upload_id).unwrap();
        thread::sleep(Duration::from_millis(150));
        assert_eq!(
            store.inspect(&alice, &record.evidence_id).unwrap_err(),
            EvidenceError::Expired
        );
        assert!(matches!(
            store.pin(&alice, &record.evidence_id, Duration::from_secs(1)),
            Err(EvidenceError::InvalidInput(_))
        ));
    }

    #[test]
    fn injected_write_publish_and_finalize_failures_are_retryable_without_false_handles() {
        for point in [
            FailurePoint::BlobWrite,
            FailurePoint::BlobPublish,
            FailurePoint::DatabaseFinalize,
        ] {
            let store =
                MemoryEvidenceStore::new_with_failures(config(), FailOnce::at(point)).unwrap();
            let alice = owner("alice");
            let reservation = store
                .reserve(
                    &alice,
                    reserve_request(vec![manifest("p", PartKind::Binary, b"abc")]),
                )
                .unwrap();
            let append = || {
                store.append_chunk(
                    &alice,
                    AppendChunk {
                        upload_id: &reservation.upload_id,
                        part: "p",
                        offset: 0,
                        bytes: b"abc",
                    },
                )
            };
            if point == FailurePoint::DatabaseFinalize {
                append().unwrap();
            } else {
                assert!(matches!(append(), Err(EvidenceError::InjectedFailure(_))));
                append().unwrap();
            }
            store
                .finalize_part(&alice, &reservation.upload_id, "p")
                .unwrap();
            if point == FailurePoint::DatabaseFinalize {
                assert!(matches!(
                    store.commit(&alice, &reservation.upload_id),
                    Err(EvidenceError::InjectedFailure(_))
                ));
            }
            assert!(
                store
                    .commit(&alice, &reservation.upload_id)
                    .unwrap()
                    .complete
            );
        }
    }

    #[test]
    fn finalized_hash_mismatch_does_not_commit() {
        let store = MemoryEvidenceStore::new(config()).unwrap();
        let alice = owner("alice");
        let reservation = store
            .reserve(
                &alice,
                reserve_request(vec![manifest("p", PartKind::Binary, b"abc")]),
            )
            .unwrap();
        store
            .append_chunk(
                &alice,
                AppendChunk {
                    upload_id: &reservation.upload_id,
                    part: "p",
                    offset: 0,
                    bytes: b"abd",
                },
            )
            .unwrap();
        assert_eq!(
            store
                .finalize_part(&alice, &reservation.upload_id, "p")
                .unwrap_err(),
            EvidenceError::HashMismatch
        );
        assert!(matches!(
            store.commit(&alice, &reservation.upload_id),
            Err(EvidenceError::Incomplete(_))
        ));
    }

    #[test]
    fn pin_release_race_is_serialized_and_release_wins_final_state() {
        let store = Arc::new(MemoryEvidenceStore::new(config()).unwrap());
        let alice = owner("alice");
        let record = upload(&*store, &alice, vec![("p", PartKind::Binary, b"data")]);
        let release_store = Arc::clone(&store);
        let release_owner = alice.clone();
        let release_id = record.evidence_id.clone();
        let release = thread::spawn(move || release_store.release(&release_owner, &release_id));
        let pin_store = Arc::clone(&store);
        let pin_owner = alice.clone();
        let pin_id = record.evidence_id.clone();
        let pin =
            thread::spawn(move || pin_store.pin(&pin_owner, &pin_id, Duration::from_secs(20)));
        assert!(release.join().unwrap().is_ok());
        let pin_result = pin.join().unwrap();
        assert!(pin_result.is_ok() || matches!(pin_result, Err(EvidenceError::Released)));
        assert_eq!(
            store.inspect(&alice, &record.evidence_id).unwrap_err(),
            EvidenceError::Released
        );
    }
}

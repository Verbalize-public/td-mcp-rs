//! Typed port for bounded, immutable historical evidence.
//!
//! This module deliberately defines storage operations only. Tool dispatch and
//! producer/transport integration live at the daemon composition boundary.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;
use thiserror::Error;

const MAX_OPAQUE_ID_BYTES: usize = 256;

macro_rules! opaque_id {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::parse(value).map_err(serde::de::Error::custom)
            }
        }

        impl $name {
            /// Validate and construct an opaque identifier received at a trust boundary.
            pub fn parse(value: impl Into<String>) -> Result<Self, EvidenceError> {
                let value = value.into();
                if value.is_empty()
                    || value.len() > MAX_OPAQUE_ID_BYTES
                    || value.chars().any(char::is_control)
                {
                    return Err(EvidenceError::InvalidInput(format!(
                        "{} must be nonempty, bounded, and contain no control characters",
                        stringify!($name)
                    )));
                }
                Ok(Self(value))
            }

            /// Return the opaque wire value.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

opaque_id!(
    OwnerId,
    "Explicit transport-authenticated evidence principal."
);
opaque_id!(EvidenceId, "Opaque identifier of a committed observation.");
opaque_id!(UploadId, "Opaque identifier of an incomplete upload.");
opaque_id!(GrantId, "Opaque identifier of an access grant.");
opaque_id!(
    EvidenceCursor,
    "Opaque cursor bound to one authorized read position."
);

/// Logical nature of a retained part.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PartKind {
    /// Bytes are required to be valid UTF-8 and pages preserve code-point boundaries.
    Utf8,
    /// Arbitrary exact bytes.
    Binary,
}

/// Optional exact layout information for numeric or image bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BinaryLayout {
    /// Producer dtype token, retained verbatim.
    pub dtype: String,
    /// Producer byte-order token, retained verbatim.
    pub endianness: String,
    /// Logical dimensions in producer order.
    pub dimensions: Vec<u64>,
    /// Optional byte strides in producer order.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strides: Option<Vec<i64>>,
    /// Optional producer layout token, retained verbatim.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layout: Option<String>,
}

/// Immutable manifest for one part, declared before transfer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartManifest {
    /// Opaque part name. It is never interpreted as a filesystem path.
    pub name: String,
    /// Exact media/content type supplied by the producer.
    pub content_type: String,
    /// Exact optional content-encoding label supplied by the producer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_encoding: Option<String>,
    /// Whether paging is UTF-8 text or binary.
    pub kind: PartKind,
    /// Exact expected byte count.
    pub bytes: u64,
    /// Lowercase hexadecimal SHA-256 of the complete exact bytes.
    pub sha256: String,
    /// Optional numeric/image byte layout.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binary_layout: Option<BinaryLayout>,
    /// Inert producer metadata retained with this part.
    #[serde(default)]
    pub metadata: Value,
    /// Explicit captured coverage/limits for this part.
    #[serde(default)]
    pub coverage: Value,
}

/// Reservation request for a complete immutable observation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReserveObservation {
    /// Requested lease lifetime. The backend clamps only by rejecting values above its limit.
    #[serde(with = "duration_millis")]
    pub ttl: Duration,
    /// Inert observation metadata.
    #[serde(default)]
    pub metadata: Value,
    /// Explicit captured coverage/limits.
    #[serde(default)]
    pub coverage: Value,
    /// Complete part manifest; all logical bytes are reserved before transfer.
    pub parts: Vec<PartManifest>,
}

/// Successful reservation, not yet readable as committed evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reservation {
    /// Upload token scoped to its owner.
    pub upload_id: UploadId,
    /// Total admitted logical bytes.
    pub reserved_bytes: u64,
    /// Maximum bytes accepted by one transfer call.
    pub max_chunk_bytes: u64,
}

/// Request to append one strictly ordered chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppendChunk<'a> {
    /// Incomplete upload token.
    pub upload_id: &'a UploadId,
    /// Opaque manifest part name.
    pub part: &'a str,
    /// Exact next byte offset; gaps and overlap are rejected.
    pub offset: u64,
    /// Exact bytes to append.
    pub bytes: &'a [u8],
}

/// Progress after a successfully durable chunk metadata write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferProgress {
    /// Next required byte offset.
    pub next_offset: u64,
    /// Expected total part bytes.
    pub total_bytes: u64,
}

/// Complete immutable observation metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceRecord {
    /// Opaque observation identifier.
    pub evidence_id: EvidenceId,
    /// Owner that admitted and committed the observation.
    pub owner: OwnerId,
    /// Creation time as Unix epoch milliseconds.
    pub created_at_ms: i64,
    /// Current lease expiry as Unix epoch milliseconds.
    pub expires_at_ms: i64,
    /// Whether the lease has been explicitly pinned/extended.
    pub pinned: bool,
    /// Total logical bytes across all parts.
    pub bytes: u64,
    /// Inert observation metadata.
    pub metadata: Value,
    /// Explicit captured coverage/limits.
    pub coverage: Value,
    /// Authorized immutable parts in manifest order.
    pub parts: Vec<PartManifest>,
    /// `true` because records are returned only after durable publication and commit.
    pub complete: bool,
}

/// Requested wire representation for one page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadEncoding {
    /// UTF-8 text. Valid only for [`PartKind::Utf8`].
    Utf8,
    /// Standard padded base64 carrying exact bytes.
    Base64,
}

/// Narrow immutable page request. `max_bytes` counts decoded content bytes only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadRequest {
    /// Evidence identifier (still authorization-checked; a hash is never sufficient).
    pub evidence_id: EvidenceId,
    /// Opaque part name.
    pub part: String,
    /// Cursor returned by a prior page, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<EvidenceCursor>,
    /// Maximum decoded content bytes in this page.
    pub max_bytes: u64,
    /// Requested page representation.
    pub encoding: ReadEncoding,
}

/// Page content in the requested representation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "encoding", content = "data", rename_all = "snake_case")]
pub enum PageContent {
    /// A valid UTF-8 fragment ending on a code-point boundary.
    Utf8(String),
    /// Standard padded base64 of exact bytes.
    Base64(String),
}

/// One bounded immutable evidence page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidencePage {
    /// Evidence identifier.
    pub evidence_id: EvidenceId,
    /// Part name.
    pub part: String,
    /// Exact content type.
    pub content_type: String,
    /// Exact optional producer encoding label.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_encoding: Option<String>,
    /// Zero-based decoded byte start, inclusive.
    pub byte_start: u64,
    /// Zero-based decoded byte end, exclusive.
    pub byte_end: u64,
    /// Exact total decoded bytes.
    pub total_bytes: u64,
    /// Decoded bytes returned; framing/base64 expansion is excluded.
    pub returned_bytes: u64,
    /// Encoded page content.
    pub content: PageContent,
    /// Cursor for the next stable page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<EvidenceCursor>,
    /// Whether this page reaches the immutable end.
    pub complete: bool,
    /// Binary layout retained from the manifest, when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binary_layout: Option<BinaryLayout>,
}

/// Scope of a cross-owner grant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantScope {
    /// Read metadata and every part.
    Observation,
    /// Read metadata and only these exact part names.
    Parts(Vec<String>),
}

/// Request to grant bounded read access to another explicit owner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrantRequest {
    /// Evidence owned by the caller.
    pub evidence_id: EvidenceId,
    /// Explicit grantee principal.
    pub grantee: OwnerId,
    /// Authorized scope.
    pub scope: GrantScope,
    /// Grant lifetime, which may not exceed the evidence lease.
    #[serde(with = "duration_millis")]
    pub ttl: Duration,
}

/// Created evidence grant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceGrant {
    /// Opaque revocation token.
    pub grant_id: GrantId,
    /// Granted evidence.
    pub evidence_id: EvidenceId,
    /// Grant owner.
    pub grantor: OwnerId,
    /// Explicit grantee.
    pub grantee: OwnerId,
    /// Authorized scope.
    pub scope: GrantScope,
    /// Absolute Unix epoch millisecond expiry.
    pub expires_at_ms: i64,
}

/// Export collision behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportPolicy {
    /// Fail if the destination exists.
    NoOverwrite,
    /// Move an existing regular file to a unique sibling backup before publication.
    BackupAndReplace,
}

/// Explicit export request for one exact part.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportRequest {
    /// Evidence identifier.
    pub evidence_id: EvidenceId,
    /// Exact part name.
    pub part: String,
    /// Caller-supplied destination. It is a write destination, never a read source.
    pub destination: PathBuf,
    /// Collision behavior.
    pub policy: ExportPolicy,
}

/// Completed export receipt. Exported files are outside evidence garbage collection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportReceipt {
    /// Destination written.
    pub destination: PathBuf,
    /// Existing destination backup, when requested and present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backup: Option<PathBuf>,
    /// Exact bytes exported.
    pub bytes: u64,
    /// SHA-256 of the exported bytes.
    pub sha256: String,
}

/// Typed evidence storage failures.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum EvidenceError {
    /// Invalid or unsafe caller input.
    #[error("invalid evidence input: {0}")]
    InvalidInput(String),
    /// The opaque object does not exist.
    #[error("evidence object not found")]
    NotFound,
    /// The explicit owner is not authorized.
    #[error("evidence access denied")]
    AccessDenied,
    /// The observation lease has expired.
    #[error("evidence has expired")]
    Expired,
    /// The observation was explicitly released.
    #[error("evidence was released")]
    Released,
    /// Admission would exceed a configured logical quota.
    #[error("evidence quota exceeded: {0}")]
    QuotaExceeded(String),
    /// One transfer or page exceeds its configured bound.
    #[error("evidence transfer exceeds configured bound")]
    TransferTooLarge,
    /// A chunk was not supplied at the exact required offset.
    #[error("chunk offset mismatch: expected {expected}, got {actual}")]
    OffsetMismatch {
        /// Required next offset.
        expected: u64,
        /// Supplied offset.
        actual: u64,
    },
    /// Declared and observed content hashes differ.
    #[error("evidence hash mismatch")]
    HashMismatch,
    /// Stored immutable bytes failed an integrity check.
    #[error("stored evidence is corrupt")]
    Corrupt,
    /// Stored metadata uses a representation this build does not recognize.
    ///
    /// Unknown kinds fail closed instead of being mapped onto a known
    /// representation, so a newer or corrupted store cannot be silently
    /// reinterpreted as binary content.
    #[error("stored evidence uses an unrecognized representation: {0}")]
    UnknownRepresentation(String),
    /// Cursor is unknown, forged, expired, or bound to another read.
    #[error("invalid evidence cursor")]
    InvalidCursor,
    /// A finalize/commit operation was repeated or conflicts with state.
    #[error("evidence object is already finalized")]
    AlreadyFinalized,
    /// Upload is missing bytes or finalized parts.
    #[error("evidence upload is incomplete: {0}")]
    Incomplete(String),
    /// A concurrent state transition won the race.
    #[error("evidence state conflict")]
    Conflict,
    /// Filesystem operation failed.
    #[error("evidence I/O failed: {0}")]
    Io(String),
    /// SQLite or in-memory metadata operation failed.
    #[error("evidence backend failed: {0}")]
    Backend(String),
    /// A commit may have succeeded but its result could not be established.
    #[error("evidence commit outcome is uncertain: {0}")]
    CommitUncertain(String),
    /// On-disk schema is newer or otherwise incompatible.
    #[error("incompatible evidence schema version {found}; supported version is {supported}")]
    IncompatibleSchema {
        /// Observed schema version.
        found: i64,
        /// Supported schema version.
        supported: i64,
    },
    /// Backend cannot enforce its privacy contract on this platform.
    #[error("evidence backend unsupported on this platform: {0}")]
    UnsupportedPlatform(String),
    /// Deterministic failure injected at a named durability boundary.
    #[error("injected evidence failure at {0}")]
    InjectedFailure(String),
}

/// Synchronous evidence storage port.
///
/// Implementations may perform blocking disk/database work. Async callers must
/// invoke them off executor and TD main threads (for example with
/// `tokio::task::spawn_blocking`). Every client operation requires an explicit
/// owner; implementations must not infer one from a PID, hash, or global state.
pub trait EvidenceStore: Send + Sync {
    /// Atomically reserve all logical bytes before producer transfer begins.
    fn reserve(
        &self,
        owner: &OwnerId,
        request: ReserveObservation,
    ) -> Result<Reservation, EvidenceError>;

    /// Append one bounded, strictly ordered chunk.
    fn append_chunk(
        &self,
        owner: &OwnerId,
        request: AppendChunk<'_>,
    ) -> Result<TransferProgress, EvidenceError>;

    /// Verify exact length/hash (and UTF-8 when declared) and finalize one part.
    fn finalize_part(
        &self,
        owner: &OwnerId,
        upload_id: &UploadId,
        part: &str,
    ) -> Result<(), EvidenceError>;

    /// Commit an observation only after every part is finalized and readable.
    fn commit(
        &self,
        owner: &OwnerId,
        upload_id: &UploadId,
    ) -> Result<EvidenceRecord, EvidenceError>;

    /// Abort an incomplete upload and release its reservation.
    fn abort(&self, owner: &OwnerId, upload_id: &UploadId) -> Result<(), EvidenceError>;

    /// Inspect authorized historical metadata without touching a live producer.
    fn inspect(
        &self,
        owner: &OwnerId,
        evidence_id: &EvidenceId,
    ) -> Result<EvidenceRecord, EvidenceError>;

    /// Read one bounded stable page from committed bytes.
    fn read(&self, owner: &OwnerId, request: ReadRequest) -> Result<EvidencePage, EvidenceError>;

    /// Extend/pin an owned lease by a bounded duration.
    fn pin(
        &self,
        owner: &OwnerId,
        evidence_id: &EvidenceId,
        ttl: Duration,
    ) -> Result<EvidenceRecord, EvidenceError>;

    /// Release owned evidence and invalidate grants/cursors.
    fn release(&self, owner: &OwnerId, evidence_id: &EvidenceId) -> Result<(), EvidenceError>;

    /// Create an explicit, expiring cross-owner read grant.
    fn grant(&self, owner: &OwnerId, request: GrantRequest)
        -> Result<EvidenceGrant, EvidenceError>;

    /// Revoke a grant owned by the caller and invalidate its future reads.
    fn revoke_grant(&self, owner: &OwnerId, grant_id: &GrantId) -> Result<(), EvidenceError>;

    /// Export one authorized immutable part to an explicit destination.
    fn export(
        &self,
        owner: &OwnerId,
        request: ExportRequest,
    ) -> Result<ExportReceipt, EvidenceError>;
}

mod duration_millis {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    pub fn serialize<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u64(duration.as_millis().try_into().unwrap_or(u64::MAX))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        u64::deserialize(deserializer).map(Duration::from_millis)
    }
}

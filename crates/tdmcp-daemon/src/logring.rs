//! Bounded in-memory log ring (tail buffer for admin/GUI consumers) and the
//! on-disk retention sweep. Capacity 2048 matches the GUI render cap so a
//! full ring is always fully displayable.

use std::collections::VecDeque;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use tracing_appender::non_blocking::NonBlocking;

use crate::logrecord::{Level, Record, Src};

/// Records kept in memory (matches the GUI local render cap).
pub const RING_CAPACITY: usize = 2048;

/// One day, for the periodic sweep cadence.
const SWEEP_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

struct Inner {
    queue: VecDeque<std::sync::Arc<Record>>,
    /// Highest sequence ever assigned; monotonic across eviction.
    next_seq: u64,
}

/// Bounded FIFO of log records with daemon-assigned monotonic `seq`.
///
/// Sequence assignment and insertion happen under one lock, so deque order
/// always equals seq order.
pub struct LogRing {
    inner: Mutex<Inner>,
    capacity: usize,
}

impl LogRing {
    /// New ring holding at most [`RING_CAPACITY`] records by default.
    ///
    /// `seq` starts at 1, not 0: callers query with `after=0` for "from the
    /// beginning", so seq 0 must never be assigned to a real record or it
    /// would be permanently unreachable through that cursor.
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Mutex::new(Inner {
                queue: VecDeque::with_capacity(capacity.min(4096)),
                next_seq: 1,
            }),
            capacity,
        }
    }

    /// Assign the next `seq`, append, evict oldest beyond capacity.
    pub fn push(&self, mut record: Record) -> std::sync::Arc<Record> {
        let mut inner = self.lock();
        record.seq = inner.next_seq;
        inner.next_seq += 1;
        let arc = std::sync::Arc::new(record);
        inner.queue.push_back(arc.clone());
        while inner.queue.len() > self.capacity {
            inner.queue.pop_front();
        }
        arc
    }

    /// Records with `seq > after` (oldest first), server-side filtered by
    /// level/src, plus the highest seq observed (cursor even when empty).
    pub fn snapshot_after(
        &self,
        after: u64,
        limit: usize,
        min_level: Option<Level>,
        srcs: &[Src],
    ) -> (Vec<std::sync::Arc<Record>>, u64) {
        let inner = self.lock();
        let cursor = inner.next_seq.saturating_sub(1);
        let records = inner
            .queue
            .iter()
            .filter(|r| r.seq > after)
            .filter(|r| match min_level {
                Some(min) => r.level >= min,
                None => true,
            })
            .filter(|r| srcs.is_empty() || srcs.contains(&r.src))
            .take(limit)
            .cloned()
            .collect();
        (records, cursor)
    }

    /// Current live length (status badge hint).
    pub fn path_hint(&self) -> usize {
        self.lock().queue.len()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        // A panicking holder must not take logging down with it; recover the
        // (structurally consistent) data instead of poisoning forever.
        self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Shared handle for persisting one record to both the ring and the rotating
/// JSONL file. Cheap to clone (Arc ring + a `tracing_appender::NonBlocking`
/// writer handle, itself cloneable). The single write path used by the
/// tracing sink layer and by bridge/proxy log ingestion alike.
#[derive(Clone)]
pub struct LogSink {
    ring: Arc<LogRing>,
    writer: NonBlocking,
}

impl LogSink {
    /// New sink writing into `ring` and appending JSONL lines via `writer`.
    pub fn new(ring: Arc<LogRing>, writer: NonBlocking) -> Self {
        Self { ring, writer }
    }

    /// The backing ring (for admin/GUI reads).
    pub fn ring(&self) -> &Arc<LogRing> {
        &self.ring
    }

    /// Assign `seq` via the ring, then append the JSONL line to the file.
    /// Never panics; a file-write failure is dropped silently (the ring copy
    /// still stands, and the stderr fmt layer is unaffected).
    pub fn push(&self, record: Record) -> Arc<Record> {
        let arc = self.ring.push(record);
        let line = crate::logrecord::to_line(&arc);
        let mut writer = self.writer.clone();
        let _ = writer.write_all(line.as_bytes());
        let _ = writer.write_all(b"\n");
        arc
    }
}

/// Delete rotated `*.log*` files older than `retention_days` under `log_dir`
/// and any legacy `{data_dir}/daemon.log`. Returns removed file count.
pub fn sweep_logs(log_dir: &Path, data_dir: &Path, retention_days: u32) -> usize {
    let now = SystemTime::now();
    let mut removed = 0;

    if let Ok(entries) = fs::read_dir(log_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !is_rotated_log_name(&path) {
                continue;
            }
            let modified_ok = entry
                .metadata()
                .and_then(|m| m.modified())
                .map(|modified| is_older_than(modified, now, retention_days))
                .unwrap_or(false);
            if modified_ok && fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
    }

    // One-time migration: pre-sink daemons appended to this file directly.
    let legacy = data_dir.join("daemon.log");
    if fs::remove_file(&legacy).is_ok() {
        removed += 1;
    }
    removed
}

/// Run one sweep immediately, then every 24 h until `shutdown` fires.
/// Intended to be spawned onto the daemon runtime after tracing init.
pub async fn run_sweep_loop(
    log_dir: std::path::PathBuf,
    data_dir: std::path::PathBuf,
    retention_days: u32,
    shutdown: tokio_util::sync::CancellationToken,
) {
    let mut interval = tokio::time::interval(SWEEP_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        let removed = sweep_logs(&log_dir, &data_dir, retention_days);
        if removed > 0 {
            tracing::info!(removed, dir = %log_dir.display(), "log retention swept");
        }
        tokio::select! {
            _ = interval.tick() => {}
            _ = shutdown.cancelled() => break,
        }
    }
}

/// Cap on one record's `msg` (bytes) — a runaway bridge print must not blow
/// the ring or the file up.
const MAX_BRIDGE_MSG_BYTES: usize = 64 * 1024;
/// Cap on records ingested from one `log` event batch.
const MAX_BRIDGE_BATCH: usize = 256;

/// Ingest one bridge `Message::Event{name:"log"}` payload
/// (`{"records":[{level,target,msg,kvs?,code?,ts?}, ...]}`). `pid` comes from
/// the handshake identity, never trusted from the payload. Malformed entries
/// are skipped, not fatal. Returns the number of records ingested.
pub fn ingest_bridge_logs(pid: u32, payload: &serde_json::Value, sink: &LogSink) -> usize {
    let Some(records) = payload.get("records").and_then(|v| v.as_array()) else {
        return 0;
    };
    let mut ingested = 0;
    for entry in records.iter().take(MAX_BRIDGE_BATCH) {
        let Some(record) = bridge_entry_to_record(pid, entry) else {
            continue;
        };
        sink.push(record);
        ingested += 1;
    }
    ingested
}

fn bridge_entry_to_record(pid: u32, entry: &serde_json::Value) -> Option<Record> {
    let obj = entry.as_object()?;
    let level = obj
        .get("level")
        .and_then(|v| v.as_str())
        .and_then(|s| serde_json::from_value(serde_json::Value::String(s.to_owned())).ok())
        .unwrap_or(Level::Info);
    let target = obj
        .get("target")
        .and_then(|v| v.as_str())
        .unwrap_or("bridge")
        .to_owned();
    let mut msg = obj.get("msg").and_then(|v| v.as_str()).unwrap_or("").to_owned();
    truncate_to_byte_cap(&mut msg, MAX_BRIDGE_MSG_BYTES);
    let code = obj.get("code").and_then(|v| v.as_str()).map(str::to_owned);
    let mut kvs: std::collections::BTreeMap<String, String> = obj
        .get("kvs")
        .and_then(|v| v.as_object())
        .map(|m| {
            m.iter()
                .map(|(k, v)| (k.clone(), json_value_to_string(v)))
                .collect()
        })
        .unwrap_or_default();
    if let Some(ts) = obj.get("ts").and_then(|v| v.as_str()) {
        kvs.insert("sentTs".to_owned(), ts.to_owned());
    }
    Some(Record {
        seq: 0, // assigned by the ring on push
        ts: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        level,
        src: Src::Bridge,
        pid,
        target,
        msg,
        code,
        kvs,
    })
}

fn json_value_to_string(v: &serde_json::Value) -> String {
    match v.as_str() {
        Some(s) => s.to_owned(),
        None => v.to_string(),
    }
}

fn truncate_to_byte_cap(s: &mut String, max_bytes: usize) {
    if s.len() <= max_bytes {
        return;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s.truncate(end);
}

fn is_rotated_log_name(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    name.starts_with("daemon.") && name.contains(".log")
}

fn is_older_than(modified: SystemTime, now: SystemTime, retention_days: u32) -> bool {
    match now.duration_since(modified) {
        Ok(age) => age >= Duration::from_secs(u64::from(retention_days) * 24 * 60 * 60),
        Err(_) => false,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;
    use crate::logrecord::Record;
    use std::collections::BTreeMap;

    fn rec(msg: &str) -> Record {
        Record {
            seq: 0,
            ts: "2026-01-01T12:00:00.000Z".to_owned(),
            level: Level::Info,
            src: Src::Daemon,
            pid: 1,
            target: "test".to_owned(),
            msg: msg.to_owned(),
            code: None,
            kvs: BTreeMap::new(),
        }
    }

    #[test]
    fn seq_is_monotonic_across_eviction() {
        let ring = LogRing::new(3);
        let mut last = None;
        for i in 0..10 {
            last = Some(ring.push(rec(&format!("m{i}"))));
        }
        let last = last.expect("pushed");
        assert_eq!(last.seq, 10);
        assert_eq!(ring.path_hint(), 3);
        // Oldest survivor is seq 8.
        let (all, cursor) = ring.snapshot_after(0, 512, None, &[]);
        assert_eq!(all.iter().map(|r| r.seq).collect::<Vec<_>>(), vec![8, 9, 10]);
        assert_eq!(cursor, 10);
    }

    #[test]
    fn first_ever_record_is_reachable_from_after_zero() {
        // seq must start at 1: an `after=0` query is "from the beginning",
        // so seq 0 can never be assigned to a real record.
        let ring = LogRing::new(8);
        let pushed = ring.push(rec("a"));
        assert_eq!(pushed.seq, 1);
        let (recs, cursor) = ring.snapshot_after(0, 8, None, &[]);
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].seq, 1);
        assert_eq!(cursor, 1);
    }

    #[test]
    fn snapshot_cursor_advances_even_when_empty() {
        let ring = LogRing::new(8);
        assert_eq!(ring.snapshot_after(0, 8, None, &[]).1, 0, "nothing pushed yet");
        ring.push(rec("a"));
        let (_, first) = ring.snapshot_after(0, 8, None, &[]);
        assert_eq!(first, 1);
        let (empty, second) = ring.snapshot_after(first, 8, None, &[]);
        assert!(empty.is_empty());
        assert_eq!(second, 1);
    }

    #[test]
    fn snapshot_filters_by_level_and_src() {
        let ring = LogRing::new(16);
        let mut warn_bridge = rec("w");
        warn_bridge.level = Level::Warn;
        warn_bridge.src = Src::Bridge;
        ring.push(rec("i1"));
        ring.push(warn_bridge);
        ring.push(rec("i2"));

        let (warns, _) = ring.snapshot_after(0, 512, Some(Level::Warn), &[]);
        assert_eq!(warns.len(), 1);
        assert_eq!(warns[0].msg, "w");

        let (bridges, _) = ring.snapshot_after(0, 512, None, &[Src::Bridge]);
        assert_eq!(bridges.len(), 1);

        let (none, _) = ring.snapshot_after(0, 512, Some(Level::Error), &[]);
        assert!(none.is_empty());
    }

    #[test]
    fn sweep_removes_stale_and_legacy_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log_dir = dir.path().join("logs");
        let data_dir = dir.path().join("data");
        std::fs::create_dir_all(&log_dir).expect("mkdir");
        std::fs::create_dir_all(&data_dir).expect("mkdir");

        // Fresh rotated file survives; legacy root daemon.log always goes.
        std::fs::write(log_dir.join("daemon.2999-01-01.log"), "{}\n").expect("fresh");
        std::fs::write(data_dir.join("daemon.log"), "legacy\n").expect("legacy");
        assert_eq!(sweep_logs(&log_dir, &data_dir, 30), 1);
        assert!(log_dir.join("daemon.2999-01-01.log").exists());
        assert!(!data_dir.join("daemon.log").exists());

        // Unrelated files are untouched.
        std::fs::write(log_dir.join("other.txt"), "keep").expect("other");
        std::fs::write(data_dir.join("config.toml"), "keep").expect("cfg");
        assert_eq!(sweep_logs(&log_dir, &data_dir, 30), 0);
        assert!(log_dir.join("other.txt").exists());
    }

    #[test]
    fn age_predicate_matches_cutoff() {
        let now = SystemTime::now();
        assert!(is_older_than(now - Duration::from_secs(31 * 24 * 3600), now, 30));
        assert!(!is_older_than(now - Duration::from_secs(3600), now, 30));
        // Future mtimes are never stale.
        assert!(!is_older_than(now + Duration::from_secs(3600), now, 30));
    }

    fn test_sink(dir: &Path) -> (LogSink, tracing_appender::non_blocking::WorkerGuard) {
        let appender = tracing_appender::rolling::never(dir, "test.log");
        let (writer, guard) = tracing_appender::non_blocking(appender);
        (LogSink::new(Arc::new(LogRing::new(64)), writer), guard)
    }

    #[test]
    fn ingest_bridge_logs_stamps_pid_and_src_never_trusts_payload() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (sink, _guard) = test_sink(dir.path());
        let payload = serde_json::json!({"records": [
            {"level": "warn", "target": "bridge::x", "msg": "hi",
             "kvs": {"ms": 42, "note": "ok"}, "ts": "2026-01-01T00:00:00.000Z",
             "pid": 999999, "src": "daemon"}
        ]});
        let n = ingest_bridge_logs(42, &payload, &sink);
        assert_eq!(n, 1);
        let (recs, _) = sink.ring().snapshot_after(0, 8, None, &[]);
        assert_eq!(recs.len(), 1);
        let r = &recs[0];
        assert_eq!(r.pid, 42, "pid must come from handshake identity, not payload");
        assert_eq!(r.src, Src::Bridge);
        assert_eq!(r.level, Level::Warn);
        assert_eq!(r.msg, "hi");
        assert_eq!(r.kvs.get("ms").map(String::as_str), Some("42"));
        assert_eq!(r.kvs.get("note").map(String::as_str), Some("ok"));
        assert_eq!(
            r.kvs.get("sentTs").map(String::as_str),
            Some("2026-01-01T00:00:00.000Z")
        );
    }

    #[test]
    fn ingest_bridge_logs_skips_malformed_entries_and_clamps_batch() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (sink, _guard) = test_sink(dir.path());
        let mut records: Vec<serde_json::Value> = (0..300)
            .map(|i| serde_json::json!({"level": "info", "target": "t", "msg": format!("m{i}")}))
            .collect();
        records.push(serde_json::json!("not an object"));
        let payload = serde_json::json!({"records": records});
        let n = ingest_bridge_logs(1, &payload, &sink);
        assert_eq!(n, MAX_BRIDGE_BATCH);
    }

    #[test]
    fn ingest_bridge_logs_ignores_payload_without_records() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (sink, _guard) = test_sink(dir.path());
        assert_eq!(ingest_bridge_logs(1, &serde_json::json!({}), &sink), 0);
    }

    #[test]
    fn truncate_to_byte_cap_respects_char_boundaries() {
        let mut s = "héllo".to_owned(); // 'é' is 2 bytes
        truncate_to_byte_cap(&mut s, 2);
        assert_eq!(s, "h");
    }
}

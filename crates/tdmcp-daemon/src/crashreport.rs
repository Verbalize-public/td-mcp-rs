//! Crash reports: a panic hook persisting one text report per panic under
//! `{data_dir}/crash/`, so a crashed daemon/GUI process leaves an inspectable
//! record (payload, location, backtrace, recent log tail) on the same machine.
//!
//! Aborts and OOM kills are not catchable and therefore not reported. The hook
//! itself never panics: every fallible step degrades to "no report".

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::Local;

use crate::logring::LogRing;

/// Newest reports kept in `{data_dir}/crash`; older ones pruned on each write.
const KEEP_REPORTS: usize = 10;

/// Recent ring lines appended to every report (tail of the in-memory ring).
const TAIL_LINES: usize = 30;

/// Install the process-wide panic hook writing reports into `{data_dir}/crash`.
///
/// Call once early in `main`, before worker threads spawn, so panics on any
/// thread (daemon runtime, GUI render) are captured into the same directory.
pub fn install(data_dir: &Path, ring: Option<Arc<LogRing>>) {
    let dir = crash_dir(data_dir);
    std::panic::set_hook(Box::new(move |info: &std::panic::PanicHookInfo<'_>| {
        let payload = info.payload();
        let message = payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "<non-string panic payload>".to_string());
        let report = Report {
            message,
            location: info.location().map(|l| l.to_string()),
            backtrace: format!("{}", std::backtrace::Backtrace::force_capture()),
        };
        let _ = write_report(&dir, ring.clone(), &report);
    }));
}

/// Directory holding crash reports (`{data_dir}/crash`).
pub fn crash_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("crash")
}

/// Plain-data crash description extracted from the panic machinery, kept free
/// of [`std::panic::PanicHookInfo`] so the write path is unit-testable.
pub struct Report {
    /// Panic payload rendered as text.
    pub message: String,
    /// `file:line:column` of the panic site, when known.
    pub location: Option<String>,
    /// Forced backtrace at panic time.
    pub backtrace: String,
}

/// Write one report file (and prune old ones); returns the path written.
///
/// Never panics: failures surface as `io::Result` and callers inside the hook
/// ignore them.
pub fn write_report(
    dir: &Path,
    ring: Option<Arc<LogRing>>,
    report: &Report,
) -> std::io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    // Filename-encoded timestamp sorts lexicographically, which the prune
    // relies on — keep this format stable.
    let now = Local::now();
    let name = format!(
        "crash-{}-p{}.log",
        now.format("%Y%m%d-%H%M%S%.3f"),
        std::process::id()
    );
    let path = dir.join(name);

    let thread_name = std::thread::current()
        .name()
        .unwrap_or("<unnamed>")
        .to_string();

    let mut text = String::with_capacity(4096);
    text.push_str("td-mcp-rs crash report\n");
    text.push_str(&format!("time:     {}\n", now.to_rfc3339()));
    text.push_str(&format!("version:  {}\n", env!("CARGO_PKG_VERSION")));
    text.push_str(&format!("pid:      {}\n", std::process::id()));
    text.push_str(&format!("thread:   {thread_name}\n"));
    text.push_str(&format!(
        "location: {}\n",
        report.location.as_deref().unwrap_or("<unknown>")
    ));
    text.push_str(&format!("message:  {}\n", report.message));
    text.push_str("\n--- backtrace ---\n");
    text.push_str(&report.backtrace);

    if let Some(ring) = ring {
        let (records, _) = ring.snapshot_after(0, usize::MAX, None, &[]);
        text.push_str(&format!("\n--- recent log (last {TAIL_LINES}) ---\n"));
        for r in records.iter().rev().take(TAIL_LINES).rev() {
            text.push_str(&crate::record_to_line(r));
            text.push('\n');
        }
    }

    fs::write(&path, text)?;
    prune(dir);
    Ok(path)
}

/// Keep only the newest [`KEEP_REPORTS`] reports (filename timestamp order).
fn prune(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("crash-") && n.ends_with(".log"))
        })
        .collect();
    if files.len() <= KEEP_REPORTS {
        return;
    }
    files.sort_unstable(); // timestamp-prefixed names sort oldest-first
    let stale_count = files.len() - KEEP_REPORTS;
    for stale in files.into_iter().take(stale_count) {
        let _ = fs::remove_file(stale);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, reason = "test assertions may panic")]
    #![allow(clippy::panic, reason = "the hook test must raise a real panic")]

    use super::*;

    fn sample(message: &str) -> Report {
        Report {
            message: message.to_string(),
            location: Some("src/lib.rs:1:1".to_string()),
            backtrace: "stack frame A\nstack frame B".to_string(),
        }
    }

    #[test]
    fn write_report_writes_markers() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_report(dir.path(), None, &sample("boom")).expect("write");
        assert!(path.file_name().is_some_and(|n| {
            let n = n.to_string_lossy();
            n.starts_with("crash-") && n.ends_with(".log") && n.contains("-p")
        }));
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(text.contains("td-mcp-rs crash report"));
        assert!(text.contains("message:  boom"));
        assert!(text.contains("src/lib.rs:1:1"));
        assert!(text.contains("stack frame A"));
    }

    #[test]
    fn prune_keeps_newest_ten() {
        let dir = tempfile::tempdir().expect("tempdir");
        for i in 0..12u32 {
            let name = format!("crash-20260101-0000{:02}.000-p1.log", i);
            std::fs::write(dir.path().join(name), "x").expect("seed");
        }
        write_report(dir.path(), None, &sample("latest")).expect("write");
        let remaining: Vec<_> = fs::read_dir(dir.path())
            .expect("readdir")
            .collect::<Result<Vec<_>, _>>()
            .expect("entries");
        assert_eq!(remaining.len(), KEEP_REPORTS);
        // Oldest seeds pruned; exactly one freshly written report survives.
        let names: Vec<String> = remaining
            .iter()
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        assert_eq!(
            names.iter().filter(|n| !n.contains("-20260101-")).count(),
            1
        );
    }

    #[test]
    fn hook_fires_and_writes_report() {
        let dir = tempfile::tempdir().expect("tempdir");
        let prev = std::panic::take_hook();
        install(dir.path(), None);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // panic_any, not panic!: clippy::panic is deny-listed even in
            // tests, and this test's whole job is to raise a real panic.
            std::panic::panic_any("crash-hook-probe");
        }));
        std::panic::set_hook(prev);
        assert!(result.is_err());
        let hits: Vec<PathBuf> = fs::read_dir(crash_dir(dir.path()))
            .expect("crash dir created by hook")
            .map(|e| e.expect("entry").path())
            .collect();
        assert_eq!(hits.len(), 1);
        let text = std::fs::read_to_string(&hits[0]).expect("read report written by hook");
        assert!(text.contains("crash-hook-probe"), "report: {text}");
    }
}

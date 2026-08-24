//! Tracing setup: central JSONL file sink + ring buffer, plus the historical
//! stderr fmt layer.
//!
//! Filters: `[logging].filter` > `RUST_LOG` > built-in default for the file
//! layer; `[logging].console_level` > `RUST_LOG` > historical defaults for
//! the console layer. There is deliberately no `TDMCP_LOG` — the doc-comment
//! that promised one was a lie and is purged with this rewrite (`RUST_LOG` +
//! `[logging]` cover the need).

use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use tracing::field::{Field, Visit};
use tracing::{Event, Metadata};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::layer::Layer;
use tracing_subscriber::{prelude::*, EnvFilter};

use crate::config::Config;
use crate::logrecord::{Level, Record, Src};
use crate::logring::{LogRing, LogSink};

/// Built-in file-layer filter when neither config nor env specifies one.
const DEFAULT_FILE_FILTER: &str = "info,tdmcp_daemon=debug";

/// Historical stderr defaults, kept byte-comparable for the console layer.
const DEFAULT_CONSOLE_FILTER: &str =
    "tdmcp_daemon=info,tdmcp_gui=info,tdmcp_core=info,tdmcp_mcp=info,tdmcp_ipc=info";

/// Handles the process must keep alive for the sink's lifetime. Dropping
/// [`Self::guard`] flushes-and-stops the background writer thread, so the
/// binding lives in `main`'s scope until shutdown.
pub struct LogHandles {
    /// Shared ring + file writer — also handed to the bridge session layer
    /// so bridge-uplinked log events land through the same path (M2).
    pub sink: LogSink,
    /// Buffered writer flush guard — hold until shutdown.
    #[allow(dead_code, reason = "kept alive intentionally; M4 wires readers")]
    pub guard: WorkerGuard,
}

/// Install the global subscriber: JSONL rotating-file sink + ring, stderr fmt.
pub fn init(cfg: &Config) -> Result<LogHandles> {
    let ring = Arc::new(LogRing::new(crate::logring::RING_CAPACITY));

    let rust_log = std::env::var("RUST_LOG").ok();
    let file_filter = EnvFilter::new(pick_filter(
        cfg.logging_filter.as_deref(),
        rust_log.as_deref(),
        DEFAULT_FILE_FILTER,
    ));
    let console_filter = EnvFilter::new(pick_filter(
        cfg.logging_console_level.as_deref(),
        rust_log.as_deref(),
        DEFAULT_CONSOLE_FILTER,
    ));

    std::fs::create_dir_all(&cfg.logging_dir)
        .with_context(|| format!("create logging dir {}", cfg.logging_dir.display()))?;
    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("daemon")
        .filename_suffix("log")
        .max_log_files(cfg.logging_max_files as usize)
        .build(&cfg.logging_dir)
        .with_context(|| format!("build rolling appender in {}", cfg.logging_dir.display()))?;
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let sink = LogSink::new(ring, writer);

    let sink_layer = SinkLayer {
        filter: file_filter,
        sink: sink.clone(),
    };

    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_writer(std::io::stderr)
        .with_filter(console_filter);

    tracing_subscriber::registry()
        .with(sink_layer)
        .with(fmt_layer)
        .try_init()
        .map_err(|e| anyhow::anyhow!(e))
        .context("install global tracing subscriber")?;

    Ok(LogHandles { sink, guard })
}

/// Filter precedence: valid config value > valid `RUST_LOG` > default.
/// An invalid explicit value falls through rather than failing startup.
fn pick_filter(explicit: Option<&str>, rust_log: Option<&str>, default: &str) -> String {
    if let Some(f) = explicit {
        if EnvFilter::try_new(f).is_ok() {
            return f.to_owned();
        }
    }
    if let Some(env) = rust_log {
        if EnvFilter::try_new(env).is_ok() {
            return env.to_owned();
        }
    }
    default.to_owned()
}

/// Registry layer writing each event as one JSONL record into both the ring
/// and the rotating file writer (via [`LogSink`]). Never panics; file-write
/// failures are dropped silently (the stderr layer keeps working regardless).
struct SinkLayer {
    filter: EnvFilter,
    sink: LogSink,
}

impl<S: tracing::Subscriber> Layer<S> for SinkLayer {
    fn enabled(&self, metadata: &Metadata<'_>, cx: tracing_subscriber::layer::Context<'_, S>) -> bool {
        self.filter.enabled(metadata, cx)
    }

    fn on_event(&self, event: &Event<'_>, cx: tracing_subscriber::layer::Context<'_, S>) {
        if !self.filter.enabled(event.metadata(), cx) {
            return;
        }
        let mut visitor = RecordVisitor::default();
        event.record(&mut visitor);
        let target = event.metadata().target();
        let record = Record {
            seq: 0, // assigned by the ring on push
            ts: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            level: Level::from(event.metadata().level()),
            src: Src::infer_from_target(target),
            pid: std::process::id(),
            target: target.to_owned(),
            msg: visitor.msg,
            code: visitor.code,
            kvs: visitor.kvs,
        };
        self.sink.push(record);
    }
}

#[derive(Default)]
struct RecordVisitor {
    msg: String,
    code: Option<String>,
    kvs: BTreeMap<String, String>,
}

impl RecordVisitor {
    fn store(&mut self, name: &str, value: String) {
        match name {
            "message" => self.msg = value,
            "code" => self.code = Some(value),
            _ => {
                self.kvs.insert(name.to_owned(), value);
            }
        }
    }
}

impl Visit for RecordVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.store(field.name(), value.to_owned());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.store(field.name(), format!("{value:?}"));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;

    #[test]
    fn filter_precedence_config_beats_env_beats_default() {
        assert_eq!(
            pick_filter(Some("warn"), Some("debug"), DEFAULT_FILE_FILTER),
            "warn"
        );
        assert_eq!(
            pick_filter(None, Some("debug"), DEFAULT_FILE_FILTER),
            "debug"
        );
        assert_eq!(pick_filter(None, None, DEFAULT_FILE_FILTER), DEFAULT_FILE_FILTER);
        // Invalid config value falls through to env, then default. EnvFilter's
        // directive grammar accepts almost any bare string as a target name,
        // so use a `target=level` form with a garbage level to force a
        // genuine parse error.
        assert_eq!(
            pick_filter(Some("tdmcp=notalevel"), Some("debug"), DEFAULT_FILE_FILTER),
            "debug"
        );
        assert_eq!(
            pick_filter(Some("tdmcp=notalevel"), None, DEFAULT_FILE_FILTER),
            DEFAULT_FILE_FILTER
        );
    }

    #[test]
    fn visitor_routes_fields_by_name() {
        let field_names = ["message", "code", "ms"];
        // Visit is driven by tracing internals; exercise `store` directly.
        let mut v = RecordVisitor::default();
        v.store(field_names[0], "hello".to_owned());
        v.store(field_names[1], "tdmcp.x.y".to_owned());
        v.store(field_names[2], "42".to_owned());
        assert_eq!(v.msg, "hello");
        assert_eq!(v.code.as_deref(), Some("tdmcp.x.y"));
        assert_eq!(v.kvs.get("ms").map(String::as_str), Some("42"));
    }
}

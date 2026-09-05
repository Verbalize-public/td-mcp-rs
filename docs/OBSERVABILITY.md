# Logs and troubleshooting

Open the dashboard's **Logs** tab to filter by level/source and follow new
records. Use **Open logs folder** to inspect files. The headless equivalent is
`tdmcp-daemon logs` (see `--help` for options).

Logs combine daemon lifecycle events, stdio proxy events, and forwarded TD
bridge logs. TD operator errors still need `inspect`; logs are not a complete
poll of every operator's error state.

## Files and retention

JSONL files rotate daily under `{dataDir}/logs`. The default retention keeps
at most 14 rotated files and removes files older than 30 days.
The `[logging]` section controls directory, filter, console level, file count,
and retention days; see the
[commented defaults](../crates/tdmcp-config/assets/default.toml).
Changes require a daemon restart.

Each record includes timestamp, level, source, PID, target, message, and
optional diagnostic code/structured fields. The in-memory tail uses a sequence
cursor independent of the on-disk history.

## Admin endpoints

- `GET /admin/logs?after=<seq>&limit=<n>&level=<level>&src=<source>` returns
  `{records, next}`.
- `GET /admin/logs/path` returns the log directory.
- `POST /admin/logs/ingest` accepts records from cooperating local processes.

The logging endpoints follow admin authentication. Treat files and responses
as sensitive: TD messages can contain project paths or script output.
Never include access keys in logs or bug reports.

## Debugging a failure

Capture the affected PID, request, diagnostic code, daemon/TD versions, and
a short log excerpt around the failure. Distinguish a request timeout from a
confirmed cancellation: Python already running in TD may continue.

For extra detail, configure `logging.filter` (for example
`info,tdmcp_daemon=debug`) and restart. Reproduce once, then return to the
normal level. Avoid dumping whole projects, token values, or large binary data.

Implementation: [logring.rs](../crates/tdmcp-daemon/src/logring.rs),
[admin.rs](../crates/tdmcp-daemon/src/admin.rs), and the GUI's
[log view](../crates/tdmcp-gui/src/dashboard/logs.rs).

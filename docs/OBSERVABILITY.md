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

TD owns its Textport stdout/stderr streams as well as its operators and UI.
The bridge defers worker-thread writes and flushes to the main-thread pump;
worker output can therefore appear later in the Textport. The deferred queue
keeps the newest 256 chunks, each capped at 8192 characters. The separate log
uplink and debug-DAT queues retain their own limits. Request dispatch and
Python execution reject off-main entry before touching TD.

Agent-created threads may perform plain Python computation or I/O, but must
not access `td`, operators, parameters, UI, or native Textport handles. Prepare
and schedule TD callbacks from the main thread; passing an OP to a worker or
calling `td.run` from that worker is not a supported handoff. A safe logging
wrapper does not make other TD API calls thread-safe.

The reconnect watchdog uses an independent TDResources reference **and**
wall-time delays, so pausing a non-realtime project does not stretch its
two-second retry into many project frames. This fix is baked into
`bootstrap.tox`: existing projects containing an older bridge COMP need the
fresh installed tox dragged in again. Reinstalling the daemon alone does not
update a project's embedded callbacks. Timing jobs are cancelled on bridge
reload, including when bootstrap purges Python modules; old callbacks must
not retain transport ownership or a temporary recorder.

For extra detail, configure `logging.filter` (for example
`info,tdmcp_daemon=debug`) and restart. Reproduce once, then return to the
normal level. Avoid dumping whole projects, token values, or large binary data.

Implementation: [logring.rs](../crates/tdmcp-daemon/src/logring.rs),
[admin.rs](../crates/tdmcp-daemon/src/admin.rs), and the GUI's
[log view](../crates/tdmcp-gui/src/dashboard/logs.rs).

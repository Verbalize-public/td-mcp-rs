# RISKS.md — accepted exceptions

Release-path panics, `unwrap`/`expect`, `unsafe`, or process-exit allows must
be listed here **in the same change** that introduces them
([`CONSTITUTION.md`](CONSTITUTION.md)).

| ID | Location | Exception | Justification | Date |
| --- | --- | --- | --- | --- |
| R1 | `crates/tdmcp-daemon/src/main.rs` (`start_daemon`) | Background OS thread owns a dedicated `tokio::Runtime` while the main thread runs `tdmcp_gui::run` (eframe/winit) | eframe requires the real main thread; splitting keeps the control plane async without nesting runtimes. Daemon errors surface via admin poll / join after GUI exit. | 2026-07-30 |
| R2 | `crates/tdmcp-daemon/src/main.rs` (`join_daemon_thread`) | `std::process::exit(0)` on **main** if daemon-thread join exceeds the drain deadline | Stop/idle/restart cancel a shared token and drain axum (≤2s); bg/tokio paths must not call `process::exit` (Windows + eframe split). Main-owned hard exit is the last resort when join hangs. `/admin/restart` still spawn-then-cancel in the handler. | 2026-08-01 |
| R3 | `crates/tdmcp-gui` (`notify-rust` toasts) | Windows Action Center toasts may be silent without a registered AppUserModelID / Start Menu shortcut | Out of scope for v1; failures are logged via `tracing::warn`. Tray icon + dashboard remain the primary presence signal. | 2026-07-30 |
| R4 | Bridge Python `serve_queued` + daemon call timeout | Daemon timeout fails the **wait** only — it does not cancel in-flight TD `exec` / main-thread dispatch. Worker now times out `response_slot` (`maxCallWaitSecs`) and returns `tdmcp.bridge.main_thread_timeout`, but a hung script can still occupy the TD main thread until it returns. | TD has no safe cross-thread abort for arbitrary Python. Session supersede + idle-dead / disconnect is the recovery path. Documented in CONTRACT. | 2026-08-03 |
| R5 | `BridgeSessions::spawn` same-pid supersede | Prior actor is cancelled via token; if the old IPC stream is still open, OS-level dual pipes can exist briefly until Python closes / idle-dead. | Generation-guarded teardown skips `on_bridge_lost` (so the new session stays Connected) but calls `cancel_queue_keep_connected` so in-flight/pending `TaskQueue` slots cannot wedge exclusive callers. Full pipe close remains peer-owned. | 2026-08-03 |

When adding a row: cite crate path, exact lint allow (if any), and why a typed
`Result` / diagnostic is insufficient.

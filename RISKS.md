# RISKS.md — accepted exceptions

Release-path panics, `unwrap`/`expect`, `unsafe`, or process-exit allows must
be listed here **in the same change** that introduces them
([`CONSTITUTION.md`](CONSTITUTION.md)).

| ID | Location | Exception | Justification | Date |
| --- | --- | --- | --- | --- |
| R1 | `crates/tdmcp-daemon/src/main.rs` (`start_daemon`) | Background OS thread owns a dedicated `tokio::Runtime` while the main thread runs `tdmcp_gui::run` (eframe/winit) | eframe requires the real main thread; splitting keeps the control plane async without nesting runtimes. Daemon errors surface via admin poll / join after GUI exit. | 2026-07-30 |
| R2 | `crates/tdmcp-daemon/src/admin.rs` (`shutdown`, `restart_daemon`) | `std::process::exit(0)` after admin request | Process-boundary stop/restart; graceful axum shutdown alone cannot replace spawn-then-exit for `/admin/restart`. Allowed at binary/admin boundary with `clippy::exit`. | 2026-07-30 |
| R3 | `crates/tdmcp-gui` (`notify-rust` toasts) | Windows Action Center toasts may be silent without a registered AppUserModelID / Start Menu shortcut | Out of scope for v1; failures are logged via `tracing::warn`. Tray icon + dashboard remain the primary presence signal. | 2026-07-30 |

When adding a row: cite crate path, exact lint allow (if any), and why a typed
`Result` / diagnostic is insufficient.

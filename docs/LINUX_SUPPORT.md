# Linux support — TCP transport + Wine platform

**Status:** Proposed v0 (rev 1, 2026-08-29). Implementation not started — checkboxes in §7 are the state.
**Decided by:** user (transport TCP-only, no token in v0, feature split in §5).
**Touches:** `crates/tdmcp-ipc`, `crates/tdmcp-daemon`, `crates/tdmcp-mcp`, `bridge/`, `docs/CONTRACT.md`, `docs/CONFIG.md`, `docs/ARCHITECTURE.md`→`ARCHITECTURE.md`, CI, packaging.
**Live-TD gates** (§8) need the user; everything else is automatable.

## 1. Why

TouchDesigner ships for macOS and Windows only. On Linux it runs under Wine;
`tdmcp-daemon` runs as a native Linux binary. Two consequences drive this spec:

1. **Transport.** The current daemon↔bridge transport is a Windows named pipe
   (`\\.\pipe\tdmcp-rs`) or Unix UDS (`{dataDir}/bridge.sock`). Sharing a UDS
   into Wine is fragile. A project built on Windows must connect on Linux and
   macOS with no edits, which means one standard `.tox` speaking one transport
   on every OS.
2. **Platform surface.** GUI-control features (dialogs, popup watching,
   window status) probe OS windowing APIs that have no Wine implementation in
   v0. They must fail loudly as unsupported, not silently return empty.

## 2. Decisions

| ID | Decision | Rationale |
| --- | --- | --- |
| D1 | Migrate the bridge transport to **TCP on all platforms**; delete named pipe, UDS, and `winsec` entirely | One `.tox` fits every OS; removes per-OS dial/accept code; two daemon tests un-gate from Windows |
| D2 | **Loopback-only** bind; **no shared-secret token in v0** | Same trust model as MCP HTTP without PSK; token deferred to v1 (§9 R1) |
| D3 | The bridge-reported `pid` (a Wine pid) stays the **opaque registry identity**; OS operations use a **mapped Linux pid** (env on spawn, `/proc` scan fallback) | Wine `os.getpid()` does not match Linux pids; registry never does OS lookups on the raw pid |
| D4 | Spec and plan live in this **single document** | Lightest artifact that satisfies the request |
| D5 | Old pipe-based `.tox` inside existing `.toe` projects will **not** connect; the daemon version-gates the handshake with an actionable diagnostic | The tox is re-embedded via `project_install_bridge` / fresh bootstrap tox; silent hang is worse than a clean error |

## 3. Transport — TCP (normative)

Framing does not change: `u32` little-endian length + UTF-8 JSON
(`crates/tdmcp-ipc/src/framing.rs`, `bridge/tdmcp_bridge/transport.py`). Only
addressing and dial/accept plumbing change.

- **T-1 (MUST)** The daemon binds the bridge listener on `127.0.0.1:9861` by default.
- **T-2 (MUST)** `[bridge] host` and `[bridge] port` in `config.toml` override the default; `TDMCP_IPC_HOST` / `TDMCP_IPC_PORT` override the config file. Resolution order: env → config → default (matches the existing `[server]` precedence in `docs/CONFIG.md`).
- **T-3 (MUST)** A bind failure (port taken) exits the daemon with a clear error naming host:port. No silent port hopping — the bridge must be able to find the daemon at a deterministic endpoint.
- **T-4 (MUST)** The bridge port never binds a non-loopback address in v0; a configured `host` other than loopback fails at startup with an explanatory error.
- **T-5 (MUST)** The handshake messages (`HandshakeRequest`/`HandshakeResponse`) and the post-connect I/O timeout (`HANDSHAKE_IO_TIMEOUT`) carry over unchanged; `IpcListener::accept_handshake` semantics (one connection, one handshake, 5 s budget) are preserved over TCP.
- **T-6 (MUST)** A bridge whose first frame is missing/garbled, or whose `protocol_version` predates TCP support, gets a framed error then a close — the daemon log records `tdmcp.bridge.*` with the "re-embed the shipped bootstrap tox" hint (D5).
- **T-7 (MUST)** On Linux the daemon sends `HandshakeResponse.bridgePackageDir` as a Wine-readable path (`Z:\…` form of the absolute Unix path). This is verified live in gate G-L2 before P2 starts.
- **T-8 (MUST)** `bridge/tdmcp_bridge/transport.py` dials TCP (`socket.AF_INET`, `connect((host, port))`) with endpoint resolution: `TDMCP_IPC_ENDPOINT` (`host:port`) if set, else `TDMCP_IPC_PORT`/default host, else `127.0.0.1:9861`. The named-pipe and UDS dialers and their stream wrappers are deleted.
- **T-9 (MUST)** `TDMCP_IPC_PIPE` and the Windows `first-instance`/`winsec` machinery are removed; `crates/tdmcp-ipc/src/winsec.rs` is deleted with its `windows-sys` dependency.
- **T-10 (SHOULD)** `crates/tdmcp-daemon/tests/multi_client_freeze.rs` and `federation_proxy.rs` (currently `#![cfg(windows)]`) run on all platforms after migration.
- **T-11 (SHOULD)** `tdmcp-test-support` (fake bridge peer) dials TCP so integration tests cover the real socket path on every platform.

## 4. Linux platform behavior (normative)

- **L-1 (MUST)** `tdmcp-daemon` builds and runs natively on Linux with no new runtime dependencies beyond what CI already installs for the GUI feature.
- **L-2 (MUST)** `spawn_td` on Linux launches `<wine> <TouchDesigner.exe>` where `<wine>` is `[official_tools] wine_exe` (default `"wine"`), passing the existing `TDMCP_BRIDGE_DIR`/`TDMCP_DATA_DIR` env plus `TDMCP_LINUX_PID=<child linux pid>`. The child's Linux pid registers as the `SpawnRecord`.
- **L-3 (MUST)** The daemon resolves a bridge's Wine pid to a Linux pid for OS operations in this order: (1) `TDMCP_LINUX_PID` captured at spawn; (2) scan `/proc/*/cmdline` for a process whose cmdline contains the TD image name; (3) on zero or multiple matches, return a diagnostic listing what was found. The raw Wine pid is never used for `/proc` or signal calls.
- **L-4 (MUST)** `process_alive` / `process_image_name` on Linux answer from `/proc` for the **mapped** pid (`sys::stub.rs` is replaced by `sys::linux.rs` for these two functions only).
- **L-5 (MUST)** `kill_td` on Linux: `graceful` = `SIGTERM` + grace window, then report remaining popups as unavailable; `force` = `SIGKILL` on the mapped pid. The `graceful` result notes that WM_CLOSE window-closing is a Windows/macOS behavior.
- **L-6 (MUST)** `td_installs` on Linux scans Wine prefixes: `$WINEPREFIX`, `~/.wine`, then `~/.local/share/wineprefixes/*`, at `<prefix>/drive_c/Program Files/Derivative/TouchDesigner.*/bin/`, with the same completeness check (tool files exist) as Windows. `[official_tools] td_exe` pin still wins.
- **L-7 (MUST)** Official tools (`toeexpand`, `toecollapse`, bundled `python`) discovered in a Wine prefix execute through `wine` in `tdmcp-projectio`.
- **L-8 (MUST)** On Linux, the `dialogs` tool (`list`/`describe`/`dismiss`) returns the coded error `tdmcp.dialog.unsupported` with a hint naming the platform limitation — never a silently-empty snapshot (`crates/tdmcp-mcp/src/dialogs_tool.rs` maps `NullDialogSource` to the coded error on Linux).
- **L-9 (MUST)** The daemon popup watcher does not run on Linux (no per-pid probes, no `window_status` writes); `/admin` status reports dialogs as `unsupported_platform` rather than `ok` with empty data.
- **L-10 (SHOULD)** The daemon dashboard/tray (`tdmcp-gui`) builds and runs on Linux best-effort; it is not a v0 acceptance item and may be disabled with `--no-gui` if the tray fails.
- **L-11 (MAY)** `tdmcp-daemon ensure` / autostart on Linux may defer to a manual `systemd --user` unit in v0 (out of scope to automate).

## 5. v0 platform matrix

| Feature | Windows | macOS | Linux v0 |
| --- | --- | --- | --- |
| Bridge transport | TCP | TCP | TCP |
| `spawn_td` | native | native | via `wine` (L-2) |
| `kill_td` graceful | WM_CLOSE | close windows | SIGTERM + grace (L-5) |
| `kill_td` force | TerminateProcess | SIGKILL | SIGKILL (L-5) |
| `td_installs` | registry/ProgramFiles scan | /Applications scan | Wine-prefix scan (L-6) |
| Official tools | direct | direct | via `wine` (L-7) |
| `dialogs` tool | Win32 | CGWindowList/AX | `tdmcp.dialog.unsupported` (L-8) |
| Popup watcher / `window_status` | yes | yes | off (L-9) |
| Dashboard/tray | yes | yes | best-effort (L-10) |

## 6. Acceptance criteria

Automated evidence runs on this Linux host; live-TD rows need the user (§8).

| ID | Criterion | Evidence |
| --- | --- | --- |
| A1 | Workspace builds clean on Linux, no warnings | `cargo clippy --workspace --all-targets -- -D warnings` |
| A2 | Workspace tests pass on Linux, incl. un-gated T-10 tests | `cargo test --workspace` |
| A3 | Bridge pytest suite passes with the TCP dialer | `python3 -m pytest bridge/tests -v` |
| A4 | Fake bridge peer completes handshake over real TCP; garbage first frame gets error+close (T-5, T-6) | `tdmcp-test-support` integration test run in A2 |
| A5 | Config/env endpoint precedence (T-2, T-4) | unit tests on resolution + bind-failure exit message |
| A6 | Spawn through a stub `wine` script registers the Linux pid and `TDMCP_LINUX_PID` (L-2) | integration test with fake wine + fake TD process |
| A7 | pid mapping fallback finds exactly one `/proc` candidate; ambiguity produces the diagnostic (L-3) | unit test against a fixture process |
| A8 | `kill_td` force SIGKILLs a stubbed TD process (L-5) | integration test |
| A9 | `td_installs` reports completeness from a fixture Wine prefix (L-6) | unit test with `tempdir` prefix layout |
| A10 | `dialogs` tool returns `tdmcp.dialog.unsupported` on Linux (L-8) | unit test |
| A11 | Live E2E rows L1–L4 pass | `docs/E2E_CHECKLIST.md` (rows added during P5) |

## 7. Execution plan

Order matters: T-7 (Wine path translation) is a **gate before** P2 — it needs
one live TD run, so schedule it with the user early rather than after Rust work.

- [x] **P0 — Linux build baseline (done 2026-08-29, this host).** Findings F1–F4, all fixed except F5:
  - F1 `clippy 1.98 byte_char_slices` in `crates/tdmcp-projectio/src/ops.rs:312` — fixed (`*b"10"`).
  - F2 `clippy 1.98 unneeded_wildcard_pattern` in `crates/tdmcp-projectio/src/ops.rs:67` — fixed (`Fs { .. }`).
  - F3 **Linux-only compile error**: `crates/tdmcp-dialogs/src/sys/stub.rs:26` `is_hung` took `u64`; facade passes `u32` (windows/macos backends all `u32`). Stub never compiled before Linux — fixed.
  - F4 `crates/tdmcp-mcp/src/td_installs.rs:62-63` test imports unused on Linux (all tests OS-gated) — imports gated to `#[cfg(any(windows, target_os = "macos"))]` until L-6 tests land.
  - F5 **Resolved 2026-08-29 (root-caused + fixed, parent-verified)** — was: respawn never comes up after mid-session daemon death. Real chain (evidence-ranked, hypothesis corrected): **(1) primary** — the daemon-link watcher only heals when `is_unhealthy()`, and the link is only marked by a *failed forwarded call*; an idle session (the test, and a real idle IDE) never triggers respawn — fixed with a self-probe respawn watchdog in `crates/tdmcp-daemon/src/main.rs` (probes `/mcp/health`, fires `ensure_daemon` at `stale` downtime, 10 s cooldown; durable one-hunk follow-up for `daemon_link.rs` `spawn_watcher`'s healthy branch noted in P2); **(2)** the mcp path ensured/respawned a tray daemon (`no_gui: false` default) — the tray-upsert killed the healthy headless daemon and the tray respawn self-destructed headless (`eframe` fatal, exit 1 ~900 ms) — fixed with `no_gui: true` in the Mcp arm; **(3)** zombie hygiene: a SIGKILLed child lingers `Z` in `/proc` (Rust `Child` never auto-reaps) and the bare-existence `pid_alive` counted it alive, defeating `reclaim_stale_daemon_lock` (proved NOT the blocker: the test passes with the child a zombie the whole window; `refuse_if_daemon_owned` needs `pid_alive && health_ok`) — fixed: Linux `pid_alive` parses `/proc/<pid>/stat` via `proc_stat_state` (after last `)`; malformed/unreadable → alive; `Z` → dead). Confounder found en route: the test spawned daemons on default bridge port 9861, colliding with the live gate daemon (T-3 fatal) — test now sets its own free `TDMCP_IPC_PORT`. Verified: `mcp_respawn` green ×3 (+ parent rerun 2.84 s), `--lib` exactly the 2 known failures, clippy clean.
  - Evidence: `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo test --workspace` all green except F5; `.venv/bin/pytest bridge/tests -q` → **265 passed, 2 skipped** (skips are the Windows named-pipe classes). Bridge tests need `python3 -m venv .venv && .venv/bin/pip install pytest` on this host (no system pip; pytest added to `.gitignore` via `.venv/`).
- [x] **P1 — TCP transport (Rust) (code done 2026-08-29, parent-verified).** `tdmcp-ipc` listener/stream over TCP, endpoint resolution + validation (T-1…T-5, T-9), daemon wiring, config `[bridge] host/port`, delete `winsec`/pipe paths, un-gate tests (T-10, T-11). Scope bound: no behavior change outside transport; `bridge/` untouched.
  - Parent-verified 2026-08-29: A1 clippy clean; A4 `bridge_handshake_tcp` 4/4 over real TCP (handshake+ping; garbage→`tdmcp.bridge.handshake_invalid`+EOF; version "999"→`tdmcp.bridge.protocol_mismatch`+EOF); A5 resolver unit tests; T-9 zero `TDMCP_IPC_PIPE`/`winsec` refs in `crates/`, `winsec.rs` deleted, `Cargo.lock` `windows-sys` entries belong to other crates (`tokio`, `mio`, `winit`, `tray-icon` — platform-conditional deps); T-10 `federation_proxy` + `multi_client_freeze` green on Linux; T-11 `FakeTdPeer::connect()` dials real TCP.
  - A2 observed: daemon lib = exactly `bootstrap_tox_matches_packed_source_hash` (repack → G-L2) + `crashreport::hook_fires_and_writes_report` (flake coupled to that panic — `crashreport.rs:207` expects 1 report, the hash test's panic adds a second) + F5 (P2). **New finding — resolved 2026-08-29:** `stdio_proxy::concurrent_calls_during_heal_share_outcome` failed 3/3. Root cause (real product bug, `crates/tdmcp-mcp/src/daemon_link.rs`): `heal()`'s generation-moved early-return leaked the `mark_unhealthy()` it set at entry, so the watcher spuriously re-healed a **healthy** link and `old.cancel()` killed the live session under an in-flight call (log proof: `reconnected to daemon generation=2 downtime_ms=130` during a window where 4 stampede calls succeeded). Fix: `unmark_unhealthy()` called in both early-returns, deliberately not `clear_unhealthy()` so the healer's real `last_downtime` survives (shared-outcome errors now report real downtime instead of ~0 ms). stdio_proxy 8/8, clippy clean.
- [x] **P1b — TCP transport (Python) (code done + parent-verified 2026-08-29).** `transport.py` TCP dialer + endpoint resolution (T-8); delete pipe/UDS dialers; keep `socketpair()`-based tests valid; `default_endpoint()` rewritten. **G-L2 gate:** user runs TD under Wine once to confirm `Z:`-translated `bridgePackageDir` loads (T-7) before P2 proceeds.
  - Code complete + parent-verified 2026-08-29: `resolve_endpoint()` (TDMCP_IPC_ENDPOINT → TDMCP_IPC_PORT → 127.0.0.1:9861, `ValueError` names the variable), `_TcpStream` with full old-wrapper surface incl. `cancel_pending_io`→`shutdown(SHUT_RDWR)`; framing byte-untouched; `bridge/tests` 271 passed (−2 Windows pipe classes, +6 new). B1–B4 PASS.
  - **Repacked + stamped 2026-08-30 — CLOSED.** User ran the "Live pack" script in TD's textport (Wine, `REPO = Z:\home\acorbeau\Repos\td-mcp-rs`); first attempt saved a valid-but-empty 710-byte container (diagnosed with a hardened script printing read/assign/save sizes), second attempt produced the full **10,854-byte** blob (sha `55da3bcc…`). `cargo run -p xtask -- stamp-tox` recorded source hash `0cfea5b3019ebf64`; `bootstrap_tox_matches_packed_source_hash` green and the coupled crashreport flake gone — daemon lib 50/50, workspace A2 fully green. Rebuilt + `install --force`; installed copy = embedded = fresh blob.
- [ ] **P2 — Linux lifecycle.** Wine spawn + env (L-2), pid mapping (L-3, L-4), kill (L-5), installs scan + tool exec through wine (L-6, L-7). `sys::linux.rs` replaces `stub.rs` for alive/image only. **L-10 follow-ups from G-L2 prep (2026-08-29/30):** daemon startup is fatal without a display (`eframe: winit … neither WAYLAND_DISPLAY nor DISPLAY is set`) — headless Linux must degrade to tray-less, not exit; AND a missing tray library is equally fatal: `libappindicator-sys` **panics** (`Failed to load ayatana-appindicator3 or appindicator3 dynamic library`, silent exit 101, report in `{data_dir}/crash/`) killing the daemon on a GUI machine without the package — tray failure must degrade too (catch_unwind / ksni fallback). Linux tray deps for README/P4: `gtk3` + `libayatana-appindicator`. **F5 follow-up:** move the self-probe respawn watchdog into `daemon_link.rs` `spawn_watcher`'s healthy branch (one hunk; the temporary home is `main.rs`, which works today).
- [ ] **P3 — Unsupported surfacing.** Coded `tdmcp.dialog.unsupported` (L-8), watcher off + admin status (L-9), `kill_td` graceful wording (L-5).
- [ ] **P4 — CI + packaging + docs.** `linux-gate` CI job (fmt/clippy/test/pytest), release matrix linux target, amend `docs/CONTRACT.md` §Bridge transport, `ARCHITECTURE.md` topology/surfaces, `docs/CONFIG.md`, `README.md` (Linux quickstart + old-tox migration note per D5), `docs/DELIVERY.md`.
- [ ] **P5 — Live E2E under Wine (user handoff).** Add rows L1–L4 to `docs/E2E_CHECKLIST.md`; user executes with real TD (§8).

Each phase ends with a handoff note appended to this file's log (§10) so the
next session resumes from disk, not chat.

## 8. User handoff points (live-TD work)

1. **G-L1 — Wine TD install:** install a TD build under Wine (`winecfg`, DXVK if needed), activate the license, note the prefix path. User step; agent can only prep scripts.
2. **G-L2 — Path-translation gate (after P1b):** (a) ~~repack `bootstrap.tox` inside TD~~ **done 2026-08-30** (textport pack, 10,854 B, stamped `0cfea5b3…`), (b) ~~stamp-tox + rebuild + `install --force`~~ **done 2026-08-30** (lib 50/50, installed copy = fresh blob, daemon restarted with it), (c) start daemon + TD under Wine; confirm the bridge package loads and handshake completes over TCP. Blocks P2.
3. **G-L3 — Lifecycle round-trip (after P2):** `spawn_td` → tools → `kill_td` against real TD; confirm pid mapping on a user-launched TD too (fallback path of L-3).
4. **G-L4 — Cross-OS project check (after P5 rows):** open the Windows-built probe project (`fixtures/v2-probes/`) under Wine; bridge connects with no edits (D1 acceptance).

## 9. Risks

| ID | Risk | Mitigation |
| --- | --- | --- |
| R1 | TCP loopback is reachable by any local user/process (D2) | Accepted for v0; token auth is the first v1 transport item; bridge port MUST stay loopback (T-4) |
| R2 | Wine path translation for `bridgePackageDir` (T-7) behaves differently across Wine versions | Gate G-L2 before lifecycle work; fallback is a `winepath` conversion step documented in README |
| R3 | Wine pid semantics differ from assumptions (e.g. `os.getpid()` under Wine) | L-3 fallback scans `/proc`; G-L3 verifies both spawn and fallback paths live |
| R4 | Port 9861 collision on user machines | T-3 clear error + T-2 config override |
| R5 | Old `.toe` projects carry the pipe-era tox (D5) | T-6 actionable diagnostic; README migration note in P4 |
| R6 | `tdmcp-gui` tray may misbehave on Wayland (L-10) | Best-effort; `--no-gui` documented fallback |

## 10. Session log

- 2026-08-29 — Spec rev 1 written from user decisions (TCP-only, no token v0, confirmed feature split).
- 2026-08-29 — P0 executed on this host: 4 lint/compile findings fixed (F1–F4), respawn failure recorded (F5, routed to P2). Evidence: clippy clean, tests green except F5, bridge pytest 265/2. Next: P1 (TCP transport, Rust side).
- 2026-08-29 — P1b (Python TCP dialer) settled, parent-verified: `resolve_endpoint()`/_TcpStream, framing byte-untouched, `bridge/tests` 271 passed. Bootstrap.tox repack relocated to G-L2 — per `scripts/pack_bootstrap_tox.md` a `.tox` can only be produced inside live TD; `stamp-tox` without repack would silence the drift guard (forbidden). G-L2 checklist now carries repack → stamp-tox → rebuild.
- 2026-08-29 — P1 (Rust TCP transport) settled, parent-verified (see §7 for evidence). One new verification finding: `stdio_proxy::concurrent_calls_during_heal_share_outcome` red on untouched code — root-cause delegated.
- 2026-08-29 — stdio_proxy finding resolved: real product bug in `daemon_link.rs` `heal()` (generation-moved early-return leaked the unhealthy mark → spurious watcher re-heal cancelled the live session under an in-flight call). Fixed with `unmark_unhealthy()` (+11 lines); test file untouched. stdio_proxy 8/8, clippy clean. A2 residual reds: bootstrap hash (G-L2 repack), its coupled crashreport flake, F5 (P2). Next: G-L2 user gate (needs G-L1 install first), then P2.
- 2026-08-29 — G-L2 prep on this host (dual-boot discovery): real TD install is **2025.32460** at `/mnt/windows/Program Files/Derivative/TouchDesigner.2025.32460/` (Windows partition; 2025.33070 is a partial update — Samples only). TD has run under Wine (runtime caches in `~/.wine/.../AppData/Local/Derivative`, license `ins5.dat` 2026-08-28) via the prefix's `z: → /` mapping. New TCP daemon verified live on this host: 9860 MCP HTTP health `{"ok":true}` + 9861 bridge TCP LISTEN. Two findings: headless startup fatal without `--no-gui` (routed to P2/L-10 above), and T-7 Z:-translation not implemented — pulled ahead of P2 and delegated (composition point `bridge.rs` `bridge_dir_str`).
- 2026-08-29 — **T-7 implemented + parent-verified** (pulled ahead of P2): `crates/tdmcp-daemon/src/bridge.rs` `to_wine_path_string()` — Linux maps absolute Unix paths to Wine `Z:\…` form (`WINE_ROOT_DRIVE = 'Z'`, `/`→`\`, relative/empty pass through unchanged); non-Linux twin byte-identical to old behavior. 3 Linux + 1 non-Linux unit tests; clippy clean; lib tests still exactly the 2 known failures. Daemon rebuilt and restarted with T-7 (pid 214558, `--no-gui`, managed background job): 9860+9861 LISTEN, `idle exit disabled`, awaiting TD. MCP control path pre-verified live (rmcp streamable `initialize` over `POST /mcp/rpc` returns session id + tools capabilities) — the repack will be fired through `execute_python_script` on the live bridge. Wine-variant REPO path documented in `scripts/pack_bootstrap_tox.md` (`Z:\home\acorbeau\Repos\td-mcp-rs`).
- 2026-08-29 — **T-7 wire-verified live (G-L2 half-proven without TD)**: the repo's own Python bridge package (the code the tox runs) dialed `127.0.0.1:9861`, performed the T-6 handshake against the live daemon, and received `bridgePackageDir = "Z:\home\acorbeau\Repos\td-mcp-rs\crates\tdmcp-daemon\..\..\bridge"` — T-7 `Z:`-translation confirmed on the wire (non-canonical `..` segments ride along; Wine resolves them). Wine `Z:` mapping further verified headless: `wine cmd /c dir Z:\…\bridge\tdmcp_bridge\__init__.py` lists the file (wine-staging 11.16) — R2's mapping risk retired for this host. Remaining gate unknowns are real-TD only: package import inside TD's Python, bridge worker start, and the repack through `execute_python_script`.
- 2026-08-29 — **F5 root-caused + fixed (parent-verified)**: three-defect chain — watcher never self-probes (primary; idle sessions can never respawn), mcp-path respawn ran with tray GUI and self-destructed headless, zombie-aware Linux `pid_alive` (hygiene). Fixes in `main.rs` (`no_gui: true` + self-probe watchdog), `ensure.rs` (`proc_stat_state`, `Z` → dead), `tests/mcp_respawn.rs` (own free `TDMCP_IPC_PORT` — found because the test's spawned daemons collided with the live gate's 9861; lesson: a live daemon on default ports confounds any test that doesn't isolate them). `mcp_respawn` green ×3 + parent rerun; durable `daemon_link.rs` self-probe follow-up parked in P2.
- 2026-08-30 — **Bootstrap.tox repacked + stamped (G-L2 a+b done, user + agent)**: textport pack inside TD-under-Wine — first attempt produced a valid-but-empty 710 B container (caught by the ≫1 KB check; hardened script with read/assign/save diagnostics isolated it), second attempt the full **10,854 B** blob (sha `55da3bcc276f097e`). `stamp-tox` → source hash `0cfea5b3019ebf64`. **A2 fully green**: daemon lib 50/50 (hash guard + its coupled crashreport flake both closed), whole workspace green. Rebuild + `install --force` (installed copy = embedded = fresh blob); daemon restarted with it (9860+9861 healthy). G-L2 (c) remains: delete the textport-created `tdmcp_rs` COMP in the pack project (it has no sibling `bridge/` → errors `No module named 'tdmcp_bridge'`), drag the fresh `Z:\home\acorbeau\.local\share\tdmcp-rs\bootstrap.tox`, live TCP handshake — then P2.
- 2026-08-30 — **GUI/tray crash root-caused** (user asked "how do i open the gui"; daemon exited ~1 s in their session): crash report shows `libappindicator-sys` panics — `libayatana-appindicator3`/`appindicator3` not installed — killing the whole daemon (silent exit 101; the crashreport hook captures the panic, the console shows nothing). Fix for now: install `libayatana-appindicator` (+ gtk3) and start the daemon in the user's graphical session (`~/.local/share/tdmcp-rs/bin/tdmcp-daemon start` — no `--no-gui`); tray appears in the Wayland status bar. Filed under P2/L-10 (tray failure must degrade, not kill) and P4 (document Linux tray deps). Note: the zombie-aware `pid_alive` fix proved itself live — the user's start logged `reclaiming stale daemon.lock` from the previous killed instance and proceeded.

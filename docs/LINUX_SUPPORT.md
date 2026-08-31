# Linux support — TCP transport + Wine platform

**Status:** In progress. Shipped: the TCP-transport migration on **all**
platforms (P0, P1, P1b — named pipes / UDS / `winsec` are gone; the bridge
speaks TCP loopback everywhere). Open: P2–P5 (Linux lifecycle, unsupported
surfacing, CI + packaging, live E2E under Wine). Live-TD gates (§8) need the
user; everything else is automatable.

## 1. Why

TouchDesigner ships for macOS and Windows only. On Linux it runs under Wine;
`tdmcp-daemon` runs as a native Linux binary. Two consequences drive this
spec:

1. **Transport.** A project built on Windows must connect on Linux and macOS
   with no edits — one standard `.tox` speaking one transport on every OS.
   (Solved: the daemon↔bridge transport is TCP loopback everywhere.)
2. **Platform surface.** GUI-control features (dialogs, popup watching,
   window status) probe OS windowing APIs that have no Wine implementation.
   They must fail loudly as unsupported, not silently return empty.

## 2. Decisions

| ID | Decision | Rationale |
| --- | --- | --- |
| D1 | Bridge transport is **TCP on all platforms**; named pipe, UDS, and `winsec` are deleted | One `.tox` fits every OS; removes per-OS dial/accept code; two daemon tests un-gate from Windows |
| D2 | **Loopback-only** bind; **no shared-secret token in v0** | Same trust model as MCP HTTP without PSK; token deferred to v1 (§9 R1) |
| D3 | The bridge-reported `pid` (a Wine pid) stays the **opaque registry identity**; OS operations use a **mapped Linux pid** (env on spawn, `/proc` scan fallback) | Wine `os.getpid()` does not match Linux pids; registry never does OS lookups on the raw pid |
| D4 | Spec and plan live in this single document | Lightest artifact that satisfies the request |
| D5 | Old pipe-based `.tox` inside existing `.toe` projects will not connect; the daemon version-gates the handshake with an actionable diagnostic | The tox is re-embedded via `project_install_bridge` / fresh bootstrap tox; silent hang is worse than a clean error |

## 3. Transport — TCP (normative, shipped)

Framing does not change: `u32` little-endian length + UTF-8 JSON
(`crates/tdmcp-ipc/src/framing.rs`, `bridge/tdmcp_bridge/transport.py`). Only
addressing and dial/accept plumbing changed.

- **T-1 (MUST)** The daemon binds the bridge listener on `127.0.0.1:9861` by default.
- **T-2 (MUST)** `[bridge] host` and `[bridge] port` in `config.toml` override the default; `TDMCP_IPC_HOST` / `TDMCP_IPC_PORT` override the config file. Resolution order: env → config → default (matches the existing `[server]` precedence in `docs/CONFIG.md`).
- **T-3 (MUST)** A bind failure (port taken) exits the daemon with a clear error naming host:port. No silent port hopping — the bridge must be able to find the daemon at a deterministic endpoint.
- **T-4 (MUST)** The bridge port never binds a non-loopback address in v0; a configured `host` other than loopback fails at startup with an explanatory error.
- **T-5 (MUST)** The handshake messages (`HandshakeRequest`/`HandshakeResponse`) and the post-connect I/O timeout (`HANDSHAKE_IO_TIMEOUT`) carry over unchanged; `IpcListener::accept_handshake` semantics (one connection, one handshake, 5 s budget) are preserved over TCP.
- **T-6 (MUST)** A bridge whose first frame is missing/garbled, or whose `protocol_version` predates TCP support, gets a framed error then a close — the daemon log records `tdmcp.bridge.*` with the "re-embed the shipped bootstrap tox" hint (D5).
- **T-7 (MUST)** On Linux the daemon sends `HandshakeResponse.bridgePackageDir` as a Wine-readable path (`Z:\…` form of the absolute Unix path). Verified on the wire: the real bridge package dialed `127.0.0.1:9861`, completed the T-6 handshake, and received the `Z:\`-translated path; Wine (`wine cmd /c dir Z:\…`) lists the files it points to.
- **T-8 (MUST)** `bridge/tdmcp_bridge/transport.py` dials TCP (`socket.AF_INET`, `connect((host, port))`) with endpoint resolution: `TDMCP_IPC_ENDPOINT` (`host:port`) if set, else `TDMCP_IPC_PORT`/default host, else `127.0.0.1:9861`. Named-pipe and UDS dialers are deleted.
- **T-9 (MUST)** `TDMCP_IPC_PIPE` and the Windows `first-instance`/`winsec` machinery are removed; `crates/tdmcp-ipc/src/winsec.rs` was deleted with its `windows-sys` dependency (remaining `windows-sys` entries in `Cargo.lock` belong to other crates' platform-conditional deps).
- **T-10 (SHOULD)** `crates/tdmcp-daemon/tests/multi_client_freeze.rs` and `federation_proxy.rs` run on all platforms after migration. Verified green on Linux.
- **T-11 (SHOULD)** `tdmcp-test-support` (fake bridge peer) dials TCP so integration tests cover the real socket path on every platform. Verified.

## 4. Linux platform behavior (normative — lands with P2/P3)

- **L-1 (MUST)** `tdmcp-daemon` builds and runs natively on Linux with no new runtime dependencies beyond what CI already installs for the GUI feature.
- **L-2 (MUST)** `spawn_td` on Linux launches `<wine> <TouchDesigner.exe>` where `<wine>` is `[official_tools] wine_exe` (default `"wine"`), passing the existing `TDMCP_BRIDGE_DIR`/`TDMCP_DATA_DIR` env plus `TDMCP_LINUX_PID=<child linux pid>`. The child's Linux pid registers as the `SpawnRecord`.
- **L-3 (MUST)** The daemon resolves a bridge's Wine pid to a Linux pid for OS operations in this order: (1) `TDMCP_LINUX_PID` captured at spawn; (2) scan `/proc/*/cmdline` for a process whose cmdline contains the TD image name; (3) on zero or multiple matches, return a diagnostic listing what was found. The raw Wine pid is never used for `/proc` or signal calls.
- **L-4 (MUST)** `process_alive` / `process_image_name` on Linux answer from `/proc` for the **mapped** pid (`sys::stub.rs` is replaced by `sys::linux.rs` for these two functions only).
- **L-5 (MUST)** `kill_td` on Linux: `graceful` = `SIGTERM` + grace window, then report remaining popups as unavailable; `force` = `SIGKILL` on the mapped pid. The `graceful` result notes that WM_CLOSE window-closing is a Windows/macOS behavior.
- **L-6 (MUST)** `td_installs` on Linux scans Wine prefixes: `$WINEPREFIX`, `~/.wine`, then `~/.local/share/wineprefixes/*`, at `<prefix>/drive_c/Program Files/Derivative/TouchDesigner.*/bin/`, with the same completeness check (tool files exist) as Windows. `[official_tools] td_exe` pin still wins.
- **L-7 (MUST)** Official tools (`toeexpand`, `toecollapse`, bundled `python`) discovered in a Wine prefix execute through `wine` in `tdmcp-projectio`.
- **L-8 (MUST)** On Linux, the `dialogs` tool (`list`/`describe`/`dismiss`) returns the coded error `tdmcp.dialog.unsupported` with a hint naming the platform limitation — never a silently-empty snapshot (`crates/tdmcp-mcp/src/dialogs_tool.rs` maps `NullDialogSource` to the coded error on Linux).
- **L-9 (MUST)** The daemon popup watcher does not run on Linux (no per-pid probes, no `window_status` writes); `/admin` status reports dialogs as `unsupported_platform` rather than `ok` with empty data.
- **L-10 (SHOULD)** The daemon dashboard/tray (`tdmcp-gui`) builds and runs on Linux best-effort; it is not a v0 acceptance item and may be disabled with `--no-gui` if the tray fails. **Implemented:** the GUI thread runs under `catch_unwind` and any GUI-stack failure (missing display, missing session bus, a panic inside eframe) degrades to headless serving instead of exiting; the Linux tray is `ksni` (pure-Rust StatusNotifierItem over DBus — no GTK / libappindicator dependency).
- **L-11 (MAY)** `tdmcp-daemon ensure` / autostart on Linux may defer to a manual `systemd --user` unit in v0 (out of scope to automate).

## 5. v0 platform matrix

| Feature | Windows | macOS | Linux v0 |
| --- | --- | --- | --- |
| Bridge transport | TCP | TCP | TCP (shipped) |
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
| A3 | Bridge pytest suite passes with the TCP dialer | `python3 -m pytest bridge/tests -v` (no system pytest → create a venv first) |
| A4 | Fake bridge peer completes handshake over real TCP; garbage first frame gets error+close (T-5, T-6) | `tdmcp-test-support` integration test run in A2 |
| A5 | Config/env endpoint precedence (T-2, T-4) | unit tests on resolution + bind-failure exit message |
| A6 | Spawn through a stub `wine` script registers the Linux pid and `TDMCP_LINUX_PID` (L-2) | integration test with fake wine + fake TD process |
| A7 | pid mapping fallback finds exactly one `/proc` candidate; ambiguity produces the diagnostic (L-3) | unit test against a fixture process |
| A8 | `kill_td` force SIGKILLs a stubbed TD process (L-5) | integration test |
| A9 | `td_installs` reports completeness from a fixture Wine prefix (L-6) | unit test with `tempdir` prefix layout |
| A10 | `dialogs` tool returns `tdmcp.dialog.unsupported` on Linux (L-8) | unit test |
| A11 | Live E2E rows L1–L4 pass | `docs/E2E_CHECKLIST.md` (rows added during P5) |

A1–A5 are green on this host (post-P1b, including the repacked bootstrap tox
— see §7). A6–A10 land with P2/P3.

## 7. Execution plan

- [x] **P0 — Linux build baseline.** Compile/lint findings fixed
  (`byte_char_slices`, `unneeded_wildcard_pattern` in `tdmcp-projectio`;
  `stub.rs` `is_hung` type mismatch — the stub had never compiled on Linux;
  unused test imports gated). Plus a real product bug found and fixed while
  reaching green: **respawn after mid-session daemon death never came up.**
  Three-defect chain — (1) the daemon-link watcher only healed on a *failed
  forwarded call*, so idle sessions could never trigger respawn (added a
  self-probe respawn watchdog, currently in `crates/tdmcp-daemon/src/main.rs`;
  durable home is `daemon_link.rs`'s healthy branch — parked as a P2
  follow-up); (2) the MCP path respawned a tray daemon and the tray respawn
  self-destructed headless (`no_gui: true` fixed); (3) zombie hygiene —
  SIGKILLed children linger `Z` in `/proc` and the bare-existence
  `pid_alive` counted them alive (Linux `pid_alive` now parses
  `/proc/<pid>/stat`, `Z` → dead). Lesson recorded: tests that spawn daemons
  must isolate bridge ports — the test collided with a live gate daemon on
  default 9861.
- [x] **P1 — TCP transport (Rust).** TCP listener/stream, endpoint
  resolution + validation (T-1…T-5, T-9), daemon wiring, `[bridge] host/port`
  config, `winsec.rs` + pipe paths deleted, tests un-gated (T-10, T-11).
  Also fixed a second real product bug the new tests exposed:
  `daemon_link.rs` `heal()` leaked its unhealthy mark on the
  generation-moved early-return, letting the watcher spuriously re-heal a
  healthy link and cancel the live session under an in-flight call
  (`unmark_unhealthy()` on both early-returns; shared-outcome errors now
  report real downtime).
- [x] **P1b — TCP transport (Python) + bootstrap repack.** `transport.py`
  TCP dialer + endpoint resolution (T-8); pipe/UDS dialers deleted;
  `socketpair()`-based tests kept valid; framing byte-untouched. The
  bootstrap tox was repacked inside live TD under Wine (first attempt saved
  a valid-but-empty container — caught by the size check; hardened script
  isolated it — second attempt produced the full 10,854-byte blob), stamped
  with `cargo run -p xtask -- stamp-tox`, and `install --force` refreshed
  the installed copy (installed = embedded = fresh blob). A2 fully green
  after this.
- [ ] **P2 — Linux lifecycle.** Wine spawn + env (L-2), pid mapping (L-3,
  L-4), kill (L-5), installs scan + tool exec through wine (L-6, L-7).
  `sys::linux.rs` replaces `stub.rs` for alive/image only. Blocked on gate
  G-L2(c) (live TCP handshake under Wine). Carries the `daemon_link.rs`
  self-probe follow-up from P0.
- [ ] **P3 — Unsupported surfacing.** Coded `tdmcp.dialog.unsupported` (L-8),
  watcher off + admin status (L-9), `kill_td` graceful wording (L-5).
- [ ] **P4 — CI + packaging + docs.** `linux-gate` CI job (fmt/clippy/test/
  pytest), release matrix linux target, amend `docs/CONTRACT.md` § Bridge
  transport, `ARCHITECTURE.md` topology/surfaces, `docs/CONFIG.md`,
  `README.md` (Linux quickstart + old-tox migration note per D5),
  `docs/DELIVERY.md`.
- [ ] **P5 — Live E2E under Wine (user handoff).** Add rows L1–L4 to
  `docs/E2E_CHECKLIST.md`; user executes with real TD (§8).

## 8. User handoff points (live-TD work)

1. **G-L1 — Wine TD install:** install a TD build under Wine (`winecfg`,
   DXVK if needed), activate the license, note the prefix path. Done on this
   host: TD 2025.32460 (Windows partition) runs under Wine via the prefix's
   `z: → /` mapping.
2. **G-L2 — Path-translation gate (after P1b):** (a) repack `bootstrap.tox`
   inside TD — **done 2026-08-30**; (b) stamp-tox + rebuild + `install --force`
   — **done 2026-08-30**; (c) **open:** delete the textport-created `tdmcp_rs`
   COMP in the pack project (it has no sibling `bridge/` → import error),
   drag the fresh installed `bootstrap.tox`, confirm the live TCP handshake
   completes under Wine. Blocks P2.
3. **G-L3 — Lifecycle round-trip (after P2):** `spawn_td` → tools →
   `kill_td` against real TD; confirm pid mapping on a user-launched TD too
   (fallback path of L-3).
4. **G-L4 — Cross-OS project check (after P5 rows):** open the Windows-built
   probe project (`fixtures/v2-probes/`) under Wine; bridge connects with no
   edits (D1 acceptance).

## 9. Risks

| ID | Risk | Mitigation |
| --- | --- | --- |
| R1 | TCP loopback is reachable by any local user/process (D2) | Accepted for v0; token auth is the first v1 transport item; bridge port MUST stay loopback (T-4) |
| R2 | Wine path translation for `bridgePackageDir` (T-7) behaves differently across Wine versions | Verified for wine-staging 11.16 on this host; gate G-L2(c) is the general check; fallback is a `winepath` conversion step documented in README |
| R3 | Wine pid semantics differ from assumptions (e.g. `os.getpid()` under Wine) | L-3 fallback scans `/proc`; G-L3 verifies both spawn and fallback paths live |
| R4 | Port 9861 collision on user machines | T-3 clear error + T-2 config override |
| R5 | Old `.toe` projects carry the pipe-era tox (D5) | T-6 actionable diagnostic; README migration note in P4 |
| R6 | `tdmcp-gui` tray may misbehave on Wayland (L-10) | Best-effort; GUI-stack failures degrade to headless serving, `--no-gui` documented fallback |

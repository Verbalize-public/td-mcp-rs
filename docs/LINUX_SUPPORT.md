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
- **T-12 (MUST)** The shipped tox's **pre-handshake import** resolves under
  Wine: TD's Python is win32 there, so the native conventional lookup lands
  in the prefix's `LOCALAPPDATA`, which can never contain the Linux-side
  install. `bridge/bootstrap.py` therefore tries, after the native
  candidates, the `Z:\…` form of the Unix conventional data dir
  (`$XDG_DATA_HOME`/`$HOME` → `tdmcp-rs/bridge`), mirroring
  `to_wine_path_string` (`crates/tdmcp-daemon/src/bridge.rs:689`). Found at
  gate G-L2(c) 2026-09-01 (`No module named 'tdmcp_bridge'` — the handshake
  `Z:\` translation only applies after the dial). Ships with the next
  bootstrap tox repack; until then the stopgap is a prefix symlink (§8.1).

## 4. Linux platform behavior (normative — lands with P2/P3)

- **L-1 (MUST)** `tdmcp-daemon` builds and runs natively on Linux with no new runtime dependencies beyond what CI already installs for the GUI feature.
- **L-2 (MUST)** `spawn_td` on Linux launches `<wine> <TouchDesigner.exe>` where `<wine>` is `[official_tools] wine_exe` (default `"wine"`), passing the existing `TDMCP_BRIDGE_DIR`/`TDMCP_DATA_DIR` env plus `TDMCP_LINUX_PID=<child linux pid>`. The child's Linux pid registers as the `SpawnRecord`. **Implemented:** `tdmcp_projectio::wine::command_for` wraps the exe through Wine (prefix taken from `TDMCP_WINE_PREFIX` when set, else derived from the resolved exe's `drive_c` ancestor — no config needed for plain Wine/Proton/Lutris/Bottles layouts); the pid export uses a `sh -c 'export TDMCP_LINUX_PID="$$"; exec "$0" "$@"'` wrapper since the real pid isn't knowable until after spawn, and `bridge/tdmcp_bridge/identity.py::_td_pid` reports it at handshake when set.
- **L-3 (MUST)** The daemon resolves a bridge's Wine pid to a Linux pid for OS operations in this order: (1) `TDMCP_LINUX_PID` captured at spawn; (2) scan `/proc/*/cmdline` for a process whose cmdline contains the TD image name; (3) on zero or multiple matches, return a diagnostic listing what was found. The raw Wine pid is never used for `/proc` or signal calls. **Partially implemented:** (1) ships via L-2 above, which is enough for `spawn_td`'s own connect-wait to succeed instead of false-timing-out. (2)/(3) — the `/proc` scan fallback for TD instances started outside `spawn_td` — remain open; `kill_td`/dialogs on Linux still use the `sys::stub.rs` no-ops below and are not part of this pass (out of scope: only `spawn_td` and `project_install_bridge` were targeted).
- **L-4 (MUST)** `process_alive` / `process_image_name` on Linux answer from `/proc` for the **mapped** pid (`sys::stub.rs` is replaced by `sys::linux.rs` for these two functions only). **Implemented** 2026-09-05: `crates/tdmcp-dialogs/src/sys/linux.rs` (`process_alive` mirrors `ensure.rs`'s `/proc/<pid>/stat` zombie-state check; `process_image_name` via `/proc/<pid>/exe` readlink).
- **L-5 (MUST)** `kill_td` on Linux: `graceful` = `SIGTERM` + grace window, then report remaining popups as unavailable; `force` = `SIGKILL` on the mapped pid. The `graceful` result notes that WM_CLOSE window-closing is a Windows/macOS behavior. **Implemented** 2026-09-05 for `spawn_td`-launched/registered pids (`kill_td` in `crates/tdmcp-mcp/src/lifecycle.rs` gains a `#[cfg(target_os = "linux")]` arm alongside Windows/macOS; graceful close has no window layer to post to, so it sends SIGTERM directly). L-3's `/proc/*/cmdline` ambiguity-resolution fallback for a TD instance launched *outside* `spawn_td` remains open — deliberately deferred, not attempted here.
- **L-6 (MUST)** `td_installs` on Linux scans Wine prefixes: `$WINEPREFIX`, `~/.wine`, `~/.local/share/wineprefixes/*`, then `~/.local/share/touchdesigner-linux/prefix` (the AUR `touchdesigner-linux` package's prefix), at `<prefix>/drive_c/Program Files/Derivative/TouchDesigner.*/bin/`, with the same completeness check (tool files exist) as Windows. `[official_tools] td_exe` pin still wins. **Implemented** (`resolve::linux_scan_roots`/`linux_scan_install_exes`); `[official_tools] wine_prefix` (promoted to `TDMCP_WINE_PREFIX` env by `tdmcp_config::load`) scans just that one prefix instead, for layouts the autodetect roots miss (Steam Proton `compatdata`, CrossOver bottles). The same env also pins the invocation-time prefix: `wine::prefix_for` returns it outright before trying the `drive_c`-ancestor walk (which yields nothing for `/opt`-style exes like the AUR package's pinned Wine).
- **L-7 (MUST)** Official tools (`toeexpand`, `toecollapse`, bundled `python`) discovered in a Wine prefix execute through `wine` in `tdmcp-projectio`. **Implemented:** `ProcessRunner::run` routes every invocation through `wine::command_for`, a no-op on Windows/macOS and for non-`.exe` programs.
- **L-8 (MUST)** On Linux, the `dialogs` tool (`list`/`describe`/`dismiss`) returns the coded error `tdmcp.dialog.unsupported` with a hint naming the platform limitation — never a silently-empty snapshot (`crates/tdmcp-mcp/src/dialogs_tool.rs` maps `NullDialogSource` to the coded error on Linux). **Implemented** 2026-09-05: a new `DialogSource::supports_dialogs()` flag (default `true`) is `false` on `NullDialogSource` and the new `LinuxDialogSource` (`crates/tdmcp-dialogs/src/lib.rs`); `dialogs_tool.rs`'s `list` action checks it before calling `snapshot()`, closing the gap where `list` previously returned an empty-but-`ok` snapshot instead of the coded error `describe`/`dismiss` already produced.
- **L-9 (MUST)** The daemon popup watcher does not run on Linux (no per-pid probes, no `window_status` writes); `/admin` status reports dialogs as `unsupported_platform` rather than `ok` with empty data. **Implemented** 2026-09-05: `crates/tdmcp-daemon/src/main.rs` still installs the shared `DialogsShared` state on Linux (so `kill_td`'s pid ops get a real backend) but skips spawning `run_dialogs_watcher` when `supports_dialogs()` is `false`; `/admin/status` gained a `dialogsStatus` field (`"ok"` / `"unsupported_platform"` / `"disabled"`) computed from the same flag.
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
| A7 | pid mapping fallback finds exactly one `/proc` candidate; ambiguity produces the diagnostic (L-3) | unit test against a fixture process — **open**, deferred with the `/proc/*/cmdline` fallback (L-3 items 2/3) |
| A8 | `kill_td` force SIGKILLs a stubbed TD process (L-5) | **green**: `crates/tdmcp-mcp/src/lifecycle.rs::kill_tests::linux_force_kill_sigkills_real_child` (real spawned child, no live TD needed) |
| A9 | `td_installs` reports completeness from a fixture Wine prefix (L-6) | unit test with `tempdir` prefix layout |
| A10 | `dialogs` tool returns `tdmcp.dialog.unsupported` on Linux (L-8) | **green**: `crates/tdmcp-mcp/src/dialogs_tool.rs::tests::list_action_reports_unsupported_when_backend_lacks_dialogs` |
| A11 | Live E2E rows L1–L4 pass | `docs/E2E_CHECKLIST.md` (rows added during P5) |

A1–A5 are green on this host (post-P1b, including the repacked bootstrap tox
— see §7). A6, A9 land with P2/P3. A7 stays open (deferred). A8, A10 green
as of 2026-09-05.

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
  `sys::linux.rs` replaces `stub.rs` for alive/image only. Unblocked:
  gate G-L2(c) cleared 2026-09-01. Carries the `daemon_link.rs`
  self-probe follow-up from P0, plus the T-12 repack (the `bootstrap.py`
  Wine fallback is in the working tree, not yet in a repacked tox).
  **`spawn_td` + `project_install_bridge` slice done 2026-09-05:** L-2, L-6,
  L-7 implemented (`tdmcp-projectio::wine`, `resolve::linux_scan_*`,
  `[official_tools] wine_exe`/`wine_prefix`), plus enough of L-3 (TDMCP_LINUX_PID
  export + bridge-side pid override) for `spawn_td`'s own connect-wait to
  succeed. Still open (at that point): L-3's `/proc` scan fallback for
  externally-launched TD, L-4/L-5 (`sys::linux.rs`, `kill_td` still stubbed
  on Linux).
  **`kill_td` slice done 2026-09-05:** L-4/L-5 implemented
  (`crates/tdmcp-dialogs/src/sys/linux.rs`, `LinuxDialogSource`); `kill_td`
  graceful (SIGTERM + grace window) and force (SIGKILL) both work on Linux
  for `spawn_td`-launched or otherwise registry-known pids. L-3's
  `/proc/*/cmdline` ambiguity-resolution fallback for a TD instance launched
  *outside* `spawn_td` remains open — deliberately deferred (separable,
  materially larger/more heuristic; needs live-TD verification of its own).
  Needs live-TD verification (§8) before calling P2 fully done.
- [x] **P3 — Unsupported surfacing.** Coded `tdmcp.dialog.unsupported` (L-8),
  watcher off + admin status (L-9), `kill_td` graceful wording (L-5). Done
  2026-09-05 alongside the P2 `kill_td` slice above (`DialogSource::supports_dialogs()`
  flag drives all three: `dialogs_tool.rs` `list`, the watcher-spawn gate in
  `main.rs`, and `/admin/status`'s new `dialogsStatus` field).
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
   completes under Wine. First (c) attempt 2026-09-01 failed at the
   pre-handshake import (`No module named 'tdmcp_bridge'`) — diagnosed as
   T-12; after the §8.1 prefix symlink + textport reload, (c) **done
   2026-09-01** (bridge registered in the daemon log, Wine pid 292).
3. **G-L3 — Lifecycle round-trip (after P2):** `spawn_td` → tools →
   `kill_td` against real TD; confirm pid mapping on a user-launched TD too
   (fallback path of L-3). Code-side: `spawn_td`→`kill_td` pid mapping is
   implemented and unit-tested (2026-09-05); the round-trip itself and the
   user-launched-TD fallback path (deferred — no `/proc/*/cmdline`
   resolution yet) both still need live-TD execution.
4. **G-L4 — Cross-OS project check (after P5 rows):** open the Windows-built
   probe project (`fixtures/v2-probes/`) under Wine; bridge connects with no
   edits (D1 acceptance).

### 8.1 Manual bridge install under Wine (no drag & drop)

A manually launched TD gets no `TDMCP_BRIDGE_DIR` (the touchdesigner-linux
wrapper sets none), so its win32 Python must find the bridge package
unaided. Until T-12 ships in a repacked tox, point the prefix at the
installed data dir:

```bash
# Prefix + Windows user of the running TD (this host: the
# touchdesigner-linux wrapper, Windows user `steamuser`):
tr '\0' '\n' </proc/$(pgrep -f -i touchdesigner | head -1)/environ | grep '^WINEPREFIX='
PFX=~/.local/share/touchdesigner-linux/prefix
ln -s ~/.local/share/tdmcp-rs \
  "$PFX/drive_c/users/steamuser/AppData/Local/tdmcp-rs"
```

Then load the installed tox from TD's textport. Paste as standalone lines —
the textport mangles indented blocks — and call `loadTox` **bare**:
assigning its result (`loaded = op(...).loadTox(BOOT)`) fails loudly; the
unassigned call is the reliable form.

```python
import os
BOOT = os.path.join(os.environ["LOCALAPPDATA"], "tdmcp-rs", "bootstrap.tox")
print("tox exists:", os.path.isfile(BOOT), BOOT)
old = op("/project1/tdmcp_rs")
if old: old.destroy()
op("/project1").loadTox(BOOT)
```

`tox exists: True`, a quiet textport (no `tdmcp-rs bootstrap:` line), then
the handshake shows in `~/.local/share/tdmcp-rs/logs/` as
`pid handshake — new registration`. The prefix needs a `z:` dosdevice (→
`/`) for the daemon's `Z:\` handshake path. Once T-12 ships in the tox, the
symlink is unnecessary — the bootstrap tries the `Z:\` form of the Unix
data dir itself. Applied on this host 2026-09-01.

### 8.2 `kill_td` / `dialogs` live round-trip — done 2026-09-05

Ran against a real TD 2025.32460 install under Wine on this host (the
`~/.local/share/touchdesigner-linux` prefix — outside the auto-detected scan
roots, found via the `wine_prefix` escape hatch), using an isolated
daemon instance (separate port/data dir, `TDMCP_BRIDGE_DIR` pointed at the
source tree) so the live production daemon on this host was untouched
throughout:

1. `td_installs` found the install only after setting `wine_prefix` —
   confirms the escape hatch is needed and works for a non-standard prefix
   layout (`~/.local/share/touchdesigner-linux/prefix`, not `~/.wine`).
2. `project_install_bridge` (`strategy: force`) on a copy of the shipped
   template succeeded (`{ok: true, updated: true, rewritten: [...]}`) —
   `toeexpand`/`toecollapse` ran through Wine cleanly (one-time Wine
   first-run prompt for the `wine-mono` package, unrelated to this tool,
   accepted once and not seen again).
3. `spawn_td` against that project connected cleanly twice in a row —
   `{ok: true, pid, handshake: {...}}`, no `wait_timeout`, no manual
   textport/`loadTox` steps. `fleet` showed `bridge: "connected"` under the
   real Linux pid.
4. `kill_td` graceful (SIGTERM) exited TD in ~300ms, well inside an 8s grace
   window — `{ok: true, how: "graceful"}`. The process briefly showed as a
   Linux zombie (`<defunct>`) before reaping; `sys::linux::process_alive`'s
   `/proc/<pid>/stat` state-char check correctly read it as dead immediately
   (this is the real-world case the `process_alive_false_for_unreaped_zombie`
   unit test was written to cover — confirmed against a genuine Wine-hosted
   process, not just a synthetic one).
5. `kill_td` force (SIGKILL) on a second `spawn_td` run also succeeded
   (`{ok: true, how: "force"}`).
6. `dialogs` `action: "list"` against a live connected pid returned
   `tdmcp.dialog.unsupported` (not an empty-but-`ok` snapshot).
7. `GET /admin/status` reported `"dialogsStatus": "unsupported_platform"`
   throughout.

All seven checks passed on the first attempt after the `wine_prefix`
override was set. A7 (the `/proc/*/cmdline` fallback for externally-launched
TD) remains untested/unimplemented — out of scope for this pass.

Update (same day, later): `~/.local/share/touchdesigner-linux/prefix` is now
one of the default scan roots (`resolve::linux_scan_roots`), so step 1's
`wine_prefix` override is no longer needed for this AUR layout; the host
config now carries only `wine_exe`. `wine_prefix`/`TDMCP_WINE_PREFIX` still
works as before and additionally pins the invocation-time prefix
(`wine::prefix_for`).

## 9. Risks

| ID | Risk | Mitigation |
| --- | --- | --- |
| R1 | TCP loopback is reachable by any local user/process (D2) | Accepted for v0; token auth is the first v1 transport item; bridge port MUST stay loopback (T-4) |
| R2 | Wine path translation for `bridgePackageDir` (T-7) behaves differently across Wine versions | Verified for wine-staging 11.16 on this host; gate G-L2(c) is the general check; fallback is a `winepath` conversion step documented in README |
| R3 | Wine pid semantics differ from assumptions (e.g. `os.getpid()` under Wine) | L-3 fallback scans `/proc`; G-L3 verifies both spawn and fallback paths live |
| R4 | Port 9861 collision on user machines | T-3 clear error + T-2 config override |
| R5 | Old `.toe` projects carry the pipe-era tox (D5) | T-6 actionable diagnostic; README migration note in P4 |
| R6 | `tdmcp-gui` tray may misbehave on Wayland (L-10) | Best-effort; GUI-stack failures degrade to headless serving, `--no-gui` documented fallback |

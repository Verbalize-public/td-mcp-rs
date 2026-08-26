# V2 Implementation Plan — Project I/O + Lifecycle + Dialogs

Status: **EXECUTING** (approved rev 2). Spec of record for behavior:
[`SKILLS_CONTRACT_PROPOSAL.md`](SKILLS_CONTRACT_PROPOSAL.md) (rev 2 cuts applied);
dialogs mechanics: [`DIALOGS.md`](DIALOGS.md) rev 4. This document owns execution order,
gates, testing cadence, and the **blocked ledger** (§5).

Done-condition: all 8 tools (`td_installs`, `project_unpack`, `project_pack`, `project_lint`,
`project_install_bridge`, `spawn_td`, `kill_td`, `dialogs`) callable through a live daemon
against TD 2025.32460 with recorded MCP-first evidence; `scripts/check.ps1` green at every
commit; E2E records landed; docs + operate-pack cards + DSH skill pages shipped; ledger empty
or every row dispositioned.

---

## 1. Verified anchors (re-grep before each phase — lines drift)

| Anchor | Location |
|---|---|
| Tool registration chain | `crates/tdmcp-mcp/src/tools.rs:104-123` (`ToolName`), `dispatch_tool` :704, `enqueue_and_call` :1312 |
| Registry | `crates/tdmcp-core/src/registry.rs` — `BridgeStatus{Connected,Disconnected}`, `handshake()` :92 |
| Fleet | `crates/tdmcp-mcp/src/fleet.rs` — `FleetInclude::Popups` reserved :33 |
| Handshake fill / teardown TTL | `crates/tdmcp-daemon/src/bridge.rs` — `ProcessAttrs` :715, `on_bridge_lost` :651, `DISCONNECTED_TTL` :44, teardown sleep→evict :660-663 |
| Config pattern | `crates/tdmcp-config/src/lib.rs` sections :51-192, `save()` :395 |
| Diagnostics | `crates/tdmcp-diagnostics/src/codes.rs` + `diagnostics/catalog.yaml` completeness tests both directions |
| Skills pipeline | `crates/tdmcp-mcp/src/lib.rs:59-63` embeds `skills/MANIFEST.yaml`+templates; `resources.rs` serves `tdmcp://docs/*`; catalog doc-ref tests police cross-links |
| Embedded bridge sources | `crates/tdmcp-daemon/src/install.rs:15` `include_dir!("../../bridge")`; `:27` `TOX_SOURCE_HASH` |
| Test seams | `crates/tdmcp-test-support` `FakeTdPeer` (real IPC handshake); schema goldens `crates/tdmcp-mcp/tests/fixtures/schemas/*.json` via `tests/schema_golden.rs` |
| Quality gate | `scripts/check.ps1`: fmt → clippy `-D warnings` → cargo test --workspace → pytest bridge/tests |

## 2. Locked design decisions (from the challenge pass)

K1 spawn surface-only (no auto-dismiss anywhere); K2 no round-trip diff engine — targeted
byte-verify inside install_bridge only; K3 no cache/[spawn] section/env arg/lint-subset/
requestedBy/buildProbe; K4 errata fixed (bridge family not new; `project.bridge_subtree_missing`);
K5 single FFI home — `process_image_name(pid)` added to dialogs sys facade, consumed by kill_td;
K6 two crates, dialog domain types + seam trait in core.

Mechanics:
- **Registration checklist per tool commit:** ToolName variant+wire_str+description /
  params struct (deny_unknown_fields camelCase JsonSchema) / input_schema_for arm /
  dispatch arm / ALL list / golden fixture json / codes.rs / catalog.yaml (same commit) /
  describe_tools coverage / parity untouched / daemon-restart dance before live verify.
- **Unpack/pack movement:** run official tool beside input, validate artifacts
  (dir+toc parse+escape check), rename into destination (cross-volume fallback recursive copy);
  failure deletes partials beside source.
- **Starting lifecycle:** detached tokio waiter owns row cleanup (client disconnect cannot
  orphan); handshake() heals Starting→Connected preserving SpawnRecord; ghost-eviction
  untouched; daemon restart clears rows (in-memory).
- **Spawn wait:** session-gate exempt like fleet; default 60s within 120s write budget; polls
  THAT pid Connected + popup snapshot per tick; build-skew = inline warning.
- **kill pid check:** registry membership OR image basename == TouchDesigner.exe.
- **Interception locality:** gate atop enqueue_and_call only (local bridged path); offline
  family/dialogs/spawn/kill never traverse it; federation proxies unaffected (unit-tested
  non-routing for pid-less tools).
- **install_bridge content:** bytes verbatim from embedded `bridge/` tree; ensure/force =
  SHA-256 compare of existing DAT bodies; fs::write byte-exact (no newline translation);
  only existing DAT bodies rewritten ⇒ toc untouched. **V2-0 amendment: rewrite THREE bodies**
  (`bootstrap`, `callbacks`, `tdmcp_exec` — the exec DAT mirrors callbacks byte-for-byte).
  Injected bodies normalized CRLF→LF.
- **V2-0 probe laws (2026-08-25, evidence in `fixtures/v2-probes/`):** `.text` sidecar
  envelope = `"2\n"` + u32LE(42) + 4×u32LE(1) + tag `0x02` + u32BE(len) + UTF-8 body
  (header exactly 27 B); `.toc` is strict-LF/no-BOM — CRLF makes `toecollapse` emit a silent
  **0-byte** file with exit 0; official-tool exit codes lie in BOTH directions ⇒ FS-evidence
  checks are the only oracle; `project.save(path)` rebinds the session and collision-naming
  creates `.N.toe` + `Backup/`; TD tolerates any toc entry position (re-derives canonical
  order on load/save); spawn→handshake observed 8–27 s locally (60 s default wait has ~2×
  headroom).

## 3. Agentic loop

```
for each phase P in [R0, A, B, C‖D, E, F, G]:
  ENTRY GATE   deps merged; anchors re-grep'd; todo in_progress
  SLICE LOOP   failing test first → minimal impl → scripts/check.ps1 green
               → conventional commit (scoped files only) → micro-review diff vs spec
  BLOCKED?     §5 protocol; continue next slice
  EXIT GATE    phase checklist + ledger updated + phase report
               → auto-continue except user gates G1/G2/G3
```

Exit criteria per phase:
- **R0:** probe artifacts committed; findings into proposal §6/§7; **G1** nested-tox verdict
  confirms/reshapes install_bridge scope.
- **A:** projectio/dialogs/core tests green incl. fake-runner envelope + classifier fixtures;
  SAFETY-comment audit → **G2**.
- **B:** transition-matrix units (starting→connected; failure removed; reuse-on-starting;
  ghost-eviction ignores starting); fleet golden updated.
- **C:** FakeRunner suite: happy / exit-nonzero-with-artifacts=warning / tool-missing /
  dest_exists / toc_invalid / toc_escape / build_skew opt-in.
- **D:** fake-source watcher integration (popups emission, blocking fail-fast, fail-open,
  kill-switch); #[ignore] MessageBox live PASS recorded.
- **E:** live records: spawn real TD → handshake == spawned pid → fleet owner=spawned; kill
  graceful+force exercised; orphan-waiter test green.
- **F:** flagship: install_bridge(fixture.toe) → spawn_td → handshake → execute_python through
  installed bridge; byte-equality assertion green.
- **G:** **G3** docs flip traced to shipped behavior; resources/read smokes for 3 new cards;
  DSH skill pages load.

## 4. Commit graph (23 commits)

C0.1 docs planning package · C0.2 probe scripts+artifacts · C0.3 findings (**G1**) ·
A1 projectio skeleton · A2 core dialog types+seam · A3 dialogs Win32 backend (**G2**) ·
B1 Starting+spawn records · B2 fleet schema · C0 config [official_tools] · C1 td_installs ·
C2 project_unpack · C3 project_pack · D1 watcher · D2 dialogs tool · D3 interception ·
E1 spawn service · E2 spawn_td (live record) · E3 kill_td (live record) · F1 project_lint ·
F2 install_bridge (flagship live record) · G1 contract flip (**G3**) · G2 skills cards ·
G3 e2e records. Parallelism: C-track and D-track interleave after B; E after B; F after C.

## 5. Blocked-work protocol

Never fake success, never silently skip a test (`#[ignore]` requires a ledger row), never
sketchy workarounds (no retry storms, no disabled lints, no done-looking stubs). 3-strike per
approach: 2 failures ⇒ different approach; 3rd ⇒ ledger + next slice. Degraded-but-shippable
defaults pre-agreed: td-cli absent ⇒ lint native-only (`backends` reports it); UIA content gap
on some Qt dialog ⇒ user32-only detection recorded per class; macOS stays NullDialogSource.

### Live verification records

| Date | Phase | Scenario | Evidence |
|---|---|---|---|
| 2026-08-26 | D1–D3 | Forced real modal (ctypes MessageBoxW in TD thread) on dev host pid 14928 | `execute_python` returned `tdmcp.dialog.blocking` naming "GateProbe"; `fleet include=popups` listed `#32770` id=7081234; `dialogs dismiss` → `{dismissed:true, via:"button:OK", stillOpen:[]}`; post-dismiss list = 0; bridged call recovered (result 42). Helper-window false positives (ConsoleWindowClass/MSCTFIME UI/Default IME) found live → denylist `aee72da`. |
| 2026-08-26 | C1/C2/C3 | `td_installs` dedup rows; unpack 115 entries canonical toc; unpack→pack round-trip 15,770 B, build-skew satisfied | daemon HTTP transcripts in-session; artifacts `fixtures/v2-probes/r0/` |
| 2026-08-26 | F2 FLAGSHIP | install_bridge(force) on r1_live copy → spawn_td → execute_python through INSTALLED bridge | rewritten=[bootstrap,callbacks,tdmcp_exec]; handshake pid 28140 title==flagship_final.toe; python result 42 |
| 2026-08-26 | E2/E3 | `kill_td graceful` on old instance (clean exit); `spawn_td` dev project → deterministic handshake ~27 s, identity exact, fleet spawn provenance | hang-probe false positive (WM_NULL result==0 misread as hung) found live and fixed in `3734c35` → `windowStatus:responsive` |

### Blocked ledger

| Date | Phase/commit | Blocker | Evidence | Disposition |
|---|---|---|---|---|
| 2026-08-26 | A3a | UIA COM content fill-in split into A3b (reviewability) | Popups detect via user32 with `message:null` until then — pre-agreed degraded subset | `deferred` → A3b |
| 2026-08-25 | V2-0 observation | One stale TD session answered ping but never serviced main-thread dispatch (no dialog, OS-responsive) | execute_python timeouts ~45s + `discarding stale bridge response`; recycle fixed | recorded → feeds window_status/dialogs design |

## 6. Standing risks

Doc-anchor drift (re-grep each edit); daemon file locks (taskkill dance, stop after 3);
licence prompts during live spawns (surface-only keeps them visible, non-fatal); GUI WIP
contamination (stage explicitly); scope guard: no GUI work, no bootstrap.tox repack, no P3
create-from-scratch injection.

# v2 Curated Handoff — for the next agent

Recorded 2026-08-26. Branch `main`, HEAD `5b4b860`, working tree clean.
This file is the entry point; everything else it points to is committed.

## What just finished (v2 unified plan — executed end to end)

- 8 new MCP tools live: `td_installs`, `project_unpack`, `project_pack`,
  `project_lint`, `project_install_bridge`, `spawn_td`, `kill_td`, `dialogs`.
- 2 new crates: `crates/tdmcp-projectio` (official-tool expand/collapse,
  sniff/stage/toc/sidecar), `crates/tdmcp-dialogs` (Win32 popup shim + policy
  ladder); plus pre-handshake registry support (`BridgeStatus::Starting`,
  `SpawnRecord`) and skill cards.
- User gates G1 (probes) / G2 (unsafe audit) / G3 (contract flip) all passed;
  evidence lives in `docs/V2_IMPLEMENTATION_PLAN.md` §Live verification records
  and `docs/E2E_CHECKLIST.md` v2 rows V1–V9 (all PASS).
- Spec-of-record: `docs/SKILLS_CONTRACT_PROPOSAL.md` (rev2). Dialogs annex:
  `docs/DIALOGS.md` (rev4). Contract catalogue updated in `docs/CONTRACT.md`.

## Live environment at pause

- Daemon v0.1.3 headless, `127.0.0.1:9860`, pid 29932, `noGui:true`
  (started from `%LOCALAPPDATA%\tdmcp-rs\bin\tdmcp-daemon.exe`, refreshed with
  the latest build including the `project_lint` packed-target fix).
- No TouchDesigner processes left running. Dev install with full tooling:
  `C:\Program Files\Derivative\TouchDesigner.2025.32460\bin\` (toeexpand /
  toecollapse / python beside `TouchDesigner.exe`). Two other dirs are stubs
  (`TouchDesigner\` empty, `.33070` partial) — discovery correctly reports them
  `complete:false` because it validates actual tool FILES.
- Probe project: `fixtures/v2-probes/r0/flagship_final.toe` has the current
  bridge baked in (used for V4/V5).

## Recipes that win (learned the hard way)

### Probe the daemon over HTTP (curl.exe, NOT Invoke-WebRequest)

PowerShell mangles the `Mcp-Session-Id` header on follow-ups; curl works:

```powershell
$resp = curl.exe -sS -i -X POST 'http://127.0.0.1:9860/mcp/rpc' `
  -H 'Content-Type: application/json' -H 'Accept: application/json, text/event-stream' `
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"probe","version":"0"}}}'
$sid = ($resp | Select-String '^mcp-session-id:\s*(\S+)').Matches[0].Groups[1].Value
# then every call needs ALL THREE headers as separate -H flags:
curl.exe -sS -X POST 'http://127.0.0.1:9860/mcp/rpc' `
  -H 'Content-Type: application/json' -H 'Accept: application/json, text/event-stream' `
  -H "Mcp-Session-Id: $sid" -d '<jsonrpc body>'
```

Replies are SSE: take lines starting `data:`, strip the prefix, join,
`ConvertFrom-Json`. Passing `-Header` an ARRAY of strings breaks curl arg
parsing ("Malformed input to a URL") — always use individual `-H` flags.

### Swap the installed daemon binary

The IDE stdio proxy respawns a killed daemon within ~1s, making a plain
kill→copy racy. Winning sequence:

```powershell
taskkill /IM tdmcp-daemon.exe /F
# wait until Get-Process tdmcp-daemon returns NOTHING (~1.7s quiet window)
Copy-Item target\debug\tdmcp-daemon.exe $env:LOCALAPPDATA\tdmcp-rs\bin\tdmcp-daemon.exe -Force
Start-Process "$env:LOCALAPPDATA\tdmcp-rs\bin\tdmcp-daemon.exe" -ArgumentList 'start','--no-gui' -WindowStyle Hidden
# poll http://127.0.0.1:9860/admin/status until ok:true
```

Alternative trick: `Move-Item` the locked exe aside works on NTFS while
running; copy the fresh one into place.

### Gates and hygiene (non-negotiable, enforced all session)

- Before EVERY commit: `cargo clippy --workspace --all-targets -- -D warnings`
  AND `cargo test --workspace` green. Conventional Commits only.
- `cargo fmt` scoped: `cargo fmt -p <crate>`. Bare `cargo fmt` reformats the
  whole workspace INCLUDING parallel GUI WIP that isn't yours (~37 files of
  fmt drift exist there by design — leave them).
- Never write Rust/YAML via pwsh string interpolation into files (backtick
  sequences produce literal garbage) — use proper edit tools.

## Laws the code now encodes (don't regress)

- **Filesystem-evidence law (V2-0):** official tools are judged by artifacts
  (`ops::expand` checks dir+toc existence and parses strict-LF toc; exit codes
  are informational). Proven adversarially: toeexpand exits 1 ON SUCCESS and
  toecollapse can exit 0 after open errors.
- **toc is strict LF.** CRLF made toecollapse emit silent empty output.
  `.gitattributes` pins `*.toc` LF and `*.toe/*.tox/**/*.bin/**/*.bak` `-text`.
- **collapse stages copies under the OUTPUT name** (`<out>.dir/.toc`)
  because toecollapse reads siblings of the target.
- Handshake identity only at bridge connect; spawn records survive
  Starting→Connected but ghost-eviction ignores `Starting`.
- Kill path: graceful WM_CLOSE → popup-aware timeout → TerminateProcess;
  hung = WM_NULL call FAILED (result 0 is responsive, see `3734c35`).
- Dialogs gate sits inside `begin_session_slot` (single choke point,
  fail-open). `dialogs` requires `pid` even for `action:"list"` — it scans a
  process's windows without needing a bridge (verified on explorer.exe).
- `project_lint` accepts packed OR dir targets; packed goes through a private
  `%TEMP%/tdmcp-lint-<uuid>` staging expand, cleaned afterwards; user's file +
  siblings never touched. `backends.tdCli` is wired-but-always-false today.

## Deliberately NOT done (P3/backlog, agreed de-scopes)

- Deeper round-trip diffing than structural byte-compare re-expand verify —
  de-scoped on purpose ("no round-trip diff engine").
- td-cli delegation backend discovery/wiring for `project_lint`.
- No caches anywhere in the offline path (user directive).

## Known spec deviations (audited 2026-08-26 — decide, don't rediscover)

These are places the shipped code does **not** match
`SKILLS_CONTRACT_PROPOSAL.md`. Docs and skill cards now describe the *shipped*
behavior; the spec still describes the intent. Close the gap or amend the spec.

- **`tdmcp.spawn.blocked_by_dialog` is never emitted.** Spec §3.6.5 wants
  timeout-with-popups to be its own coded failure. Reality: `spawn_td` returns
  `Ok({ok:false, outcome:"wait_timeout", stillAlive, startupDialogs})` — the
  agent discriminates on `startupDialogs` being non-empty. The code constant
  exists in `codes.rs` + `catalog.yaml` with zero call sites.
  `tdmcp.spawn.exited_early` is dead the same way (`outcome:"exited_early"`
  instead). Changing this flips a soft result into a hard error — a wire-shape
  change that needs a live probe, so it was left alone.
- **Fleet `owner: "external"|"spawned"` (spec §4.4) is not implemented.**
  Spawned rows carry `spawn: {startedAt, exePath}`; `owner` is absent. Only a
  permissive assertion in `fleet.rs` tests references it.
- **`spawn_td` success payload omits `installId` and `waitedMs`** (spec §3.6.5).

macOS dialogs backend shipped 2026-08-26 (`MacDialogSource`: CGWindowList +
Accessibility). Grant TCC Accessibility for describe/dismiss.

## Where things live

| Topic | File |
| --- | --- |
| v2 spec (tools, schemas, guarantees) | `docs/SKILLS_CONTRACT_PROPOSAL.md` |
| Execution order + anchors + live records | `docs/V2_IMPLEMENTATION_PLAN.md` |
| E2E acceptance rows V1–V9 | `docs/E2E_CHECKLIST.md` |
| Dialogs design + dismiss ladder | `docs/DIALOGS.md` |
| Config keys `[official_tools]` / `[dialogs]` | `docs/CONFIG.md` |
| Official-tool resolution + env names | `crates/tdmcp-projectio/src/resolve.rs` |
| Expand/collapse semantics | `crates/tdmcp-projectio/src/ops.rs` |
| Sidecar codec (27-byte envelope) | `crates/tdmcp-projectio/src/sidecar.rs` |
| Tool dispatch entries | `crates/tdmcp-mcp/src/tools.rs` |
| Spawn/kill services | `crates/tdmcp-mcp/src/lifecycle.rs` |
| Dialog watcher sweep | `crates/tdmcp-daemon/src/dialogs.rs` (`run_dialogs_watcher`) |

## Four-copies bootstrap warning still applies

Editing `bridge/bootstrap.py` / `bridge/tox_callbacks.py` invalidates
`crates/tdmcp-daemon/embedded/bootstrap.tox` — read
`scripts/pack_bootstrap_tox.md` BEFORE touching either; the hash test will
catch drift, do not silence it.

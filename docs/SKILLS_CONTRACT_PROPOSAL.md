# Proposal — v2 Unified Tools & Skills Contract: Project I/O + Lifecycle + Dialogs

Status: **PROPOSAL** (not yet accepted). Extends [`CONTRACT.md`](CONTRACT.md) v1 with eight
tools and three skills-layer cards, and **absorbs [`DIALOGS.md`](DIALOGS.md)** (kept as the
full implementation annex for the `dialogs` tool; this document owns the unified scope,
roadmap, and interaction contracts). On acceptance, apply the change-list in §8.

Rev 2 (challenge pass) — simplification cuts applied throughout: `spawn_td` is surface-only
(no auto-dismiss policy), no round-trip diffing in `project_pack` (targeted byte-verify lives
in `project_install_bridge`), no installs cache, no `[spawn]` config section, no `env` arg,
lint checks run all-native+delegation (no subset param). Execution is governed by
[`V2_IMPLEMENTATION_PLAN.md`](V2_IMPLEMENTATION_PLAN.md).

Author evidence base: full read of the **opendesigner** workspace
(`C:\Users\corbe\Documents\Derivative\Projects\opendesigner` — especially
`crates/td-project-io/src/official.rs` and `docs/project-io/README.md`),
live probes of local TD installs, and the v1 contract + bootstrap-tox flow of this repo.

---

## 0. Summary

v1 declared two non-goals this proposal deliberately reverses:

> *Live operate only — no `.toe`/`.tox` binary editing*
> *Offline ToeDigest / `.toe` write / inject — separate MCP*

Reversing them is cheap because **we do not write a parser**. We orchestrate Derivative's own
`toeexpand` / `toecollapse` (installed beside every `TouchDesigner.exe`) exactly like
opendesigner's proven `td-official` backend, and edit only the small text grammar of the
expand dir. Deep semantic analysis stays opendesigner's job (`td-cli check/lint` is delegated,
never re-implemented). This adds offline project I/O, project linting, in-project bridge
installation (closing the "four copies" manual-re-drags gap from
[`scripts/pack_bootstrap_tox.md`](../scripts/pack_bootstrap_tox.md)), and deterministic
process lifecycle with spawn-owner awareness.

The lifecycle work also unlocks [`DIALOGS.md`](DIALOGS.md): its watcher can only see pids that
have handshaken a bridge, so a TD blocked by a **startup modal** (version-mismatch /
"Backwards Compatiblity Issue" / licence prompts — the exact popups opening a foreign project
throws) was invisible. `spawn_td` registers the pid at spawn time, before any handshake,
closing that gap; the spawn wait-loop then surfaces startup dialogs instead of dying as an
opaque timeout.

New tools (all **Planned**, phased V2-0 probes → V2-A…V2-G in §6):

| # | Tool | One-line contract | Phase |
|---|------|-------------------|-------|
| 1 | `td_installs` | List TouchDesigner installations on disk + which official tools each provides | V2-C |
| 2 | `project_unpack` | `.toe`/`.tox` → expand dir via `toeexpand`, staged + verified | V2-C |
| 3 | `project_pack` | expand dir → `.toe`/`.tox` via `toecollapse`, staged + verified | V2-C |
| 4 | `project_lint` | Lint an unpacked project (native checks; optional `td-cli check --json` delegation) | V2-F |
| 5 | `project_install_bridge` | Install/override the tdmcp bridge inside a given `.toe`/`.tox` (backup + repack) | V2-F |
| 6 | `spawn_td` | Spawn TouchDesigner, deterministically wait for *that pid's* handshake or fail with cause (incl. startup dialogs) | V2-E |
| 7 | `kill_td` | Graceful→forced kill ladder for a known pid | V2-E |
| 8 | `dialogs` | List/describe/dismiss OS popups owned by a TD pid (full spec: [`DIALOGS.md`](DIALOGS.md)) | V2-D |

---

## 1. Evidence base

### 1.1 Official-tools invocation contract (from opendesigner `official.rs`)

These are observed behaviors of Derivative's binaries; the reliability law below is built on
them and is **non-negotiable** in our implementation too:

- Env/config surface: explicit pair → `TD_PROJECT_IO_TOEEXPAND` / `TD_PROJECT_IO_TOECOLLAPSE`
  → `TOUCHDESIGNER_EXE` (tools sit **beside** the exe in `bin\`) → Program Files scan.
- `toeexpand <packed>` writes `{packed}.dir\` + `{packed}.toc` **and is observed to exit
  non-zero even on success** ⇒ success is judged by **filesystem evidence**
  (expand dir + toc exist), never by exit code. Same for `toecollapse` (packed file exists).
- Missing tools are an availability condition, never a panic: typed error
  (opendesigner: `ProjectIoError::OfficialTool` = `TD0019`).
- Structural round-trip is guaranteed; **byte-identical repack is not** — never claim it.
- Packed sniffing: `.toe` magic `b"10"` at offset 0 (+u32be len@2); `.tox` u32be prefix then
  magic at offset 4. Useful for cheap "is this really a packed project" pre-checks.
- **Never redistribute Derivative binaries** — invoke installed ones only.

### 1.2 Local machine ground truth (probed)

- `C:\Program Files\Derivative\TouchDesigner.2025.32460\bin\` — full install:
  `TouchDesigner.exe`, `toeexpand.exe`, `toecollapse.exe`, `python.exe`, `ffmpeg.exe`, …
- `C:\Program Files\Derivative\TouchDesigner.2025.33070\` — **stub: 3 items, no bin tools**.
- Bare `C:\Program Files\Derivative\TouchDesigner\` — **empty dir**.

⇒ Discovery must **validate actual tool files**, not directory names or dir presence.

### 1.3 Expand-dir grammar (from `fixtures/mini/project.toe.dir` + `.toc`)

Small, text-only, stable enough to edit surgically:

- Root: `.build` (`version/build/time/osname/osversion` lines), `.start` (`cookrate 60`),
  `local.n`, `project1.n`; one subdirectory per COMP.
- Per-op files: `<path>.n` (kind line, `tile`, `flags`, `color`, `end`),
  `<path>.parm`, DAT bodies `<path>.text`, wire blocks `<path>.network`,
  external-tox pointer sidecars (`*_ext.text`).
- `.toc` = canonical ordered flat list of tree paths (LF endings).
  **Any file added/removed must update `.toc`** or `toecollapse` output drifts.

### 1.4 The four-copies problem (`scripts/pack_bootstrap_tox.md`)

Copies #1–#3 (git sources, embedded `.tox`, data-dir copy) are guarded by tests and
`xtask stamp-tox`. Copy #4 — the bridge baked inside a user's `.toe` — has **no automated
path**: TD never re-reads it; the fix today is manual re-drag. `project_install_bridge`
eliminates exactly this gap.

---

## 2. Scope statement (v2 deltas vs v1)

| v1 position | v2 position |
|---|---|
| Non-goal: no `.toe`/`.tox` editing | **Goal:** offline project unpack/pack via official tools |
| Non-goal: offline inject = separate MCP | **Goal:** same daemon, new offline tool family (no live pid needed) |
| Lifecycle listed "Planned P2" unspecified | **Specified:** §3.6–§3.7 with deterministic spawn ownership |
| `dialogs` Planned P1, plan parked in DIALOGS.md, startup modals out of scope (§9 there) | **Scheduled V2-D** and extended: startup-dialog coverage via pre-handshake registration (§3.6); start-flow dialog policy defined here, mechanics stay in the annex |

Everything else in v1 stands unchanged: pid-only addressing, dual-gate queue, uniform
diagnostic envelope, never-panic, no sticky targets, no WAN federation changes.

**Rejected alternative (for the record):** embedding a native Rust toe parser/packer
(opendesigner's `td-parse`/emit path). Rejected — enormous grammar surface, already exists
upstream, and our use cases (bridge injection + lint) only need surgical text edits plus
official-tool round-trips. Reuse > rewrite.

---

## 3. Tool catalogue additions

All tools follow v1 conventions: JSON-schema args with `tdmcp.args.*` shape errors, uniform
result envelope, status `Planned` until shipped. Offline tools need **no pid** and bypass the
session gate (like `fleet`/`describe_tools`/`dialogs`); the dialogs interception gate
([`DIALOGS.md`](DIALOGS.md) §5.4) applies to bridged tools only and never touches the offline
family. Live tools (`spawn_td`, `kill_td`) join the existing queue rules.

### 3.1 `td_installs`

List TouchDesigner installations discoverable on this machine.

- Args: none. Every call scans Program Files (ms-cost; no cache).
- Result rows: `{ installId, versionLabel ("2025.32460" — from the directory name), rootPath,
  exePath, tools: { toeexpand: path|null, toecollapse: path|null, python: path|null },
  complete: bool, default: bool }`.
- `complete` = all three tool files exist (§1.2 lesson). `default` = highest version with
  complete tools. A stub install (33070) lists with `complete:false` and null tool paths.
- Annotations: `readOnlyHint: true`. Family: `tdmcp.installs.*`.

### 3.2 `project_unpack`

Expand a packed project into a directory tree using `toeexpand`.

- Args: `sourcePath` (required, absolute), `destDir?` (default: sibling `{sourcePath}.dir`),
  `installId?` (pin install; default = resolved default), `overwrite: "fail"|"replace" = "fail"`,
  `verifyToc: bool = true`.
- Behavior: pre-check packed sniff (§1.1) → resolve tools (§4.1) → run `toeexpand` in a
  **staging dir** → validate FS evidence (`*.dir` + `*.toc` exist; toc parses; paths escape-checked)
  → atomic rename into place → cleanup staging best-effort.
- Result: `{ expandDir, tocPath, entryCount, toolVersion, warnings[] }`. A non-zero child exit
  with valid artifacts is a **warning**, not an error (observed behavior).
- Errors: `project.source_not_found`, `project.not_packed_format`, `project.tool_missing`,
  `project.expand_failed` (no artifacts), `project.dest_exists` (overwrite=fail),
  `project.toc_escape`.
- Annotations: destructive only toward `destDir` on `replace`.

### 3.3 `project_pack`

Collapse an expand dir back into a packed project using `toecollapse`.

- Args: `srcDir` (required; must contain `.build` + `.toc`), `outPath` (required),
  `overwrite: "fail"|"replace" = "fail"`.
- Behavior: sanity-check srcDir (`.toc` present, no escaped/symlinked paths) → **build-skew
  guard**: read `.build` and compare against the selected install; mismatch ⇒
  `project.build_skew` error unless `allowBuildSkew: true` (repacking with tools of a
  different build is how compat-dialog churn starts — make the agent opt in) → stage →
  `toecollapse` → validate packed file exists → atomic rename out.
  No post-pack verification here by design; corruption risk comes from edits, and the only
  tool that edits (`project_install_bridge`) owns its own targeted re-expand verify.
- Result: `{ outPath, bytes }`.
- Guarantees restated in result prose: structural fidelity only; never byte-identical claims.
- Errors: mirror §3.2 + `project.src_not_expand_dir`, `project.roundtrip_broken`.

### 3.4 `project_lint`

Lint an unpacked project directory (or a packed file — auto-unpacks to a temp staging dir).

- Args: `targetPath` (required, either form), `checks?: string[]` (subset; default all),
  `installId?` (for td-cli delegation), `maxDiagnostics: int = 200`.
- Check backends:
  1. **Native checks** (always available, cheap, text-grammar based):
     bridge present (`tdmcp_rs` COMP subtree) + bootstrap hash matches embedded source hash;
     external-tox pointers (`*_ext.text`) resolve to existing files;
     `.toc` ↔ filesystem consistency (missing/extra/orphan entries);
     DAT bodies referenced but absent; duplicate op paths.
  2. **Delegated deep checks** (when opendesigner's `td-cli` is discovered):
     invoke `td-cli check --json <dir>` and pass through its typed `TD00xx` diagnostics,
     capped and attributed `source: "td-cli"`.
- Result: `{ targetKind, diagnostics: [{code, severity, path?, message}], counts, backends }`.
- Annotations: `readOnlyHint: true` (temp staging cleaned up). Family: `tdmcp.lint.*`.

### 3.5 `project_install_bridge`

Install or override the tdmcp bridge inside a given `.toe`/`.tox`. Closes four-copies #4.

- Args: `targetPath` (required), `strategy: "ensure"|"force" = "force"`
  (`ensure` = skip when embedded bootstrap hash already matches; `force` = always rewrite),
  `backup: bool = true`.
- Behavior (phase-gated, see §6):
  - **V2-F — update-existing:** backup original (`{data_dir}/backups/<name>.<ts>.<ext>.bak`)
    → unpack to staging → locate existing root `tdmcp_rs` COMP subtree → rewrite the two Text
    DAT bodies (`bootstrap.text`, `callbacks.text`) with the current embedded sources
    (byte-exact writes from the daemon's embedded `bridge/` tree) → pack →
    **targeted verify**: re-expand to a second staging dir and byte-compare the two rewritten
    `.text` files + `.toc` equality → atomic replace. Only existing files are rewritten, so
    `.toc` content never changes in V2-F.
  - **P3 — create-from-scratch (shipped):** a missing subtree is materialized instead of
    refused. The shipped `bootstrap.tox` is expanded in staging and TD's own grammar files
    are copied into the host COMP dir (`project1` for a `.toe`, the single root COMP for a
    `.tox`), with their `.toc` lines appended (strict LF, position-free per §6.1 R3). **No
    `.n`/`.parm` text is hand-authored** — TouchDesigner stays the author of its own grammar,
    which retires the R3 authoring risk rather than accepting it. The three DAT bodies are
    then rewritten and verified on the normal path, so `created:true` installs get the same
    re-expand byte-compare as updates. `project.bridge_subtree_missing` now fires only when
    the expand root has no unambiguous host COMP.
- Result: `{ updated: bool, previousHash, newHash, backupPath?, verify }`.
- Annotations: destructive (rewrites the project file) — mitigated by mandatory backup.
- Source of truth for contents: the same embedded `bridge/` package the daemon ships
  (single source, no drift by construction; hash reported in result).

### 3.6 `spawn_td`

Spawn a TouchDesigner process and deterministically await **its** bridge handshake.

- Args: `exePath?` XOR `installId?` (exactly one; `both_set` → arg error),
  `projectPath?` (passed as open argument), `args?: string[]`,
  `waitTimeoutMs: int = 60000`.
  Surface-only by design: popups are observed and reported, never auto-dismissed — dismissal
  is always an explicit agent decision via `dialogs`.
- Semantics — the deterministic-ownership core:
  1. Resolve exe via `td_installs` machinery; refuse stub installs (`spawn.exe_incomplete`);
     if `projectPath` is set, warn when its saved build (from a cheap sniff or a prior
     `project_unpack`) skews from the install build (`spawn.build_skew` warning — TD will
     likely pop a compatibility dialog; see step 4).
  2. **Register pre-handshake:** insert the fleet row at spawn time with
     `bridge: "starting"` + the spawn record
     `{ pid → { ownerSession, startedAt, exePath, expectedProject } }`. This is the keystone
     change to the v1 registry (rows were handshake-created only): it makes the process
     visible to `fleet` and to the dialogs watcher from t=0, so startup modals are seen even
     though no handshake will ever arrive while one blocks the main thread.
  3. Wait until the registry shows **that exact pid** connected (handshake identity filled:
     title/project name/toePath/startTime). Connections from any other pid never satisfy this
     wait — no "hope it's the right process".
  4. **Startup-dialog watch (merged from [`DIALOGS.md`](DIALOGS.md)):** while waiting, poll
     the spawned pid's popups each tick.
     - Never auto-dismiss (surface-only by design): popups ride along in every outcome
       payload; the agent dismisses explicitly via `dialogs`, and the wait continues
       (a dismissed blocker lets the handshake land before timeout).
  5. Outcomes:
     - success → `{ pid, installId, handshake: {...}, waitedMs, startupDialogs? }`;
     - timeout with popups present → **`spawn.blocked_by_dialog`** carrying the popup list +
       remediation hint (`dialogs {pid} action=list/dismiss`) — never an opaque timeout;
     - timeout with no popups → `spawn.wait_timeout` `{ stillAlive, hint }`;
     - early death → `spawn.exited_early` with exit code.
     On terminal failure the registry row is marked dead (then evicted per v1 TTL rules).
  6. The pid is addressable by all live tools through normal pid routing once connected; its
     fleet row carries `owner: "spawned"`. Human-opened instances stay `owner: "external"` —
     visible distinction, equal service (but their pre-handshake window stays invisible, as in
     v1; only spawned pids get startup coverage).
- Annotations: creates a process (`readOnlyHint: false`). Family: `tdmcp.spawn.*`.

### 3.7 `kill_td`

Kill a known TouchDesigner pid.

- Args: `pid` (required), `mode: "graceful"|"force" = "graceful"`, `graceMs: int = 5000`.
- `graceful`: post `WM_CLOSE`; if the process persists past `graceMs` → `kill.graceful_timeout`
  (typical cause: unsaved-project prompt) and the agent may retry with `force`. The timeout
  payload includes any open popups of the pid (from the dialogs snapshot cache) so the agent
  can dismiss the prompt first instead of force-killing through it.
  `force`: `TerminateProcess`, unconditional.
- Refuses pids that are not a known TD process (`kill.not_td_pid`) — known = in the registry,
  or image basename is `TouchDesigner.exe` (via a safe `process_image_name(pid)` facade call).
  Protects against fat-fingering while still covering human-opened, never-handshaken TDs.
- Result: `{ pid, exited, how, exitCode? (best-effort) }`. Spawn records cleaned up.
- Annotations: destructive.

### 3.8 `dialogs` (annex tool — full spec in [`DIALOGS.md`](DIALOGS.md))

Summarized here only for roadmap completeness; [`DIALOGS.md`](DIALOGS.md) §5.5 remains the
authoritative contract. Daemon-side watcher enumerates/classifies OS popups per registered
TD pid (`#32770` + style/owner corroboration, chrome guard, POC severity regexes); the tool
exposes `list | describe | dismiss`; an interception gate fails bridged calls fast with
`tdmcp.dialog.blocking` while a modal wedges TD's main thread. v2 deltas to the annex:

- Watcher predicate widens from `bridge == Connected` to `{ starting, connected }` so spawned
  pids are covered pre-handshake (§3.6 step 2).
- Start-flow dialog handling is no longer a non-goal: `spawn_td` observes and reports startup
  popups pre-handshake (§3.6 step 4); dismissal always stays an explicit `dialogs` call
  (surface-only), reusing the annex's classify/ladder mechanics unchanged.
- `kill_td` consumes the snapshot cache for graceful-timeout payloads (§3.7).

---

## 4. Cross-cutting contracts

### 4.1 Official-tools discovery & reliability law (shared by §3.1–§3.5)

Resolution order (config → env → scan), mirroring the proven chain:

1. `[official_tools]` config table: `expand_path` / `collapse_path` (XOR-pair rule:
   setting one without the other is a config error) or `td_exe`.
2. Env: `TDMCP_TOEEXPAND`, `TDMCP_TOECOLLAPSE`, `TDMCP_TOUCHDESIGNER_EXE`.
3. Scan `%ProgramFiles%\Derivative` and `%ProgramFiles(x86)%\Derivative`: child directories
   sorted newest-version-first; candidate `<child>\bin\TouchDesigner.exe`; tools resolved
   beside the exe; a candidate **counts only if the needed tool files exist** (§1.2).

Reliability law (carried verbatim from evidence):
- Success = filesystem evidence, never exit code.
- All work in staging dirs under `{data_dir}/tmp/projectio/<uuid>`; publish via atomic rename;
  best-effort cleanup.
- Missing tools ⇒ typed `project.tool_missing`, never panic, never download/bundle.
- Structural round-trip only; results never claim byte-identical output.
- Never redistribute Derivative binaries.

Placement: new crate `tdmcp-projectio` (owns process spawning of official tools, staging,
sniffing, grammar touch-points). Keeps `tdmcp-core` zero-I/O per [`ARCHITECTURE.md`](ARCHITECTURE.md);
`tdmcp-daemon` remains composition root; `tdmcp-mcp` owns the new schemas.

### 4.2 Config additions ([`CONFIG.md`](CONFIG.md))

```toml
[official_tools]
# all optional; absence triggers env + scan resolution
td_exe        = ""   # pin one install
expand_path   = ""
collapse_path = ""

[dialogs]            # per DIALOGS.md §6 (7-touchpoint config pattern)
enabled   = true     # master switch (watcher + tool)
intercept = true     # fail-fast gate on bridged tool calls
poll_ms   = 1000     # watcher cadence
```

### 4.3 Diagnostics additions ([`catalog.yaml`](../crates/tdmcp-diagnostics/catalog.yaml))

New families: `tdmcp.installs.*`, `tdmcp.project.*`, `tdmcp.lint.*`, `tdmcp.spawn.*`,
`tdmcp.kill.*`, plus the annex's `tdmcp.dialog.*`
(codes already enumerated in [`DIALOGS.md`](DIALOGS.md) §5.5; the v1 `tdmcp.bridge.*` family
already exists and gains no new members in v2). Codes enumerated in §3; all
follow the uniform envelope, `references[]` support, and `tdmcp.args.*` shape-error rules.
Notable mappings: `project.tool_missing` carries the scanned search locations
(config/env/paths tried); `project.build_skew` names both builds and the opt-out flag;
`spawn.exited_early` carries the child exit code; `spawn.blocked_by_dialog` embeds the popup
list; `project.bridge_subtree_missing` now means "no unambiguous host COMP to install into".

### 4.4 Fleet schema extension

Fleet rows gain: `owner: "external"|"spawned"`, a `bridge` state that now includes
**`"starting"`** (registered pre-handshake, §3.6) beside the v1 states, and for spawned rows
`spawn: { startedAt, exePath }`. No addressing changes — pid-only stands.
Dialogs fields (`windowStatus`, `include=popups`) flow per [`DIALOGS.md`](DIALOGS.md) §5.3.

### 4.5 Storage additions

- `{data_dir}/backups/` — pre-replace backups from `project_install_bridge`
  (`<stem>.<yyyymmdd-HHMMSS>.<ext>.bak`).
- `{data_dir}/tmp/projectio/<uuid>/` — staging trees (best-effort cleanup; swept at daemon start).
  No installs cache — every `td_installs` call scans live.

### 4.6 Feature interaction matrix

| Interaction | Contract |
|---|---|
| `spawn_td` → dialogs watcher | Pre-handshake registration makes startup modals (version/compat/licence popups) visible before any handshake; spawn wait-loop surfaces them (surface-only, never auto-dismiss). Resolves [`DIALOGS.md`](DIALOGS.md) §9 limitation #1 for spawned pids. |
| dialogs gate → offline family | Never applies: interception lives in `enqueue_and_call`, which only bridged tools traverse. Unpack/pack/lint run with TD fully wedged if needed. |
| `kill_td` → dialogs | Graceful-timeout payload includes open popups; agent dismisses save-prompt via `dialogs` instead of force-killing through it. |
| build skew chain | `td_installs` records builds; `project_pack` guards `.build` vs install skew; `spawn_td` warns when opening a foreign-build project (compat dialog likely); dialogs classifies whatever pops anyway. |
| `project_install_bridge` → lint/spawn | Installed bridge hash is reported and checked by `project_lint`; after install + spawn, handshake identity confirms the in-project bridge actually came up. |
| queue gates | Offline tools exempt like `fleet`; spawned pid joins normal dual-gate routing once connected; dialogs tool itself is exempt (local, no bridge). |

---

## 5. Skills layer changes

Two layers, consistent with the existing pattern (templates here → served as
`tdmcp://docs/<id>` resources; mirrored as DSH agent-skill references):

### 5.1 Operate-pack cards (`skills/MANIFEST.yaml` + `skills/templates/touchdesigner/*.jinja.md`)

| Card id | Content |
|---|---|
| `project-io` | Offline workflow: `td_installs` → `project_unpack` → edit → `project_pack`; reliability law (FS-evidence success, staging, structural-only fidelity, `.toc` consistency rule, never bundle binaries); build-skew guard and why repacking across builds seeds compat dialogs; when to work offline vs live-operate; interplay with `project_install_bridge`. |
| `lifecycle` | Spawn/own/kill playbook: deterministic pid-wait semantics, spawn outcome taxonomy (`blocked_by_dialog` vs plain timeout vs early exit), reading `owner` + `bridge:"starting"` rows in fleet output, graceful-vs-force decision table, kill-time save-prompt handling. |
| `popups` | Dialog triage: why a modal wedges every bridged call (main-thread dispatch) and what `tdmcp.dialog.blocking` means; triage flow (`fleet include=popups` → `dialogs list` → severity → dismiss soft / escalate hard → verify-gone); **startup case**: opening a foreign-build project pops version/backwards-compat warnings ("Backwards Compatiblity Issue" = soft, TD's own typo verbatim; node-name duplication & THREAD CONFLICT = hard) — prefer fixing the install/project skew over dismissing forever; safety rails (chrome-protected, never auto-answer save-prompts). |
| *(edit)* `tox-toe-components` | Add cross-links to the three new cards + one-line summaries of the eight v2 tools. |

### 5.2 DSH agent skill (`~/.dsh/skills/touchdesigner/`)

Add matching reference pages (`reference/project-io.md`, `reference/lifecycle.md`,
`reference/popups.md`) and three routing rows in `SKILL.md` so agents load them for
project-file, process-management, and dialog-triage tasks.

---

## 6. Phasing (unified roadmap — supersedes the P-tables here and in DIALOGS.md §8)

| Phase | Ships | Depends on |
|---|---|---|
| **V2-0** (probes, no shipped tool) | (R1) nested-tox expansion probe; (R2) `TouchDesigner.exe <file.toe>` CLI-open behavior incl. dialog/licence interaction; (R3) grammar-authoring probe (hand-written extra COMP round-trips through real TD) | — |
| **V2-A** platform crates | `tdmcp-projectio` skeleton + `tdmcp-dialogs` M1 content (sys shim, classify, policy, fake-able source trait) | V2-0 R1 for projectio parts only |
| **V2-B** registry foundation | Pre-handshake registration (`bridge:"starting"` rows), fleet schema extension, spawn-record side-map. **Keystone**: unblocks both dialogs startup coverage and `spawn_td` | V2-A types |
| **V2-C** offline I/O tools | `td_installs`, `project_unpack`, `project_pack`; config `[official_tools]` | V2-A |
| **V2-D** dialogs ship | Watcher task (predicate `{starting, connected}`), snapshot cache, `dialogs` tool, interception gate, `[dialogs]` config = DIALOGS.md M2–M4 | V2-B |
| **V2-E** lifecycle tools | `spawn_td` (startup-dialog surfacing + outcome taxonomy), `kill_td` (popup-aware graceful timeout) | V2-B; popup payloads richer with V2-D but functional without |
| **V2-F** quality tools | `project_lint`, `project_install_bridge` (update-existing + create-from-scratch), backups dir | V2-C |
| **V2-G** docs & E2E | CONTRACT/README/CONFIG updates, skills cards per §5, E2E checklist incl. DIALOGS.md M5 live-dialog row and a foreign-build-project open (compat-popup e2e) | each feature phase |

C/D run in parallel; E may land before D (degrades to plain timeouts, never wrong ones).
P3 carry-over: deeper round-trip diffing (create-from-scratch injection shipped — §3.5). Each phase lands with catalog codes, docs, tests
(`FakeOfficialRunner`-style injectable runner for CI, fake `DialogSource` for watcher tests),
and a live smoke against the 2025.32460 install.

### 6.1 V2-0 probe results — executed 2026-08-25 against TD 2025.32460 (all three decisive)

Evidence: `fixtures/v2-probes/` + reusable scripts under `scripts/probes/v2/`.

- **R1 nested-tox — INLINE, not opaque.** Live capture (`r1_live.toe`) expanded cleanly:
  dragged-tox subtrees (`tdmcp_rs`, `e2e_kit`) materialize as ordinary per-op grammar files.
  ⇒ `project_install_bridge` update-existing needs no opaque-payload fallback.
- **Bridge DAT mirror (critical for F):** `tdmcp_exec.text` ≡ `callbacks.text` byte-identical
  (SHA-256); exec parm has start/create/exit/framestart all ON — install_bridge must rewrite
  **three** bodies (`bootstrap`, `callbacks`, `tdmcp_exec`), never two, or stale code runs.
- **`.text` sidecar v2 envelope (derived from 4 samples incl. empty + 101,500 B):**
  `"2\n"` + u32LE(42) + 4×u32LE(1) + tag byte `0x02` + **u32BE payload length** + raw UTF-8
  body; header = exactly 27 bytes. TD stores LF-only: repo CRLF sources equal payloads after
  CR-strip ⇒ live baked bridge == current repo sources (zero drift). Writer rule: normalize
  CRLF→LF, write the envelope verbatim.
- **`.toc` law:** LF-only, no BOM, tree-walk order with extensions. CRLF ⇒ `toecollapse`
  writes a **silent 0-byte output with exit 0**. Exit codes of both official tools lie in both
  directions (expand success=1 twice observed; collapse open-error=0 once) — filesystem
  evidence is the only oracle, now proven adversarially.
- **R2 lifecycle numbers:** spawn→window 6 s; spawn→handshake 27 s cold / 23 s warm / 8 s hot;
  handshake identity exact-matches the opened file every time (deterministic ownership works).
  Graceful `WM_CLOSE` exits <8 s with no prompt on clean projects. TD quirks captured:
  `project.save(path)` is save-as (**rebinds** the session); collision naming creates `.N.toe`
  siblings plus a `Backup/` dir next to saves.
- **Wedged-pump phenomenon (feeds DIALOGS design):** one stale session answered ping (worker
  thread) but never serviced main-thread dispatch — no dialog, OS-responsive, queue empty;
  recycle fixed it. Exactly the failure mode `window_status` + the dialogs watcher exist for.
- **R3 grammar authoring — PASS end-to-end:** hand-written 6-line `.n` (clone-shape of real
  files, trailing space after color values preserved) + one `.toc` line → strict-LF rewrite →
  `toecollapse` → spawned real TD → `inspect` over MCP shows `/project1/authored_v2`
  (`baseCOMP`). TD tolerates any toc position and re-derives canonical order itself.

---

## 7. Risks & open questions

- **R1 — nested tox opacity.** ~~Open~~ **RESOLVED by V2-0 (§6.1):** expansion is inline;
  no fallback needed. Kept for the record.
- **R2 — spawn-time dialogs.** Licence expiry / first-run / compat prompts can block handshake
  indefinitely. Merged mitigation: pre-handshake registration + popup watch turn these into
  `spawn.blocked_by_dialog` with actionable popups instead of opaque timeouts; hard-severity
  startup dialogs are surfaced, never auto-dismissed; DIALOGS.md §7 risk table (UIPI, hwnd
  reuse, save-prompt loss) applies unchanged to the shared watcher.
- **R3 — grammar authoring risk.** Creating brand-new nodes from text templates is easy to get
  subtly wrong (flags, defaults). **V2-0 R3 round-trip PASSED** for a minimal COMP (§6.1),
  but create-from-scratch shipped by *sidestepping* authoring entirely: it expands the shipped
  `bootstrap.tox` and copies TD's own files (§3.5). The risk is retired, not mitigated — there
  is no template to drift. Live-validated end to end (E2E V10).
- **R4 — version skew.** Expand grammar may shift between TD builds; record
  `toolVersion`/build in every result; pin per-operation via `installId`; never mix installs
  within one unpack→pack cycle (enforced in `tdmcp-projectio`).
- **R5 — concurrency.** Two agents mutating the same project file: staging + atomic rename
  gives last-writer-wins, not merge. Documented limitation; backups are the recovery path.
- **R6 — kill blast radius.** `kill_td` refuses unknown/non-TD pids; `graceful` default gives
  TD its save-prompt chance; `force` is explicit opt-in.

---

## 8. Change-list upon acceptance

1. `docs/CONTRACT.md` — move the two reversed items from *Non-goals* to *Goals* with
   "reversed in v2, see SKILLS_CONTRACT_PROPOSAL" markers; extend tool catalogue with the
   eight Planned entries (`dialogs` row stays, now scheduled V2-D); extend phases table
   (V2-A…V2-G above); storage + queue-exempt notes (offline family + `dialogs` exempt like
   `fleet`); fleet schema note for `bridge:"starting"` / `owner`.
2. `docs/DIALOGS.md` — rev 4: §2 non-goal line (start-flow) and §9 limitation #1 annotated as
   resolved by this proposal; watcher predicate widened (§5.3); milestones M1–M5 mapped onto
   the unified roadmap (§6 here).
3. `docs/CONFIG.md` — `[official_tools]`, `[dialogs]` tables.
4. `crates/tdmcp-diagnostics/catalog.yaml` — new families/codes (§4.3) incl. annex dialog codes.
5. `ARCHITECTURE.md` — add `tdmcp-projectio` + `tdmcp-dialogs` crate boundaries.
6. `README.md` — capability table rows; dialogs roadmap checkbox retargets to V2-D.
7. `skills/MANIFEST.yaml` + templates — cards per §5.1.

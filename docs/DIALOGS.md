# OS dialogs support (`dialogs`, P1) — research & implementation plan

Status: **Plan / not implemented.** Prepared from codebase reconnaissance
(td-mcp-rs @ `522ed577`), prior-art analysis of the JS POC at
`../td-mcp`, and Win32 API research. No code changed.

Rev 3: platform strategy is a **vendored in-house wrapper** over the OS APIs
(review feedback); the `uiautomation` *crate* is dropped, but UI Automation
*itself* stays — as an in-house module, because TD's own dialogs are Qt-hosted
and expose no classic controls (see §4). Windows first, macOS later behind the
same internal facade.

Rev 4: scheduled into the unified v2 roadmap as phase **V2-D** by
[`SKILLS_CONTRACT_PROPOSAL.md`](SKILLS_CONTRACT_PROPOSAL.md); this annex keeps the full
implementation spec, that document owns scope/phasing/interactions. Deltas: watcher predicate
widens to `{starting, connected}` (pre-handshake spawned pids, resolving limitation §9-1 for
spawned processes), start-flow dialog policy moves into `spawn_td`
(`spawn_td` startup-dialog surfacing — surface-only, never auto-dismiss; proposal §3.6)
instead of being a non-goal, and milestones M1–M5 map
onto V2-A/V2-D/V2-G.

Reserves already in the tree this feature fills:

| Reserve | Location |
| --- | --- |
| `dialogs` tool row, **Planned** (P1, Win) | [`docs/CONTRACT.md`](CONTRACT.md) catalogue (line 222) |
| `fleet` `include=popups` — deferred/empty until P1 | CONTRACT.md line 270; `crates/tdmcp-mcp/src/fleet.rs:32-33` (`FleetInclude::Popups`); fixture `crates/tdmcp-mcp/tests/fixtures/schemas/fleet.json` |
| `window_status: Option<String>` — "empty until P1 dialogs / hang probe" | `crates/tdmcp-core/src/registry.rs:33-35` (`ProcessAttrs`); surfaced via `crates/tdmcp-mcp/src/fleet.rs:45-47` |
| Roadmap checkbox | `README.md:186` |

---

## 1. Problem

TouchDesigner surfaces failures (save errors, node-name duplication on load,
thread conflicts) as **native Windows dialogs** owned by the TD process.
While one is open:

- TD's main thread runs the modal loop; every bridge method that dispatches on
  the main thread stalls until its budget expires
  (`tdmcp.bridge.timeout`, or python-side `tdmcp.bridge.main_thread_timeout`
  after `maxCallWaitSecs`). The agent sees an opaque timeout with no hint that
  a dialog is the cause.
- The bridge stays "connected" throughout: `ping` is answered on the IPC
  worker thread (`bridge/tdmcp_bridge/task_queue.py`), so liveness probes
  succeed while all real work is blocked.
- A TD that pops a modal during startup may never handshake at all (out of
  scope for P1 — see §9 Limitations).

Goal: detect these popups out-of-band (daemon-side), tell the agent about them
proactively on every affected tool call, and provide one tool to inspect and
dismiss them.

## 2. Requirements

1. **Automatic detection** of popup windows belonging to a registered TD pid,
   maintained continuously by the daemon (not only on demand).
2. **Interception:** when any TD-touching tool is called for a pid that has a
   popup open, the call fails fast with a structured error describing the
   popup(s) and how to proceed — before work is enqueued and left to time out.
3. **One new tool** to list/describe/dismiss those popups through OS APIs.
   Contract reserves the name `dialogs`.
4. Windows first (matches contract "P1, Win"); macOS behind the same seam is
   P2. Linux: TouchDesigner doesn't run there — out of scope.

Non-goals (P1): lifecycle/start-flow dialog handling (no lifecycle tools exist
in rs v1; *rev 4: the v2 proposal adds `spawn_td` and owns start-flow policy there — this
annex supplies the mechanics*), detecting popups of never-handshaken TD processes (*rev 4:
resolved for spawned pids via pre-handshake registration, proposal §3.6*), event-push over
the bridge protocol, UI automation *inside* TD's own windows (that is what
`editor_context`/`capture` are for).

## 3. Prior art — JS POC (`../td-mcp`) and lessons

Mechanism (all Node → PowerShell): enumeration via .NET UIA
(`RootElement.FindAll(TreeScope::Children, ProcessIdProperty == pid)`), message
extraction by joining descendant element names, dismissal via inline C#
P/Invoke (`FindWindow('#32770', title)` → `SetForegroundWindow` → post
`WM_KEYDOWN/VK_RETURN` → 400 ms sleep → `WM_CLOSE`). Classification by regex:
hard = `/unexpected node(?: name)?\s+duplicat|THREAD CONFLICT|cannot be
referenced from separate threads/i`; soft = `/Backwards Compatiblity Issue/i`
(TD's own typo — keep verbatim); else unknown. Safety rail: never touch titles
that are empty or match main-window chrome (`^touchdesigner(\s|$)`, not
followed by `/`). Timeouts everywhere (2 s inspect / dismiss), degraded to a
"light" probe (`Get-Process …Responding`) after 2 consecutive timeouts;
start-flow integrated inspect+dismiss each poll tick; live e2e forced a
duplication dialog by staging a root-level `.tox` copy.

What was fragile — fix in rs:

| POC weakness | rs fix |
| --- | --- |
| Enumeration had no class/style check — any named non-chrome top-level window listed as a "dialog"; `#32770` checked only at dismiss time so listed-but-not-dialog windows silently no-op'd | Classify at enumerate time: dialog class `#32770` (+ owned-window / style corroboration), keep chrome exclusion |
| Dismissal fire-and-forget: printed `{"dismissed":true}` once the handle was found, never verified closure | Verify-gone loop after dismiss (re-enumerate ≤ ~1.5 s), report `stillOpen` |
| Message extraction lossy ("names > 3 chars joined with `\|`") | Filter child controls by class (Button/Static/Edit), map buttons with labels + default flag |
| Exact-title matching conflates duplicate-titled dialogs | Identify by hwnd-derived id, not title |
| PowerShell spawn per probe (~100 ms+ overhead, SIGKILL cleanup) | In-process Rust backend on one dedicated thread; hard budget + cached snapshot |
| Detection lived beside the thing it diagnosed (same-process tooling) | Daemon-side OS-level detection — works precisely when TD's main thread is wedged (see §4) |

Portable verbatim: severity regexes, chrome-title guard, light/degraded-probe
idea (maps to `window_status`), "hard ⇒ surface loudly, don't auto-fix" policy.

## 4. Architecture decision — daemon-side detection, vendored platform wrapper

Rejected alternatives:

- **Bridge-side (python ctypes inside TD).** Precedent exists
  (`transport.py` does win32 via ctypes), but (a) methods other than `ping`
  dispatch on TD's main thread — exactly what a modal dialog blocks, so
  detection dies with the disease; (b) wire `Message::Event` push is a dead
  letter today in both directions (daemon treats non-Response inbound frames
  as disconnect; python skips non-request frames) — enabling push is protocol
  work with mid-call hazards. 
- **Scattering raw FFI across consumer crates.** Uncontrolled `unsafe`
  surface in daemon/mcp code would gut the constitution's guarantees.
  Whatever calls Win32 must be one isolated place with a safe public surface.

**Chosen:** vendor our own safe wrapper over the OS APIs inside the new
**`crates/tdmcp-dialogs`** crate. Windows ships first with a **hybrid user32 +
in-house UIA** backend; macOS later implements the same narrow internal `sys`
facade (§5.1) over CGWindowList + the Accessibility API. Only the
third-party [`uiautomation`](https://docs.rs/uiautomation) *crate* is
rejected — its shape is Windows-COM-specific (advances the macOS story not at
all) and drags opaque COM-apartment behavior into the daemon — **not** the UI
Automation technology, which we wrap ourselves over the official
[`windows` crate](https://microsoft.github.io/windows-docs-rs/) inside our
enclave.

Division of labor, and why both layers exist:

| Layer | Job | Why |
| --- | --- | --- |
| **user32** (always) | Per-pid enumeration, classification, dismissal posts, hang probe | Cheap (<5 ms/pid), no COM, safe for 1 s polling |
| **UIA** (on demand) | Message text + button labels/defaults for windows where classic controls don't exist; `InvokePattern` click fallback; `WindowInteractionState` corroboration | **TouchDesigner is a Qt app** — its own error dialogs are Qt-hosted: `EnumChildWindows` finds few/no `Button`/`Static` controls, so pure user32 would detect the popup but report *empty message and zero buttons* — failing the primary use case. Evidence: the JS POC dismissed `#32770` frames but read message text via UIA descendant names |

user32 primitives (classic Win32 dialogs — OS file dialogs, MessageBox):

| Need | API |
| --- | --- |
| Per-pid enumeration | `EnumWindows` + `GetWindowThreadProcessId` |
| Classification | `GetClassNameW` (`#32770`), `GetWindowLongW(GWL_STYLE/GWL_EXSTYLE)` (`WS_POPUP`, `WS_DLGFRAME`, `WS_EX_DLGMODALFRAME`), `GetWindow(GW_OWNER)` owner chain |
| Content | `EnumChildWindows` filtered by class (`Button` / `Static` / `Edit`…), `GetWindowTextW`, `GetDlgCtrlID` (`IDOK=1`, `IDCANCEL=2`, `IDYES=6`, `IDNO=7`) |
| Default button | per-button style check `GWL_STYLE & BS_DEFPUSHBUTTON` — no messages sent |
| Dismissal | `PostMessageW(BM_CLICK)` on the button hwnd (no focus steal — improves on the POC's `SetForegroundWindow`+Enter hack), fallback `WM_COMMAND(IDCANCEL)` to the dialog, then `WM_CLOSE` |
| Hang probe (`window_status`) | `SendMessageTimeoutW(hwnd, WM_NULL, SMTO_ABORTIFHUNG)` |

UIA additions (raw COM via `windows` crate features `Win32_UI_Accessibility` +
`Win32_System_Com`; `CoCreateInstance(CLSID_CUIAutomation)`): element lookup by
`NativeWindowHandleProperty`, subtree walk for name/role pairs (message
statics, buttons), `IUIAutomationInvokePattern::Invoke` for buttons with no
native ctrl id, `IUIAutomationWindowPattern::CurrentWindowInteractionState`
(`BlockedByModalWindow` / `NotResponding`). Content extraction composes:
user32 controls first, UIA fills gaps — deduped by role+name.

Unsafe policy (constitution carve-out, done properly):

- ALL `unsafe` lives under `crates/tdmcp-dialogs/src/sys/windows.rs` (FFI shim;
  UIA COM calls included — split into a `sys/windows/uia.rs` submodule for
  reviewability); its public API is **100% safe**, and portable logic above
  the facade (classification, dismiss ladder, budgets) contains none.
- `[workspace.lints.rust] unsafe_code = "forbid"` cannot be allow-overridden,
  so `tdmcp-dialogs` opts out of wholesale workspace-lint inheritance and
  restates the workspace lints except `unsafe_code`; that change lands with a
  `RISKS.md` entry + constitution amendment note in the same PR, per
  [CONSTITUTION.md](../CONSTITUTION.md). Clippy `undocumented_unsafe_blocks`
  warned in-crate; every block carries a `// SAFETY:` comment.
- Dependency cost: single `windows` crate dep, features `Win32_Foundation`,
  `Win32_UI_WindowsAndMessaging`, `Win32_UI_Accessibility`,
  `Win32_System_Com` — gated behind cfg; zero new deps on non-Windows targets.

Threading rule: ops are serialized through ONE dedicated worker thread; that
thread owns COM init (`CoInitializeEx`) for the UIA client — created once,
never touched from other threads. user32-only ops (poll path) never initialize
COM, so a UIA failure cannot degrade detection. Every async call is bounded
and fails open on timeout — detection must never make a healthy call worse.

## 5. Design

### 5.1 Types & seam

New crate **`crates/tdmcp-dialogs`** (platform logic; depends on
`tdmcp-core` + safe platform crates only). Domain types live in
**`tdmcp-core`** (pure, serde camelCase — fleet/mcp serialize them):

```rust
// tdmcp-core/src/dialogs.rs (new)
pub struct PopupInfo {
    pub id: String,            // hwnd-derived, stable while the window lives
    pub title: String,
    pub class: Option<String>, // "#32770", …
    pub kind: PopupKind,       // MessageBox | FileDialog | Custom | Unknown
    pub severity: DialogSeverity, // Hard | Soft | Unknown (POC regexes)
    pub message: Option<String>,
    pub buttons: Vec<PopupButton { id: String, label: String, is_default: bool }>,
    pub is_main_chrome: bool,  // true ⇒ never dismissable
}
pub enum WindowStatus { Responsive, BlockedByModalWindow, NotResponding }
```

Seam trait in core (justified despite constitution's "reject single-impl
traits": it carries a null impl for non-Windows + test fakes + macOS P2):

```rust
pub trait DialogSource: Send + Sync {
    fn snapshot(&self, pid: u32) -> DialogSnapshot;     // popups + window_status
    fn describe(&self, pid: u32, id: &str) -> Result<PopupInfo, DialogError>;
    fn dismiss(&self, pid: u32, id: &str, button: Option<&str>)
        -> Result<DismissOutcome, DialogError>;        // outcome includes stillOpen
}
```

Backends: `Win32Source` (cfg(windows)) and `NullDialogSource` (other targets /
feature-off; empty snapshots, `tdmcp.dialog.unsupported`). The daemon
constructs the source at startup and passes it into MCP dispatch next to the
registry.

Crate layout — the extension point for macOS is the `sys` facade only;
classification/policy reuse is free:

```text
crates/tdmcp-dialogs/src/
  lib.rs        safe public surface: DialogSource impls, budgets
  classify.rs   severity regexes, kind detection, chrome guard   (portable, unit-tested)
  policy.rs     dismiss ladder + verify-gone loop                (portable)
  sys/
    mod.rs      facade contract + cfg dispatch
    windows.rs  user32 FFI (unsafe) → safe facade fns
      uia.rs   raw-COM UIA module (unsafe) → content/invoke/state fill-ins
    macos.rs    P2 placeholder (returns unsupported today)
```

Facade call set (the entire future macOS port surface — note the accessibility
layer is *inside* each backend: UIA on Windows, AX on macOS):
`top_level_windows() -> Vec<SysWindow{pid, id, class, title, visible, styles,
owner}>`, `child_controls(id) -> Vec<SysControl{id, class, label, ctrl_id,
is_default}>` (backend-composed: classic controls ∪ accessibility tree),
`post_click(id, ctrl_id) -> bool` (`false` ⇒ caller tries `press`),
`press(id, button) -> bool` (accessibility invoke), `post_close(id)`,
`is_hung(id, budget_ms) -> bool`.

### 5.2 Windows backend mechanics

- **Enumerate** (`top_level_windows`): visible top-level windows of the pid,
  classified at enumerate time: dialog iff class `#32770`, corroborated by an
  owner chain pointing at the pid's main window and/or popup/dialog styling.
  Chrome guard ported from POC: skip exact `"TouchDesigner"` and
  `"TouchDesigner *"` titles (main editor chrome
  `"TouchDesigner 2023.…: path/to.toe"`).
- **Content** (`child_controls`, composed): classic controls first
  (`Button`/`Static`/`Edit` → labels, ctrl ids, default flag); when that comes
  up empty for a classified popup (Qt/DirectUI hosts), UIA fills in from the
  accessibility subtree (message statics, buttons with names). Result cached
  per hwnd+generation — UIA runs once per new popup (~10–50 ms), never on the
  poll path.
- **Severity:** ported POC regexes over title+message (§3).
- **Dismiss ladder** (`policy.rs`): explicit `button` (ci label or ctrl id) →
  default button (`BS_DEFPUSHBUTTON`, else UIA IsDefault/name match) →
  `WM_CLOSE`. Steps post via `PostMessageW(BM_CLICK)` / UIA `Invoke` /
  `PostMessageW(WM_CLOSE)` — none block on the possibly-wedged target thread;
  then a verify-gone re-enumeration runs ≤ ~1.5 s; result carries `stillOpen`.
  Refuse `is_main_chrome` targets with `tdmcp.dialog.chrome_protected`;
  pre-flight `is_hung(dialog)` guards sends (SMTO_ABORTIFHUNG semantics).
- **Budgets:** snapshot ≤150 ms (cached, see §5.3), describe ≤500 ms, dismiss
  ≤3 s wall including verification. `window_status`: owned popup present ⇒
  `BlockedByModalWindow`; `is_hung(main)` ⇒ `NotResponding`; else
  `Responsive`.

### 5.3 Detection loop & state

Daemon background task (spawned in `run_daemon` wiring, `cfg(windows)` +
config-gated): every `[dialogs].poll_ms` (default 1000) iterate registry pids
with `bridge == Connected` (*rev 4: predicate widens to `{starting, connected}` so spawned
pids are watched pre-handshake — see proposal §4.4*), call
`snapshot()`, store:

- `ProcessAttrs.window_status` ← `WindowStatus` (fills the reserved field);
- per-pid `DialogSnapshot` cache (side-map keyed by pid in the daemon, TTL =
  poll interval; also written opportunistically on interception probes).

Fleet: `include=popups` emits `popups: [PopupInfo…]` from the cache
(already schema-reserved); `windowStatus` starts flowing for free. Idle-exit
interaction: the watcher touches nothing the idle clock counts (bridges +
MCP leases), so it cannot keep an idle daemon alive; with zero bridges it
loops over an empty set.

### 5.4 Interception gate

Choke point: `enqueue_and_call` — `crates/tdmcp-mcp/src/tools.rs:1221-1259`
(single pass-through for all six bridged tools; `fleet` / `describe_tools` /
`dialogs` stay exempt naturally). At its top:

1. cheap check against cached snapshot for `pid` (cache hit ⇒ ~free);
2. if non-empty popups → return `ToolCallError::Failed(ToolFailPayload)`
   with code **`tdmcp.dialog.blocking`**: summary names count + titles;
   items per popup (severity/title/message-first-line); mitigation lines:
   *"Run `dialogs {pid} action=list`… dismiss via `action=dismiss`…"*; flat
   `{ ok:false, summary, diagnostics, … }` per CONTRACT conventions;
3. cache miss/stale → bounded refresh (~150 ms budget); probe failure or
   timeout ⇒ fail-open (log warn, let the tool proceed).

Config kill-switches: `[dialogs] enabled` (master), `[dialogs] intercept`
(gate only). Default both **true** — requirement #2 is the point of the
feature; operators can opt out.

### 5.5 Tool spec — `dialogs`

Local tool (no `BridgeMethod`, no python changes). Registration chain:
`ToolName` variant + `wire_str` + `description` + `ALL` + `from_wire`
(`tools.rs:63-154`), params struct (`#[serde(rename_all="camelCase",
deny_unknown_fields)]` + `JsonSchema`), `input_schema_for` arm, `dispatch_tool`
match arm returning `json!(…)` directly (local-tool pattern like
`describe_tools`).

```jsonc
{
  "pid": 12345,                       // required
  "action": "list" | "describe" | "dismiss",
  "id": "78910",                      // describe/dismiss; from list output
  "button": "OK",                     // optional; ci label or id; default = default button
}
```

- `list` → `{ ok:true, pid, windowStatus, popups:[PopupInfo…], truncated? }`
  (cap 16, truncation field per house style).
- `describe` → `{ ok:true, pid, popup: PopupInfo(full) }`.
- `dismiss` → `{ ok:true, pid, dismissed: bool, via: "button:OK"| "close",
  stillOpen:[ids], remaining:[PopupInfo…] }`. Never ok-fakes success (POC
  lesson).
- Errors: `tdmcp.dialog.unsupported` (non-Windows/feature-off),
  `tdmcp.dialog.not_found` (stale id — hwnd reuse race is why ids are
  re-verified against title/class before acting), `tdmcp.dialog.dismiss_failed`,
  `tdmcp.dialog.chrome_protected`. All codes land in
  `crates/tdmcp-diagnostics/src/codes.rs` `ALL` **and**
  `diagnostics/catalog.yaml` in the same change (completeness tests enforce
  both directions).
- Annotations: none exist anywhere in the repo today (`tool_from_descriptor`
  builds plain `Tool::new`). Minimal move consistent with the codebase: ship
  without annotations; note in description that `dismiss` mutates OS window
  state. If annotations are wanted, add them for this tool only in
  `rmcp_handler.rs` (`destructiveHint=true` via action presence isn't
  expressible per-tool — another reason to skip).

## 6. Config additions

Follow the 7-touchpoint pattern (`crates/tdmcp-config/src/lib.rs`):
section struct + `Default` + field on `ConfigFile` → commented block in
`assets/default.toml` → `FIELD_DESCS` entry (GUI tooltips) → `save()` write
lines (toml_edit comment-preserving writer) → tests
(`load_parses_all_sections`, `default_toml_parses`).

```toml
[dialogs]
enabled   = true    # master switch (watcher + tool)
intercept = true    # fail-fast gate on bridged tool calls
poll_ms   = 1000    # watcher cadence
```

## 7. Risks & mitigations

| Risk | Mitigation |
| --- | --- |
| UIPI: elevated TD + non-elevated daemon blocks posted messages | Detect access-denied class of failures (`PostMessage` last-error) → distinct error/help text in catalog entries |
| Dismissing save-prompts discards unsaved work | Severity surfacing; chrome guard; dismiss requires explicit tool call (agent's choice); doc warning; never auto-dismiss outside start-flow (*rev 4: start-flow = `spawn_td` surfacing only — nothing is ever auto-dismissed, save-prompts included*) |
| hwnd reuse between snapshot and dismiss | Re-verify title/class before acting; stale ⇒ `tdmcp.dialog.not_found` |
| False positives (named non-dialog top-levels) | Enumerate-time `#32770`/style/owner classification + chrome guard (POC lesson) |
| Probe cost on hot path | Poll path is user32-only (no COM); UIA content runs once per new popup hwnd and is cached; ops serialized through one worker off the tokio runtime |
| UIA provider quirks/hangs on wedged processes | Hard per-call budgets + fail-open to user32-only data (popup still detected, message shown as absent); COM client confined to the worker thread |
| Watcher keeps idle daemon alive | It counts nothing toward idle liveness (bridges+leases only) |
| Constitution: `unsafe_code` forbid | Single sanctioned enclave: `tdmcp-dialogs/src/sys/windows*`; safe public API; lint opt-out + RISKS.md entry land in the same change |

macOS (P2, sketch): implement `sys/macos.rs` only — enumerate via
`CGWindowListCopyWindowInfo` (pid attribution, `kCGWindowNumber` as id),
read/press via the Accessibility API
([`axuielement`](https://docs.rs/axuielement) / `accessibility` crates, safe
wrappers over objc2; `AXUIElementCreateApplication(pid)` → windows →
`AXPress` on buttons). Requires granting the daemon TCC Accessibility
permission — document as an install step. Everything above the facade
(classification, ladder, budgets, tool surface, interception) is reused
unchanged; no other crate changes.

## 8. Implementation plan (phased)

*Rev 4: milestones below map onto the unified v2 roadmap — M1 → V2-A, M2–M4 → V2-D,
M5 → V2-G; sequencing/dependencies live in [`SKILLS_CONTRACT_PROPOSAL.md`](SKILLS_CONTRACT_PROPOSAL.md) §6.*

| Milestone | Content | Verification |
| --- | --- | --- |
| **M1 platform crate** | `tdmcp-core/src/dialogs.rs` types + trait; `crates/tdmcp-dialogs`: `sys/windows.rs` user32 shim + `sys/windows/uia.rs` COM module (all unsafe; lint/RISKS carve-out), portable `classify.rs` + `policy.rs`, `Win32Source`, `NullDialogSource` | Unit tests on classifier/ladder (fixture-driven, no OS); `#[ignore]`-gated live test that spawns a real MessageBox (`powershell System.Windows.Forms.MessageBox`) then lists+describes+dismisses it through the real user32 path — no TD needed; UIA content path covered by unit fixtures against recorded element trees + manual E2E on real TD; `cargo clippy --workspace --all-targets -- -D warnings` incl. in-crate unsafe hygiene |
| **M2 detection loop** | Watcher task in daemon; `window_status` + snapshot cache; `fleet include=popups` emission (fixture `fleet.json` update) | Integration test with fake `DialogSource`: connected FakeTdPeer pid + stubbed popup ⇒ fleet shows `popups`/`windowStatus` |
| **M3 tool** | `dialogs` registration chain + local dispatch arm + codes/catalog entries | Unit (arg parse, shapes) + HTTP/rmcp integration via FakeTdPeer; parity untouched (no python changes) |
| **M4 interception** | Gate in `enqueue_and_call` + config keys (7-touchpoint pattern) | Integration: stubbed popup ⇒ `execute_python` returns `tdmcp.dialog.blocking` without enqueue; kill-switch off ⇒ passes through; probe-timeout ⇒ fail-open |
| **M5 docs** | CONTRACT.md rows Planned→Shipped (+ result-shapes row), README checkbox, CONFIG.md section, E2E_CHECKLIST new section (live-TD rows incl. forcing a real TD dialog e.g. duplicate-node load trick from POC e2e) | Manual E2E run record |

Decision checkpoint after the first live-TD pass: confirm which real TD
dialogs are classic (`#32770` + controls) vs Qt-hosted, verify UIA content
extraction covers the Qt-hosted ones, and confirm poll-path cost stays
~<5 ms/pid (user32-only). If any popup class resists both layers, record it
as a limitation with its window class for follow-up.

## 9. Limitations & open questions

- Popups of TD processes that never handshook are invisible (registry rows are
  handshake-created only; no daemon-side process scan exists). Startup-modal
  TDs therefore stay undetectable until lifecycle (P2) adds process launch.
  *Rev 4: resolved for spawned pids — `spawn_td` registers pre-handshake
  (proposal §3.6), and the watcher predicate includes `starting` rows. Human-opened,
  never-handshaken TDs remain invisible by design.*
- Duplicate-titled dialogs disambiguate by id only after `list`.
- Open questions (recommendations in §5; adjust during review):
  1. Interception default-on? (recommended yes, `intercept=false` escape)
  2. Dismiss default target = default button? (recommended yes)
  3. Keep contract name `dialogs`? (recommended — it is already reserved)

## 10. References

- [`windows` crate](https://microsoft.github.io/windows-docs-rs/) — official Win32/COM bindings for the shim (`Win32_Foundation`, `Win32_UI_WindowsAndMessaging`, `Win32_UI_Accessibility`, `Win32_System_Com`).
- [IUIAutomation](https://learn.microsoft.com/en-us/windows/win32/api/uiautomationclient/nn-uiautomationclient-iuiautomation) / [WindowInteractionState](https://learn.microsoft.com/en-us/windows/win32/api/uiautomationcore/ne-uiautomationcore-windowinteractionstate) — raw COM surface we wrap in-house.
- [`uiautomation` crate](https://docs.rs/uiautomation) — evaluated, rejected as a dependency (§4); useful API-shape reference.
- [axuielement crate](https://docs.rs/axuielement) (macOS accessibility, P2).
- Prior art: `../td-mcp` `src/lifecycle/tdDialogs.ts`, `src/features/tools/handlers/tdTools.ts` (`td_ui_dialogs` tool), `scripts/liveTdDialogs.mjs`.
- td-mcp-rs anchors: `crates/tdmcp-mcp/src/tools.rs:63-154` (ToolName), `:647-963` (dispatch_tool), `:1221-1259` (enqueue_and_call); `crates/tdmcp-diagnostics/src/codes.rs` + `diagnostics/catalog.yaml`; `crates/tdmcp-config/src/lib.rs`; `crates/tdmcp-core/src/registry.rs:26-38`; `crates/tdmcp-mcp/src/fleet.rs:32-47`; `bridge/tdmcp_bridge/task_queue.py` (main-thread pumps, ping-on-worker).

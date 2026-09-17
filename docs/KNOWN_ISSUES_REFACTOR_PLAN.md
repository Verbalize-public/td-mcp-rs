# Known-issues audit and refactoring plan

Implementation progress and evidence: [acceptance ledger](KNOWN_ISSUES_IMPLEMENTATION.md).

## Decision summary

Audit target: [KNOWN_ISSUES.md](KNOWN_ISSUES.md), originally requested as
`KNOW_ISSUES.md`. Baseline source commit:
`9f0df39d2762e28b48df59c8cd7d48e95edbad74`, plus the working-tree documentation
and skill edits present during this audit. This is a **plan, not an implemented
fix or live-TD acceptance report**.

The field notes are valuable incident reports, but not yet a defect specification.
Their symptoms, causal explanations, severity labels, and suggested fixes need
separate treatment. In particular:

1. **Do not rewrite capture routing on the strength of T1 alone.** The current
   explicit-PID dispatch/bridge regression passes. Locate the failing boundary
   and installed version first; keep capture recovery the first investigation.
2. **Do not silently introduce auto-append or a moving batch context.** T2 and
   T6 describe documented defaults. Improve intent, diagnostics, and safe usage
   without changing existing graph construction behind callers’ backs.
3. **Fix false confidence before adding shader cleverness.** The current shader
   classifier positively labels empty/unrecognized logs as compiled. Inspection
   also loses the distinction between unreadable and clean observations.
4. **Treat most D-items as experiments, not universal TD rules.** Cooking an
   output repeatedly is not the same test as the existing callback-based timing
   runner. The report did not attempt timed capture. No new timing engine is
   justified yet.
5. **Ship a small reproducible scene suite, not just the final lightEngine.**
   Isolate wiring, attributes, render visibility, alpha, clocks, and CHOP
   signals before combining them. Preserve a separate integrated showcase.

Completion means every issue has either a verified product fix, a corrected
contract/recipe with regression evidence, or a narrowly reproduced upstream
limitation with an explicit workaround and open upstream disposition. An
upstream limitation is not “fixed” merely because it is documented.

## Scope, evidence, and constraints

- This pass inspected local source, contracts, reference templates, and selected
  tests. It did not inspect the external `test.2.toe` or `le_*.png` artifacts.
- Live TD capabilities are unavailable in this session. No native/Wine behavior,
  rendered look, performance, or installed daemon identity is independently
  verified here. No external TD documentation was fetched. Repository cards
  supply context, not independent confirmation of the field notes.
- The original issue file was untracked. Existing staged `skills/` and
  `claude-skills/` changes are concurrent work; this audit does not overwrite or
  re-render them. Source claims below refer to the inspected working tree.
- Independent audit routing was attempted but the available task-object schema
  was not established after three validation failures. The audit continued
  inline; there is no claimed independent reviewer verdict.
- No runtime implementation, installation, daemon restart, commit, or publication
  is part of this change. Preserve PID-only targeting and crate boundaries.

Evidence labels used below: **confirmed** means source or a local executable
probe establishes the stated narrow fact; **contract mismatch** means the
reported expectation conflicts with the current API; **unresolved** means the
symptom is reported but its cause or generality still needs reproduction. A
passing fake test does not disprove a live incident.

## Source map

Line references describe the audit baseline, not permanent identifiers.

| Ref | Source and relevant boundary |
| --- | --- |
| E1 | [tools.rs](../crates/tdmcp-mcp/src/tools.rs), `CaptureParams` 489–520, `CaptureMode` 438–468, capture dispatch 1350–1416. PID is required, parsed before routing, then passed separately to the bridge call. |
| E2 | [rmcp_handler.rs](../crates/tdmcp-mcp/src/rmcp_handler.rs), 140–174: `request.arguments` passes to `dispatch_tool`; image promotion tests 328–391. [args_diag.rs](../crates/tdmcp-mcp/src/args_diag.rs), `parse_args`, `args_error`, `join_path`. |
| E3 | [bridge_session.rs](../crates/tdmcp-daemon/tests/bridge_session.rs), `capture_round_trip` 197–217; [error_surface.rs](../crates/tdmcp-daemon/tests/error_surface.rs), invalid capture PID 164–173; [federation_proxy.rs](../crates/tdmcp-daemon/tests/federation_proxy.rs), capture proxy assertions around 430. |
| E4 | [mutate.py](../bridge/tdmcp_bridge/mutate.py), `_apply_values` 147–182, `_apply_expressions` 213–248, create 529–620, set 788–861, connect 892–978, alias/batch handling 1039–1153. [tools.rs](../crates/tdmcp-mcp/src/tools.rs), `MutationStep::Connect` 646–657. |
| E5 | [paths.py](../bridge/tdmcp_bridge/paths.py), `tdmcp_resolve` 10–17 and `_absolutize_path` 42–50. [CONTRACT.md](CONTRACT.md), mutation steps, partial-apply semantics, and OpPath sections. |
| E6 | [shader_lint.py](../bridge/tdmcp_bridge/shader_lint.py), stage map 14–44, classifier 74–110, consumer discovery 113–200; [test_shader_lint.py](../bridge/tests/test_shader_lint.py). |
| E7 | [inspect.py](../bridge/tdmcp_bridge/inspect.py), wire/message reads 74–103, parameter evaluation 192–203, shader content 283–302, top-level inspection 449–543. |
| E8 | [timing.py](../bridge/tdmcp_bridge/timing.py), callback schedule/reset/check/advance/sample 273–370; [test_timing.py](../bridge/tests/test_timing.py), reset/warmup, interference, reload, cleanup tests. |
| E9 | [test_mutate.py](../bridge/tests/test_mutate.py), fake connector 39–53; connection, context, stop-on-failure, and rename tests around 692–850. |
| E10 | [POPs reference](../skills/templates/touchdesigner/reference/pops.jinja.md), especially 35–37 and 62–74; [render primer](../skills/templates/touchdesigner/primer/glsl-and-render.jinja.md), 16–26; [Python reference](../skills/templates/touchdesigner/reference/python-api.jinja.md), scope, paths, flags, methods, and menus. |
| E11 | [timed capture reference](../skills/templates/touchdesigner/reference/timed-capture.jinja.md); [look grading](../skills/templates/touchdesigner/reference/look-grade.jinja.md), 24–32; [testing](TESTING.md); [current limitations](OPEN_WORK.md). |

## Audit of tool findings

### T1 — Capture rejects a supplied PID

**Verdict: incident unresolved; current local dispatch counterexample confirmed.**
The existing `capture_round_trip` passes an explicit numeric PID and reaches a
fake bridge successfully. The invalid-string PID test also produces its expected
curated error. E1/E2 do not show a PID being removed before `CaptureParams`
validation. Its absence from the Python payload is intentional: the Rust bridge
call already routes by PID. Do not “fix” this by making PID optional, forwarding
it everywhere, or inventing sticky targets.

The original exact transmitted JSON, request/response ID, harness/tool adapter,
daemon executable/version, and schema snapshot are missing. An intended call
is not proof of the serialized call. Conversely, a local passing test does not
clear a deployed old binary, stdio adapter, federation route, or MCP envelope.
The `..pid` spelling may be diagnostic root-path formatting: E2 `join_path`
handles an empty root but not a literal `.`. Test that separately; it does not
explain why PID was absent.

**Next action:** replay one minimal numeric-PID capture over each actual boundary
used by the failing client, retaining sanitized arguments and argument-layer
diagnostics. Distinguish client validation, server deserialization, dispatch,
bridge execution, and image promotion. Compare current source, built artifact,
installed artifact, running PID, and `describe_tools` schema. Timed job status
also requires PID; a job ID is not an implicit target. See P1.

**Priority:** first investigation because visual verification depends on it;
repository-wide “P0, unusable” is not established by this one session.

### T2 — Multi-input connect replaces previous wiring

**Verdict: contract mismatch plus a confirmed safety/observability gap.** E4/E5
explicitly default `dstInput` to 0 and call the selected connectors. No append
semantics are promised. Resolved `srcOutput`/`dstInput` already appear in
`detailLevel: detailed`; the missing feature is useful summary feedback and
replacement detection, not an entirely absent echo. Actual TD replacement
behavior remains live-reported here.

Auto-append can miswire fixed-role inputs, fill an intentional hole, or duplicate
a retried request. An unconditional hard error would break intentional rewires.
The fake connector in E9 appends both sides, so it cannot currently model the
reported replacement hazard faithfully.

**Decision:** retain legacy default selection; add summary connector identity,
previous-connection evidence, and an explicit occupied-input policy. Use explicit
indices in all multi-input recipes. Improve the fake only after observing actual
TD connector behavior for fixed and variable input families. See P2.

### T3 — Merge POP wires and parameter references collide

**Verdict: unresolved TD semantics; proposed read-only restriction rejected.**
The bridge directly assigns values and connects wires (E4), without understanding
`inputNpop`. The report associates a count change with mixed input mechanisms;
that does not prove duplication explains a reduction from 3001 to 960. A source
with the same path may also intentionally appear more than once.

Build wires-only, references-only, and mixed variants from two tiny distinguishable
inputs. Inspect evaluated references, actual edges, point/primitive counts, and
attribute compatibility at each hop. Test reference clearing with the parameter’s
actual supported empty value; do not generalize `None` into an all-parameter
clear operation.

**Decision:** document a verified simple construction pattern first. Add only a
bounded, non-mutating overlap warning if live evidence establishes meaningful
equivalence. Never clear references automatically, forbid legitimate parameter
inputs, or call writable parameters “read-only hints.” See P3/P4.

### T4 — GLSL POP compile diagnostics unavailable

**Verdict: confirmed bridge capability gap, not proof TD has no diagnostic API.**
E6 scans `glslPOP` but excludes it from the verified compile-result allowlist.
E7 omits its `compileState`. This proves what this implementation supports,
not what every TD build exposes. The observed one-line failure is still useful
evidence and should be preserved with its source, rather than fabricated into
a compiler log or injected into TD-owned `op.errors()`.

**Decision:** probe available surfaces using one valid and one intentionally
invalid shader in the same scratch scene. Candidate surfaces include OP messages
and available info/compile members discovered on that build; none is promised
until tested. Centralize observation and expose explicit unknown/unsupported
status when there is no reliable source. Do not require an upstream API to
implement honest reporting. See P1/P4 and N1 below.

### T5 — Output attribute menu case silently loses Color

**Verdict: generic validation gap confirmed; exact live coercion unresolved.**
`_apply_values` assigns `.val` and treats no exception as success; it neither
checks menu membership nor verifies an evaluated result (E4). The Python card
already documents `menuNames`/`menuLabels`/`menuIndex` (E10). Catching an invalid
closed-menu token is narrower and more useful than parsing arbitrary GLSL.

A regex for every written shader symbol cannot soundly handle generated headers,
macros, includes, aliases, local variables, or attribute classes. It would
confuse static suspicion with a compiler verdict. `outputattrs` passthrough and
new-attribute declaration must be observed separately.

**Decision:** introspect menu capabilities, reject demonstrably invalid tokens
for verified closed menus, provide suggestions without autocorrection, and
expose post-write value/evaluation evidence. Preserve legitimate custom/dynamic
menus and expression/export/bind modes. A shader-specific declaration warning
is secondary and must be explicitly heuristic. See P3.

### T6 — Relative creation uses the wrong context

**Verdict: contract mismatch; adjacent rename hazard confirmed.** All batch
steps use the supplied `contextPath` or `/project1` (E4/E5). Creating a parent
does not enter it. Creating `/project1/lightEngine` followed by `lfoSwell` without
a new context therefore does not establish an implementation defect.

Use `lightEngine/lfoSwell` relative to `/project1`, or create the COMP first,
read its returned canonical path, then use that path as the next batch context.
The latter also handles TD renames. Do not add hidden “current COMP” state.

There is a separate real limitation: alias rewriting looks up **exact** paths.
If `lightEngine` becomes `lightEngine1`, `lightEngine/child` is not remapped. A
local function probe confirmed this. The existing contract only clearly
promises exact-path remapping; descendant behavior needs explicit design and
collision tests, not an assumption that it already works. See P2 and N2.

## Audit of TD semantic findings

All live symptoms below remain attributed to the reported TD 2025.33070
Wine/Linux session. Tests in this table are required experiments, not completed
results. Run on that environment first, then a supported native environment
before calling a behavior upstream-general or Wine-specific.

| ID | Assumption challenged | Discriminating experiment and intended improvement |
| --- | --- | --- |
| D1 | “The containing network is the base” and “absolute paths are the only reliable form” overgeneralize several resolution mechanisms. A bad reference and missing Color do not establish the entire shader-failure causal chain. | In a nested fixture record `me.path`, `parent().path`, evaluated `op()` results, owner-relative OP method results, OP-path parameter values, and `tdmcp_resolve` results separately. Test sibling/child/parent/missing paths and move the whole COMP. E5 is the MCP rule, not the TD expression rule. Publish a context matrix and use relocatable internal references once proven; do not mechanically absolutize component internals. Surface evaluation failures through P1/P3. |
| D2 | Default torus creation is attributed to TD, but “every recipe must delete it” is unsafe outside a newly owned fixture. | Snapshot a newly created Geometry COMP’s children, parameter defaults, selected render source, and flags; compare supported initialization choices. Remove only the positively identified fresh default in the recipe-owned COMP, or explicitly select the intended geometry. Never delete arbitrary `torus1` nodes in user graphs. |
| D3 | “All POPs come with false flags” and “torus rendered despite false flags” do not isolate what the renderer consumed. E4 does not unconditionally force these flags. | Hold camera, MAT, source and primitive data fixed; vary terminal POP display/render, COMP render state, bypass/lock/cooking, output wiring and selected geometry source one at a time. Record the actual rendered object and a captured difference. Document the observed gating, not a global flags workaround. |
| D4 | Closed linestrips always fill and points cannot render are not proven by one scene. The existing POP card distinguishes point data from Point primitives (E10). | Use equal visible positions with no primitives, Point primitives, lines, open/closed strips, and triangles. Record primitive metadata, MAT/camera configuration and clipping, culling and color/alpha. Test verified point rendering settings or instancing as a separate route. Publish a supported recipe, not a blanket primitive ban. |
| D5 | Alpha-zero background proves a render is empty, and opaque rendering/POP-only compositing is universally required. | Preserve and inspect raw RGB and alpha separately; compare the same frame over checkerboard and known black/white backgrounds. Sweep only background alpha, source alpha and premultiply state. Test a simple TOP-only reference before POP rendering. State the output’s straight/premultiplied-alpha convention. Keep opaque output as one recipe option, not a global fix. |
| D6 | One `choptoPOP` parameter toggle proves how all channel mappings work. | Feed deterministic CHOP channels with explicit sample counts and ranges; enumerate available mapping menus; inspect P.x/P.y/P.z and dimensions after each mapping. Build a minimal x-index/y-amplitude scope with no shader or noise, then add placement/scale. Verify the claimed absence of wire inputs on the target build. |
| D7 | Repeated forced cooks establish that feedback is unusable outside live playback or cannot be frame-stepped. | Compare repeated same-frame cooks, actual live playback, and the existing reset/initialize/sequential-callback timing job (E8/E11) using a deterministic impulse/decay scene. Measure frame identity and output history. The timing runner already separates callbacks and demands intervening frames; add acceptance, not another engine. Reset behavior needs its own test before declaring reset broken. |
| D8 | Trail POP is the cause of corruption rather than source selection, mixed references, primitive mismatch, stale state or clock handling. | Start with a moving two-point input and trail alone; inspect per-frame counts/primitive types, reset/history length, and `surftype` choices. Add rendering, then merge, then parameter references in separate variants. Compare playback and timed stepping. Count 960 is a clue, not a diagnosed corruption signature. |
| D9 | Identical saved output proves bloom is broken; blur/add is “cheaper.” | Use a controlled HDR bright feature, verify source/operator identity, bypass/lock/cook state, input format/range and evaluated parameters, then inspect numeric output deltas and viewed images. Compare fresh frames, not merely encoded-file equality. Benchmark both alternatives during live playback before any performance claim. Retain blur/add as an aesthetic alternative, not equivalent bloom semantics. |
| D10a | Merge CHOP universally inherits only the first channel name; downstream rename never works. T2 could have left only one input connected. | Feed uniquely named sources into explicit distinct inputs; inspect all channels at each node, then test the actual supported rename parameter on a specific operator. Separate duplicate-name handling from lost wiring and unsupported parameter guesses. |
| D10b | LFO bias is ineffective for sine generally. | Measure min/max/mean over a full cycle after recording waveform mode, parameter mode/evaluation, amplitude, bias, units and sampling/time-slice state. Isolate consumer scaling. Preserve an offset workaround only with stated semantics. |
| D10c | Noise normalization necessarily collapses modulation range. | Fix seed, sample count, rate and mode; compare min/max/mean/stddev with normalization on/off. A ±0.05 observation alone cannot establish the normalization algorithm. Record the useful configuration without generalizing it. |
| D10d | Trigger requires threshold crossings but also “auto-fires on load” as a universal property. | Compare fresh load, explicit reset, constant below-threshold input, and one controlled crossing. Record trigger state over time, retrigger modes and Analyze CHOP windowing. Distinguish initialization from an actual edge. Test the full control signal, not just a peak screenshot. |
| D10e | Channel iteration/sample members and wrapped cook metadata are universal across builds. | Probe exact Channel and CHOP APIs on the target build; retain CHOP sample-count plus indexed access as a verified recipe only. `errors()` and `warnings()` are already called by E7; `allowCooking` is already listed as a property in E10. Never infer freshness from one cook counter: retain clock/frame, reset/run identity and captured output. |

## Audit of documentation gaps and claimed baseline

| Original gap | Current finding and disposition |
| --- | --- |
| No renderable POP scene card | Partly confirmed: E10 contains a short Geometry/MAT/Render summary, not an executable end-to-end fixture with visibility and alpha checks. Extend the canonical references with a minimal recipe and fixture links rather than claiming there is no guidance at all. |
| Add all POP caveats | Do not copy D4/D7/D8/D9 conclusions as rules. Add verified prerequisites, build/platform-scoped incident notes and unresolved reproduction links. |
| Python API omissions | Methods, cooking flags and menu access are partly documented already. Add the missing context matrix and tested sample-access example; correct conflicting broad path wording instead of adding another contradictory paragraph. |
| Capture fallback in look-grade | The current working-tree card already contains explicit PID and transmitted-argument guidance. Do not overwrite this concurrent improvement. A fallback can be documented as a local contingency only: unique authorized file, matching daemon filesystem namespace, verified TOP/frame, bounded resolution, actual image viewing, and cleanup. It must not conceal a broken MCP capture or assume federated files exist locally. |
| Canned lightEngine regression COMP | Useful as integration acceptance, insufficient as a minimal repro. The external project/images were not inspected. Reconstruct or import with permission and record asset identity before treating them as fixtures. |
| “Known-good” final network | It establishes a reported working configuration, not correctness of every internal choice. A clean `inspect` is not shader success or visual proof. The documented inert `Trail` parameter violates a meaningful public control API: implement and test it, or remove/clearly mark it unsupported before shipping the fixture. |

## Additional findings from challenging the assumptions

### N1 — Shader classification can report false success (confirmed)

E6 only detects lines beginning exactly `ERROR:`. Any other non-null text on an
allowlisted type returns `tdmcp.shader.compiled`, including an empty string.
A local direct probe returned `compiled` for `""`, `"Error: Compile failed"`, and
`"unrecognized driver log"` on `glslTOP`. These are function-level test inputs,
not claims that those exact strings occur on a particular live TOP build.

Fix the default verdict to unknown, require affirmative recognized success,
preserve bounded raw evidence, and retain error precedence for mixed logs. Share
this logic between mutation and inspect. Existing 18 shader tests pass while
this issue remains, illustrating a coverage gap.

### N2 — Exact aliases do not cover renamed descendants (confirmed)

A direct `_rewrite_step_aliases` probe with alias
`/project1/lightEngine -> /project1/lightEngine1` left
`lightEngine/child` unchanged. This is not T6’s alleged moving-context bug.
Choose explicit multi-call canonical contexts now; extend descendant mapping
only under the narrowly defined create-intent contract in P2.

### N3 — A current recipe advertises an unsupported capture mode (confirmed)

E11 look-grade lists `pop_data`; E1 `CaptureMode` does not. Its supported set is
`top`, `preview`, `auto`, `chop_data`, `chop_image`, `pop`. Correct the template
and regenerate its checked-in render during implementation, coordinated with
the existing staged edits. Add a schema-backed vocabulary check so rendering
parity alone cannot preserve a wrong option.

### N4 — Unavailable observations look clean (confirmed source behavior)

E7 `_op_messages` converts exceptions into `[]`; `_inspect_param_entry` converts
evaluation exceptions into `val: null`; `_wire_peers` converts iteration failures
into `[]`. Those values are indistinguishable from genuine emptiness or null.
E6 `_eval_par` similarly loses the reason a shader reference could not resolve.
This can conceal D1 and encourage false confidence in the claimed baseline.
Preserve compatibility fields, but add explicit bounded availability/evaluation
status and reasons. Unknown is neither clean nor a hard failure of the entire
inspect batch.

## Target architecture and compatibility decisions

Use existing module boundaries; no new crate, universal graph engine, custom
GLSL parser, transaction framework, or parallel timing subsystem.

- **Rust MCP boundary:** typed arguments and schema remain in `tdmcp-mcp`.
  Capture routing keeps explicit PID/daemon identity. Add tests at actual client
  envelopes before refactoring transport. Keep `tdmcp-core` free of MCP/IPC I/O.
- **Python mutation boundary:** `mutate.py` resolves explicit intent, validates
  known constraints before irreversible work, applies the operation, then
  returns useful evidence. Extract small helpers only at genuine shared seams.
- **Observation boundary:** shader classification remains in `shader_lint.py`;
  inspect and mutation use one observation representation. Missing/unreadable/
  unknown must not become success. Keep inspection bounded and distinguish
  passive metadata reads from compilation/cooking side effects.
- **Time boundary:** retain `timing.py` ownership, callback progression and
  cleanup semantics. Fix only experimentally demonstrated scheduler defects.
- **Knowledge boundary:** executable minimal scenes supply recipe evidence;
  `skills/` remains the source and `claude-skills/` its generated deliverable.
  Field notes retain provenance, while CONTRACT owns shipped behavior.

Proposed wire additions below are design targets, not currently supported API:

| Area | Proposed contract | Compatibility rule |
| --- | --- | --- |
| Connect intent | Optional `onOccupied` with values `"replace"` or `"error"`; omission retains replacement behavior. Explicit indices stay deterministic. | No automatic append. Warn on actual replacement even in legacy mode; explicit safe mode errors before any rewire. Identical existing connection should be a no-op after live identity semantics are established. |
| Connect evidence | Summary includes canonical source/destination, both resolved indices, observed previous peers, and an availability indicator. | Preserve existing `path` and detailed fields. If occupancy cannot be read, safe mode fails closed; legacy mode proceeds with an explicit observation warning. Bound peer lists and report truncation. |
| Parameter validation | Closed-menu token checks using verified parameter capabilities; separate requested, applied, evaluated values and evaluation state where requested/needed. | No lowercasing, arbitrary `None` conversion, or silent mode switching. Dynamic/custom/unavailable menus remain unknown, not invalid. Validate intra-step dependencies in the order TD needs them; no false atomicity promise. |
| Observation status | Add availability/evaluation evidence; shader state distinguishes compiled/error/unknown/unsupported. | Preserve existing result shapes and mutation `ok` meaning (write applied, not shader correct). New diagnostic codes require catalog entries, emitters, tests, and documented consumer behavior. |
| Batch paths | Fixed context; canonical path echoed. Extend alias matching to descendants on segment boundaries only if shipping the P2 extension. | Longest matching create-intent prefix; no string-prefix collisions, no expression/value rewriting, no cross-call alias memory. Document changed descendant collision behavior. |

A `set` can already partially modify a node before a later field fails (E4).
Earlier batch steps remain applied and later steps are skipped. New validation
must not claim rollback of existing nodes, pulses, or elapsed time. Report the
applied portion where needed; do not automatically replay a failed whole batch.

## Implementation sequence

One integration owner owns contract decisions, merged test evidence, generated
artifacts and final live acceptance. Roles below are responsibilities, not
preselected people or models. Source changes are not installed/running changes.

Dependency order:

```text
P0 evidence baseline -> P1 capture + truthful observations
                              |
                              v
                  P4 discovery probes (connector/menu/API semantics)
                              |
                              v
                  P2 mutation safety -> P3 parameter validation
                              |
                              v
                  P4 complete matrix + post-change live acceptance
                              |
                              v
                  P5 recipes + fixture integration
                              |
                              v
                  P6 installed acceptance + closure
```

P4 has an early discovery pass and a post-change acceptance pass: connector,
menu, and diagnostic capability evidence must precede the corresponding P2/P3
behavior change. Offline tests and documentation preparation can proceed while
live probes are unavailable; do not ship guessed TD semantics.

P1 shader/inspect work and capture-envelope work can be separately researched.
P2/P3 both touch `tools.rs`, `mutate.py`, and the catalog: serialize those edits
or assign a single mutation owner. P4 uses one writer per scratch PID; separate
PID ownership is required for parallel live experiments. Use `implement-spec`
when executing dependent stages, not an all-at-once rewrite.

### P0 — Establish reproductions and evidence identity

**Owner:** integration/test owner. **Inputs:** this audit and the field notes.

1. Create a reproducible evidence manifest for each T/D identifier and N1–N4:
   source commit, client/daemon/bridge versions, schema hash, TD build, OS/Wine,
   GPU/driver, explicit daemon/PID, fixture identity, exact sanitized requests,
   clock/reset state, expected/actual result, artifact hashes and limitations.
2. Retain original field notes. Mark observation vs hypothesis vs workaround vs
   accepted fix; never replace the source account with a guessed explanation.
3. Recover the original capture request and minimal reproduction if available.
   If unavailable, state that and recreate a small failing boundary case; do
   not spend the whole refactor trying to reconstruct a private project.
4. Define the live scratch scene builder and cleanup ownership. Prefer a new
   text recipe plus generated scene to an opaque `.toe` as the sole source.

**Exit:** issue-to-test ledger exists; baseline environment is identifiable.
**Recovery:** preserve original projects and logs; redact secrets and private
paths/content from shared artifacts. No runtime changes required for this phase.

### P1 — Restore trustworthy capture and diagnostic evidence

**Owner:** MCP/diagnostics owner. **Inputs:** P0 identities, E1–E3/E6/E7.
**Files:** `tools.rs`, `args_diag.rs`, `rmcp_handler.rs`, relevant daemon transport
tests; `shader_lint.py`, `inspect.py`, their tests; `diagnostics/catalog.yaml`;
`docs/CONTRACT.md`. Touch stdio/federation code only if the reproduction points
there.

- Add capture argument tests for numeric/string PID, missing/null/wrong-type PID,
  explicit path/mode/maxSize, unknown fields, and status calls with explicit PID.
  Validate missing fields without dispatch; assert the exact bridge PID on valid
  calls and returned image content/structured identity at the MCP boundary.
- Exercise JSON fallback, actual MCP `tools/call`, stdio proxy, and federation
  separately where supported by the reproducer. Reuse existing harnesses and
  tests; a direct `dispatch_tool` test alone is not full MCP acceptance.
- Fix the responsible boundary only once localized. Normalize root diagnostic
  references if the `..pid` formatting case reproduces, independently of routing.
- Replace “no ERROR prefix means compiled” with affirmative success recognition
  and error precedence; unknown/unreadable results preserve bounded evidence.
  Cover TOP/MultiTOP/MAT without falsely promoting POP support.
- Centralize safe shader observation so a raising compile property does not
  unnecessarily erase all other inspection data. Add explicit status for failed
  parameter/message/wire reads; preserve actual null and empty values as valid.

**Acceptance:** current happy paths and malformed PID tests pass; valid capture
never loses its target; empty/foreign/mixed shader logs cannot silently pass;
inspection distinguishes clean, unavailable and error. Live T1 remains open
until the originally affected transport/installed runtime is exercised.

**Recovery:** revert narrowly by boundary; keep evidence/status fields additive.
No global log of raw scripts, credentials, or arbitrary project payloads.

### P2 — Make wiring and batch intent explicit

**Owner:** mutation owner. **Inputs:** P0 and observed connector behavior.
**Files:** `tools.rs`, mutation schema fixtures, `mutate.py`, `test_mutate.py`,
`paths.py` only where necessary, catalog and CONTRACT.

1. Characterize connector occupancy/replacement on fixed-input TOPs and
   variable-input Merge CHOP/POP. Correct fake peer bookkeeping to match the
   observed contract, including disconnecting previous peers.
2. Preserve requested index presence if needed: current typed defaults erase
   omitted-vs-explicit zero. Do not infer omission after serialization.
3. Add `onOccupied` and summary evidence from the table above. Validate policies
   before changing any edge. Add explicit replacement warnings and safe failures.
4. Keep batch context fixed. Document the two-call create/canonical-context
   workflow immediately. If implementing descendant aliases, extend the existing
   alias helper rather than adding implicit context or a second path language.
5. For descendant aliases, use longest segment-boundary match and coherent
   requested-to-actual mappings for nested creates/places. Test repeated names,
   nested renames, exact aliases, parent failure, and skipped-step paths. Treat
   the pre-existing occupant explicitly; callers needing it must split batches.

**Acceptance matrix:** explicit inputs 0/1/2 retain all sources; omitted zero
remains compatible; occupied/error leaves graph unchanged; explicit replacement
reports displaced peer; identical connection is safe; invalid policy/index and
unreadable occupancy fail as documented. Replaying a failed batch must not be
recommended. Child creation under a renamed parent targets the canonical parent
if the extension ships; names such as `fx` and `fx2` never alias each other.

**Recovery:** new safe policy is opt-in for old clients; migrate recipes first.
A future default change requires a separate versioned contract decision. If
descendant remapping cannot be safely specified, ship the canonical-context
workflow and retain the limitation rather than silently guessing.

### P3 — Validate parameter intent before blaming shaders

**Owner:** mutation owner with TD capability evidence. **Inputs:** P0 and P1
observation statuses; coordinate after P2 for shared files.
**Files:** `mutate.py`, `inspect.py`, bridge tests, catalog, CONTRACT, Python/GLSL
reference templates in the later documentation integration phase.

- Introduce a small parameter-capability helper, not an operator-name blacklist.
  Verify closed versus dynamic/custom menu behavior on the target build before
  hard rejection. `Color` versus `color` should produce a useful allowed-token
  diagnostic when the menu is demonstrably closed. Correct tokens and supported
  index assignments must remain valid.
- Expose bounded menu metadata in detailed parameter inspection where useful;
  distinguish `.val`, mode, `.expr` and evaluated value. Report evaluation failure
  separately from legitimate null; do not evaluate arbitrary expressions twice
  just to make an echo prettier. Document evaluation/cooking side effects.
- Preflight independent invalid fields before mutation where possible; handle
  dependent menus after their controlling field is applied. Test partial-apply
  reporting rather than claiming a transactional update. Preserve expression,
  bind/export and custom parameter semantics.
- For GLSL POP, record declared output attributes and observed downstream
  attributes in the scratch fixture. Add any declaration warning only after
  its detection boundary and false-positive cases are specified.
- Gate a mixed-reference/wire lint on P4 evidence. Warning only; valid repeated
  sources, external references and empty references remain legal.

**Acceptance:** invalid closed token fails before that assignment, with field,
node, requested value and bounded alternatives; valid custom/dynamic choices
are not rejected; a raising metadata getter degrades explicitly; multi-field
updates preserve documented partial results. Attribute declaration and a valid
shader yield observed Color downstream in live acceptance.

**Recovery:** narrow validators to verified capability cases; do not disable
all validation to accommodate one unknown TD menu. Never auto-rewrite shaders
or user parameter values during a read/diagnostic call.

### P4 — Run the controlled TD experiment matrix

**Owner:** TD verification owner; integration owner adjudicates causes.
**Inputs:** P0 manifest, usable capture path, P1 truthful diagnostics.
**Outputs:** small reproducible scenes, structured observations and viewed
images; confirmed statements or scoped unresolved/upstream reports.

Run the D-table experiments in this order to eliminate confounders:

1. Known TOP-only capture/color/alpha reference (T1, D5).
2. Explicit wiring and nested path resolution (T2/T6, D1, D10a).
3. Two-input POP merge and output attribute declaration (T3/T5).
4. Deliberately valid/invalid shaders and available logs (T4/N1).
5. Geometry source/defaults/flags/primitive rendering (D2–D4).
6. Deterministic CHOP mapping and signal measurements (D6, D10b–D10e).
7. Resettable feedback/trail: same-frame cook vs callback stepping vs playback
   (D7/D8). Reuse timing job status/cancel/cleanup, not polling-driven frames.
8. Bloom versus known bright input; blur/add quality and separately measured
   runtime costs (D9).

Keep identity, camera, material, clock, seeds and inputs fixed while changing
one variable. Save graph/config differences as well as images. For each run
record actual sample offsets, play state, output identity, RGB/alpha statistics
and the observation availability fields. Reopen saved fixtures to catch
initialization-only success. Repeat a deterministically reset run twice before
calling its outcome stable; do not demand byte-identical GPU output across
platforms without an established tolerance.

**Acceptance:** every D-item has a discriminating result, or remains explicitly
unverified with the missing probe named. Confirmed upstream defects get a
minimal sanitized project, build/platform scope, expected/actual behavior and
workaround. Repeated failure without new evidence stops after three probes;
record the blocker and continue unrelated cases, not an unbounded trial loop.

**Recovery:** cancel owned timing jobs and verify cleanup before further writes;
restore recorded transport policy where possible. A timeout is not cancellation.
No process-name-wide kills or automatic removal of user scene content.

### P5 — Integrate executable recipes and a meaningful public fixture API

**Owner:** documentation/fixture owner with integration review.
**Inputs:** accepted P1–P4 evidence; concurrent skill edits reconciled explicitly.
**Files:** `skills/MANIFEST.yaml` if adding a card; relevant `skills/templates/`
POPs, Python, render primer, GLSL, timed-capture, look-grade, and reset references;
matching `claude-skills/`; `docs/RECIPES.md`, `docs/E2E_CHECKLIST.md`,
`docs/KNOWN_ISSUES.md`, and `docs/OPEN_WORK.md` where appropriate. Proposed new
fixture location: `scripts/fixtures/known_issues/` with a text builder and
evidence manifest, plus a thin live runner under `scripts/` using existing
project conventions. These proposed files do not exist as outputs of this audit.

- Publish a minimal renderable POP recipe with explicit indices, inspected
  source selection, verified menu tokens, material/camera references, intended
  alpha convention, and captured visible output. Link deeper facts rather than
  duplicating long rules across cards.
- Publish the resolution-context matrix and deterministic CHOP-to-geometry
  recipe. Document feedback/trail as supported configurations or scoped
  limitations based on P4, not the original blanket conclusions.
- Correct `pop_data` and other vocabulary drift using generated tool schemas.
  Build a narrow schema-backed check for documented enum examples; existing
  link checking and generated-render parity do not establish semantic accuracy.
- Keep minimal fixtures independent of lightEngine. Then integrate the
  showcase: signal channels, attributes, geometry, output, reset, Hue/Intensity/
  Scale/Spread/Glow/Points controls. Each public control needs an observable
  assertion. Implement `Trail` with a tested reset contract or remove/mark it
  unsupported; an inert control is not a passed API.
- Re-render checked-in skills using the documented renderer, review only the
  intended diff, and retain existing concurrent contributions. Add issue links
  and actual evidence status rather than deleting the historical notes.

**Acceptance:** recipes can be followed from an empty owned COMP, then saved,
reopened and retested; published statements have evidence scope; no unsupported
tool enum; rendered skills match templates; each original issue has an explicit
disposition and verification link.

**Recovery:** text fixture builder and manifest remain the source of truth; do
not ship a private `.toe` or unexplained binary artifact as the only regression.

### P6 — Verify the installed path and close the issue ledger

**Owner:** integration owner. **Inputs:** P1–P5 changes and attributable checks.

1. Run relevant unit, integration, schema/catalog and documentation gates below.
2. Rebuild and install the affected daemon/bridge through the supported workflow;
   positively identify owned processes before restart. Record executable path,
   build/version and bridge generation after installation. Do not treat test
   binaries or a successful source build as the running MCP server.
3. If `bridge/bootstrap.py` or `bridge/tox_callbacks.py` changed, follow
   [bootstrap packing](../scripts/pack_bootstrap_tox.md) and preserve its drift
   test. Ordinary bridge-module edits do not by themselves authorize blindly
   stamping an opaque tox.
4. Verify the actual harness transport with immediate and timed capture, including
   one malformed request, unknown PID, lost reply recovery, and job cleanup.
   Exercise federation if changed; never assume a remote artifact path is local.
5. Run the integrated scene acceptance on the incident environment, then record
   native/build coverage separately. Reopen the saved project and verify every
   shipped public control and the claimed look from viewable artifacts.

**Exit:** no silent wire replacement in the recommended safe workflow; no false
compiled/clean verdict from absent evidence; valid menus and explicit contexts
work; capture works on the installed path; temporal recipes have real sample
evidence; every T/D/N row is resolved or honestly left open with owner, precise
limitation and next evidence. Do not call the whole objective fixed while
unresolved upstream/runtime acceptance remains.

**Rollback:** retain previous known-good package and fixture, revert by coherent
phase, reinstall/restart the positively identified owned runtime, and repeat
its smoke. Stop after three failed build/install probes without new evidence.

## Verification commands and evidence from this audit

Executed at the baseline above:

| Check | Result and evidence limit |
| --- | --- |
| `cargo test --locked -p tdmcp-daemon --test bridge_session capture_round_trip -- --exact` | PASS, 1 test. Direct dispatch and fake bridge, not the failing harness or live TD. |
| `cargo test --locked -p tdmcp-daemon --test error_surface garbage_string_pid_is_curated_wrong_type -- --exact` | PASS, 1 test. Invalid capture PID gives expected argument diagnostic. |
| `python -m unittest discover -s bridge/tests -p test_mutate.py` | PASS, 71 tests. Fake TD mutation behavior only. |
| `python -m unittest discover -s bridge/tests -p test_shader_lint.py` | PASS, 18 tests. Existing coverage does not catch N1. |
| `.venv/bin/python -m pytest bridge/tests/test_timing.py -q` | PASS, 34 tests. Fake clock/callback/cleanup behavior, not feedback or trail pixels. |
| Direct Python classifier and alias probes | Confirm N1 false-success classification and N2 exact-only alias behavior. No persistent test files added during planning. |
| Initial system-Python pytest command | Could not run: system Python has no pytest. Recovered with dependency-free unittest and the existing repository virtual environment; no dependency installation. |

For implementation, run focused regressions first, then the normal repository
gates after integration (use the established Python environment):

```sh
python scripts/check_docs.py
python -m unittest discover -s scripts/tests
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
.venv/bin/python -m pytest bridge/tests -q
```

Also retain schema golden tests, diagnostic-catalog completeness, embedded-tox
drift checks and rendered-skill parity tests. Run package smoke and relevant
platform checks for the final installed deliverable as described in
[TESTING.md](TESTING.md); do not publish a release as part of this plan.
Rust/Python language servers are unavailable in this session; documentation
link checking and repository-native tests are the applicable checks. No full
workspace quality gate or live acceptance is claimed for this planning pass.

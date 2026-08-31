# Shader compile lint (`text` writes + consumer status) — research & implementation plan

> **Archived 2026-08-31** — shipped; wire contract lives in [CONTRACT.md](../CONTRACT.md) (mutate_nodes / inspect sections).

Status: **Shipped** (implemented 2026-08-23; see CONTRACT.md §`mutate_nodes`
"Text writes & shader lint" and §`inspect` content tables for the wire
contract). Prepared from codebase reconnaissance and
**live verification against TouchDesigner** (pid 52168, `NewProject.1.toe`,
2026-08-23) using `api_help` / `execute_python`.

Scope: two shipped tools gain one shared capability.

1. `mutate_nodes` learns to write DAT bodies (`text`) and, after each write,
   lints the shader-compilation state of the GLSL ops consuming that DAT.
2. `inspect` keeps returning DAT bodies under `include: ["content"]` but the
   same content payload now also carries per-consumer compilation state
   (hint/note when compiled, error when compilation failed).

Both directions share one classifier and one severity vocabulary.

---

## 1. Problem

A Text/Table DAT is often a **shader source**: `glslTOP`, `glslmultiTOP`,
`glslMAT` consume DAT bodies via stage-reference parameters. Today:

- The agent cannot set a DAT body through `mutate_nodes` at all — every write
  bag is `.par.*`-scoped (`values`) or the 8-flag allowlist (`flags`);
  `op.text` is neither. Writes fall back to raw `execute_python`.
- After a write, nothing in the tool return says whether the change broke a
  shader. TD routes GLSL compile errors into `OP.compileResult` **only**:
  `OP.errors()` stays empty for shader failures (verified), so the existing
  `inspect` `errors`/`warnings` channels never see them. The agent must hand-roll
  an `execute_python` probe to notice it just destroyed a render.

Goal: first-class `text` writes plus a best-effort "shader lint" riding the
existing diagnostics style of both tools — note on successful compilation,
error on failure — without ever flipping tool-level `ok`.

## 2. Verified well-known patterns (live TD)

All items below were probed live (create → bind → set text → read status → fix
→ re-read). Exact strings are load-bearing for the classifier.

| # | Pattern | Verified behavior |
| --- | --- | --- |
| V1 | Write body | `dat.text = str` works on `textDAT`/tableDAT (R/W attribute, not a par) |
| V2 | Status surface | `OP.compileResult` (str) exists on `glslTOP`, `glslmultiTOP`, `glslMAT`. **Absent on `glslPOP`.** No `.compile()` method on any of them (`hasattr → False`) |
| V3 | Read forces sync compile | After dirtying a bound DAT, *reading* `compileResult` bumped `totalCooks` 2→3 with fresh output already present. No explicit `.cook()` needed; result is authoritative-at-read |
| V4 | Success string | `"Vertex Shader Compile Results:\n\nCompiled Successfully\n\n=============\nPixel Shader Compile Results:\n\nCompiled Successfully\n"`; `glslMAT` appends `"\n=============\n\nLinked Successfully\n"`. Unset stages still report success (TD internal defaults); section set varies by op — classification must not key off `Linked Successfully` presence |
| V5 | Failure string | Failing stage section contains lines prefixed `"ERROR:"`, last one `"ERROR: N compilation errors.  No code generated."`. Each error line embeds the full DAT path + line: `ERROR: /project1/probe_shaderlint/frag_ok:5: '' : syntax error, unexpected RIGHT_BRACE …` |
| V6 | Recovery + multi-consumer | Fixing the source flipped all consumers back to success on next read; one DAT fed `glslTOP` + `glslMAT` simultaneously |
| V7 | `errors()` silent | `OP.errors()` returned `""` while `compileResult` held compile errors → linter must use `compileResult`, never `errors()`. On `glslPOP`, `errors()` holds network-level messages instead (`' Error: No input POP …'`) |
| V8 | Consumer scan | `op('/project1').findChildren(type=td.glslTOP)` etc. — instant (~2 ms for the four type scans on a small project). Quirks: from `root` the `type=` filter returns `[]`; `ops('*')` does not enumerate all ops |
| V9 | Stage par map | glslTOP/glslmultiTOP: `pixeldat/vertexdat/computedat/predat`; glslMAT: `pdat/vdat/gdat/predat`. `par.<name>.eval()` returns the DAT OP itself → compare `.path`. Matches `_GLSL_STAGE_PARS` already coded in `bridge/tdmcp_bridge/inspect.py` |
| V10 | Auto-companion DATs | Creating GLSL ops spawns companion textDATs (`<name>_info`, `<name>_pixel`, `<name>_compute`) — scans/lints treat them as ordinary DATs |

Residual risks (stated, not silently assumed):

- Link-stage failure format is assumed to share the `ERROR:`-prefix family;
  only compile-stage format was captured verbatim. Classifier keys off the
  prefix, so this is low-risk.
- `glslPOP` has no discovered compile-status surface in this build → excluded
  from classification, reported as `unsupported_consumer` (see §3).

## 3. Shared lint contract

Severity vocabulary (both tools): `"note"` (compiled ok — informational hint)
and `"error"` (compilation failed). Lints are **best-effort enrichment**: they
never flip step/node/tool `ok`, mirroring how existing near-miss hints behave.

Diagnostic item shape (shared):

```json
{
  "severity": "note" | "error",
  "code": "tdmcp.shader.compiled" | "tdmcp.shader.compile_failed"
        | "tdmcp.shader.unsupported_consumer",
  "consumer": "/project1/fx/glsl1",
  "consumerOpType": "glslTOP",
  "role": "pixel",
  "message": "human summary",
  "lines": ["ERROR: /project1/…:5: …", "…"]
}
```

Classifier (bridge-side, pure function over `compileResult` string):

- Any line starting `"ERROR:"` → `error`; `lines` = those lines verbatim;
  `code = tdmcp.shader.compile_failed`.
- Else if the op has `compileResult` (non-null read) → `note`;
  `code = tdmcp.shader.compiled`; `message` may carry the compact
  `"Compiled Successfully"` echo.
- Consumer opType without `compileResult` (e.g. `glslPOP`) → `note` +
  `code = tdmcp.shader.unsupported_consumer`, message explains exclusion.

Consumer discovery (shared): scan `findChildren(type=…)` rooted at the call's
`contextPath` subtree, default `/project1` (V8), over
`glslTOP / glslmultiTOP / glslMAT`; match any `_GLSL_STAGE_PARS` entry whose
`par.eval().path == <dat path>` (V9). Caps: ≤512 ops scanned, ≤16 consumers
reported per DAT, overflow adds `consumersTruncated` + standard `truncation`
object. Scan/compute failures degrade silently (lint omitted), never failing
the parent call.

Side effect (documented at both call sites): reading `compileResult` forces a
synchronous recompile of that consumer (V3). Status is therefore
fresh-by-read, but a lint-enabled call can spend GPU compile time. This is
acceptable for an agent-driven workflow and matches what any manual check
would do.

## 4. `mutate_nodes` changes

Step schemas (`crates/tdmcp-mcp/src/tools.rs` `MutateStep::Create/Set`) gain:

```text
"text": string   (optional; DAT body write, applied before values)
```

Bridge semantics (`bridge/tdmcp_bridge/mutate.py`):

- Apply order inside a step: `text` → `values` → `expressions` → `pulse` → `flags`.
- Target must be a DAT (`isDAT`/family check): otherwise hard step error
  `tdmcp.mutate.not_dat`. On `create`, existing rollback applies (node destroyed,
  no orphan).
- Post-write lint: for every step that wrote `text`, run §3 discovery +
  classifier against the written DAT; attach results as
  `steps[i].shaderDiagnostics[]` (array; omitted when empty). Summary gains
  counts (`shaderNotes`, `shaderErrors`) when nonzero. Step `ok` unaffected.
- `detailLevel: detailed` additionally echoes the written `text` length
  (not the body — keep echoes bounded).

## 5. `inspect` changes

Today `include` allowlist is `nodes | params | errors | warnings | content`
(`bridge/tdmcp_bridge/inspect.py handle_inspect`). Changes:

- **DAT nodes with `content`:** the existing `content` object (kind `"dat"`)
  gains `consumers: []<diagnostic-item>` shaped per §3 — computed with the
  same discovery/classifier. Opting into `content` therefore opts into the
  lint (one opt-in, one mental model); the forced-recompile side effect is
  documented in the tool description.
- **GLSL nodes with `content`:** the existing shader content object
  (`kind: "shader"`, raw `compileResult` + stages) gains a classified
  `compileState: "compiled" | "error"` field derived by the same classifier
  (cheap: no extra reads — the string is already fetched).
- `consumers` respects caps/truncation per §3. Content read/follow failures
  keep the existing rule: never flip node or top-level `ok`.
- `errors`/`warnings` channels stay pure TD `OP.errors()/warnings()` — no
  synthesized entries mixed in (V7 means they'd be the wrong place anyway).

## 6. Implementation plan

1. Bridge: new `bridge/tdmcp_bridge/shader_lint.py` — stage-par map re-export,
   `discover_consumers(ctx, dat_path, scope_root, caps)`,
   `classify_compile_result(op_type, compile_result) -> item`, pure seams
   (fake-friendly like `mutate.py`'s `MutateContext`).
2. Bridge: wire into `mutate.py` (`_step_create/_step_set` post-write hook)
   and `inspect.py` (`_attach_content` / `_shader_content`).
3. Rust schema: `tools.rs` `MutateStep::{Create,Set}` gain `text: Option<String>`;
   inspect description strings updated; golden schema fixtures regenerated
   (`crates/tdmcp-mcp/tests/schema_golden.rs`).
4. Docs: CONTRACT.md §`mutate_nodes` (step table + codes) and §inspect
   `content` (DAT `consumers`, GLSL `compileState`).
5. Tests: `bridge/tests/test_shader_lint.py` (classifier matrix incl. exact
   V4/V5 strings, discovery matching, caps), extend `test_mutate.py`
   (`text` on create/set, non-DAT rejection + rollback, lint attachment),
   extend `test_inspect_summary.py` (consumers shaping, truncation),
   Rust schema tests.
6. Live MCP verification pass (MCP-first hard rule): end-to-end against a
   scratch project — write broken shader via `mutate_nodes`, observe error
   lint; fix, observe note; `inspect` parity check.

## 7. Limitations / non-goals (v1)

- `glslPOP` compile status: no verified surface; reported as
  `unsupported_consumer`. Revisit when TD exposes one.
- Link-stage-only failures: covered by the `ERROR:`-prefix heuristic, format
  not captured verbatim.
- Non-shader DAT consumers (scriptDAT callbacks, executeDATs, parameter
  expressions referencing DAT cells) are out of scope.
- No daemon-side caching of compile state; every lint read recompiles (V3
  semantics, acceptable at operate scale).

## 8. Open questions

- Severity naming: proposal is `note`/`error` everywhere (user phrased mutate
  as "note/error", inspect as "hint/warning" — unified here; bikeshed welcome).
- Whether `mutate_nodes` lint should also fire when `values`/`expressions`
  change something a shader expression depends on — v1: no, `text` writes only.

## 9. Live E2E evidence (shipped)

Run against live TouchDesigner (pid 52168, NewProject.6.toe) after the review
fixes; scratch COMP `/project1/probe_slint_e2e` created and destroyed
(`gone: true` verified). Observed verbatim:

- Broken write → `steps[0].shaderDiagnostics[0]`: severity `error`,
  code `tdmcp.shader.compile_failed`, role `pixel`, lines include
  `ERROR: /project1/probe_slint_e2e/shader:2: '' :  syntax error, unexpected SEMICOLON`.
- Fixed write → note `tdmcp.shader.compiled`, message `Compiled Successfully`;
  with the MAT consumer attached: `Compiled Successfully, Linked Successfully`.
- Two consumers on one DAT → both flagged in one return (glslMAT + glslTOP).
- `text` on glslTOP → hard error `tdmcp.mutate.not_dat`
  (`"text write requires a DAT target (family=TOP, opType=glslTOP)"`),
  follower step skipped.
- inspect DAT content → `consumers[]` with both consumers; GLSL content →
  `compileState: "compiled"` / `"error"` matching the broken/fixed writes;
  legacy fields (`isText`, `bytes`, `compileResult`, `stages`) intact.
- glslPOP wired via `computedat` → note `tdmcp.shader.unsupported_consumer`,
  message `glslPOP exposes no compileResult surface; compile state not checked`,
  role `compute`.

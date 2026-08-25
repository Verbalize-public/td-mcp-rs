# Curated Tool-Call Error Diagnostics — Plan

> **Status: implemented & verified (2026-08-25).** All milestones below are
> done; live probes confirmed the curated payloads through the daemon's HTTP
> surface, and `stdio_proxy_forwards_curated_arg_errors` covers the stdio
> path. Companion to
> [`OBSERVABILITY_PLAN.md`](OBSERVABILITY_PLAN.md); follows the same
> milestone/task layout.

## 0. Problem

When an agent sends malformed tool arguments, the error it sees is a raw serde
string with no code, no hint, no mitigation:

```
Error: MCP error -32602: missing field `op`
```

Concrete example: `mutate_nodes` with `steps: [{ "path": "/project1" }]` (step
missing its `"op"` tag). `MutateStep` is an internally-tagged enum
(`#[serde(tag = "op", rename_all = "lowercase", deny_unknown_fields)]`,
`crates/tdmcp-mcp/src/tools.rs:452-455`), so serde fails with `missing field
'op'`, and that raw string is all the agent gets.

### Current flow (root cause chain)

| Step | Location | Behavior |
| --- | --- | --- |
| 1. Parse | `tools.rs:666-667, 699-700, 746-747, 788-789, 849-850, 896-897, 935-936` | `serde_json::from_value::<XxxParams>(args).map_err(\|e\| ToolCallError::InvalidArgs(e.to_string()))` |
| 2. Hand checks | `tools.rs:790-794` (inspect `paths`), `tools.rs:898-902` (api_help `queries`) | Human sentences, but still free strings |
| 3. rmcp surface | `crates/tdmcp-mcp/src/rmcp_handler.rs:167` | `InvalidArgs(msg)` → `ErrorData::invalid_params(msg)` = `-32602` with bare text |
| 3b. JSON fallback | `crates/tdmcp-mcp/src/server.rs:169-171` | Same two variants → **bare HTTP 400, empty body** |
| 3c. stdio proxy | `crates/tdmcp-mcp/src/stdio_proxy.rs:442-448` | Passes `ErrorData` through untouched (pinned by `crates/tdmcp-daemon/tests/stdio_proxy.rs:156`) |

### The contrast

Every *other* failure in the app is already curated: catalog-backed codes,
mitigation steps, references, did-you-mean lints (`build_diag` at
`crates/tdmcp-mcp/src/outcomes.rs:784`; envelope types in
`crates/tdmcp-diagnostics/src/envelope.rs`; entries in
`diagnostics/catalog.yaml`; bridge-side fuzzy hints in
`bridge/tdmcp_bridge/suggest.py`). Argument errors are the last raw-string
surface.

Also misclassified while we're here: `serde_json::to_value` **serialization**
failures mapped to `InvalidArgs` at `tools.rs:690, 694, 874, 920` — those are
internal errors, not caller mistakes.

## 1. Goal & non-goals

**Goal:** no raw serde string ever reaches an agent. Every argument-level
failure carries the same shape as every other diagnostic:

```jsonc
{
  "ok": false,
  "summary": "1 error, 1 lint",
  "items": [{
    "severity": "error",
    "code": "tdmcp.args.missing_field",
    "layer": "args",
    "message": "mutate_nodes: steps[0] is missing required field \"op\" (one of create | set | delete | connect | disconnect)",
    "span": { "tool": "mutate_nodes", "field": "steps[0].op" },
    "lints": [{ "severity": "lint", "code": "tdmcp.args.similar_field", "message": "…", "suggestion": { "replace": "op" }, "confidence": "high" }],
    "mitigation": ["Add \"op\": \"create\" (…) to each step object", "Call describe_tools for the full schema"],
    "references": [{ "kind": "tool", "id": "describe_tools" }]
  }]
}
```

**Non-goals:** no schema/tool-surface changes; no bridge-side changes; success
envelopes untouched; no new dependency beyond promoting `serde_path_to_error`
(already in `Cargo.lock` transitively) to a direct dep.

## 2. Design decisions

**D1 — Carrier (the one open decision).** Schema-level arg failures become
`isError` structured tool results (same path as `ToolCallError::Failed` today),
**not** protocol errors. `-32602 invalid_params` remains only for genuinely
protocol-level issues: unknown tool name and non-object `arguments`.
*Rationale:* agents self-correct from the structured payload (proven by the
existing `Failed` path), and it unifies all failure shapes. *Alternative A
(lower risk):* keep everything on `-32602` and only enrich the message string —
wire-compatible, but agents must parse prose and clients truncate long
messages. If A is preferred, M2/M3 shrink to message formatting only.

**D2 — Codes & layer.** New namespace `tdmcp.args.*`: `missing_field`,
`unknown_field`, `unknown_variant`, `wrong_type`, `similar_field` (lint), plus
**reuse** of existing `tdmcp.op.paths_required` / `tdmcp.api_help.queries_required`
for the hand checks at `tools.rs:791/899` instead of free strings. New
`DiagnosticLayer::Args` variant in `envelope.rs:23-36` (additive, serializes as
`"args"`).

**D3 — SSOT for hints.** Expected fields / required lists / allowed variants
come from `input_schema_for(tool)` (`crates/tdmcp-mcp/src/schema.rs:22`),
including `$defs` for the `MutateStep` op variants. No hand-maintained field
lists → hints can't drift from the advertised schema (golden-fixture tested).

**D4 — Parser precision.** Wrap deserialization with
`serde_path_to_error::deserialize` to get exact JSON paths (`steps[0].op`);
truncate any echoed "got" value snippet (~200 chars) to bound payload size.

**D5 — Did-you-mean.** Small Rust helper mirroring `suggest.py` semantics:
casefold-exact wins, then near-miss over schema property names (cutoff ≈0.5);
emit as a nested `LintItem` with `suggestion.replace` + `confidence`, same as
`tdmcp.par.similar_name`. Prefer silence over wrong hints.

**D6 — Reclassify serialization failures.** `to_value` sites
(`tools.rs:690, 694, 874, 920`) map to internal-error handling, never
`InvalidArgs`.

## 3. Milestones

### M1 — Catalog & envelope foundation

- [x] **T1.1** `crates/tdmcp-diagnostics/src/codes.rs`: add `ARGS_MISSING_FIELD`,
      `ARGS_UNKNOWN_FIELD`, `ARGS_UNKNOWN_VARIANT`, `ARGS_WRONG_TYPE`,
      `ARGS_SIMILAR_FIELD`; append to `ALL`.
- [x] **T1.2** `diagnostics/catalog.yaml`: one entry per new code, following the
      existing pattern (message + `mitigation:` list + `references:` pointing at
      `describe_tools` / relevant docs). Drift tests in `codes.rs`
      (`every_code_constant_is_in_catalog`, orphan scan, literal scan) enforce
      completeness automatically.
- [x] **T1.3** `envelope.rs`: add `DiagnosticLayer::Args`.

### M2 — Curated parser (`crates/tdmcp-mcp/src/args_diag.rs`, new)

- [x] **T2.1** `parse_args<T>(catalog, tool, args) -> Result<T, ToolCallError>`
      wrapping `serde_path_to_error::deserialize`; classify the inner serde
      error (missing field / unknown field / unknown variant / type mismatch)
      into D2 codes; build the `DiagnosticItem` via
      `catalog.build_error(code, span{tool, field}, message, …)`; on unknown
      field, attach the T2.3 lint.
- [x] **T2.2** Schema-driven context extraction from `input_schema_for(tool)`:
      required-list lookup, per-op-variant property sets via `$defs`, enum
      value lists for `unknown_variant` messages.
- [x] **T2.3** Near-miss helper (D5) + unit tests: missing `op`, typo'd top key
      (`contextpath` → suggests `contextPath`), bad `include` value, wrong
      type, empty-array pre-check codes.

### M3 — Wiring across surfaces

- [x] **T3.1** `dispatch_tool`: swap all seven parse sites to `parse_args`;
      migrate hand checks (`tools.rs:791/899`) to coded items reusing
      `OP_PATHS_REQUIRED` / `API_HELP_QUERIES_REQUIRED`.
- [x] **T3.2** `rmcp_handler.rs:163-172`: `InvalidArgs` now carries the payload
      and returns via `call_tool_error_result(...)` like `Failed`; `UnknownTool`
      stays `-32602` but with enriched text ("call describe_tools").
- [x] **T3.3** `server.rs:141-172`: JSON fallback returns the same failure body
      with HTTP 400 (no more empty body).
- [x] **T3.4** stdio proxy needs no logic change; keep
      `stdio_proxy_preserves_invalid_params_code`
      (`crates/tdmcp-daemon/tests/stdio_proxy.rs:156`) green for the remaining
      `-32602` class; add a sibling test asserting the structured arg-error
      payload flows through untouched.
- [x] **T3.5** Apply D6 to the four `to_value` sites.
- [x] **T3.6** Update the pinned expectations that assert old behavior
      (`rg InvalidArgs crates/*/tests crates/*/src` before starting; also grep
      tests for `"invalid arguments"` / `missing field`).

### M4 — Contract & docs

- [x] **T4.1** `docs/CONTRACT.md`: update the failure paragraph (~line 260),
      the `fleet`/`include` rejection note (line 275), the `inspect`/`paths`
      note (line 279), the inspect include note (line 292), and the
      "Decided contract (summary)" section (~line 604): argument-shape failures
      are structured `isError` results with `tdmcp.args.*` codes;
      `-32602` reserved for unknown tool / malformed request.
- [x] **T4.2** Cross-ref the new codes from `docs/OBSERVABILITY.md`; add this
      doc's row to the AGENTS.md Documentation Reference table when done.

### M5 — Verification (MCP-first)

- [x] `cargo test --workspace` + `scripts/check.ps1`.
- [x] Live probe per AGENTS.md ritual (kill daemons → rebuild →
      `tdmcp-daemon ensure`), then through a real MCP client:
      1. `mutate_nodes` step without `op` → curated payload with
         `tdmcp.args.missing_field` + hint (**not** `missing field 'op'`).
      2. Typo'd field → lint suggestion present.
      3. `inspect` with empty `paths` → `tdmcp.op.paths_required`.
      4. Unknown tool → still `-32602`, enriched message.

## 4. Error-class mapping (the curation core)

| Serde condition | Detected via | Code | Message template |
| --- | --- | --- | --- |
| Missing required field | `serde_path_to_error` inner `missing field` | `tdmcp.args.missing_field` | `<tool>: <path> is missing required field "<f>" (<allowed values when known>)` |
| Unknown field (`deny_unknown_fields`) | inner `unknown field` | `tdmcp.args.unknown_field` + `similar_field` lint when near-miss | `<tool>: unknown field "<f>" at <path>; expected one of […]` |
| Bad enum value (`include`, `detailLevel`, `op`) | inner `unknown variant` | `tdmcp.args.unknown_variant` | `<tool>: "<v>" is not a valid <field>; one of […]` |
| Type mismatch | inner `invalid type` | `tdmcp.args.wrong_type` | `<tool>: <path> expected <type>, got <snippet>` |
| Empty required array (hand check) | pre-check | `tdmcp.op.paths_required` / `tdmcp.api_help.queries_required` | existing catalog text |
| Unknown tool name | `from_wire` miss | *(no code)* | stays `-32602`, message points at `describe_tools` |

## 5. Risks

- **Behavior change:** clients branching on `-32602` for arg errors will see
  `isError` results instead. Deliberate (D1); contract updated in T4.1; the
  proxy passthrough test is re-scoped in T3.4 rather than deleted.
- **Envelope addition** `DiagnosticLayer::Args` is additive; readers that match
  exhaustively in-repo need the new arm (compiler-guided).
- **If D1-alternative-A is chosen**, scope shrinks to M2 (message formatting
  only) + T3.2/T4.1 wording; skip codes/envelope changes entirely.

## 6. Acceptance criteria

1. `rg "\.map_err\(\|e\| ToolCallError::InvalidArgs\(e\.to_string\(\)\)\)"`
   returns nothing under `crates/`.
2. Every observable argument error carries a catalog-registered code; the three
   drift tests in `codes.rs` stay green.
3. M5 live probe shows curated diagnostics end-to-end through rmcp, the JSON
   fallback, and the stdio proxy.

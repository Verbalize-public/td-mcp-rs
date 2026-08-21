---
name: touchdesigner
description: >-
  TouchDesigner operate umbrella for td-mcp-rs.
  MUST READ before any TouchDesigner work: inspecting, creating, wiring,
  or mutating nodes/networks/components/parameters, elaborating a plan,
  writing Python, or porting GLSL for TouchDesigner.
---

# TouchDesigner (td-mcp-rs)

This skill is the sole TD entrypoint — read this body before other TD cards.
Also available as `tdmcp://docs/operate`.

## Quick tool routing

| Task | Tool |
|------|------|
| List TD processes, pick a `pid` | `fleet` |
| Where is the user looking / selection | `editor_context` (hint only) |
| Structure / params / errors / network | `inspect` (**default** — `paths[]` required) |
| Create / set / delete / wire | `mutate_nodes` |
| Arbitrary Python (not network walks) | `execute_python` — see hard rules |
| Perception / look claims | `capture` |
| TD Python / opType cards | `api_help` |
| Tool manifest | `describe_tools` |
| On-demand operate cards | MCP `resources/read` `tdmcp://docs/<id>` |

Lookup routing: exact opType/class → `api_help`; parameter **names** on an
existing node → `inspect`; conceptual family → tables below +
`tdmcp://docs/operator-families`. Network understanding → always `inspect`
first.

## Target identification

1. `fleet` → pick a connected `pid`. Pass `pid` on every call.
2. Resolve a mutation zone: prefer a self-created named COMP over polluting an
   existing network; user-named subtree; or `editor_context` `ownerPath` as a
   **hint only** — confirm with `inspect` before mutating. Depth:
   `tdmcp://docs/mutation-zones`.
3. Prefer `detailLevel: summary` / `diagnosticLevel: summary`. Store-first for
   `capture` — only inject image artifacts when the current model can see them;
   otherwise delegate to a vision-capable helper. Depth:
   `tdmcp://docs/look-grade`.

## Operator families (quick table)

| Family | Data | Runs on | Reach for it when |
|--------|------|---------|-------------------|
| **TOP** | images/textures | GPU | video, compositing, feedback, shaders, output |
| **CHOP** | channels | CPU (some GPU) | motion, audio, control, LFOs, device I/O |
| **POP** | points + attributes | GPU | 3D geometry, particles — modern 3D default |
| **SOP** | polygons/surfaces | CPU | legacy 3D meshes, booleans, NURBS |
| **DAT** | text/tables | CPU | scripts, callbacks, config |
| **MAT** | materials | GPU | shading rendered geometry |
| **COMP** | networks/containers | — | structure, UI, 3D scene |

Pull-based cook: a node cooks only when downstream needs it *and* an
input/parameter changed. Moving values: expression > export > bind > script.
Depth: `tdmcp://docs/operator-families` / `tdmcp://docs/primer/cook-and-families`.

## Hard rules

### Sequential bridged tools

- Call bridged tools **one at a time**; wait for each result before the next.
- `fleet` / `describe_tools` stay available during an in-flight call.
- On `tdmcp.mcp.session_busy` or `tdmcp.bridge.queue_busy`: wait, then retry —
  do **not** disconnect or restart. After 3 consecutive failures, hand off to
  the user.

Depth: `tdmcp://docs/tooling-concurrency`.

### OpSketch before non-trivial networks — HARD RULE

When thinking about, planning, building, or mutating a network of more than
3 nodes (or any non-trivial operator: COMP hub, multi-family chain, data-flow
with branches), **describe it in OpSketch first** — before creating or
mutating anything. Use the sketch to validate the design, then build from it.

Depth: `tdmcp://docs/opsketch-notation` (+ gating, examples).

### `inspect` over `execute_python` for networks — HARD RULE

`inspect` is the primary tool for network analysis. Do **not** use
`execute_python` as the main network inspector. After any mutation pass, a
**final `inspect` on the touched network parent** is mandatory before claiming
done.

### Python cheatsheet before Python — HARD RULE

Before `execute_python` or any TD expression/script, **`resources/read`
`tdmcp://docs/python-api`** in this turn. Exact live names still go through
`api_help`.

### In/Out operator harness — HARD RULE

Reusable COMP boundaries use dedicated In/Out operators — never path-parameter
inputs or reaching into another COMP's internals. Depth:
`tdmcp://docs/component-checklist`.

### Relative references — HARD RULE

Never use absolute `/project1/...` inside a reusable network. Prefer relative
paths: `parent().par.Foo`, `parent().op('null_out')`, bare sibling names,
`./child`, `../cousin`. Depth: `tdmcp://docs/network-design`.

### Play state before "why isn't it updating" — HARD RULE

Paused transport stalls most cooks; captures can look stale. Check play state
before debugging a network or grading a capture. Depth:
`tdmcp://docs/play-state`.

## Custom component basics

Custom parameters are the COMP **control** API — small, curated, page-grouped.
In/Out ops create wiring pins; do not duplicate them as OP-path parameters.
Before scripting custom pars: `resources/read` `tdmcp://docs/custom-parameters`.
Packaging / About / reuse: `tdmcp://docs/component-checklist`.

## Threading & pipeline

TD Python runs on the main thread only — use `run(code, delayFrames=0)` to hand
work back safely. See also `tdmcp://docs/play-state` and
`tdmcp://docs/primer/scripting-surfaces`.

## Python / scripting quick intro

**Gate:** `tdmcp://docs/python-api` before any `execute_python` or authored
expression. Orientation only here: `me`, `op()`/`ops()`, `parent(n)`, `run()`,
always `.eval()` parameters. `execute_python` sandbox injects `td` / `op` /
`result` / closed aliases — not `me` / `parent` / bare opTypes. Prefer
`editor_context` for pane/selection. Network roster: use `inspect`.

## GLSL awareness (quick)

Frag-only → GLSL TOP; vert+frag → GLSL MAT; multi-buffer → one TOP per buffer +
Feedback. Feed every uniform explicitly (unfed = silent black FAIL). Watch
`TDTexInfo.res` (`.res.xy` vs `.res.zw`). Depth: `tdmcp://docs/glsl`,
`tdmcp://docs/td-glsl-ground-truth`, `tdmcp://docs/shadertoy-conversion`,
`tdmcp://docs/primer/glsl-and-render`.

## Definition of Done

Observable runtime evidence only — never PASS from code or docs alone.
Doubt → **FAIL**.

| Verdict | Meaning |
|---------|---------|
| `PASS` | Evidence matches claim |
| `FAIL` | Evidence contradicts claim (incl. black frame) |
| `BLOCKED` | Surface unreachable / capture unreadable |
| `SKIP` | No runtime surface, or deliberate cost-reduction |

Depth: `tdmcp://docs/definition-of-done` (full checklist + doubt rules).
Look claims: `tdmcp://docs/look-grade`.

## Resource index (deepen here)

| Need | Resource |
|------|----------|
| This umbrella | `tdmcp://docs/operate` |
| OpSketch | `tdmcp://docs/opsketch-notation` (+ gating, examples) |
| Python | `tdmcp://docs/python-api` |
| Custom pars | `tdmcp://docs/custom-parameters` |
| Mutation zones | `tdmcp://docs/mutation-zones` |
| Network / relative refs | `tdmcp://docs/network-design` |
| Components | `tdmcp://docs/component-checklist` |
| Families | `tdmcp://docs/operator-families` |
| POP | `tdmcp://docs/pops` |
| GLSL dialect | `tdmcp://docs/glsl` |
| Shadertoy port | `tdmcp://docs/shadertoy-conversion` |
| GLSL traps | `tdmcp://docs/td-glsl-ground-truth` |
| Structural DoD | `tdmcp://docs/definition-of-done` |
| Look / capture | `tdmcp://docs/look-grade` |
| Parallel / session_busy | `tdmcp://docs/tooling-concurrency` |
| Paused / stale | `tdmcp://docs/play-state` |
| Cook / families depth | `tdmcp://docs/primer/cook-and-families` |
| Editor / layout | `tdmcp://docs/primer/editor-and-layout` |
| Params / channels | `tdmcp://docs/primer/parameters-and-channels` |
| Scripting surfaces | `tdmcp://docs/primer/scripting-surfaces` |
| tox / toe | `tdmcp://docs/primer/tox-toe-components` |
| GLSL / render chain | `tdmcp://docs/primer/glsl-and-render` |
| Performance | `tdmcp://docs/primer/performance` |
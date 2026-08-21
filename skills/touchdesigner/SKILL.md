---
name: touchdesigner
description: >-
  TouchDesigner operate umbrella for td-mcp-rs.
  **MUST READ** before doing anything related to TouchDesigner:
  inspecting, creating, wiring, or mutating TouchDesigner nodes/networks/components/parameters,
  Ealborating plan, writing Python or porting GLSL for TouchDesigner.
---

# TouchDesigner (td-mcp-rs)

This skill is the sole TD (TouichDesigner) entrypoint — read this body before other TD cards.

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
2. Resolve a mutation zone: prefers self-created named COMP over polluting existing network, user-named
   subtree, or `editor_context` `ownerPath` as a **hint only** — confirm with
   `inspect` before mutating. Depth: `resources/read` `tdmcp://docs/mutation-zones`.
3. Prefer `detailLevel: summary` / `diagnosticLevel: summary`. Store-first/sub-agent for
   `capture`, always make sure that vision capability are available before capturing, rely on programatic image analysis when needed or no vision capability.

## Operator meaning (quick table)

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

- On `tdmcp.mcp.session_busy` or `tdmcp.bridge.queue_busy`: wait, then retry —
  do **not** disconnect or restart. After 3 failed attempt handoff to the user.

Depth: `resources/read` `tdmcp://docs/tooling-concurrency`.

### `inspect` over `execute_python` for networks — HARD RULE

`inspect` is the primary tool for network analysis. Do **not** use
`execute_python` as the main network inspector. After any mutation pass, a
**final `inspect` on the touched network parent** is mandatory before claiming
done.

### Python cheatsheet before Python — HARD RULE

Before `execute_python` or any TD expression/script, **`resources/read`
`tdmcp://docs/python-api`** in this turn.
Exact live names still go through `api_help`.

### In/Out operator harness — HARD RULE

Reusable COMP boundaries use dedicated In/Out operators — never path-parameter
inputs or reaching into another COMP's internals. Depth:
`tdmcp://docs/component-checklist`.

### Relative references — HARD RULE

Never use absolute `/project1/...` inside a reusable network. Prefer relative
paths: Depth: **MUST READ WHEN EDITING/CREATING/REMOVING reference** `tdmcp://docs/network-design`.

### Play state before “why isn’t it updating” — HARD RULE

Paused transport stalls most cooks; captures can look stale. Check play state
first. Depth: **MUST READ WHEN DEBUGING A NETWORK OR CAPTURING IT** `tdmcp://docs/play-state`.

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
Doubt → **FAIL**. Depth: `tdmcp://docs/definition-of-done`, look claims
`tdmcp://docs/look-grade`.

| Verdict | Meaning |
|---------|---------|
| `PASS` | Evidence matches claim |
| `FAIL` | Evidence contradicts claim (incl. black frame) |
| `BLOCKED` | Surface unreachable / capture unreadable |
| `SKIP` | No runtime surface, or deliberate cost-reduction |

- [ ] Final `inspect` on touched network **parent** — errors/warnings clean
- [ ] Network understanding used `inspect` (Python last resort)
- [ ] Any Python/expression preceded by `tdmcp://docs/python-api`
- [ ] Touched subtree reorganized, zero overlapping nodes
- [ ] Relative-refs + In/Out hard rules held
- [ ] Custom COMP: About page, In/Out pins; pars per `tdmcp://docs/custom-parameters`
- [ ] Look claims: non-black `capture` (store-first, `maxSize: 256`) → operating agent grades via `tdmcp://docs/look-grade`
- [ ] Stop after 3 failed probes with no new evidence

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

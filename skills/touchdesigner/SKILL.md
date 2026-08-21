---
name: touchdesigner
description: >-
  TouchDesigner operate umbrella for td-mcp-rs. Use when inspecting, creating,
  wiring, or mutating TouchDesigner nodes/networks/components/parameters, writing
  TD Python, porting GLSL into TD, or verifying live TD state/look/FPS. Covers
  tool routing, mutation zones, OpSketch, In/Out + relative-ref hard rules, and
  MCP resource deepen paths (tdmcp://docs/*).
---

# TouchDesigner (td-mcp-rs)

**SoT (tools):** td-mcp-rs `docs/CONTRACT.md` + live tools.
**SoT (operate deepen):** MCP resources `tdmcp://docs/*` (same files under
`skills/touchdesigner/` on disk). This skill is the sole TD entrypoint — read
this body before other TD cards. Also available as `tdmcp://docs/operate`.

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

1. **One shot** `fleet` → pick a connected `pid`. Pass `pid` on every call.
2. Resolve a mutation zone: self-created named COMP (default), user-named
   subtree, or `editor_context` `ownerPath` as a **hint only** — confirm with
   `inspect` before mutating. Depth: `resources/read` `tdmcp://docs/mutation-zones`
   (file: `reference/mutation-zones.md`).
3. Prefer `detailLevel: summary` / `diagnosticLevel: summary`. Store-first for
   `capture`.

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

### Sequential bridged tools — HARD RULE

Never fire parallel bridged MCP tools (`execute_python`, `inspect`, `capture`,
`mutate_nodes`, `api_help`, `editor_context`) against the same `pid`.

- Call bridged tools **one at a time**; wait for each result before the next.
- `fleet` / `describe_tools` stay available during an in-flight call.
- On `tdmcp.mcp.session_busy` or `tdmcp.bridge.queue_busy`: wait, then retry —
  do **not** disconnect or restart.

Depth: `resources/read` `tdmcp://docs/tooling-concurrency`
(file: `reference/tooling-concurrency.md`).

### `inspect` over `execute_python` for networks — HARD RULE

`inspect` is the primary tool for network analysis. Do **not** use
`execute_python` as the main network inspector. After any mutation pass, a
**final `inspect` on the touched network parent** is mandatory before claiming
done.

### Python cheatsheet before Python — HARD RULE

Before `execute_python` or any TD expression/script, **`resources/read`
`tdmcp://docs/python-api`** in this turn (file: `reference/python-api.md`).
Exact live names still go through `api_help`.

### In/Out operator harness — HARD RULE

Reusable COMP boundaries use dedicated In/Out operators — never path-parameter
inputs or reaching into another COMP's internals. Depth:
`tdmcp://docs/component-checklist`.

### Relative references — HARD RULE

Never use absolute `/project1/...` inside a reusable network. Prefer relative
paths / `parent().par` / `parent(n)`. Depth: `tdmcp://docs/network-design`.

### Play state before “why isn’t it updating” — HARD RULE

Paused transport stalls most cooks; captures can look stale. Check play state
first. Depth: `tdmcp://docs/play-state`.

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

| Need | Resource | Disk |
|------|----------|------|
| This umbrella | `tdmcp://docs/operate` | `SKILL.md` |
| OpSketch | `tdmcp://docs/opsketch-notation` (+ gating, examples) | `reference/opsketch-*.md` |
| Python | `tdmcp://docs/python-api` | `reference/python-api.md` |
| Custom pars | `tdmcp://docs/custom-parameters` | `reference/custom-parameters.md` |
| Mutation zones | `tdmcp://docs/mutation-zones` | `reference/mutation-zones.md` |
| Network / relative refs | `tdmcp://docs/network-design` | `reference/network-design.md` |
| Components | `tdmcp://docs/component-checklist` | `reference/component-checklist.md` |
| Families | `tdmcp://docs/operator-families` | `reference/operator-families.md` |
| POP | `tdmcp://docs/pops` | `reference/pops.md` |
| GLSL dialect | `tdmcp://docs/glsl` | `reference/glsl.md` |
| Shadertoy port | `tdmcp://docs/shadertoy-conversion` | `reference/shadertoy-conversion.md` |
| GLSL traps | `tdmcp://docs/td-glsl-ground-truth` | `reference/td-glsl-ground-truth.md` |
| Structural DoD | `tdmcp://docs/definition-of-done` | `reference/definition-of-done.md` |
| Look / capture | `tdmcp://docs/look-grade` | `reference/look-grade.md` |
| Parallel / session_busy | `tdmcp://docs/tooling-concurrency` | `reference/tooling-concurrency.md` |
| Paused / stale | `tdmcp://docs/play-state` | `reference/play-state.md` |
| Cook / families depth | `tdmcp://docs/primer/cook-and-families` | `primer/cook-and-families.md` |
| Editor / layout | `tdmcp://docs/primer/editor-and-layout` | `primer/editor-and-layout.md` |
| Params / channels | `tdmcp://docs/primer/parameters-and-channels` | `primer/parameters-and-channels.md` |
| Scripting surfaces | `tdmcp://docs/primer/scripting-surfaces` | `primer/scripting-surfaces.md` |
| tox / toe | `tdmcp://docs/primer/tox-toe-components` | `primer/tox-toe-components.md` |
| GLSL / render chain | `tdmcp://docs/primer/glsl-and-render` | `primer/glsl-and-render.md` |
| Performance | `tdmcp://docs/primer/performance` | `primer/performance.md` |

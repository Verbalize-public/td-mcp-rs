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
Also available as [`operate`](./SKILL.md).

## Quick tool routing

| Task | Tool |
|------|------|
| List TD processes, pick a `pid` | `fleet` |
| Where is the user looking / selection | `editor_context` (hint only) |
| Structure / params / errors / network | `inspect` (**default** — `paths[]` required) |
| DAT text/table bodies / GLSL sources | `inspect` with `include: ["content"]` (opt-in; follows GLSL DAT refs; reports shader compile status) |
| Create / set / delete / wire / **write DAT text** | `mutate_nodes` (`text` on create/set auto-lints consuming shaders in the return) |
| Stock component exists for this? Find / place one | `palette_index` → `mutate_nodes` `place` — [`palette`](./reference/palette.md) |
| Describe *why* a node exists / read that back | `mutate_nodes` `comment` on create/set; `inspect` returns it — [`node-comments`](./reference/node-comments.md) |
| Arbitrary Python (not network walks) | `execute_python` — see hard rules |
| Perception / look claims | `capture` |
| TD Python / opType cards | `api_help` |
| Tool manifest | `describe_tools` |
| No TD running / start or stop one | `spawn_td` / `kill_td` — [`lifecycle`](./reference/lifecycle.md) |
| Calls stalling, TD wedged, startup modal | `dialogs` — [`popups`](./reference/popups.md) |
| Offline `.toe`/`.tox`, installs, bridge install | `td_installs` / `project_unpack` / `project_pack` / `project_lint` / `project_install_bridge` — [`project-io`](./reference/project-io.md) |
| Build / refresh palette knowledge, blacklist a bad comp | `palette_probe` + `palette_index` `describe` — [`palette-scan`](./reference/palette-scan.md) |
| On-demand operate cards | open the card's `.md` file (Resource index below) |

Lookup routing: exact opType/class → `api_help`; parameter **names** on an
existing node → `inspect`; conceptual family → tables below +
[`operator-families`](./reference/operator-families.md). Network understanding → always `inspect`
first.

## Target identification

1. `fleet` → pick a connected `pid` (and `daemonId` when the fleet shows multiple daemons). Pass `pid` on every call; pass `daemonId` when federated / ambiguous.
   No connected pid? Start one with `spawn_td` — never wait for a human.
   Depth: [`lifecycle`](./reference/lifecycle.md).
2. Resolve a mutation zone: prefer a self-created named COMP over polluting an
   existing network; user-named subtree; or `editor_context` `ownerPath` as a
   **hint only** — confirm with `inspect` before mutating. Depth:
   [`mutation-zones`](./reference/mutation-zones.md).
3. Prefer `detailLevel: summary` / `diagnosticLevel: summary`. Store-first for
   `capture` — only inject image artifacts when the current model can see them;
   otherwise delegate to a vision-capable helper. Depth:
   [`look-grade`](./reference/look-grade.md).

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
Depth: [`operator-families`](./reference/operator-families.md) / [`primer/cook-and-families`](./primer/cook-and-families.md).

## Hard rules

### Sequential bridged tools

- Call bridged tools **one at a time**; wait for each result before the next.
- `fleet` / `describe_tools` stay available during an in-flight call.
- On `tdmcp.mcp.session_busy` or `tdmcp.bridge.queue_busy`: wait, then retry —
  do **not** disconnect or restart. After 3 consecutive failures, hand off to
  the user.

Depth: [`tooling-concurrency`](./reference/tooling-concurrency.md).

### OpSketch before non-trivial networks — HARD RULE

When thinking about, planning, building, or mutating a network of more than
3 nodes (or any non-trivial operator: COMP hub, multi-family chain, data-flow
with branches), **describe it in OpSketch first** — before creating or
mutating anything. Use the sketch to validate the design, then build from it.

Depth: [`opsketch-notation`](./reference/opsketch-notation.md) (+ gating, examples).

### `inspect` over `execute_python` for networks — HARD RULE

`inspect` is the primary tool for network analysis. Do **not** use
`execute_python` as the main network inspector. For DAT `.text` bodies and
GLSL shader sources, prefer `inspect` with `include: ["content"]` (follows
shader DAT refs + `compileResult`; DAT content also reports shader
`consumers[]` compile status) over hand-rolled Python reads. After any
mutation pass, a **final `inspect` on the touched network parent** is mandatory
before claiming done.

### Comment what you build — HARD RULE

Every operator you create that is not self-evident gets a `comment` in the
**same `mutate_nodes` batch that creates it** — COMP hubs, terminal nulls that
other nodes reference, feedback loops, magic constants, GLSL ops, DAT
callbacks. `opType` and node name say what a node *is*; the comment is the only
place *why* survives the end of this session. `inspect` returns comments on
every node and on each child-roster entry, so reading an unfamiliar network
starts there. Depth: [`node-comments`](./reference/node-comments.md).

### Python cheatsheet before Python — HARD RULE

Before `execute_python` or any TD expression/script, **see [`python-api.md`](./reference/python-api.md)** in this turn. Exact live names still go through
`api_help`.

### Look in the Palette before hand-building — HARD RULE

Before building any non-trivial subsystem — particles, audio analysis, a widget
set, a video player, a mapper — check `palette_index` for a stock component
first. Derivative ships hundreds of them, already debugged and already carrying
a designed custom-parameter API. Hand-building what the Palette already has is
the most expensive mistake available here. Depth: [`palette`](./reference/palette.md).

### In/Out operator harness — HARD RULE

Reusable COMP boundaries use dedicated In/Out operators — never path-parameter
inputs or reaching into another COMP's internals. Depth:
[`component-checklist`](./reference/component-checklist.md).

### Relative references — HARD RULE

Never use absolute `/project1/...` inside a reusable network. Prefer relative
paths: `parent().par.Foo`, `parent().op('null_out')`, bare sibling names,
`./child`, `../cousin`. Depth: [`network-design`](./reference/network-design.md).

### Play state before "why isn't it updating" — HARD RULE

Paused transport stalls most cooks; captures can look stale. Check play state
before debugging a network or grading a capture. Depth:
[`play-state`](./reference/play-state.md).

## Custom component basics

Custom parameters are the COMP **control** API — small, curated, page-grouped.
In/Out ops create wiring pins; do not duplicate them as OP-path parameters.
Before scripting custom pars: see [`custom-parameters.md`](./reference/custom-parameters.md).
Packaging / About / reuse: [`component-checklist`](./reference/component-checklist.md).

## Threading & pipeline

TD Python runs on the main thread only — use `run(code, delayFrames=0)` to hand
work back safely. See also [`play-state`](./reference/play-state.md) and
[`primer/scripting-surfaces`](./primer/scripting-surfaces.md).

## Python / scripting quick intro

**Gate:** see [`python-api.md`](./reference/python-api.md) before any `execute_python` or authored
expression — it documents the sandbox scope (`td` / `op` / `result` / closed
aliases; no `me` / `parent` / bare opTypes), `.eval()` discipline, and the
create/wire pattern. Prefer `editor_context` for pane/selection; `inspect` for
network rosters.

## GLSL awareness (quick)

Frag-only → GLSL TOP; vert+frag → GLSL MAT; multi-buffer → one TOP per buffer +
Feedback. Everything else — bridge wrapper, uniforms, res traps, port
procedure — lives in [`glsl`](./reference/glsl.md), [`td-glsl-ground-truth`](./reference/td-glsl-ground-truth.md),
[`shadertoy-conversion`](./reference/shadertoy-conversion.md), and [`primer/glsl-and-render`](./primer/glsl-and-render.md).

## Definition of Done

Observable runtime evidence only — never PASS from code or docs alone.
Doubt → **FAIL**.

| Verdict | Meaning |
|---------|---------|
| `PASS` | Evidence matches claim |
| `FAIL` | Evidence contradicts claim (incl. black frame) |
| `BLOCKED` | Surface unreachable / capture unreadable |
| `SKIP` | No runtime surface, or deliberate cost-reduction |

Depth: [`definition-of-done`](./reference/definition-of-done.md) (full checklist + doubt rules).
Look claims: [`look-grade`](./reference/look-grade.md).

## Resource index (deepen here)

| Need | Resource |
|------|----------|
| This umbrella | [`operate`](./SKILL.md) |
| OpSketch | [`opsketch-notation`](./reference/opsketch-notation.md) (+ gating, examples) |
| Python | [`python-api`](./reference/python-api.md) |
| Custom pars | [`custom-parameters`](./reference/custom-parameters.md) |
| Mutation zones | [`mutation-zones`](./reference/mutation-zones.md) |
| Network / relative refs | [`network-design`](./reference/network-design.md) |
| Operator comments | [`node-comments`](./reference/node-comments.md) |
| Components | [`component-checklist`](./reference/component-checklist.md) |
| Families | [`operator-families`](./reference/operator-families.md) |
| POP | [`pops`](./reference/pops.md) |
| GLSL dialect | [`glsl`](./reference/glsl.md) |
| Shadertoy port | [`shadertoy-conversion`](./reference/shadertoy-conversion.md) |
| GLSL traps | [`td-glsl-ground-truth`](./reference/td-glsl-ground-truth.md) |
| Structural DoD | [`definition-of-done`](./reference/definition-of-done.md) |
| Look / capture | [`look-grade`](./reference/look-grade.md) |
| Parallel / session_busy | [`tooling-concurrency`](./reference/tooling-concurrency.md) |
| Paused / stale | [`play-state`](./reference/play-state.md) |
| Cook / families depth | [`primer/cook-and-families`](./primer/cook-and-families.md) |
| Editor / layout | [`primer/editor-and-layout`](./primer/editor-and-layout.md) |
| Params / channels | [`primer/parameters-and-channels`](./primer/parameters-and-channels.md) |
| Scripting surfaces | [`primer/scripting-surfaces`](./primer/scripting-surfaces.md) |
| tox / toe | [`primer/tox-toe-components`](./primer/tox-toe-components.md) |
| GLSL / render chain | [`primer/glsl-and-render`](./primer/glsl-and-render.md) |
| Performance | [`primer/performance`](./primer/performance.md) |
| Spawn / kill TD | [`lifecycle`](./reference/lifecycle.md) |
| Popup triage | [`popups`](./reference/popups.md) |
| Offline project I/O | [`project-io`](./reference/project-io.md) |
| Palette components | [`palette`](./reference/palette.md) |
| Palette scan / describe | [`palette-scan`](./reference/palette-scan.md) |
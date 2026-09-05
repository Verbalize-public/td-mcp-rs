---
name: touchdesigner
description: >-
  Inspect, build, debug, and capture TouchDesigner projects through td-mcp-rs.
  Use for live operators, parameters, Python, GLSL, Palette components, and
  .toe/.tox project operations when the td-mcp-rs tools are available.
---

# TouchDesigner (td-mcp-rs)

Use live evidence to choose the next operation. Read the references relevant
to the task; there is no need to load the entire manual.

## Working loop

1. Call `fleet`. Select the intended connected `pid`; include `daemonId`
   for a remote or ambiguous target. Inspect the project/path before editing.
   If the task needs a new process, use `spawn_td` and
   [`lifecycle`](./reference/lifecycle.md).
2. Use `inspect` for structure, parameters, wires, and errors. Add
   `include: ["content"]` for DAT text and shader sources.
   `editor_context` supplies a location hint, not permission to edit.
3. Apply changes with `mutate_nodes`. Keep bridged calls sequential on each
   pid. Explain a complex or branched network with OpSketch before building;
   a small parameter fix needs no separate design ceremony.
4. Inspect the changed parent COMP. For visual claims, capture the output
   and examine the image. Report observed failures and unverified behavior
   separately; successful execution alone does not prove the result.

## Choose the tool

| Need | Tool / reference |
| --- | --- |
| Processes and connection state | `fleet` |
| Parameters, hierarchy, wires, DAT text, errors | `inspect` |
| Create, set, delete, connect, place a .tox, write DAT text | `mutate_nodes` |
| Exact operator types and Python members | `api_help` |
| Python beyond the structured tools | `execute_python`; read [`python-api`](./reference/python-api.md) first |
| Rendered output or CHOP samples | `capture`; [`look-grade`](./reference/look-grade.md) |
| Stock components | `palette_index` → `mutate_nodes` with `op: "place"`; [`palette`](./reference/palette.md) |
| Start / stop TouchDesigner | `spawn_td` / `kill_td`; [`lifecycle`](./reference/lifecycle.md) |
| Blocking dialogs | `dialogs`; [`popups`](./reference/popups.md) |
| Offline projects and bridge installation | [`project-io`](./reference/project-io.md) |
| Tool schemas | `describe_tools` |

## Constraints that prevent common failures

- Use explicit pids; there is no persistent selected target.
- Keep bridged requests sequential per pid. Busy means wait, not reconnect.
  Read [`tooling-concurrency`](./reference/tooling-concurrency.md) for queue/recovery behavior.
- Confirm the target subtree. Keep experiments in a named COMP and limit
  edits to the user's task. See [`mutation-zones`](./reference/mutation-zones.md).
- Prefer structured tools over Python network walks. In `execute_python`,
  use `td.noiseTOP` or a type string; `me`, bare `parent`, and bare operator
  classes are not injected.
- Check the Palette before building a substantial subsystem that may
  already exist. Explain non-obvious nodes with `comment`.
- Reusable COMPs use relative references and In/Out operators.
- Check play state when updates or captures appear stale.
- After three failed probes without new evidence, stop repeating them and
  report the blocker. A new diagnosis can justify a different probe.

## References by task

| Task | Read |
| --- | --- |
| Plan a network | [`opsketch-notation`](./reference/opsketch-notation.md), [`network-design`](./reference/network-design.md) |
| Explain design intent | [`node-comments`](./reference/node-comments.md) |
| Build a reusable component | [`component-checklist`](./reference/component-checklist.md), [`custom-parameters`](./reference/custom-parameters.md) |
| Choose an operator family | [`operator-families`](./reference/operator-families.md) |
| GPU geometry and particles | [`pops`](./reference/pops.md) |
| Write / port shaders | [`glsl`](./reference/glsl.md), [`td-glsl-ground-truth`](./reference/td-glsl-ground-truth.md), [`shadertoy-conversion`](./reference/shadertoy-conversion.md) |
| Verify the result | [`definition-of-done`](./reference/definition-of-done.md), [`look-grade`](./reference/look-grade.md) |
| Paused / stale output | [`play-state`](./reference/play-state.md) |
| Curate the Palette library | [`palette-scan`](./reference/palette-scan.md) |
| Cook behavior | [`primer/cook-and-families`](./primer/cook-and-families.md) |
| Editor layout | [`primer/editor-and-layout`](./primer/editor-and-layout.md) |
| Parameters and channels | [`primer/parameters-and-channels`](./primer/parameters-and-channels.md) |
| Scripts and callbacks | [`primer/scripting-surfaces`](./primer/scripting-surfaces.md) |
| Project / component files | [`primer/tox-toe-components`](./primer/tox-toe-components.md) |
| Render pipelines | [`primer/glsl-and-render`](./primer/glsl-and-render.md) |
| Performance | [`primer/performance`](./primer/performance.md) |

## Related

- [`operate`](./SKILL.md) — this entry point
- [`python-api`](./reference/python-api.md) — scripting scope and API details

**Canonical:** [`operate`](./SKILL.md)
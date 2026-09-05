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
   {{ skill("lifecycle") }}.
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
| Python beyond the structured tools | `execute_python`; read {{ skill("python-api") }} first |
| Rendered output or CHOP samples | `capture`; {{ skill("look-grade") }} |
| Stock components | `palette_index` → `mutate_nodes` with `op: "place"`; {{ skill("palette") }} |
| Start / stop TouchDesigner | `spawn_td` / `kill_td`; {{ skill("lifecycle") }} |
| Blocking dialogs | `dialogs`; {{ skill("popups") }} |
| Offline projects and bridge installation | {{ skill("project-io") }} |
| Tool schemas | `describe_tools` |

## Constraints that prevent common failures

- Use explicit pids; there is no persistent selected target.
- Keep bridged requests sequential per pid. Busy means wait, not reconnect.
  Read {{ skill("tooling-concurrency") }} for queue/recovery behavior.
- Confirm the target subtree. Keep experiments in a named COMP and limit
  edits to the user's task. See {{ skill("mutation-zones") }}.
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
| Plan a network | {{ skill("opsketch-notation") }}, {{ skill("network-design") }} |
| Explain design intent | {{ skill("node-comments") }} |
| Build a reusable component | {{ skill("component-checklist") }}, {{ skill("custom-parameters") }} |
| Choose an operator family | {{ skill("operator-families") }} |
| GPU geometry and particles | {{ skill("pops") }} |
| Write / port shaders | {{ skill("glsl") }}, {{ skill("td-glsl-ground-truth") }}, {{ skill("shadertoy-conversion") }} |
| Verify the result | {{ skill("definition-of-done") }}, {{ skill("look-grade") }} |
| Paused / stale output | {{ skill("play-state") }} |
| Curate the Palette library | {{ skill("palette-scan") }} |
| Cook behavior | {{ skill("primer/cook-and-families") }} |
| Editor layout | {{ skill("primer/editor-and-layout") }} |
| Parameters and channels | {{ skill("primer/parameters-and-channels") }} |
| Scripts and callbacks | {{ skill("primer/scripting-surfaces") }} |
| Project / component files | {{ skill("primer/tox-toe-components") }} |
| Render pipelines | {{ skill("primer/glsl-and-render") }} |
| Performance | {{ skill("primer/performance") }} |

## Related

- {{ skill("operate") }} — this entry point
- {{ skill("python-api") }} — scripting scope and API details

**Canonical:** {{ skill("operate") }}

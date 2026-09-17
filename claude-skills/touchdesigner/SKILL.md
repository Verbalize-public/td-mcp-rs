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

This is the canonical TD operating entry point (`operate` in the resource
catalog). With MCP, discover it through `resources/list` and load it through
`resources/read` using the host's resource interface; then follow its reference
links. With installed filesystem skills, read this file and its linked cards.
Tool availability, resource availability, and a connected TD process are
separate checks. Use schemas already exposed by the host; call `describe_tools`
only when schema information is missing or unclear, not as a routine full-catalog
fetch. It describes tools, not dynamic tool activation. Do not invent tool names
from resource names. A harness or persona should route here rather than copy
these procedures.

## Working loop

1. Call `fleet`. Select the intended connected `pid`; include `daemonId`
   for a remote or ambiguous target. Put `pid` in the actual arguments of
   every targeted call; naming it in prose does not select it. Confirm the
   daemon, PID, project and authorized subtree; reconfirm after a restart.
   If the task needs a new process, use `spawn_td` and
   [`lifecycle`](./reference/lifecycle.md).
2. Use `inspect` progressively: default sections are nodes, errors and warnings;
   non-empty `include` is an allowlist. For parameter-name discovery use
   `include:["params"], paramsMode:"names"` (only `{name}`, no value/metadata
   reads). Then select exact case-sensitive `paramNames` with `paramsMode:"values"`
   (default); omitted/null names select all, `[]` selects none. Check
   `paramsSelection.missing` / `complete` when selecting names and each value's
   `evaluation.available`; name coverage is not evaluation success.
   With `include:["nodes"]`, page direct children using `childOffset` (zero-based,
   default 0) and `childLimit` (1–256, default 256); follow `childrenPage.nextOffset`.
   A final page is not a complete roster; offsets can shift when the graph changes.
   Options are request-local, not recursive. Add `content` for DAT/shader sources;
   content and warning enrichment can evaluate separately from names-only params.
   `editor_context` supplies a location hint, not permission to edit.
3. Apply changes with `mutate_nodes`. Keep bridged calls sequential on each
   pid. Explain a complex or branched network with OpSketch before building;
   a small parameter fix needs no separate design ceremony.
4. Verify a bounded, deduplicated set of canonical changed paths, affected
   destinations and relevant parents—not just the parent COMP. Include both
   errors and warnings and check their observation availability. On partial
   failure, inspect successful steps and the failed step's possible effects
   before retrying. Verify deletions through the surviving parent roster, not
   deleted paths as success targets. Follow [`definition-of-done`](./reference/definition-of-done.md);
   shader compilation, viewed pixels and timed behavior need distinct evidence.

## Choose the tool

| Need | Tool / reference |
| --- | --- |
| Processes and connection state | `fleet` |
| Parameters, hierarchy, wires, DAT text, errors | `inspect` |
| Create, set, delete, connect, place a .tox, write DAT text | `mutate_nodes` |
| Exact operator types and Python members | `api_help` |
| Python beyond the structured tools | `execute_python`; read [`python-api`](./reference/python-api.md) first |
| Rendered output or CHOP samples | `capture`; [`look-grade`](./reference/look-grade.md) |
| Exact frame samples or video artifacts | `capture` with `timing` / `record`; [`timed-capture`](./reference/timed-capture.md) |
| Stock components | `palette_index` → `mutate_nodes` with `op: "place"`; [`palette`](./reference/palette.md) |
| Start / stop TouchDesigner | `spawn_td` / `kill_td`; [`lifecycle`](./reference/lifecycle.md) |
| Blocking dialogs | `dialogs`; [`popups`](./reference/popups.md) |
| Offline projects and bridge installation | [`project-io`](./reference/project-io.md) |
| Missing or unclear tool schemas | `describe_tools` on demand; reuse exposed schemas |

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
- Every stateful component you build or change must expose a working reset
  signal, normally a custom parameter, connected by default to the project's
  global reset at integration time. Verify reset then sequential advancement;
  follow [`reset-state`](./reference/reset-state.md) before claiming stateful work complete.
- Check play state when updates or captures appear stale.
- Before reporting completion, distinguish verified structure, observed pixels,
  timed behavior and saved artifacts. Include final transport state and whether
  the project was saved; follow [`definition-of-done`](./reference/definition-of-done.md).
- After three failed probes without new evidence, stop repeating them and
  report the blocker. A new diagnosis can justify a different probe.

## References by task

| Task | Read |
| --- | --- |
| Plan a network | [`opsketch-notation`](./reference/opsketch-notation.md), [`network-design`](./reference/network-design.md) |
| Explain design intent | [`node-comments`](./reference/node-comments.md) |
| Build a reusable component | [`component-checklist`](./reference/component-checklist.md), [`custom-parameters`](./reference/custom-parameters.md) |
| Build stateful behavior / verify repeatable timing | [`reset-state`](./reference/reset-state.md), [`play-state`](./reference/play-state.md) |
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
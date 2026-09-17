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
   {{ skill("lifecycle") }}.
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
   deleted paths as success targets. Follow {{ skill("definition-of-done") }};
   shader compilation, viewed pixels and timed behavior need distinct evidence.

## Choose the tool

| Need | Tool / reference |
| --- | --- |
| Processes and connection state | `fleet` |
| Parameters, hierarchy, wires, DAT text, errors | `inspect` |
| Create, set, delete, connect, place a .tox, write DAT text | `mutate_nodes` |
| Exact operator types and Python members | `api_help` |
| Python beyond the structured tools | `execute_python`; read {{ skill("python-api") }} first |
| Rendered output or CHOP samples | `capture`; {{ skill("look-grade") }} |
| Exact frame samples or video artifacts | `capture` with `timing` / `record`; {{ skill("timed-capture") }} |
| Stock components | `palette_index` → `mutate_nodes` with `op: "place"`; {{ skill("palette") }} |
| Start / stop TouchDesigner | `spawn_td` / `kill_td`; {{ skill("lifecycle") }} |
| Blocking dialogs | `dialogs`; {{ skill("popups") }} |
| Offline projects and bridge installation | {{ skill("project-io") }} |
| Missing or unclear tool schemas | `describe_tools` on demand; reuse exposed schemas |

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
- Every stateful component you build or change must expose a working reset
  signal, normally a custom parameter, connected by default to the project's
  global reset at integration time. Verify reset then sequential advancement;
  follow {{ skill("reset-state") }} before claiming stateful work complete.
- Check play state when updates or captures appear stale.
- Before reporting completion, distinguish verified structure, observed pixels,
  timed behavior and saved artifacts. Include final transport state and whether
  the project was saved; follow {{ skill("definition-of-done") }}.
- After three failed probes without new evidence, stop repeating them and
  report the blocker. A new diagnosis can justify a different probe.

## References by task

| Task | Read |
| --- | --- |
| Plan a network | {{ skill("opsketch-notation") }}, {{ skill("network-design") }} |
| Explain design intent | {{ skill("node-comments") }} |
| Build a reusable component | {{ skill("component-checklist") }}, {{ skill("custom-parameters") }} |
| Build stateful behavior / verify repeatable timing | {{ skill("reset-state") }}, {{ skill("play-state") }} |
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

# td-mcp-rs agent skills

Operate pack for TouchDesigner agents. Embedded in the daemon binary, extracted
to `{dataDir}/skills/` on `install` / `ensure` / `start` / `mcp`.

## Layout

```text
skills/
  README.md                 # this file
  touchdesigner/
    SKILL.md                # optional host skill ingest (also tdmcp://docs/operate)
    reference/              # operate cards → MCP resources
    primer/                 # TD software condensation → MCP resources
```

## Lambda path (preferred)

Connect td-mcp-rs MCP. Server `instructions` list when → which URI. Then:

```text
resources/read  uri=tdmcp://docs/opsketch-notation
resources/read  uri=tdmcp://docs/python-api
```

No Cursor skill copy required.

## Optional host ingest

```text
tdmcp-daemon skills path
tdmcp-daemon skills copy --dest ~/.cursor/skills
# OpenCode example:
# tdmcp-daemon skills copy --dest ~/.config/opencode/skills
```

## Resource URI catalog

| URI | Disk |
|-----|------|
| `tdmcp://docs/operate` | `touchdesigner/SKILL.md` |
| `tdmcp://docs/opsketch-notation` | `touchdesigner/reference/opsketch-notation.md` |
| `tdmcp://docs/opsketch-importance-gating` | `touchdesigner/reference/opsketch-importance-gating.md` |
| `tdmcp://docs/opsketch-examples` | `touchdesigner/reference/opsketch-examples.md` |
| `tdmcp://docs/python-api` | `touchdesigner/reference/python-api.md` |
| `tdmcp://docs/custom-parameters` | `touchdesigner/reference/custom-parameters.md` |
| `tdmcp://docs/mutation-zones` | `touchdesigner/reference/mutation-zones.md` |
| `tdmcp://docs/network-design` | `touchdesigner/reference/network-design.md` |
| `tdmcp://docs/component-checklist` | `touchdesigner/reference/component-checklist.md` |
| `tdmcp://docs/operator-families` | `touchdesigner/reference/operator-families.md` |
| `tdmcp://docs/pops` | `touchdesigner/reference/pops.md` |
| `tdmcp://docs/glsl` | `touchdesigner/reference/glsl.md` |
| `tdmcp://docs/shadertoy-conversion` | `touchdesigner/reference/shadertoy-conversion.md` |
| `tdmcp://docs/td-glsl-ground-truth` | `touchdesigner/reference/td-glsl-ground-truth.md` |
| `tdmcp://docs/definition-of-done` | `touchdesigner/reference/definition-of-done.md` |
| `tdmcp://docs/look-grade` | `touchdesigner/reference/look-grade.md` |
| `tdmcp://docs/tooling-concurrency` | `touchdesigner/reference/tooling-concurrency.md` |
| `tdmcp://docs/play-state` | `touchdesigner/reference/play-state.md` |
| `tdmcp://docs/primer/cook-and-families` | `touchdesigner/primer/cook-and-families.md` |
| `tdmcp://docs/primer/editor-and-layout` | `touchdesigner/primer/editor-and-layout.md` |
| `tdmcp://docs/primer/parameters-and-channels` | `touchdesigner/primer/parameters-and-channels.md` |
| `tdmcp://docs/primer/scripting-surfaces` | `touchdesigner/primer/scripting-surfaces.md` |
| `tdmcp://docs/primer/tox-toe-components` | `touchdesigner/primer/tox-toe-components.md` |
| `tdmcp://docs/primer/glsl-and-render` | `touchdesigner/primer/glsl-and-render.md` |
| `tdmcp://docs/primer/performance` | `touchdesigner/primer/performance.md` |

# TouchDesigner GLSL

TD GLSL (4.60, Vulkan) differs in I/O, sampler arrays, resolution uniforms. Convert via bridge-wrapper; never paste foreign shaders unmodified.

This page = dialect/port guidance + how GLSL fits TD render software.
Live host: `tdmcp://docs/operate` (tools are self-describing).

## Read first

- `tdmcp://docs/td-glsl-ground-truth` — `fragColor`, `TDOutputSwizzle`, `vUV`, `sTD2DInputs[]`, `TDTexInfo.res` trap
- `tdmcp://docs/shadertoy-conversion` — wrap-don't-rewrite bridge
- `tdmcp://docs/primer/glsl-and-render` — TOP vs MAT vs Render chain

## Workflow

| Step | Action |
|------|--------|
| 1 | Classify: frag → GLSL TOP; vert+frag → GLSL MAT; multi-buffer → one TOP/buffer + Feedback |
| 2 | Apply bridge: TD preamble + `main()` calling `mainImage`; strip `#version` / `texture2D` |
| 3 | Author via `mutate_nodes` / `execute_python` under the mutation zone; verify with `inspect` |
| 4 | FAIL → fix DAT from compile log (line nums offset by preamble); re-run |
| 5 | Feed uniforms on Vectors 1 (`vec0name` / `vec0value*`); unfed uniform = warning = FAIL |

Promote with relative exprs — see `tdmcp://docs/network-design`.

## DoD

1. Compile clean (zero errors and warnings) via live `inspect`
2. Look claim → `tdmcp://docs/look-grade` (non-black capture)
3. Inputs wired match source channel list

Black + clean compile = FAIL (unfed uniform / missing input / Extend ≠ Repeat).

## Safety

Invalid GLSL can hard-crash Vulkan TD — keep experiments in `/project1/_agent_scratch`. Same compile error after 3 distinct fixes → stop and ask.


---

**Canonical:** `tdmcp://docs/glsl` · disk: `reference/glsl.md`

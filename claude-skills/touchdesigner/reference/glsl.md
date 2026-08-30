# TouchDesigner GLSL

TD GLSL (4.60, Vulkan) differs in I/O, sampler arrays, resolution uniforms. Convert via bridge-wrapper; never paste foreign shaders unmodified.

This page = dialect/port guidance + how GLSL fits TD render software.
Live host: [`operate`](../SKILL.md) (tools are self-describing).

## Read first

- [`td-glsl-ground-truth`](./td-glsl-ground-truth.md) — `fragColor`, `TDOutputSwizzle`, `vUV`, `sTD2DInputs[]`, `TDTexInfo.res` trap
- [`shadertoy-conversion`](./shadertoy-conversion.md) — wrap-don't-rewrite bridge
- [`primer/glsl-and-render`](../primer/glsl-and-render.md) — TOP vs MAT vs Render chain

## Workflow

| Step | Action |
|------|--------|
| 1 | Classify: frag → GLSL TOP; vert+frag → GLSL MAT; multi-buffer → one TOP/buffer + Feedback |
| 2 | Apply bridge: TD preamble + `main()` calling `mainImage`; strip `#version` / `texture2D` |
| 3 | Author via `mutate_nodes` (`text` on the stage DAT) under the mutation zone; the return carries `shaderDiagnostics` — compile errors surface immediately |
| 4 | FAIL → fix from `shaderDiagnostics[].lines` (line nums offset by preamble); re-run. Verify with `inspect` content (`compileState`) |
| 5 | Feed uniforms on Vectors 1 (`vec0name` / `vec0value*`); unfed uniform = warning = FAIL |

Promote with relative exprs — see [`network-design`](./network-design.md).

## Definition of Done

1. Compile clean (zero errors and warnings) via live `inspect`
2. Look claim → [`look-grade`](./look-grade.md) (non-black capture)
3. Inputs wired match source channel list

Black + clean compile = FAIL (unfed uniform / missing input / Extend ≠ Repeat).

## Safety

Invalid GLSL can hard-crash Vulkan TD — keep experiments inside your mutation zone
([`mutation-zones`](./mutation-zones.md)), never on production nodes. Same compile error after 3
distinct fixes → stop and ask.

## Related

- [`td-glsl-ground-truth`](./td-glsl-ground-truth.md) — sampler arrays, res trap, compute
- [`shadertoy-conversion`](./shadertoy-conversion.md) — port procedure
- [`primer/glsl-and-render`](../primer/glsl-and-render.md) — TOP vs MAT vs render chain
- [`definition-of-done`](./definition-of-done.md) — structural verdicts


---

**Canonical:** [`glsl`](./glsl.md) 
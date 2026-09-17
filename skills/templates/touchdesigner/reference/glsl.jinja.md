# TouchDesigner GLSL

TD GLSL (4.60, Vulkan) differs in I/O, sampler arrays, resolution uniforms. Convert via bridge-wrapper; never paste foreign shaders unmodified.

This page = dialect/port guidance + how GLSL fits TD render software.
Live host: {{ skill("operate") }} (tools are self-describing).

## Read first

- {{ skill("td-glsl-ground-truth") }} — `fragColor`, `TDOutputSwizzle`, `vUV`, `sTD2DInputs[]`, `TDTexInfo.res` trap
- {{ skill("shadertoy-conversion") }} — wrap-don't-rewrite bridge
- {{ skill("primer/glsl-and-render") }} — TOP vs MAT vs Render chain

## Workflow

| Step | Action |
|------|--------|
| 1 | Classify: frag → GLSL TOP; vert+frag → GLSL MAT; multi-buffer → one TOP/buffer + Feedback |
| 2 | Apply bridge: TD preamble + `main()` calling `mainImage`; strip `#version` / `texture2D` |
| 3 | Author via `mutate_nodes` (`text` on the stage DAT) under the mutation zone; the return carries `shaderDiagnostics` — compile errors surface immediately |
| 4 | FAIL → fix from `shaderDiagnostics[].lines` (line nums offset by preamble); re-run. Verify with `inspect` content (`compileState`) |
| 5 | Feed uniforms on Vectors 1 (`vec0name` / `vec0value*`); unfed uniform = warning = FAIL |

Promote with relative exprs — see {{ skill("network-design") }}.

## Definition of Done

1. Affirmative `compileState:"compiled"` plus available error/warning reads via
   live `inspect`; `unknown` or `unsupported` is not a compile pass. A successful
   text write does not prove compilation. Use `compileDiagnostic` / bounded logs
   for evidence. GLSL POP uses an existing Info DAT bound to that POP, with
   `infotype:general` and passive off (verified 2025.32460). Preserve its default
   docked Info DAT: it exposes full compiler paths/lines despite the missing
   `OP.compileResult` attribute. The bridge never creates one during inspection.
   Without a usable observer, status remains unsupported and `operatorErrors`
   are best-effort fallback evidence, not proof of success.
2. Look claim → {{ skill("look-grade") }} (capture compared with requested appearance)
3. Inputs wired match source channel list

Unexpected black output needs investigation (uniforms, inputs, extend mode);
intentional masks, solids and fades are judged against the requested result.
Start from a known-rendering baseline, then change one stage at a time. Keep
failure notes scoped to the observed build, parameters and topology.

## Safety

Invalid GLSL can hard-crash Vulkan TD — keep experiments inside your mutation zone
({{ skill("mutation-zones") }}), never on production nodes. Same compile error after 3
distinct fixes → stop and ask.

## Related

- {{ skill("td-glsl-ground-truth") }} — sampler arrays, res trap, compute
- {{ skill("shadertoy-conversion") }} — port procedure
- {{ skill("primer/glsl-and-render") }} — TOP vs MAT vs render chain
- {{ skill("definition-of-done") }} — structural verdicts


---

**Canonical:** {{ skill("glsl") }} 

# Shadertoy → TouchDesigner conversion procedure

Sources: [Write a GLSL TOP](https://docs.derivative.ca/Write_a_GLSL_TOP).
Procedure verified live — see the worked example at the bottom.

## Strategy: wrap, don't rewrite

Keep the Shadertoy code intact and add a TD bridge around it. Less error-prone than
find-and-replacing `fragCoord` throughout, and diffs cleanly against the original.

```glsl
layout(location = 0) out vec4 fragColor;

// --- TD bridge: uniforms fed from the GLSL TOP's Vectors 1 page ---
uniform float iTime;
uniform int   iFrame;
uniform vec4  iMouse;
#define iResolution vec3(uTDOutputInfo.res.zw, 1.0)

// --- original Shadertoy code below, unmodified except channel/texture fixes ---
void mainImage(out vec4 c, in vec2 fragCoord) { /* ...pasted... */ }

// --- TD entry point ---
void main()
{
    vec4 c;
    mainImage(c, vUV.st * uTDOutputInfo.res.zw);
    fragColor = TDOutputSwizzle(c);
}
```

## Substitution table

| Shadertoy | TouchDesigner | Notes |
|-----------|---------------|-------|
| `void mainImage(out vec4, in vec2)` | keep; call from a new `main()` | bridge above |
| `fragCoord` | `vUV.st * uTDOutputInfo.res.zw` (arg to `mainImage`) | or `gl_FragCoord.xy` |
| `fragColor` (out param) | `layout(location = 0) out vec4 fragColor;` + `TDOutputSwizzle` | |
| `iResolution` (vec3) | `uTDOutputInfo.res.zw` via the `#define` above | `.res.xy` is 1/size — trap |
| `iChannel0..3` (2D) | `sTD2DInputs[n]` | n counts 2D inputs only, in connector order |
| `iChannel0..3` (cubemap) | `sTDCubeInputs[n]` | cube inputs counted separately |
| `iChannelResolution[n]` | `uTD2DInfos[n].res.zw` | |
| `iTime` (`iGlobalTime` in old shaders) | `uniform float iTime;` ← Vectors 1 value `absTime.seconds` | |
| `iTimeDelta` | uniform ← `1 / me.time.rate` or `absTime.stepSeconds` | |
| `iFrame` | `uniform int iFrame;` ← `absTime.frame` | |
| `iMouse` (vec4) | `uniform vec4 iMouse;` ← Mouse In CHOP or Panel CHOP u/v (×resolution) | `0,0,0,0` to just compile |
| `iDate` (vec4) | uniform ← Python `datetime` expressions | rarely load-bearing |
| `texture2D(...)` / `textureCube(...)` | `texture(...)` | removed in modern GLSL |
| audio channel | Audio CHOP → CHOP To TOP → an `sTD2DInputs[n]` | Analyze CHOP for single values |
| Buffer A/B/... tabs | one GLSL TOP per buffer; self-reference via **Feedback TOP** | direct self-wire = cook loop |

## Node-side settings (not code)

- GLSL TOP **Output Resolution → Custom** (e.g. 1280x720) — Shadertoy shaders assume a
  real resolution, and the default input-sized behavior surprises with no inputs.
- **Input Extend Mode UV → Repeat** on the Common page — Shadertoy samplers wrap by
  default; mismatch shows as edge streaks, not an error.
- Shadertoy's noise channels: Noise TOP (64x64 or 256x256, monochrome or color to match);
  `sTDNoiseMap` works for quick monochrome cases.
- Textures/cubemaps from shadertoy.com can be fetched via their API
  (`https://www.shadertoy.com/api/v1/shaders/<ID>?key=<AppKey>` lists inputs); cube
  sources connect through a Cube Map TOP (e.g. Vertical Cross layout).

## Error-reading loop

Prefer the bridge loop: `mutate_nodes` create the GLSL TOP + write the stage DAT via
`text` — the step return's `shaderDiagnostics[]` carries compile status (error lines
with DAT path + line number, or a compiled note) without extra calls. Iterate: write →
read diagnostics → fix → re-write until clean. For warnings and deeper reads wire an
Info DAT (`info.par.op = glsl`) or `inspect` content; then `capture` if look is still
unresolved (store-first).

Line numbers in errors are offset by your bridge preamble; count from the top of the DAT,
not the original Shadertoy source.

## Definition of Done

- [ ] Compile clean — zero errors **and** warnings via live `inspect` (unfed
      uniform = warning = not done)
- [ ] Every `iChannel` maps to a wired `sTD2DInputs[n]` / `sTDCubeInputs[n]`
      matching the source channel list
- [ ] Look claim graded via {{ skill("look-grade") }} — non-black `capture`;
      black + clean compile = FAIL
- [ ] Same compile error after 3 distinct fixes → stop and ask

## Other source dialects (same bridge idea)

- **Book of Shaders / generic WebGL fragment shaders:** `gl_FragCoord` works as-is;
  map `u_resolution` → `uTDOutputInfo.res.zw`, `u_time` → `iTime`-style uniform,
  `u_mouse` → Mouse In CHOP uniform; replace `gl_FragColor` with the declared out +
  `TDOutputSwizzle`; delete `precision mediump float;` and any `#version` line.
- **ISF:** strip the JSON comment block; `INPUTS` become Vectors-page uniforms or TOP
  inputs; `RENDERSIZE` → `uTDOutputInfo.res.zw`, `TIME` → `iTime`, `isf_FragNormCoord`
  → `vUV.st`.
- **Vertex+fragment pairs (materials):** these are GLSL MAT territory, not a GLSL TOP —
  positions go through `TDDeform()`/`TDWorldToProj()`; see {{ skill("td-glsl-ground-truth") }}.

## Related

- {{ skill("glsl") }} — dialect workflow
- {{ skill("td-glsl-ground-truth") }} — sampler/res traps, GLSL MAT vs TOP
- {{ skill("primer/glsl-and-render") }} — TOP vs MAT vs render chain
- {{ skill("look-grade") }} — capture-based look claims

## Worked example (verified live)

"Creation by Silexars" (Shadertoy `XsXXDn`: `iTime` + `iResolution`, no channels)
converted with the bridge above and rendered animating on a live GLSL TOP
(two captures at different `absTime` → different frames). What was actually hit:

- After the bridge compiled, the harness reported `COMPILE FAIL` with
  `Warning: Uniform 'iTime' is not assigned. Please assign it on the Colors or
  Vectors page.` — TD treats a declared-but-unfed uniform as a node **warning**
  (not a compile error, and not silent). Feed it and the warning clears.
- The uniform parameters on the GLSL TOP are named `vec0name`, `vec0valuex..w`
  (block per uniform: `vec1name`, ...). Assign via script:
  `g.par.vec0name = 'iTime'; g.par.vec0valuex.expr = 'absTime.seconds'`.
- The original's `fragColor = vec4(c/l, z)` writes time into alpha — harmless in
  Shadertoy, but TD composites alpha downstream; force alpha to 1.0 in the bridge
  `main()` if the result is used in compositing.

Known conversions to expect from older shaders (from the sources above, not hit here):
`texture2D`/`textureCube` → `texture`; any `#version` line must be deleted (TD injects
its own and errors otherwise).


---

**Canonical:** {{ skill("shadertoy-conversion") }} 

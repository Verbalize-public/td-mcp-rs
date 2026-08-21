# TD GLSL ground truth (TD 2025.x, Vulkan, GLSL 4.60)

Source: [Write a GLSL TOP](https://docs.derivative.ca/Write_a_GLSL_TOP) cross-checked
against the live build during authoring. TD compiles with Vulkan since 2022; GLSL 3.30-era
habits (`gl_FragColor`, `texture2D`, `#version` lines) are errors now.

## Pixel shader skeleton (GLSL TOP)

```glsl
layout(location = 0) out vec4 fragColor;
void main()
{
    vec4 color = vec4(1.0, 0.0, 0.0, 1.0);
    fragColor = TDOutputSwizzle(color);
}
```

- **No `#version` line** — TD injects it; including one is a compile error.
- You declare the output yourself (`fragColor` is a convention, not a builtin).
- **Always route the final color through `TDOutputSwizzle()`** — cross-platform channel
  correctness (e.g. alpha-only textures are stored red-only).
- `vUV` (vec3) is auto-declared in pixel shaders *only if you don't supply a vertex
  shader*; `vUV.st` is the 0-1 texture coordinate of the output pixel, (0.5, 0.5) at center.

## Inputs (samplers)

Inputs are auto-declared arrays, split by dimensionality — you never declare them:

```glsl
uniform sampler2D      sTD2DInputs[TD_NUM_2D_INPUTS];
uniform sampler3D      sTD3DInputs[TD_NUM_3D_INPUTS];
uniform sampler2DArray sTD2DArrayInputs[TD_NUM_2D_ARRAY_INPUTS];
uniform samplerCube    sTDCubeInputs[TD_NUM_CUBE_INPUTS];
```

- Index = order **within that dimensionality**, not connector order: with inputs
  (2D, cube, 2D), the second 2D TOP is `sTD2DInputs[1]`, the cube is `sTDCubeInputs[0]`.
- Sample with generic `texture(sTD2DInputs[0], vUV.st)` — `texture2D`/`textureCube` are gone.
- Non-compile-time-constant index → wrap in `nonuniformEXT(i)`.
- Free extras: `sTDNoiseMap` (256x256 random red-only), `sTDSineLookup` (0-1 sine ramp).

## Resolution info — the x/y vs z/w trap

```glsl
struct TDTexInfo { vec4 res; vec4 depth; };
uniform TDTexInfo uTD2DInfos[TD_NUM_2D_INPUTS];
uniform TDTexInfo uTDOutputInfo;
// res = (1.0/width, 1.0/height, width, height)
```

**`.res.xy` is 1/size; `.res.zw` is the pixel size.** Reading `.xy` expecting pixels is
the single most common porting bug (regular forum trap). Output resolution:
`uTDOutputInfo.res.zw`. Pixel-space coordinate of the current fragment:
`vUV.st * uTDOutputInfo.res.zw` (or `gl_FragCoord.xy`).

## Custom uniforms

Declare in code (`uniform float iTime;`) and create a matching entry on the GLSL TOP's
**Vectors 1** parameter page (uniform name + value/expression, e.g. `absTime.seconds`).
Unmatched declarations are fine; using an undeclared uniform is a compile error.

## Multi-pass / multi-buffer

- `uTDPass` (int uniform): current pass index when "Num Passes" > 1.
- Multiple color buffers: raise "# of Color Buffers", declare
  `layout(location = 1) out vec4 other;` etc., fetch extras with a Render Select TOP.
  Write every output every frame — unwritten pixels are undefined.
- Self-referencing chains need a Feedback TOP (direct wire = cook dependency loop).

## Compute shaders (TOP)

Output textures are pre-declared; write via
`TDImageStoreOutput(0, ivec3(gl_GlobalInvocationID.xy, 0), color)` (range-checked,
auto-swizzled — no `TDOutputSwizzle` needed). No `vUV`; derive coordinates from
`gl_GlobalInvocationID` + `TDTexInfo`, or use `texelFetch`. This replaced the pre-2025.30000
`sTDComputeOutputs[]` image-store path for sRGB correctness.

## GLSL MAT vs GLSL TOP

A GLSL TOP is a full-screen quad — image work only. Geometry/lighting shaders live in the
**GLSL MAT** (vertex + pixel, optional geometry), with a different contract:
`TDDeform(P)`, `TDWorldToProj(...)`, `TDInstanceID()`, `TDLighting*` helpers. Don't paste
TOP-style shaders into a MAT or vice versa. (Vertex-shader-driven material ports go here;
see shadertoy-conversion.md for image-style ports.)

## Built-in helper library (highlights)

`TDPerlinNoise(vec2|3|4)`, `TDSimplexNoise(...)`, `TDRotateX/Y/Z(rad)`,
`TDRotateOnAxis(rad, axis)`, `TDHSVToRGB`/`TDRGBToHSV`, `TDLuminance(c)`,
`TDRemap(v, lo1, hi1, lo2, hi2)`, `TDDither(c)`, bicubic interpolation, quaternion math,
color-space transfers (`TDTransferSRGBToLinear`, ACES, PQ/HLG...). Prefer these over
pasting generic noise/rotation boilerplate when porting.

## Crash warning

Under Vulkan, out-of-bounds or otherwise invalid GLSL can crash the whole TD process,
not just fail the node. Treat compile-warning-free as mandatory before wiring a shader
into anything, and keep experiments in the scratch container.


---

**Canonical:** `tdmcp://docs/td-glsl-ground-truth` 

# Primer: GLSL and render chain

**Canonical:** `tdmcp://docs/primer/glsl-and-render` · disk: `primer/glsl-and-render.md`

## Where shaders live

| Path | When |
|------|------|
| GLSL TOP | Fragment (2D) processing |
| GLSL MAT | Vertex + pixel on rendered geometry |
| Multi-buffer | One TOP per buffer + Feedback TOP as needed |

TD injects its own uniforms / input samplers (`sTD*Inputs`, `uTD*Infos`, etc.).
Never paste a foreign shader unmodified — use a TD bridge wrapper. Dialect and
port procedure: `tdmcp://docs/glsl`, `tdmcp://docs/shadertoy-conversion`,
`tdmcp://docs/td-glsl-ground-truth`.

## 3D to pixels (summary)

Geometry COMP + Camera + Light → Render TOP (and Render Pass TOP for multipass).
Materials (MAT) shade rendered geometry. Prefer live `inspect` of the render
chain over inventing node graphs from memory.

## Related

- `tdmcp://docs/glsl`
- `tdmcp://docs/pops` (geometry on POP path)
- Wiki: https://docs.derivative.ca/ (GLSL TOP / Render TOP)

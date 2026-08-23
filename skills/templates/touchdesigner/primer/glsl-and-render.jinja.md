# Primer: GLSL and render chain

## Where shaders live

| Path | When |
|------|------|
| GLSL TOP | Fragment (2D) processing |
| GLSL MAT | Vertex + pixel on rendered geometry |
| Multi-buffer | One TOP per buffer + Feedback TOP as needed |

TD injects its own uniforms / input samplers (`sTD*Inputs`, `uTD*Infos`, etc.).
Never paste a foreign shader unmodified — use a TD bridge wrapper. Dialect and
port procedure: {{ skill("glsl") }}, {{ skill("shadertoy-conversion") }},
{{ skill("td-glsl-ground-truth") }}.

To **read** live shader source from a GLSL TOP/MAT/POP, prefer `inspect` with
`include: ["content"]` (follows DAT refs + `compileResult`, classifies
`compileState`; DAT content lists shader `consumers[]`) over
`execute_python`. To **write** shader text, use `mutate_nodes` `text` — the
return lints consuming shaders (`shaderDiagnostics[]`) automatically.

## 3D to pixels (summary)

Geometry COMP + Camera + Light → Render TOP (and Render Pass TOP for multipass).
Materials (MAT) shade rendered geometry. Prefer live `inspect` of the render
chain over inventing node graphs from memory.

## Related

- {{ skill("glsl") }}
- {{ skill("pops") }} (geometry on POP path)
- Wiki: https://docs.derivative.ca/ (GLSL TOP / Render TOP)

---

**Canonical:** {{ skill("primer/glsl-and-render") }}

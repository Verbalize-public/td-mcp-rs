# Primer: scripting surfaces

## Where Python runs

| Surface | Role |
|---------|------|
| Textport | Interactive; symbols like `me` / `op` available |
| DAT scripts / callbacks | Execute DAT family, panel/parameter/DAT/CHOP execute |
| COMP Extensions | Promoted Python classes on components |
| Parameter expressions | Per-parameter Python |
| `execute_python` (MCP) | Sandboxed agent eval — see [`python-api`](../reference/python-api.md) |

TD Python is **main-thread only**. Hand work back with
`run(code, delayFrames=0)` when needed.

## Agent preference

1. Structure / wires / params / errors → `inspect`
2. Create / set / delete / wire → `mutate_nodes`
3. `execute_python` only when those tools cannot express the need
4. Before any Python: see [`python-api.md`](../reference/python-api.md)

## Related

- [`python-api`](../reference/python-api.md)
- [`custom-parameters`](../reference/custom-parameters.md)
- [`tooling-concurrency`](../reference/tooling-concurrency.md)
- Wiki: https://docs.derivative.ca/Python

---

**Canonical:** [`primer/scripting-surfaces`](./scripting-surfaces.md)
# Primer: projects, tox, and components

## Files

| Artifact | Role |
|----------|------|
| `.toe` | Project file |
| `.tox` | Component package (reusable network) |

Both are opaque TD binaries. Nothing outside TouchDesigner can read or patch
them by hand — go through the official tools, which td-mcp-rs wraps as
`project_unpack` / `project_pack` / `project_lint` / `project_install_bridge`
(offline, no running TD). Depth: [`project-io`](../reference/project-io.md).

Clone Master / External Tox reload are project packaging features — see Derivative
docs when cloning or externalizing. Agents operating live should still follow
In/Out + relative-ref rules inside reusable COMPs.

## Reusable COMP operate checklist (summary)

- Boundary data via In/Out family ops — not OP-path custom pars as main inputs
- Relative references inside the COMP
- Curated custom parameters + About page
- Point `par.opviewer` at primary Out when using a viewer

Full checklist: [`component-checklist`](../reference/component-checklist.md).

## Related

- [`component-checklist`](../reference/component-checklist.md)
- [`network-design`](../reference/network-design.md)
- Offline unpack / pack / bridge install: [`project-io`](../reference/project-io.md)
- Opening a `.toe` to verify it: [`lifecycle`](../reference/lifecycle.md)
- Wiki: https://docs.derivative.ca/ (search Tox / Component)

---

**Canonical:** [`primer/tox-toe-components`](./tox-toe-components.md)
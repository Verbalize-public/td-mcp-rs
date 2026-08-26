# Primer: projects, tox, and components

## Files

| Artifact | Role |
|----------|------|
| `.toe` | Project file |
| `.tox` | Component package (reusable network) |

Both are opaque TD binaries. Nothing outside TouchDesigner can read or patch
them by hand — go through the official tools, which td-mcp-rs wraps as
`project_unpack` / `project_pack` / `project_lint` / `project_install_bridge`
(offline, no running TD). Depth: {{ skill("project-io") }}.

Clone Master / External Tox reload are project packaging features — see Derivative
docs when cloning or externalizing. Agents operating live should still follow
In/Out + relative-ref rules inside reusable COMPs.

## Reusable COMP operate checklist (summary)

- Boundary data via In/Out family ops — not OP-path custom pars as main inputs
- Relative references inside the COMP
- Curated custom parameters + About page
- Point `par.opviewer` at primary Out when using a viewer

Full checklist: {{ skill("component-checklist") }}.

## Related

- {{ skill("component-checklist") }}
- {{ skill("network-design") }}
- Offline unpack / pack / bridge install: {{ skill("project-io") }}
- Opening a `.toe` to verify it: {{ skill("lifecycle") }}
- Wiki: https://docs.derivative.ca/ (search Tox / Component)

---

**Canonical:** {{ skill("primer/tox-toe-components") }}

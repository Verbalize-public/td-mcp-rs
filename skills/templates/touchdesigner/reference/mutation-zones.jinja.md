# Mutation zones (TD)

Choose the smallest verified subtree that contains the requested change.

| Situation | Target |
| --- | --- |
| User names a path | Resolve it with `inspect` and work there |
| New experiment or subsystem | Create a descriptively named Base/Container COMP |
| User refers to the editor selection | Read `editor_context`, then confirm with `inspect` |

Preserve unrelated nodes. The user's request can authorize modifying or
replacing existing nodes; do not ask again for routine changes already in
scope. Confirm before deleting work when the intended scope is unclear.

For reusable networks, keep references relative and expose family-appropriate
In/Out pins. For visual output, set the COMP viewer to its terminal TOP and
verify that output with `capture`.

Use OpSketch when a network's branches or component boundaries benefit from
an explicit design. Small edits can be applied directly.

## Related

- {{ skill("opsketch-notation") }} — network sketches
- {{ skill("network-design") }} — naming, layout, relative paths
- {{ skill("component-checklist") }} — reusable COMP packaging
- {{ skill("custom-parameters") }} — control API

**Canonical:** {{ skill("mutation-zones") }}

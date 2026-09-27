# Author or ship a component

Current-tool procedure. This is for component/subsystem authoring or release, not a mandatory ritual for every inspection or parameter fix.

## Decide the boundary before growing the graph

Use a named, authorized container for a new subsystem. Add boundaries when they improve ownership, data flow, reuse or state management—not one wrapper per trivial node. Existing user architecture and explicit task requirements take precedence over house-style preferences.

Sketch topology when branches/interfaces are still being decided or explanation benefits from it. Node count alone is not a trigger. OpSketch describes a plan or observed graph; it is not a callable tool. Mark planned versus observed, and do not invent wires/parameters from a parent roster that never returned them.

Before hand-building a substantial stock-like subsystem, make a bounded Palette search when relevant. Select a few plausible candidates, inspect their capabilities/fingerprint and stop when the choice is resolved. A missing card does not automatically authorize a whole-library scan/description project. Probe arbitrary components only in an appropriately authorized isolated target; a probe can execute component code and wedge TD.

## Build and maintain the intended API

- Discover existing controls before adding new names or shadowing built-ins. Use exact current types/names/menu tokens and the tested custom-parameter rules.
- Expose the controls and In/Out contracts the caller needs; do not expose every internal knob or leave agent-only debug controls as the public API.
- Preserve existing values, bindings, callback intent and metadata unless their change is in scope. UI `readOnly` is not permission to overwrite derived status programmatically.
- Reusable components need portable references and the appropriate reset/state contract. Test references in their actual owner context; do not apply one universal bare/`./`/`../` rule to scripts, expressions and OP-path parameters.
- Inspect newly created COMP contents; initializers may create children. Do not assume empty or blindly destroy everything in an existing target.
- Verify returned canonical paths, connector indices and partial outcomes. Follow {{ skill("observe-change") }} and {{ skill("recover") }}.

## Durable intent and useful finish

Comment new or restructured non-obvious nodes with role/contract, not a changelog. When your authorized change invalidates a comment, update it in the same change. During a read-only walkthrough, flag stale comments rather than editing them.

Apply layout once it is useful, in the affected scope; preserve intentional user layout. Current structured `values`/`flags` do not set `nodeX/nodeY`. If positioning is required, use the supported authorized property-writing path and observe actual coordinates/editor layout—not just a structural inspect reply.

For a shipped reusable asset, apply its declared release profile: sensible defaults, interface/help, public reset when stateful, portability, appropriate version/identity and relevant performance budget. Reuse the project's metadata convention; do not invent an old creation date. Presets/About pages/annotations are not automatic new requirements for every scratch probe or scalar repair.

Verify the requested behavior, capture/look where relevant, and perform the required save/reload checks. Do not label a temporary live graph a durable reusable deliverable. Finish with {{ skill("verify-handoff") }}.

## Related

- {{ skill("operate") }}
- {{ skill("network-design") }}
- {{ skill("component-checklist") }}
- {{ skill("custom-parameters") }}
- {{ skill("palette") }}
- {{ skill("node-comments") }}

**Canonical:** {{ skill("component-authoring") }}

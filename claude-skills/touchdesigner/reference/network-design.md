# Network design conventions

Sources: [First Things to Know](https://docs.derivative.ca/First_Things_to_Know_about_TouchDesigner),
[OP Class](https://docs.derivative.ca/OP_Class), [TDFunctions](https://docs.derivative.ca/TDFunctions),
and [`primer/editor-and-layout`](../primer/editor-and-layout.md) for layout hygiene.
These are the conventions agents building networks apply by default.

## Structure

- **One Base/Container COMP per subsystem** (input handling, generative data, texture
  build, render, UI, output). No sprawling single-level networks; if a network keeps
  growing, compartmentalize it.
- **Cross-component references use Select OPs** reading the producer's terminal null —
  not long wires between levels, not deep hard-coded paths into another component's
  internals.
- **Global OP Shortcuts** on major components keep references working when things move.
- Null and Select OPs are computationally free — use them generously for legibility.

## Naming

Style-guide convention: `{operator}_{descriptor}{#}`, lowercase with underscores —
`null_final`, `moviefilein_deck_a`, `null_ui_thumbnail1`. Some authors all-caps the
terminal null with the component's name (`TABLE` inside `base_TABLE`); either way, the
terminal null's name is a contract — renaming it breaks selects, so name it once,
deliberately. Keep module DAT names short (`mod.` access reads better).

## Data-flow hygiene

- Prefer parameter expressions and exports over scripts that push values (see
  [`operator-families`](./operator-families.md), "Moving values around").
- **Component boundary API (split by kind):**
  - **Operator inputs/outputs** → `In` / `Out` operators (prefer In over custom OP-path
    parameters for nodes the COMP consumes).
  - **Controls** → custom parameters on the root COMP (floats, menus, colors, pulses).
  - **Status / derived values** → custom pars marked **read-only** so users cannot edit
    what they should only observe.
  Outside code should not reach into internals; see
  [`component-checklist`](./component-checklist.md) for the full reusable-COMP checklist.
- Wires show flow at a glance; use them within a component. Selects/references replace
  them *between* components.
- Watch cook cost: middle-click info, Performance Monitor; turn off viewers you don't
  need; remember pull-based cooking means an unviewed, unreferenced node simply doesn't
  run. Design for 1 / 10 / 100 instances when the COMP will be cloned.

## Layout, docs, and cleanup

- **Wiring that works is not done.** Once all logic is verified, do one final
  grid pass: reposition the touched subtree so data flow reads left-to-right
  (or top-to-bottom) at a glance, with **zero overlapping nodes**. This is a
  one-shot cosmetic step — do not reorganize iteratively after every mutation.
  Skip during complex multi-step plans; the final pass before claiming done is
  required. Do not leave test spaghetti in shipped networks. Grid units and
  overlap avoidance are restated below; deepen with [`primer/editor-and-layout`](../primer/editor-and-layout.md)
  and Derivative `OP Class` / `TDFunctions` wiki pages.
- **Grid units, not pixels** — `nodeX`/`nodeY`/`nodeWidth`/`nodeHeight` are in
  network-editor units and depend on zoom for apparent size; **`nodeY` grows
  upward from the bottom**, so "stacking a node further down the canvas" means a
  *lower* `nodeY` value, not a higher one — a common sign-flip bug when
  scripting layout. There is no Python "Layout All" API; position nodes
  explicitly (`nodeX`/`nodeY`/`nodeCenterX`/`nodeCenterY`, or
  `TDFunctions.arrangeNode(...)`) — see Derivative docs / forum for API details.
- **Avoid overlapping operators** — before placing or moving a node, compute its
  box from `nodeX, nodeY, nodeWidth, nodeHeight` and check it against every
  sibling's box; on overlap, shift by at least the overlapping sibling's
  width/height plus a spacing margin. Use one fixed spacing constant per axis
  per container rather than ad hoc offsets. When creating multiple nodes via
  MCP (`mutate_nodes` create with `values` for `nodeX`/`nodeY`), set positions
  explicitly at creation time — do
  not rely on TD's default drop position, which stacks new nodes and guarantees
  overlap on the very next node. Preferences → Network grid-snap is a UI aid
  only; it does not prevent script-driven overlap.
- **Reorganize using one of these patterns** (pick per-subtree, mix as needed):
  - **Linear chain** — one row per subsystem stage (input → process → output);
    `nodeX` increments by a fixed step, `nodeY` constant along the spine.
  - **Family lanes** — mixed TOP/CHOP/DAT chains keep each family in its own
    horizontal band (constant `nodeY` per family) so wires don't cross vertically.
  - **Branch stagger** — one output feeding two consumers offsets the branches
    symmetrically so wires fan out visibly instead of overlapping.
  - **Utility offshoot** — debug/probe null·select·comment nodes sit on their
    own lane below/above the main spine, never inline.
  - **Backdrop + Annotate grouping** — wrap each subsystem in a labeled
    Backdrop; group boundaries must not overlap neighboring groups' backdrops
    (treat a Backdrop's box like any other node box for overlap-avoidance).
- **Document in-network** — Annotate COMPs for regions; Text COMPs/DATs for purpose,
  Ins/Outs, and key pars. Docs travel with the COMP.
- **Cleanup as you go** — remove unused/deprecated nodes you introduced; do not delete
  unrelated user work. Errors and warnings on finished work are not acceptable
  (probe `warnings()` on file/device nodes — [`python-api`](./python-api.md)).
- **Component preview** — when a COMP has a main visual, set Operator Viewer
  (`par.opviewer`) to that primary Out/null and turn the Viewer flag on — not an empty
  or unrelated node. See [`component-checklist`](./component-checklist.md).

## Relative references (required for reusable COMPs)

Verified while shipping a movable `/project1/chladni` COMP: absolute
`op('/project1/...')` expressions break as soon as the component is renamed, cloned,
or dropped into another project. **Default to relative refs inside any subsystem
container.**

| From | To reach | Prefer |
|------|----------|--------|
| Direct child of the COMP | COMP custom pars | `parent().par.Foo` |
| Direct child | Sibling under the COMP | `parent().op('audio_react/null_audio')` or path `audio_react/null_audio` |
| Grandchild (e.g. inside a nested `baseCOMP`) | Root COMP pars | `parent().parent().par.Foo` |
| Path field / `op()` on a node | Same-network **sibling** | bare name `null_out` (no `./`) |
| Path field / `op()` on a COMP | **Direct child** inside that COMP | `./out1`, `op('./null_out')` |
| Path field | Parent-network cousin | `../null_out`, `../audiofilein` — not `/project1/...` |

Rules of thumb:

- **Bare vs `./` vs `../`:** bare leaf = sibling / same network; `./` = first nested
  children (inside this COMP); `../` = climb out of this network. Do not mix them —
  `./null_out` is not a sibling, and bare `out1` on a COMP's `opviewer` is not its
  internal Out.
- Count hops carefully: from `comp/nested/node`, `op('..')` is `nested`, **not**
  `comp`. One wrong hop silently binds to the wrong OP (seen live).
- Prefer `parent()` / `parent().op('...')` over hand-rolled `../` strings in
  expressions — clearer and harder to mis-count.
- Custom pars on the root COMP are the **control** API; children bind *to those pars*
  with relative exprs, never to hard-coded numbers that only live in a script.
  Operator inputs still enter through **In** ops, not OP-path custom pars.
- Before calling a component reusable (or saving a `.tox`), audit: walk the subtree
  and fail if any `par.expr` / path `par.val` still contains `/project1` (absolute
  project paths). Keep `./` / bare / `../` matched to child / sibling / parent.
- Absolute paths remain OK in **agent-side** MCP scripts that address the live
  session (`op('/project1/...')`, probes). Do not copy that habit into
  the networks those scripts create.

## Related

- [`component-checklist`](./component-checklist.md) — reusable COMP boundary audit
- [`primer/editor-and-layout`](../primer/editor-and-layout.md) — layout hygiene depth
- [`operator-families`](./operator-families.md) — moving values around
- [`mutation-zones`](./mutation-zones.md) — where to build


---

**Canonical:** [`network-design`](./network-design.md) 
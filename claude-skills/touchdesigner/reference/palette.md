# Palette components

Find a TouchDesigner Palette component and place it, instead of rebuilding
something Derivative already shipped — `palette_index` finds it,
`mutate_nodes` `place` puts it in the network.

## What the Palette is

A folder tree of `.tox` components: Derivative's own under the TD install
(`Tools`, `Techniques`, `UI`, `Generators`, `ImageFilters`, `POPs`, …) plus
whatever the user keeps in their own palette folder. Each entry is a complete,
tested COMP — a particle system, an audio analyser, a widget set.

**Reach for one first.** A palette component beats a hand-built network on
every axis that matters: it is already debugged, it carries a designed custom-
parameter API, and it costs one mutate step instead of twenty.

## Ids

```text
{source}:{Category}/{Name}
builtin:Tools/particlesGpu
builtin:UI/Basic Widgets/buttons
user:MyRig/projector
```

`builtin` = shipped with the install; `user` = the user's own palette folder.
Ids are stable across machines, so they are safe to write into a plan or a
comment.

## Finding one

| Need | Call |
| --- | --- |
| What is in the palette at all | `palette_index` `{action:"scan"}` — builds the roster (offline, no `pid`) |
| Browse / search | `palette_index` `{action:"list", select:{category:"Tools"}}` |
| Text search | `select:{match:"*particle*"}` — `*` and `?` globs over the id |
| Read one component's card | `palette_index` `{action:"get", paletteId:"…"}` |
| Roster health | `palette_index` `{action:"stats"}` |

`list` returns one line per entry — id, category, one-line summary, card
status. It is cheap; `get` is the only call that returns a full card, so widen
with `list` and deepen with `get` on the two or three that look right.

**A `summary` is only there if someone described that component.** An
undescribed entry gives you its name and category and nothing else. Building
the descriptions is a separate, deliberate pass — [`palette-scan`](./palette-scan.md).

`palette_index` is offline: no `pid`, no bridge, exempt from the sequential-
bridged-call gate ([`tooling-concurrency`](./tooling-concurrency.md)).

## Placing one

A palette component lands through `mutate_nodes`, as a `place` step — so it
shares the batch, the ordering, and the rollback with everything else you are
building:

```json
{"op": "place", "path": "/project1/fx/parts",
 "paletteId": "builtin:Tools/particlesGpu",
 "comment": "stock GPU particles — Birthrate driven by the audio chain"}
```

| Field | Meaning |
| --- | --- |
| `path` | Where it lands; the leaf is the name you want |
| `paletteId` | Indexed component (preferred — it is checked before TD is touched) |
| `toxPath` | Absolute `.tox` outside the palette; use instead of `paletteId`, never both |
| `comment` / `values` / `flags` | Applied after load, exactly as on `create` |

The placed COMP is referenceable by later steps **in the same batch**, so place
and wire in one call:

```json
[{"op": "place", "path": "/project1/parts", "paletteId": "builtin:Tools/particlesGpu"},
 {"op": "connect", "src": "/project1/parts", "dst": "/project1/out1"}]
```

TD may rename the leaf on load (collision, illegal character). The step returns
the real path and a `tdmcp.op.renamed` lint — use the returned path, never the
one you asked for.

**You get the component, not its wrapper.** A stock palette `.tox` is a shell
holding an `icon`, a `help` DAT, and the real component inside. `place` lifts
the component out and throws the shell away, so what lands is the thing with the
parameters — usually a `containerCOMP` or `baseCOMP` with a full custom-par API.
Your own `.tox`, saved straight from a COMP, has no shell and is placed as-is.

## After placing — always

1. `inspect` the placed COMP with `include: ["params"]`. Its **custom parameters
   are its API**; drive those, never its internals ([`custom-parameters`](./custom-parameters.md)).
2. Wire through its In/Out operators, not by reaching inside
   ([`component-checklist`](./component-checklist.md)).
3. Write the `comment` in the same batch — why *this* component
   ([`node-comments`](./node-comments.md)).

In an OpSketch, a placed component gets the `[custom]` annotation, and its
custom parameters are the ones that pass the importance gate:

```text
parts baseCOMP {Birthrate:2000, Life:3.0} [custom]  # stock builtin:Tools/particlesGpu
```

## When *not* to use one

- The component is far larger than the job (a full widget framework for one button).
- You need to modify its internals — clone-and-own is a different decision, and
  a modified palette component no longer matches its card.
- It is on the probe blacklist because it opens sockets or wants hardware you do
  not have ([`palette-scan`](./palette-scan.md)).

## Failure codes

| Code | Means |
| --- | --- |
| `tdmcp.palette.not_indexed` | Nothing scanned yet — run `action:"scan"` |
| `tdmcp.palette.unknown_id` | No such id; `list` to find the real one |
| `tdmcp.palette.tox_missing` | Indexed file is gone — re-scan |
| `tdmcp.palette.load_failed` | TD refused the `.tox` (build mismatch, corrupt file) |

A `place` step carrying both `paletteId` and `toxPath`, or neither, is rejected
before the batch starts — pick one.

The first three fail on the daemon before TD is touched, so a bad id costs you
nothing.

## Definition of Done

- [ ] Component chosen from `list` evidence, not from memory of a name
- [ ] Placed with a `comment` naming the `paletteId` it came from
- [ ] Its custom parameters read with `inspect` before any are set
- [ ] Wired through In/Out operators, nothing reaching into its internals
- [ ] Final `inspect` on the parent network confirms the COMP and its wires

## Related

- Building the descriptions, and the probe blacklist: [`palette-scan`](./palette-scan.md)
- Driving a component through its custom pars: [`custom-parameters`](./custom-parameters.md)
- In/Out boundaries and reuse hygiene: [`component-checklist`](./component-checklist.md)
- What `.tox` files are: [`primer/tox-toe-components`](../primer/tox-toe-components.md)
- Sketching the placed component: [`opsketch-notation`](./opsketch-notation.md)

---

**Canonical:** [`palette`](./palette.md)
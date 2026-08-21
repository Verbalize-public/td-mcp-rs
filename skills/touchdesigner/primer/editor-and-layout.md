# Primer: editor and network layout

**Canonical:** `tdmcp://docs/primer/editor-and-layout` · disk: `primer/editor-and-layout.md`

## Shell (operate-relevant)

- **Network Editor** — canvas for OPs, wires, flags, viewers.
- **Parameter Dialog** — pages, four parameter modes (constant / expression /
  export / bind).
- **Panes** — split/dock; `editor_context` reports what the user is looking at
  (`ownerPath`, selection) as a **hint only** — confirm with `inspect`.
- **Palette / OP Create** — discover stock types; prefer exact `opType` strings
  from `api_help` / `inspect`, never invent shorthand.
- **Timeline / Perform** — transport and perform mode; see
  `tdmcp://docs/play-state`.

## Layout hygiene (before claiming done)

Agents must not leave overlapping spaghetti. After a mutation pass:

1. Place nodes with non-overlapping `nodeX` / `nodeY` (and sensible width/height).
2. Keep left-to-right / top-to-bottom data flow readable.
3. Reorganize as a last step; verify with `inspect` (and visual check if needed).

Operate conventions: `tdmcp://docs/network-design`.

## Related

- `tdmcp://docs/mutation-zones`
- `tdmcp://docs/network-design`
- Wiki: https://docs.derivative.ca/Network_Editor

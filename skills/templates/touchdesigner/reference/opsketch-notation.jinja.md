# OpSketch grammar

OpSketch is the **primary language** for thinking about, discussing, and returning
TouchDesigner networks. SoT: {{ skill("opsketch-notation") }} (+ gating / examples).

**Evidence split:** node roster / opTypes / params / errors / **positional wires**
come from default `inspect`. Positional `<- a, b` uses each consumer's
`inputs[].path` leaf (or known `mutate_nodes` connect results). Empty `inputs: []`
= unwired. See {{ skill("python-api") }} → Wiring.

## Header

```text
scope: <path>  [(<family>:<opType>[, pars: <name>,...])]  nodes=<N> wires=<E> [errors=<N>]
```

- `<path>` — the sketched root, relative (e.g. `/project1/fx1`).
- `(<family>:<opType>[, pars: ...])` — only when the scope root is itself a COMP; `pars:`
  lists its custom parameter names, omitted entirely if it has none.
- `nodes` — count from `inspect` on this subtree.
- `wires` — count of positional edges from inspect `inputs` across sketched
  consumers (or connect steps you just applied); omit rather than invent.
- `errors` — only include when known from `inspect` errors on this subtree this turn;
  omit the field rather than guess or state 0 without having checked.

## Node line

```text
<leaf> <opType> [<- <input>[, <input>...]] [{<par>:<value>[, ...]}] [[<annotation>[, ...]]]
```

- `<leaf>` — sibling-unique name, no path prefix. If a referenced node lives outside `scope`
  (rare), use the shortest unambiguous relative path instead of breaking the grammar.
- `<opType>` — the exact TD class string (`noiseTOP`, `nullCHOP`, `baseCOMP`) — the same
  string `inspect` already returns. Never invent a shorthand family tag in place of it.
- `<- inputs>` — **incident** input list, comma-separated, left-to-right = input index
  0, 1, 2… Omit entirely for 0-input ops (Movie In, Constant, Audio Device In) — never write
  a bare `<- `.
- `{...}` — only parameters that pass the importance gate
  ({{ skill("opsketch-importance-gating") }}). Omit the whole block
  when nothing passes — never write an empty `{}`.
- `[...]` — free-text annotations for facts that aren't a parameter value: `[callback]` (DAT
  has authored script), `[ext]` (COMP has a promoted Python Extension), `[err]` (node has a
  live error — cross-check `inspect` errors before writing this), `[custom]` (custom
  OP/palette entry, not a stock TD type).

## Value prefixes (parameter values only)

| Prefix | Meaning | Example |
|--------|---------|---------|
| *(none)* | Constant | `{size:8.0}` |
| `=` | Expression | `{tx:=absTime.seconds*2}` |
| `~` | Bind / export | `{x:~op('ctrl').par.x}` |

## Non-adjacent references (not positional inputs)

Wire / reference kinds map to OpSketch as follows (positional `<-` from inspect
`inputs` — never invent when keys are omitted):

| kind | OpSketch form | Evidence |
|------|----------------|----------|
| `op` (operator input) | positional `<- a, b` on the consuming node's line | inspect `inputs[].path` leaves |
| `comp` (COMP hierarchy input) | positional `<- a, b` on the consuming COMP's line | COMP inspect `inputs` |
| `select` (Select/ref parameter) | inline `{select:<path>}` on the node owning the parameter | `inspect` params |
| `export` / `bind` (parm mode) | inline `{<par>:~<source>}` on the owning node — both are live,
  non-Python links, so both use the `~` prefix; reserve `=` for actual Python expression mode | `inspect` params / mode |

## Nesting

For a COMP with sketched children, indent child node lines 2 spaces under the parent's own
node line (which may itself carry `{}`/`[]` like any other node). Print exactly one `scope:`
header per sketch call — do not repeat it per nesting level.

```text
scope: /project1  nodes=4 wires=2

fx1 baseCOMP {Speed:1.5, Seed:7} [ext]
  noise1  noiseTOP        {seed:=parent().par.Seed}
  xform1  transformTOP  <- noise1
out1 outTOP <- fx1/xform1
```

## Depth / radius

Default to depth 1–2 and widen only around the node actually in question — same stop/token
budget discipline as {{ skill("definition-of-done") }}.

## Truncation

When a sketch would exceed ~40 node lines, sketch the COMP-level outline first (one line per
child COMP, no descending into any of them), then re-sketch only the one hub that matters at
higher depth.

## When to sketch

| Situation | Sketch? |
|-----------|---------|
| >3 nodes, or any non-trivial network (COMP hub, multi-family chain, branched data-flow) | **Yes — mandatory** |
| Before/after any non-trivial wiring change | Yes |
| Reporting a network to the director / user | Yes — return shape |
| 1–3 trivial nodes ("what feeds `out1`?") | No — one line |

## Related

| File | Topic |
|------|-------|
| {{ skill("opsketch-importance-gating") }} | Gating rule + trivial types |
| {{ skill("opsketch-examples") }} | Worked transcriptions |

## Definition of Done

- [ ] Non-trivial network (>3 nodes, COMP hub, multi-family, branched data-flow) uses OpSketch
- [ ] Every shown param passes the importance gate
- [ ] Wires shown incident-per-node
- [ ] Family/opType/wire-kind matches live `inspect` / family tables — no invented synonyms
- [ ] Network-map style returns use this notation


---

**Canonical:** {{ skill("opsketch-notation") }} 

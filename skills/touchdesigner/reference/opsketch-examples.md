# OpSketch examples

Worked before → after pairs. Raw surfaces are fabricated but schema-faithful; OpSketch follows [opsketch-notation.md](opsketch-notation.md) + [opsketch-importance-gating.md](opsketch-importance-gating.md).

## Example A — live `inspect`

Raw (`inspect`, abbreviated):

```json
[
  {"opType": "moviefileinTOP", "name": "moviein1", "path": "/project1/fx/moviein1",
   "properties": {"file": "media/loop.mov", "play": 1, "repeat": "cycle"}},
  {"opType": "levelTOP", "name": "level1", "path": "/project1/fx/level1",
   "properties": {"opacity": 1.0, "brightness": 1.0}},
  {"opType": "blurTOP", "name": "blur1", "path": "/project1/fx/blur1",
   "properties": {"size": 12.0}},
  {"opType": "transformTOP", "name": "xform1", "path": "/project1/fx/xform1",
   "properties": {"tx": {"mode": "expression", "expr": "parent().par.Speed"}, "ty": 0, "sx": 1.0}},
  {"opType": "outTOP", "name": "out1", "path": "/project1/fx/out1",
   "properties": {}}
]
```

OpSketch:

```text
scope: /project1/fx  nodes=5 wires=4

moviein1 moviefileinTOP {file:media/loop.mov, repeat:cycle}
level1   levelTOP       <- moviein1
blur1    blurTOP        <- level1  {size:12.0}
xform1   transformTOP   <- blur1   {tx:=parent().par.Speed}
out1     outTOP         <- xform1
```

## Example B — wire summary from `inspect`

Raw wire summary:

```text
/project1/hub/noise1 -> /project1/hub/blur1  # in0
/project1/hub/blur1 -> /project1/hub/out1  # in0
/project1/hub/select1 -> /project1/hub/out1  # in1
/project1/media/moviein1 -> /project1/hub/select1  # select top
/project1/hub/lfo1 -> /project1/hub/blur1  # export size prefix=0
/project1/hub/lfo1 -> /project1/hub/xform1  # bind tx prefix=0
```

**Dont naively copy absolute reference/path.** Use TD relatives: bare sibling name
(`null_out`), `./child` for first nested children inside a COMP, `../cousin` when
climbing, `parent([n])` for COMP pars — keeps networks movable/maintainable.

Note every node referenced inside `scope` shows up on its own line (`lfo1`, `select1`) even
though their only role is feeding a non-adjacent param — a node named in a wire but missing
from the sketch is a bug, not a valid omission:

```text
scope: /project1/hub  (COMP:baseCOMP)  nodes=6 wires=6

noise1   noiseTOP
blur1    blurTOP        <- noise1              {size:~op('lfo1')['chan1']}
select1  selectTOP      {select:/project1/media/moviein1}
lfo1     lfoCHOP
xform1   transformTOP   {tx:~op('lfo1')['chan1']}
out1     outTOP         <- blur1, select1
```

`size` and `tx` both use `~` (export/bind are live non-Python links), not `=` — `=` is reserved
for actual Python expression mode.

## Example C — custom COMP + extension

A COMP is a node too when it isn't the scope root itself — it gets its own `{}`/`[]` line,
same as any operator:

```text
scope: /project1  nodes=2 wires=0

fx1 baseCOMP {Speed:1.5, Seed:7} [ext]
  noise1 noiseTOP {seed:=parent().par.Seed}
```

## Reading checklist

- No empty `{}`; omit the block when nothing passes the gate.
- Every `{}` entry would survive re-derivation from a param diff against defaults (or is expr/bind/export/custom/script).
- Wires are incident on the consumer (`<- …`), not a trailing edge dump.
- Annotations (`[ext]`, `[callback]`, `[err]`, `[custom]`) only for facts a `{}` cannot express.

## Example D — `api_help` class card + filtered index

Class card (`api_help`, abbreviated):

```json
{
  "pid": 12345,
  "queries": [
    { "kind": "class", "name": "noiseTOP" },
    { "kind": "classes", "family": "TOP", "prefix": "noise" }
  ],
  "detailLevel": "summary"
}
```

```json
{
  "ok": true,
  "results": [
    {
      "ok": true,
      "kind": "class",
      "name": "noiseTOP",
      "doc": "This class inherits from the TOP class.\nIt references a specific Noise TOP.",
      "opType": "noiseTOP",
      "family": "TOP",
      "members": ["cook", "par", "pars", "path"],
      "memberCount": 163
    },
    {
      "ok": true,
      "kind": "classes",
      "names": ["noiseTOP"],
      "count": 1,
      "family": "TOP",
      "prefix": "noise"
    }
  ]
}
```

Routing: use this for opType / Python member discovery. For `.par` names on a live node, call `inspect` with `include: ["params"]` — not `api_help`.

### Diagnostics follow-up

On `tdmcp.op.unknown_type`, diagnostics may include `{ "kind": "api_help", "query": "hsvAdjustTOP" }` (bad name) and/or a similar_type suggestion — retry `api_help` with the suggested exact class name, or `classes` + `prefix`. On `tdmcp.par.unknown`, the `api_help` query is the node's **opType** (class card only); still re-`inspect` with params for the parameter list.


---

**Canonical:** `tdmcp://docs/opsketch-examples` · disk: `reference/opsketch-examples.md`

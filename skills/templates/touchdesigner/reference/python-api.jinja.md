# TD Python API — compact reference

All symbols auto-available in TD scripts / textport / expressions — no imports needed.
Python 3.11.

For **exact** live class/member names on a connected TD pid, prefer `api_help`
over guessing from this cheatsheet.

Live-verified against td-mcp-rs `execute_python` (aliases + OP/Par/wiring surface).
Wrong doc is worse than no doc — demote or drop anything not confirmed.

## Scope: TD scripts vs `execute_python`

| Context | What's in scope |
|---------|-----------------|
| Textport, DAT scripts, expressions, extensions | Full auto-globals: `me`, `op`, `root`, `parent`, `ui`, `project`, `absTime`, `tdu`, `run`, … **and** bare type objects (`noiseTOP`, `waveCHOP`, …) |
| td-mcp-rs `execute_python` | **Core:** `td`, `op` (`td.op`), `result` (assign to return), `tdmcp_resolve`, `__tdmcp_context_path__`. **Aliases** (bound if present on `td`): `root`, `ui`, `project`, `absTime`, `tdu`, `run`, `ops`, `opex`, `passive`, `mod`. **Not** injected: `me`, `parent`, bare opTypes, `debug` |

```python
# Textport / DAT — bare types + me/parent OK
n = parent().create(noiseTOP, 'noise1')

# execute_python — aliases OK; opTypes via td or string
n = op('/project1/fx').create(td.noiseTOP, 'noise1')
n = op('/project1/fx').create('noiseTOP', 'noise1')
assert root.path == '/'
ui = ui  # bare alias; td.ui always safe
```

`create(opType, name, initialize=True)` — `opType` is a type object (`td.noiseTOP` /
`noiseTOP` in Textport) **or** the string `'noiseTOP'`. From an existing node:
`type(n)` / `n.OPType`.

## Global shortcuts

| Symbol | Purpose | `execute_python` |
|--------|---------|------------------|
| `me` | Current OP executing this code | **No** (no script-owner) |
| `parent(n?)` | nth parent (1=direct); `parent.shortcut` for named | **No** bare (use `n.parent()` on a node) |
| `root` | Top-level `/` component | Yes (alias) |
| `op('path')` | First matching OP from `/`; returns `None` on miss | Yes |
| `op(*pats)` | Multiple patterns, space/comma-separated | Yes |
| `opex('path')` | Like `op()` but raises on miss (prefer in par expressions) | Yes (alias; miss → `tdError`) |
| `ops(*pats)` | All matching OPs as list | Yes (alias) |
| `passive(op)` | Non-cooking OP wrapper for read-only queries | Yes (alias) |
| `run(code, delayFrames=0, delayMilliSeconds=0)` | Delayed execution → `Run` | Yes (alias) |
| `debug(*args)` | Print + source location to textport | **No** (use `print`; MCP tees stdio → Debug DAT / `logs`) |
| `absTime.frame`, `absTime.seconds` | Process-lifetime time | Yes (alias) |
| `project.cookRate`, `project.name` | Session FPS / name | Yes (alias) |
| `ui.panes`, `ui.undo`, … | Editor UI (prefer `editor_context` for panes) | Yes (alias) |
| `tdu.rand(seed)`, `tdu.split(s)`, `tdu.Color/Vec/Mat` | Utilities | Yes (alias) |
| `mod`, `iop`, `ipar` | Module-on-demand, internal shortcuts | `mod` yes; `iop`/`ipar` are COMP-local (not sandbox globals) |

`td.*` always works for anything on the TD module (including opTypes).

**Relative `op()` / path strings:** sibling / same-network → bare name
(`op('null_out')`); direct child inside this COMP → `op('./null_out')`; parent hop
→ `op('../null_out')`. See {{ skill("network-design") }} relative references.

Auto-imported stdlib (also bare in `execute_python`): `math`, `re`, `sys`, `collections`, `enum`, `inspect`

## Type objects — starter catalog

These are **classes on `td`** (and bare names in full TD scope). Use them with
`COMP.create(...)` / `changeType(...)`. Names are case-sensitive; discover more via
`api_help` `{kind:"classes", family:"TOP", prefix:"noise"}`.

### Family bases (filters / isinstance)

`OP` · `TOP` · `CHOP` · `DAT` · `COMP` · `SOP` · `POP` · `MAT` · `Par` · `ParGroup` · `Channel` · `Cell`

```python
n.findChildren(type=td.TOP, name='*', depth=1)
isinstance(n, td.CHOP)          # True for any CHOP subclass
n.asType(td.noiseTOP)           # IDE / type hint helper
```

### Common opTypes by family

| Family | Starter types (pass to `create` / `changeType`) |
|--------|--------------------------------------------------|
| **TOP** | `noiseTOP`, `nullTOP`, `constantTOP`, `rampTOP`, `levelTOP`, `blurTOP`, `transformTOP`, `compositeTOP`, `selectTOP`, `moviefileinTOP`, `feedbackTOP`, `cacheTOP`, `lookupTOP`, `displaceTOP`, `thresholdTOP`, `edgeTOP`, `resolutionTOP`, `glslTOP`, `glslmultiTOP`, `hsvadjustTOP`, `inTOP`, `outTOP` |
| **CHOP** | `waveCHOP`, `lfoCHOP`, `noiseCHOP`, `nullCHOP`, `constantCHOP`, `mathCHOP`, `mergeCHOP`, `selectCHOP`, `infoCHOP`, `audiofileinCHOP`, `inCHOP`, `outCHOP` |
| **DAT** | `textDAT`, `tableDAT`, `nullDAT`, `selectDAT`, `scriptDAT`, `executeDAT`, `inDAT`, `outDAT` |
| **COMP** | `baseCOMP`, `containerCOMP`, `geometryCOMP`, `cameraCOMP`, `lightCOMP` |
| **POP** | `spherePOP`, `noisePOP`, `transformPOP`, `nullPOP`, `glslPOP`, `inPOP`, `outPOP` |
| **SOP** | `boxSOP`, `nullSOP`, `inSOP`, `outSOP` |
| **MAT** | `phongMAT`, `pbrMAT`, `glslMAT`, `inMAT`, `outMAT` |

```python
# Pattern — create + wire + set (sandbox-safe)
fx = op('/project1').create(td.baseCOMP, 'fx')
noise = fx.create(td.noiseTOP, 'noise1')
blur  = fx.create(td.blurTOP, 'blur1')
out   = fx.create(td.outTOP, 'out1')
blur.setInputs([noise])
out.setInputs([blur])
blur.par.size = 12
size = blur.par.size.eval()   # always prefer .eval() when reading
```

Exact spelling traps (live-verified): `hsvadjustTOP` (not `hsvAdjustTOP`),
`geometryCOMP` (not `geoCOMP`). On `tdmcp.op.unknown_type`, retry `api_help` with the
suggested name or `{kind:"classes", prefix:"…"}`.

## Wiring (inputs / Connectors)

`inspect` surfaces positional wires on each node when `nodes` is included
(`inputs` / `outputs` peer lists — `{path, name, opType}` or `null` per slot).
Prefer that for network understanding. Use Python `n.inputs` / `n.outputs` (or
`mutate_nodes` connect/disconnect) only as a last resort or for one-off writes:

```python
# Reliable — OP lists (elements are OPs with .path)
blur.inputs          # -> [noiseTOP, ...]
noise.outputs        # -> [blurTOP, ...]
blur.setInputs([noise])
blur.setInputs([None])   # disconnect slot 0

# Connector API — NO .op attribute (AttributeError)
ic = blur.inputConnectors[0]
ic.owner.path              # this OP
# ic.inOP / ic.outOP are often None — do not rely on them for "who is wired"
# Peer via connections:
peer = ic.connections[0].owner if ic.connections else None
```

## OP — every node inherits this

```python
n = op('node')

# Identity
n.name, n.path, n.id, n.base, n.digits, n.valid

# Type
n.type        # 'noise'
n.family      # 'TOP'
n.opType      # 'noiseTOP' (for create())
n.OPType      # same string; also type(n) → class object
n.isCHOP, n.isCOMP, n.isDAT, n.isTOP, n.isSOP, n.isMAT, n.isPOP
n.isFilter, n.minInputs, n.maxInputs

# Hierarchy — parent is a ParentShortcut; CALL it
n.parent()            # parent(1) OP
n.parent(1).path
# n.parent.path  → tdAttributeError (ParentShortcut has no .path)
n.children            # list[OP]
n.numChildren
n.inputs, n.outputs   # wired neighbor OPs (not Connectors)
n.dock, n.docked

# Parameters
n.par.tx           # dot access
n.par['tx']        # subscript access
n.parGroup.t       # vector group (x,y,z)
n.customPars, n.customPages, n.pages

# Flags (bool, R/W)
n.bypass, n.display, n.render, n.lock, n.viewer
n.activeViewer, n.allowCooking, n.current, n.selected
n.cloneImmune, n.python

# Layout
n.nodeX, n.nodeY, n.nodeWidth, n.nodeHeight
n.nodeCenterX, n.nodeCenterY
n.color           # (r, g, b) tuple
n.comment

# Cook
n.cpuCookTime, n.gpuCookTime, n.totalCooks
n.cookedThisFrame
n.cpuMemory, n.gpuMemory

# Key methods
n.create(td.waveCHOP, 'name')     # COMP-only: create child
n.copy(src_op, name='new')        # COMP-only: copy into this
n.destroy()
n.cook(force=False, recurse=False)
n = n.changeType(td.nullCHOP)     # Returns NEW OP — reassign!
n.setInputs([op1, op2, None])     # None disconnects slot
n.copyParameters(other)
n.findChildren(type=td.CHOP, name='*', depth=1, tags=[], parName='clone')
n.evalExpression('me.digits')     # Eval code in this OP's context
n.addError('msg') / n.addWarning('msg')  # Only during cook
n.errors(recurse=False) / n.clearScriptErrors(recurse=True)
n.openViewer() / n.closeViewer()
n.asType(td.baseCOMP)             # IDE type hint

# Storage
n.store('key', val)
n.fetch('key', default=0)
n.unstore('key*')
n.fetchOwner('key')
n.tags = {'tag1', 'tag2'}
```

## Par — parameter access

```python
p = n.par.tx        # or n.par['tx']

# Value — always prefer .eval()
p.eval()            # Current value (all modes) ✅
p.val               # CONSTANT mode only — avoid unless you know mode
p.expr              # Expression string
p.mode              # ParMode.CONSTANT / EXPRESSION / EXPORT / BIND
p.default, p.name, p.label, p.owner, p.index

# Type checks
p.isMenu, p.isNumber, p.isFloat, p.isInt, p.isString
p.isToggle, p.isPulse, p.isMomentary, p.isOP, p.isCustom

# Menu params
p.menuNames, p.menuLabels, p.menuIndex

# Range
p.min, p.max, p.clampMin, p.clampMax
p.normMin, p.normMax, p.normVal

# Binding
p.bindExpr, p.bindMaster, p.bindReferences
p.enable, p.enableExpr, p.readOnly

# Methods
p.pulse(value=1, frames=0, seconds=0)
p.reset()
p.destroy()         # Custom-only; invalidates all Par refs
p.isSamePar(other)  # Identity — prefer over relying on == / is
p.evalOPs()         # Multi-OP parameter → list[OP]
p.evalFile()        # File path → tdu.FileInfo
```

## TOP

```python
t = op('noise1')          # any TOP
t.width, t.height
t.aspectWidth, t.aspectHeight, t.aspect, t.depth
t.pixelFormat, t.pixelFormatName
t.sample(x, y, u='pixel') # sample pixel / UV
t.numpyArray()            # image as numpy
t.save('file.png')
t.cudaMemory              # CUDA interop when available
```

## CHOP

```python
c = n['chan1']          # Exact name
c = n[3]                # Index
c = n.chan('chan*')     # Pattern → first or None
n.chans('a*', 'b*')     # List of channels
n.numChans, n.numSamples
n.numpyArray()          # Shape (numChans, numSamples)
c.eval(), c.eval(index)
c.name, c.index
n.save('file.clip')     # .clip/.chan/.aiff
n.export, n.rate
```

## DAT — table & text

Prefer `inspect` with `include: ["content"]` to read DAT bodies (text + table
TSV via `.text`) and GLSL shader stages. Use the Python surface below when
mutating or when `execute_python` is otherwise required.

```python
# Content
n.text                  # Tab-delimited (R/W)
n.csv                   # CSV (multi-line cells)
n.jsonObject            # Parse as dict
n.numRows, n.numCols
n.isText, n.isTable     # Discriminate text vs table presentation

# Cells
n[2, 3]                 # Index
n['row', 'col']         # Label (e.g. n['x','val'].val)
n.cell(2, 'col')        # Pattern matching
n.row('name', val=True) # → list of cells / values
n.col('name', val=True)
n.rows('A*')            # Pattern
n.findCell('pattern', cols=['name'])

# Mutate
n.appendRow([v1, v2])
n.appendCol([v1, v2])
n.insertRow([vals], nameOrIndex=0)
n.replaceRow('name', [vals])
n.deleteRow('name'), n.deleteCol('name')
n.setSize(rows, cols)
n.clear(keepSize=False)
n.copy(other_dat)

# Execute
n.run(*args, delayFrames=0)
n.module                # Import as Python module
print('...', file=n)    # Redirect stdout
n.write('text')         # Append
```

## POP / SOP (geometry)

```python
# POP — modern GPU geometry
p.numPoints, p.numPrims, p.numVerts
p.bounds, p.computeBounds()
p.pointAttributes, p.primAttributes, p.vertAttributes
p.save('file.*')

# SOP — legacy CPU geometry
s.numPoints, s.numPrims
s.center, s.bounds, s.computeBounds()
```

## COMP

```python
n.create(td.waveCHOP, 'name')
n.copy(src_op, name='new')
n.copyOPs([op1, op2])        # Preserves wires
n.loadTox('file.tox')
n.save('file.tox')
n.findChildren(type=td.CHOP, name='*', depth=1, tags=[], parName='clone')

# Variables
n.setVar('MEDIA', 'C:/path')
n.vars('A*')

# Layout
n.layout(horizontal=True)
n.layout(gridRows=10)

# Extensions / VFS
n.extensions
n.vfs
```

## Custom parameters (COMP-only)

Full when-to-use + styles + gotchas: {{ skill("custom-parameters") }}.
Only COMPs have `appendCustomPage` (TOP/CHOP/DAT cannot). Names: uppercase start +
lowercase/digits only (`Amount` OK; `speed` / `my_amount` / `myAmount` fail).

```python
comp = op('/project1/fx')
page = comp.appendCustomPage('Controls')

# append* -> ParGroup; defaults/min/max are NOT create kwargs
page.appendFloat('Gain', label='Gain')
comp.par.Gain.default, comp.par.Gain.min, comp.par.Gain.max = 1.0, 0.0, 2.0
comp.par.Gain.clampMin = comp.par.Gain.clampMax = True
comp.par.Gain.val = 1.0

page.appendToggle('Enable', label='Enable')
comp.par.Enable = True                    # fresh toggles default False
comp.par.Gain.enableExpr = 'me.par.Enable'  # bare 'Enable' does NOT work

page.appendMenu('Mode', label='Mode')
comp.par.Mode.menuNames = ['palette', 'custom']   # set on Par, not ParGroup
comp.par.Mode.menuLabels = ['Palette', 'Custom']
comp.par.Mode = 'custom'

page.appendRGB('Color', label='Color')
comp.parGroup.Color = (1.0, 0.4, 0.1)      # -> Colorr/Colorg/Colorb
page.appendWH('Resolution', label='Resolution')  # -> Resolutionw/h
page.appendPulse('Reset', label='Reset')
comp.par.Reset.pulse()

# Read
comp.par.Gain.eval()                      # scalar
list(comp.parGroup.Color.eval())          # list (also for size-1 groups)

# Child references
child = comp.op('noise1')
child.par.resolutionw.expr = 'parent().par.Resolutionw'  # .expr => EXPRESSION
Mode = type(child.par.amp.mode)           # ParMode NOT on td in execute_python
child.par.amp.bindExpr = 'parent().par.Gain'
child.par.amp.mode = Mode.BIND

# Pages / teardown
comp.customPages                          # custom only (not comp.pages)
comp.sortCustomPages('Controls', 'About')
comp.par.Gain.destroy()                   # one par
# page.destroy() / comp.destroyCustomPars()  # page / all custom+pages

# replace=True (default) on re-append WIPES prior default/min/max/help
```

## Pattern matching

`*` (any), `?` (one char), `[abc]` (one of), `[!abc]` (not). Multiple patterns space/comma-separated.

## Gotchas

1. **`.eval()` always** — `par.tx.eval()`, not `par.tx.val` unless you know it's constant
2. **`.changeType()` returns new OP** — always reassign: `n = n.changeType(td.nullCHOP)`
3. **`Par` identity** — prefer `.isSamePar()`; do not use `is` (`p1 is p2` is False for the same par)
4. **Threads can't touch TD objects** — use `run()` with delay
5. **OP `id` is session-only** — not persistent across save/load
6. **`print` in `execute_python`** — teed to Debug DAT / returned `logs`; bare `debug()` is not injected
7. **`opex()` in expressions** — raises clear error instead of `NoneType has no attribute 'par'`
8. **`passive(op)` for read-only** — skips cook, safe for info queries
9. **`execute_python` sandbox** — core + closed aliases (see Scope). Bare `noiseTOP` / `me` / `parent` / `debug` → `NameError`. Use `td.noiseTOP` or string form; use `n.parent()` on a node
10. **`n.parent` is a ParentShortcut** — call `n.parent()` / `n.parent(1)`; `n.parent.path` fails
11. **Wiring** — use `n.inputs` / `n.outputs` (OPs). Connector has **no** `.op`; `inOP`/`outOP` often `None`; peer via `connector.connections[i].owner`
12. **opType spelling** — camelCase guesses often fail (`hsvadjustTOP`, `geometryCOMP`); confirm with `api_help`

## Deferred / deepen

| Topic | Where |
|-------|----------------|
| Full opType index / member cards | Live `api_help` (`class` / `classes`+filters) |
| Extensions, Execute DAT callbacks | {{ skill("primer/scripting-surfaces") }} |
| Parameter mode decisions | {{ skill("primer/parameters-and-channels") }} |
| OpSketch create/wire recipes | {{ skill("opsketch-examples") }} |


---

**Canonical:** {{ skill("python-api") }} 

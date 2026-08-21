# Custom parameters — live-verified

Companion to [component-checklist.md](component-checklist.md). Snippets are sandbox-safe for
td-mcp-rs `execute_python` unless noted.

**Verified** against TD `2025.32460` via `execute_python` / `inspect` /
`api_help` (Page / Par / ParGroup / COMP). Wrong doc is worse than no doc —
re-probe before changing these facts.

## When to use them

Custom parameters are the **public control API of a COMP** — not a dumping ground
for every internal knob.

| Use custom pars for | Do **not** use custom pars for |
|---------------------|--------------------------------|
| Floats / toggles / menus / colors / pulses the **caller** should tune | Main data inputs (textures, CHOPs, tables) → use **In** ops |
| Compact mode switches (`Palette` / `Custom`) | Reaching into another COMP's internals via OP-path |
| Reset / bake / reseed **pulses** for stateful nets | Per-frame script writes that should be expressions/exports |
| Shared resolution (`Resolution` WH) referenced by children | Agent-only debug knobs left on the shipped API |
| Read-only **About** metadata (name / version / dates) | Hiding bad wiring behind a "Source TOP" path parameter |

Preference order for moving values into a reusable COMP (same as skill cook
plumbing): **parameter expression** (`parent().par.Foo`) > **bind** >
**export** > script. Custom pars are the *source*; children *reference* them.

Only **COMPs** have `appendCustomPage` — TOP/CHOP/DAT/etc. cannot host custom
pages (live-checked).

## Naming rules (hard)

Parameter **names** must:

1. Start with an **uppercase** letter
2. Continue with **lowercase letters and digits only**
3. Not use spaces, underscores, or internal capitals (`myAmount` fails)

| Attempt | Result |
|---------|--------|
| `Amount`, `Mode`, `Resolution`, `Compname` | OK |
| `speed`, `my_amount`, `My Speed`, `myAmount`, `1Bad` | `tdError` |

UI **labels** are free-form via `label=` (or `.label` later). Prefer short
Pascal/lowercase-tail names; put human words in the label.

Multi-value styles append fixed suffixes (not always `1/2/3`):

| Create | Par names |
|--------|-----------|
| `appendFloat('Gain')` | `Gain` |
| `appendFloat('Vec', size=3)` | `Vec1`, `Vec2`, `Vec3` |
| `appendXY('Offset')` | `Offsetx`, `Offsety` |
| `appendXYZ('Pos')` | `Posx`, `Posy`, `Posz` |
| `appendRGB('Color')` | `Colorr`, `Colorg`, `Colorb` |
| `appendRGBA('Tint')` | `Tintr`…`Tinta` |
| `appendWH('Resolution')` | `Resolutionw`, `Resolutionh` |
| `appendUV('Uv')` | `Uvu`, `Uvv` |

Prefer `appendWH('Resolution')` over `Resolutionxy` (that yields the awkward
`Resolutionxyw` / `Resolutionxyh`).

## Create / read / update

Every `page.append*` returns a **`ParGroup`** (pulse → `ParGroupPulse`). Set
defaults / min / max **after** create on the `Par` — they are **not** create
kwargs.

```python
comp = op('/project1/fx')  # COMP
page = comp.appendCustomPage('Controls')

pg = page.appendFloat('Gain', label='Gain')  # -> ParGroup
p = comp.par.Gain                            # -> Par
p.default, p.min, p.max = 1.0, 0.0, 2.0
p.clampMin = p.clampMax = True
p.val = 1.0
p.help = 'Overall gain'

# Read — always .eval()
gain = comp.par.Gain.eval()
gain = comp.par['Gain'].eval()

# Write constant
comp.par.Gain = 0.75

# Vectors / colors — ParGroup assign or per-suffix
page.appendRGB('Color', label='Color')
comp.parGroup.Color = (1.0, 0.4, 0.1)
comp.par.Colorr = 1.0
rgb = list(comp.parGroup.Color.eval())   # list of floats

page.appendWH('Resolution', label='Resolution')
comp.par.Resolutionw.val = 1280
comp.par.Resolutionh.val = 720
```

### Menus

Create with `appendMenu`, then set entries on the **Par** (not the ParGroup —
ParGroup list assign errors):

```python
page.appendMenu('Mode', label='Mode')
comp.par.Mode.menuNames = ['palette', 'custom', 'texture']
comp.par.Mode.menuLabels = ['Palette', 'Custom', 'Texture In']
comp.par.Mode = 'custom'          # by name
comp.par.Mode.menuIndex = 0       # by index
```

### Toggles / pulses / About strings

```python
page.appendToggle('Enable', label='Enable')
comp.par.Enable = True            # fresh toggles default to False

page.appendPulse('Reset', label='Reset')
comp.par.Reset.pulse()

about = comp.appendCustomPage('About')
about.appendStr('Version', label='Version')
comp.par.Version.val = '0.1.0'
comp.par.Version.readOnly = True
comp.sortCustomPages('Controls', 'About')   # About last
```

### `enableExpr` (same COMP)

Bare parameter names do **not** work. Use `me.par.…`:

```python
comp.par.Gain.enableExpr = 'me.par.Enable'                 # OK
comp.par.Colorr.enableExpr = "me.par.Mode == 'custom'"   # OK
# comp.par.Gain.enableExpr = 'Enable'                    # NO — stays enabled
```

### Children referencing parent custom pars

```python
child = comp.op('noise1')
# Expression (assigning .expr switches mode to EXPRESSION)
child.par.resolutionw.expr = 'parent().par.Resolutionw'
child.par.resolutionh.expr = 'parent().par.Resolutionh'

# Bind
Mode = type(child.par.amp.mode)          # ParMode is NOT on td in execute_python
child.par.amp.bindExpr = 'parent().par.Gain'
child.par.amp.mode = Mode.BIND
```

## Styles catalog (`Page.append*`)

Common agent surface (all return `ParGroup` unless noted):

| Method | Style | Typical use |
|--------|-------|-------------|
| `appendFloat` / `appendInt` | Float / Int | Amounts, counts (`size=` for multi fields) |
| `appendToggle` | Toggle | Feature gates (default **False**) |
| `appendPulse` / `appendMomentary` | Pulse / Momentary | Reset / bake / trigger |
| `appendMenu` / `appendStrMenu` | Menu / StrMenu | Modes / presets |
| `appendStr` / `appendPython` | Str / Python | Labels, About, callback snippets |
| `appendXY` / `appendXYZ` / `appendXYZW` | XYZW | Positions / offsets |
| `appendUV` / `appendUVW` | UVW | Texture coords |
| `appendRGB` / `appendRGBA` | RGBA | Colors (RGB is 3-float RGBA style) |
| `appendWH` | WH | Resolution |
| `appendFile` / `appendFolder` / `appendFileSave` | File* | Paths |
| `appendOP` / `appendTOP` / `appendCHOP` / … | OP family | Optional references — **not** main inputs |
| `appendHeader` | Header | Section title in the dialog |
| `appendSequence` | Sequence | Block/sequence UI (advanced) |

Shared create kwargs (floats/ints also take `size`): `name`, `label=None`,
`order=None`, `replace=True`.

## Gotchas (live)

1. **`replace=True` (default)** recreates the par and **wipes** defaults/minmax/help
   you set earlier. Prefer edit-in-place, or `replace=False` (errors if exists),
   or destroy then recreate intentionally.
2. **`ParMode` is not `td.ParMode`** in `execute_python` — use
   `type(par.mode).EXPRESSION` / `.BIND` / `.CONSTANT` / `.EXPORT`. Assigning
   `.expr` alone already switches to EXPRESSION.
3. **Menu lists** → set on `comp.par.Mode.menuNames`, not `parGroup.Mode`.
4. **`parGroup.X.eval()`** returns a **list** even for size-1; `par.X.eval()`
   returns the scalar.
5. **`pages` ≠ `customPages`** — builtin pages are on `comp.pages`; custom pages
   only on `comp.customPages`.
6. **`.hidden` is not writable** from Python on this build — page-group advanced
   pars, use `enableExpr`, or `startSection = True` for dialog separators.
7. **`destroyCustomPars()`** removes **all** custom pars **and** custom pages.
   Single par: `comp.par.Foo.destroy()`. Page: `page.destroy()`.
8. **`inspect` + `params`** lists custom pars by **member name** (`Colorr`,
   `Resolutionw`, …), mixed with builtin COMP pars.

## Minimal reusable COMP recipe

```python
comp = op('/project1').create(td.baseCOMP, 'fx')
noise = comp.create(td.noiseTOP, 'noise1')
out1 = comp.create(td.outTOP, 'out1')
out1.setInputs([noise])
comp.par.opviewer = './out1'  # ./ = child inside this COMP
comp.viewer = True

ctrl = comp.appendCustomPage('Controls')
ctrl.appendToggle('Enable', label='Enable')
comp.par.Enable = True
ctrl.appendFloat('Gain', label='Gain')
comp.par.Gain.default, comp.par.Gain.min, comp.par.Gain.max = 1.0, 0.0, 2.0
comp.par.Gain.clampMin = comp.par.Gain.clampMax = True
comp.par.Gain.val = 1.0
comp.par.Gain.enableExpr = 'me.par.Enable'
ctrl.appendWH('Resolution', label='Resolution')
comp.par.Resolutionw.val = comp.par.Resolutionw.default = 1280
comp.par.Resolutionh.val = comp.par.Resolutionh.default = 720
ctrl.appendPulse('Reset', label='Reset')

about = comp.appendCustomPage('About')
for nm, label, val in [
    ('Compname', 'Component Name', 'FX'),
    ('Version', 'Version', '0.1.0'),
    ('Lastupdate', 'Last Update', '2026-08-05'),
    ('Created', 'Creation Date', '2026-08-05'),
]:
    about.appendStr(nm, label=label)
    getattr(comp.par, nm).val = val
    getattr(comp.par, nm).readOnly = True
comp.sortCustomPages('Controls', 'About')

noise.par.resolutionw.expr = 'parent().par.Resolutionw'
noise.par.resolutionh.expr = 'parent().par.Resolutionh'
Mode = type(noise.par.amp.mode)
noise.par.amp.bindExpr = 'parent().par.Gain'
noise.par.amp.mode = Mode.BIND
```

After mutate: `inspect` the COMP with `include: ["nodes","params","errors","warnings"]`.

## Related

- Packaging / About / In-Out rules → `tdmcp://docs/component-checklist`
- Par modes (constant/expression/export/bind) → `tdmcp://docs/primer/parameters-and-channels`
- Components / tox → `tdmcp://docs/primer/tox-toe-components`
- Full Python surface → `tdmcp://docs/python-api`


---

**Canonical:** `tdmcp://docs/custom-parameters` 

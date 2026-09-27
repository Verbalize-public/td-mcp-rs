# Parameter technical reference

Use for exact Par/ParGroup metadata and COMP interface examples after resolving
the requested change. Examples are context-dependent fragments, not an automatic
migration script. Preserve existing modes, bindings and values unless authorized.

## Par — parameter access

Use `inspect` with `include:["params"]` and `detailLevel:"detailed"` to read
current menu names/labels (bounded), stored values, evaluated values and modes.
Check `evaluation.available` before interpreting `val:null`; check `menu.available`
and truncation before treating the menu list as complete. Menu labels are not
necessarily valid value tokens. Dynamic/custom menus may accept other values;
never silently lowercase or autocorrect a user value. `mutate_nodes` rejects
invalid tokens/indices for readable built-in closed `Menu` parameters with no
menu source; `StrMenu` and custom/source-driven menus are not hard-rejected.
A rejected later field does not roll back fields already applied.


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

## Related

- {{ skill("python-api") }}
- {{ skill("python-reference") }}
- {{ skill("custom-parameters") }}

**Canonical:** {{ skill("parameter-reference") }}

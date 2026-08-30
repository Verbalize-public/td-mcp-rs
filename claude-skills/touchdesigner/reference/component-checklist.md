# Custom component checklist

Reusable COMP boundary / About / reuse audit. Router: [`operate`](../SKILL.md).
Custom pars depth: [`custom-parameters`](./custom-parameters.md).

## Statefulness & resolution

- **Reset signal.** Any stateful COMP (feedback, LFO, integrator, particle sim, …)
  ships a way to reset to its original state — a pulse custom parameter wired to
  whatever internally clears the state (Feedback TOP `resetpulse`, CHOP `Reset`
  parameter, etc.). Without one, the COMP cannot recover from a bad state without
  a manual node-level fix.
- **`Resolution` (WH).** When a COMP contains a TOP whose resolution isn't fixed by
  its input (a generator, a Feedback loop, a Render TOP), expose a custom
  `appendWH('Resolution')` (`Resolutionw` / `Resolutionh`) and reference those from
  every resolution-setting parameter inside (`parent().par.Resolutionw`, …) — do not
  hard-code a resolution or leave it to inherit unpredictably. (Names must start
  uppercase; avoid `Resolutionxy` → awkward `Resolutionxyw`/`h`.)

## Boundary API

| Kind | Mechanism | Use for |
|------|-----------|---------|
| Operator input | `In TOP` / `In CHOP` / `In POP` / `In DAT` / `In SOP` | External textures, audio, point data, tables the COMP consumes | Public input to wire to
| Operator output | `Out` ops  | Public result others Select or wire to |
| Controls | Custom parameters on the root COMP | Floats, toggles, menus, RGB, pulses, op path (when not revalent to reference from In/Out only) |

### In ops over OP parameters

- Avoid custom parameters of type Operator / path strings as the main input path —
  they hide wiring, break discoverability, and fight the network view.
- Optional: an In that is unused still allows a sensible internal default (noise,
  constant color, silence) so the COMP previews standalone.

### Custom parameter hygiene

- **Read-only** for anything the user should not change (version string, measured
  cook hint, derived size). Set `par.readOnly = True` (and in the Component Editor) —
  not a normal editable float that "you shouldn't touch."
- **Page-group** advanced/internal pars (extra custom page or `startSection`); default
  page = the everyday API. Note: `par.hidden` is **not writable** from Python on
  current builds — do not rely on scripting hide.
- **enableExpr** with `me.par.…` when a mode makes a par irrelevant (e.g.
  `me.par.Mode == 'custom'`). Bare names like `'Enable'` do not gate.
- Prefer menus for modes (`Palette`, `Custom`, `Texture In`) over parallel unused knobs.
- API / naming / snippets (live-verified): [`custom-parameters`](./custom-parameters.md).

### About page (required)

Every reusable COMP ships a custom-parameter **page** named `About` (last page is fine).
All four fields are **read-only** strings (or equivalent) — users inspect, agents write.

| Parameter (suggested name) | Label | Rules |
|----------------------------|-------|-------|
| `Aboutname` / `Compname` | Component Name | Human name of the COMP; set on create; update if renamed |
| `Aboutversion` / `Version` | Version | **Agent-managed.** Semver-style string (`0.1.0`, `1.2.0`). Bump on every substantive change the agent ships |
| `Aboutupdated` / `Lastupdate` | Last Update | **Agent-managed.** ISO date `YYYY-MM-DD` (use session "today"). Set whenever version bumps |
| `Aboutcreated` / `Created` | Creation Date | ISO date `YYYY-MM-DD`. Set **once** when the COMP is first created; never overwrite on later edits |

Agent rules:

- **Create:** initialize all four; version starts at `0.1.0` (or `1.0.0` if shipping ready); both dates = today.
- **Edit (substantive):** bump version (patch for fixes, minor for features, major for breaking API) and refresh Last Update; leave Creation Date alone.
- **Edit (trivial/typo in docs only):** optional — either leave version or bump patch; still refresh Last Update if you touch the COMP.
- Missing or editable About fields = structural DoD **fail**
  ([`definition-of-done`](./definition-of-done.md)).

## Reuse rules (audit before `.tox` / clone)

Rule + preference order: [`network-design`](./network-design.md) relative refs.
Audit checklist below.

Walk the COMP subtree and fail the audit if any of these remain:

- Absolute paths: `/project1`, or any project-root absolute `op('...')` in exprs/path pars
- Wrong relative form: bare name used where a **child** is meant (`./…`), or `./…`
  used where a **sibling** is meant (bare name) — see [`network-design`](./network-design.md)
- Hard-coded parent COMP names (`op('base_foo')` from outside assumptions baked in)
- Children binding to hard-coded numbers that should be `parent().par.*`
- Wrong `parent()` hop count (grandchild binding to nested COMP instead of root)

Prefer: `parent().par.Foo`, `parent().op('null_out')`, path fields like bare
`null_out` (sibling), `./out1` (direct child inside this COMP), `../null_out`
(parent hop).

## Customization patterns

When the COMP generates appearance, ship at least:

1. **Presets** — a small palette or style menu (3–8 looks) with good defaults.
2. **Override path** — custom color / seed / amount when preset ≠ enough.
3. **External drive (when relevant)** — In TOP for texture/noise/mask, or In CHOP for
   animation rates — so the COMP plugs into a larger system without forks.

Do not expose every shader uniform. Curate.

## Performance (1 / 10 / 100 instances)

Ask at design time:

| Question | Prefer |
|----------|--------|
| Does each instance cook heavy work when idle? | Bypass, cook flags, or cheaper standby |
| TOP → CHOP → TOP per instance? | Stay on GPU (TOP/POP) unless CPU value is required |
| Shared lookup / noise / palette? | One upstream shared source + In, not N copies |
| Viewers / resolves always on? | Off for production; nulls are free, viewers are not |
| Script cook / `cook(force=True)`? | Avoid in the shipped COMP; expressions/exports instead |

If 100 instances would melt the frame, redesign before calling the COMP done — or
document a hard instance budget on a Text/Annotate inside the COMP.

## Preview

- Point Common **Operator Viewer** at the **primary Out TOP** (or final visual null):
  `par.opviewer = './out1'` (`./` = child inside this COMP). Turn the Viewer flag on: `comp.viewer = True`.
- Panel/Container: also `par.nodeview = 'opviewer'`. Optional Look Background TOP:
  `par.top` / `par.topfill` for panel chrome.
- Base COMP on current builds may lack `nodeview` — `opviewer` + Viewer flag is enough.
- Defaults must look intentional with nothing wired to optional Ins.
- Perception: `capture` + [`look-grade`](./look-grade.md).

## In-COMP documentation

Minimum to ship inside the component (not only in chat):

- **Operator comments** (`OP.comment`) on the root COMP and every non-obvious child —
  the root's comment states the component's job and its In/Out contract; children's
  state why they exist. This is the layer `inspect` returns for free, so it is the one
  a future agent actually reads. Depth: [`node-comments`](./node-comments.md).
- **Annotate COMP**(s) labeling stages: Inputs → Process → Output (or equivalent).
- **Text COMP** or short Text DAT summarizing: purpose, In/Out list, important pars,
  performance notes.
- Keep docs next to the network they describe; update or remove when the network changes.
  A comment the network has outgrown is a defect, not neutral clutter.

## Network hygiene (while building)

- Delete unused nodes as you deprecate steps; do not leave dead branches "for later."
- Do not delete unrelated user work.
- Errors and warnings are not acceptable on the finished COMP (probe `warnings()` on
  file/device nodes — [`python-api`](./python-api.md) gotcha).
- After logic works: **layout pass** — align, separate stages, readable left→right flow.
  Do not leave test-layout spaghetti in a shipped COMP.

## Minimalism red flags

Stop and simplify if you notice:

- Custom pars that only exist for agent debugging
- Nested COMPs with a single child and no boundary value
- Script DATs that set parameters every frame
- Duplicate nulls with no naming/contract role
- A second parallel "v2" chain left disabled beside the real one

## Related

- [`custom-parameters`](./custom-parameters.md) — control API (live-verified snippets)
- [`network-design`](./network-design.md) — relative refs and layout
- [`node-comments`](./node-comments.md) — per-node intent (`OP.comment`)
- [`definition-of-done`](./definition-of-done.md) — structural verdicts
- [`look-grade`](./look-grade.md) — capture-based look claims


---

**Canonical:** [`component-checklist`](./component-checklist.md) 
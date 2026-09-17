# Known issues — live-TD session findings

Field notes from a real build session on **2026-09-09** (TD build `2025.33070`
under Wine/Linux, project `test.2.toe`, single live pid). Everything below was
hit and worked around in one sitting; items are marked **[tool]** (td-mcp-rs can
fix), **[TD]** (upstream behavior — document, don't fight), or **[docs]**
(skill-card / recipe gap). Session evidence files: `le_*.png` saved under the
session workspace `Repos/verbalize/` (outside this repo; not committed).

Related: [Current limitations](OPEN_WORK.md), [Contract](CONTRACT.md),
[Recipes](RECIPES.md).

> **Audit status:** These are historical session observations, not independently
> confirmed root causes or universal TD rules. See the
> [source audit and refactoring plan](KNOWN_ISSUES_REFACTOR_PLAN.md) for a
> claim-by-claim review, additional findings, and implementation/acceptance stages.
> The original observations below are preserved. Current TD 2025.32460 results:
> [live validation](KNOWN_ISSUES_LIVE_VALIDATION.md). Original-build 2025.33070
> coverage remains unverified. Implemented changes: [implementation ledger](KNOWN_ISSUES_IMPLEMENTATION.md).

## 1. Tool bugs [tool]

### T1 `capture` rejects its own required `pid` — P0, unusable

Four consecutive calls returned `tdmcp.args.missing_field: capture: ..pid is
missing required field "pid"`, including calls that **did** pass
`pid=<live pid>` alongside `mode=top`, `path`, `maxSize`. Immediate-mode capture
never worked once this session; every visual claim was verified by the fallback
instead (`execute_python`: `top.save(path)` + harness `read_image`).

- Impact: the documented "never claim a look without a capture" loop silently
  degrades to a two-step workaround; timed capture jobs (which own the pid)
  were not attempted after this.
- Suggested fix: repro against the arg schema — the pid was present in the call
  but not in the object the validator saw; check arg routing/aliasing for
  optional-vs-required fields, and add an integration test that calls `capture`
  with an explicit pid on a scratch project.

### T2 `mutate_nodes` multi-input `connect` silently overwrites — P0

Sequential `connect` steps with the default `dstInput=0` did not append: each
new connection **replaced** the previous one, so a 3-input `mergeCHOP` and a
2-input `compositeTOP` ended up with only their last source wired. No warning
in the step echo. Discovered only because channel names downstream were wrong.

- Fix that worked: pass explicit `dstInput` indices per step.
- Suggested fix: when `dstInput` is omitted and input 0 is already occupied,
  either auto-advance to the first free connector or hard-error; silent
  rewire is the worst outcome. Add a batch-echo field showing the resolved
  connector index.

### T3 `mergePOP` wire + `inputNpop` param references collide — P1

Setting `input0pop`/`input1pop` params **and** wiring inputs on `mergePOP`
produced duplicate inputs (scope counted twice, palette dropped: point count
 went 3001 → 960) even though both param `.eval()`s resolved correctly.
Clearing the params threw `SystemError` on `.val = None`; `.val = ''` worked.

- Fix that worked: wires only, params left empty.
- Suggested fix: in `mutate_nodes`, detect param-reference + wire overlap on
  the same connector and warn; consider `inputNpop` params as read-only hints
  in docs.

### T4 `glslPOP` has no compile diagnostics — P1

Shader compile failures surface as a one-line `Error: Compile failed` with no
shader log, line, or stage. The tdmcp shader lint honestly reports
`glslPOP exposes no compileResult surface; compile state not checked`. Cost a
manual bisection (empty → `Color[id]` → uniforms → sampler) to isolate.

- Suggested fix: expose `compileResult`-equivalent state for glslPOP consumers
  (the `tdmcp.shader.*` lint already wants it), or at minimum forward the
  TD shader error text into `op.errors()`.

### T5 `glslPOP` output-attribute declaration is a silent trap — P1

`outputattrs` (the passthrough filter) does not list `Color`; the output
attribute is actually declared by the Attr page where `attr0name` is a **menu**
with lowercase values (`color`, `tex`, …). Setting `attr0name='Color'`
(capitalized) is accepted but silently attaches nothing: downstream
`pointAttributes` never gains `Color`, the shader referencing `Color[id]` fails
to compile with no diagnostics (see T4), and the render loses all color.

- Suggested fix: shader lint should cross-check every symbol written in the
  compute body against the declared attr page and error on undeclared writes
  (this exact case: `Color[id] = …` with no Color attr declared).

### T6 `mutate_nodes` relative-path creation resolves against the wrong context — P2

In one batch, `create` of `/project1/lightEngine` followed by relative
`create`s of `lfoSwell` etc. resolved the relative paths against `/project1`
(the default context), not against the COMP created earlier in the same batch.
All "successful" steps landed at the wrong level and had to be deleted/rebuilt.

- Workaround: absolute paths for cross-node batches; or create children with
  `execute_python` inside the COMP.
- Suggested fix: document/apply batch-internal context (a step should be able
  to anchor on a node created earlier in the same batch).

## 2. TD semantic traps [TD] — document in cards/recipes

### D1 Parameter-expression path base is the op's *containing network*

From `geo/palette` (a POP inside a Geometry COMP): `op('..')` → `geo`,
`op('../..')` → the container, `parent()` → `geo`. Writing `op('../../sig')`
resolved to `/project1/sig` → `None` → the CHOP-index expression raised, which
**silently aborted the GLSL build** (no `Color` attribute downstream, no compile
error). Relative OP-path *parameters* (e.g. `render.par.camera = '../cam'`)
resolve from yet another base and evaluated to `None` — absolute paths are the
only reliable form. This single trap caused the most lost time.

### D2 New `geometryCOMP` ships a default torus

Creating a `geometryCOMP` auto-creates `torus1`, which **renders** alongside
whatever you wired (it was the mystery grey blob for several rounds). Every
POP-scene recipe must delete it before trusting a render.

### D3 Render/draw flags are inconsistent on this build

All POPs created via `mutate_nodes` come with `display=False, render=False`
(auto-created `torus1` had the same flags yet rendered). Flagging the terminal
chain `True` is necessary but not obviously sufficient; the actual gating
(wires? render flag on the COMP? outPOP?) is undocumented. Needs root-cause
before the docs can state a rule.

### D4 Primitive-type rendering traps

- Closed **linestrip** renders as a *filled polygon* (the grey blob).
- **Point primitives** rendered nothing at all in this path (3000 points,
  invisible) — no point-size parameter was found on `geometryCOMP` or
  `renderTOP` (probed `pars('*')`). Points-as-lights need a different render
  path (instancing?) that no card currently describes.
- **Lines / open linestrips** render correctly with per-point Color.

### D5 `renderTOP` background/premultiply interplay

Working config: `bgcolora=1` (opaque black bg), `premultrgbbyalpha=true`.
With `bgcolora=0` the saved buffer was fully transparent/empty even with
content in the POPs; flipping premult alone did not recover it. Downstream
alpha-compositing (`over`) of a transparent frame is also a footgun: keep the
composite *inside* the POP chain and render opaque.

### D6 `choptoPOP` semantics

It is a **generator** (no wire input; references a CHOP by param). With
`specifypos=true` the channel lands only on `P.x` of a straight baseline (flat
line, verified from point data); with `specifypos=false` points were all-zero
under forced cooks. Repurposed once as a scope line, then dropped — no recipe
exists for "plot a CHOP as geometry you can actually place".

### D7 `feedbackTOP` loop is unusable outside live playback

The buffer never advanced on forced cooks of downstream ops, kept a stale frame
indefinitely (ghost ellipse), and the `reset` pulse did not clear it under
forced cooking. Only the live render loop refreshes it. Consequence: any
component whose look depends on feedback cannot be verified frame-stepped; the
afterglow feature was cut and replaced with blur+add.

### D8 `trailPOP` wedges the render path

Two independent repros: adding a `trailPOP` (fed by wire, merged via param refs
or wires) turned the render black and produced garbage point counts
(6002 → 960); deleting it restored output immediately. Also note its default
`surftype='rows'` builds ribbon surfaces, not lines. Unusable under forced-cook
verification; untested in live playback.

### D9 `bloomTOP` is a no-op on this build

Output was byte-identical to its input across intensity 1→10, radius 0.5→1,
threshold/fill sweeps. Replaced with `blurTOP` (gaussian, size 40) → `mathTOP`
gain → `composite add` — predictable and cheaper.

### D10 CHOP family quirks (all live-verified)

- `mergeCHOP` inherits the **first** input's channel name; `renameto` on
  downstream ops did not rename. Name channels at their sources (`channelname`/
  `channame` pars).
- `lfoCHOP` `bias` had no effect on a sine wave (amp 0.25, bias 0.5 still
  swung −0.25…+0.25). Apply the 0.5 offset at consumers.
- `noiseCHOP` with `normal=true` normalizes the whole channel to a tiny range
  (±0.05 observed), killing downstream modulation; use `normal=false` with
  explicit `amp`/`offset`.
- `triggerCHOP` does nothing without an input signal crossing the threshold,
  and auto-fires once on load (frame 0 showed k=0.90). Feed it a clock LFO with
  `threshup≈0.6`; sample it via `analyzeCHOP` (function=max) to a 1-sample
  control channel.
- `Channel` objects are not iterable and have no `numSamples`; use
  `chop.numSamples` + index access. `op.errors`/`op.warnings` are **methods**
  in this build; `op.allowCooking` is a property. `cookFrame` values can be
  negative/wrapped — don't use them to judge freshness.

## 3. Skill-card / docs gaps [docs]

- **No "renderable POP scene from scratch" card.** The geoCOMP → MAT →
  renderTOP wiring contract (`material` par, `camera` par needs absolute path,
  auto-torus deletion, primitive-type choice, bgcolor/premult working config)
  cost the most time and is in no card. `tdmcp://docs/pops` covers data model,
  not the render wiring contract.
- `tdmcp://docs/pops` should add: linestrip-closed renders filled; point
  primitives invisible without an undocumented point-size path; trailPOP /
  feedbackTOP / bloomTOP caveats on `2025.33070`.
- `tdmcp://docs/python-api` should record: `errors()`/`warnings()` methods,
  `Channel` iteration/numSamples, `allowCooking` property, and the
  parameter-expression `op()` base rule (D1) — the custom-parameters card is
  accurate on 2025.32460 but does not mention the base-resolution trap.
- `tdmcp://docs/look-grade` leans on `capture`; until T1 is fixed it should
  document the `op.save()` + file-read fallback.
- A canned example COMP (this session's lightEngine) would double as a living
  regression fixture: CHOP signal → uniform-array GLSL POP → glow render,
  with custom pars as the public API.

## 4. Known-good baseline (for regression tests)

Final working network in `test.2.toe`, all under `/project1/lightEngine`:

- Signal: `lfoSwell`(0.17 Hz) + `lfoPulse`(3.3 Hz) + `beatClock → beat →
  kickPk(max)` → `sig` mergeCHOP; `wave` noiseCHOP (`normal=false`, amp .5,
  offset .5, tx = `me.time.seconds*0.25`) → `waveOut` nullCHOP → `waveTOP`
  choptoTOP.
- POPs (in `geo`, rendered via `geo.par.material` + `render.par.camera`
  absolute): `ring`/`core`/`ring2` circlePOPs (`connectivity=lines`) →
  `mergeIn` → `swirl` noisePOP (amp0 expr on kick/swell) → `palette` glslPOP
  (Attr page `attr0name=color`, numcomps 4; uniforms `uCtl` = color0RGB bound
  to sig channels + custom pars; sampler `uWave` = waveTOP) → `all` mergePOP →
  `out` outPOP.
- Render chain: `render` (1280×720, `bgcolora=1`, premult=true) → `glow`
  blurTOP (gauss 40) → `glowAmp` mathTOP (gain = `parent().par.Glow`) →
  `glowadd` composite(add) → `out` outTOP.
- Custom pars: `Intensity Hue Spread Scale Glow Trail Points` (`Trail` present
  but inert — D7). Working shader writes
  `Color[id] = vec4(col * (0.35 + 2.6*wave*wave + uCtl.b*2.5) * uCtl.a, 1)`
  with hue from angle/waveform/kick.
- Verified: kick-peak vs rest frames differ visibly; `Hue` par shifts the
  palette; final `inspect` clean (0 errors, 0 warnings).

Evidence captures: `le_hero2.png` (final), `le_final_kickpeak.png` /
`le_final_rest.png` (signal proof), `le_hue_before/after.png` (par API) — in
`~/Repos/verbalize/`, outside this repo.

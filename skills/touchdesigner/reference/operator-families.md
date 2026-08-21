# Operator families — what to reach for

Sources: [First Things to Know](https://docs.derivative.ca/First_Things_to_Know_about_TouchDesigner),
[2025 Official Update](https://derivative.ca/community-post/2025-official-update/73153),
family wiki pages. Family membership is queryable live: `families['POP']` etc. via
`execute_python`. Exact opType / class names on a connected pid: live `api_help`
(`kind: classes` + `family`/`prefix`) — this cheatsheet stays conceptual (when to
reach for a family).

| Family | Data | Runs on | Reach for it when |
|--------|------|---------|-------------------|
| **TOP** | images/textures | GPU | anything pixel-based: video, compositing, feedback, shaders, particles-as-textures (legacy), final output |
| **CHOP** | channels (arrays of samples) | CPU (some GPU) | motion, audio, control signals, LFOs/envelopes, device I/O (MIDI, OSC, DMX legacy), driving parameters |
| **POP** | points + attributes | GPU | 3D geometry, point clouds, particles, large datasets — **the modern default for 3D data** (new in 2025) |
| **SOP** | polygons/surfaces | CPU | legacy 3D geometry; still needed for meshes-as-grids, booleans, NURBS, and CPU-only ops. Prefer POPs otherwise |
| **DAT** | text/tables | CPU | scripts, callbacks, config tables, string processing, web/API I/O |
| **MAT** | materials | GPU | shading rendered geometry (Phong, PBR, Line, GLSL MAT) |
| **COMP** | networks/containers | — | structure (Base/Container), UI (Slider/Button), 3D scene (Geometry/Camera/Light) |

Rules of thumb:

- Data flows through **side wires**; the top/bottom connectors on COMPs are hierarchy
  (3D parenting, panel nesting), not data.
- Crossing families costs a conversion node (`CHOP to TOP`, `TOP to POP`, `POP to CHOP`,
  `DAT to CHOP`...) and often a GPU↔CPU copy — minimize round-trips; keep GPU chains
  (TOP↔POP) on the GPU.
- SOP → POP migration: most SOP filter work has a POP equivalent (see the wiki "SOP to
  POP Equivalence" page); particles and point clouds are POP territory now.
- A Geometry COMP + MAT + Camera + Light + Render TOP is the minimal 3D render stack;
  POPs and SOPs both render by living inside the Geometry COMP.

## The cook model

TouchDesigner is **pull-based**: a node cooks (recomputes) only when something
downstream needs it *and* an input/parameter changed. Consequences:

- Nodes with no active viewer/output may never cook — "why is my script not running"
  is usually a cook-dependency question. Force with `n.cook(force=True)` when probing.
- Time-sliced CHOPs (e.g. Lag, Filter, device inputs) cook every frame regardless and
  process the frames elapsed since last cook, keeping realtime behavior smooth.
- Python expressions in parameters create automatic cook dependencies on what they read.
- Performance debugging: middle-click a node for cook time/memory, or use the
  Performance Monitor / Probe palette.

## Moving values around: reference > export > script

1. **Parameter expressions** (Python) — `op('null_lfo')['chan1']` in a parameter: the
   standard way; creates a dependency, survives edits upstream.
2. **Exports** (CHOP channel → parameter drag) — visible arrows, slightly faster than
   Python; good for many channels.
3. **Bind** — two-way parameter linking (UI widgets ↔ values).
4. **Scripts setting `par.x = ...`** — last resort; imperative writes hide the data flow
   and fight the cook model.

Custom parameters on COMPs (Component Editor) are the **control** API of a component
(scalars, menus, colors, pulses). In/Out boundary rule: main body "Hard rules" +
[component-checklist.md](component-checklist.md).


---

**Canonical:** `tdmcp://docs/operator-families` 

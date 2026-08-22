# Primer: cook model and operator families

Condensed TD software facts for agents. Prefer live `inspect` / `api_help` for
exact names.

## Pull-based cook

A node cooks when something downstream needs its output **and** an input or
parameter changed since the last cook. Unviewed, unreferenced nodes often do
not run. The Cooking flag and viewers create demand.

Dirty propagation: changing a parameter or upstream output marks dependents
dirty; they cook on next demand. Force-cook via Python
(`op('…').cook(force=True)`) only when you truly need it — `inspect` / `capture`
do not force-cook by default.

Depth (consequences, time-sliced CHOPs, performance debugging):
{{ skill("operator-families") }} — the cook model lives there; this primer only
orients.

## Seven families (operate view)

| Family | Typical data | Notes |
|--------|--------------|-------|
| TOP | pixels / textures | GPU compositing, video, feedback, GLSL TOP |
| CHOP | channels over time | Control, audio, device I/O; some GPU CHOPs |
| POP | points + attributes | Modern GPU geometry / particles |
| SOP | CPU meshes | Legacy 3D; prefer POP when possible |
| DAT | text / tables | Scripts, callbacks, config |
| MAT | materials | Phong / PBR / GLSL MAT |
| COMP | containers | Networks, UI, Geometry / Camera / Light |

Full semantics, cook-model consequences, and the conversion rules:
{{ skill("operator-families") }} (the umbrella's quick table also routes).
This primer only orients.

## Moving values

Preference for driving parameters: **expression > export > bind > script**
(last resort). CHOP export and bind are first-class TD mechanisms — do not
default to frame scripts to copy values. Depth:
{{ skill("operator-families") }} "Moving values around".

## Related

- {{ skill("operator-families") }}
- {{ skill("play-state") }}
- {{ skill("primer/parameters-and-channels") }}
- Official wiki: https://docs.derivative.ca/TouchDesigner_Glossary

---

**Canonical:** {{ skill("primer/cook-and-families") }}

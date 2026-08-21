# Primer: parameters and channels

**Canonical:** {{ skill("primer/parameters-and-channels") }} 

## Four parameter modes

| Mode | Use when |
|------|----------|
| Constant | Fixed authored value |
| Expression | Python expression on the parameter |
| Export | CHOP channel drives the parameter |
| Bind | Two-way link between parameters |

Always `.eval()` when reading a parameter from Python unless you intentionally
inspect mode/raw storage. Operate cheatsheet: {{ skill("python-api") }}.

## Driving motion / look

Prefer CHOP → export/bind (or expressions) over per-frame scripts. For reusable
COMPs, expose a small custom-par control surface ({{ skill("custom-parameters") }})
and wire data through In/Out ops ({{ skill("component-checklist") }}).

## Related

- {{ skill("python-api") }}
- {{ skill("custom-parameters") }}
- Wiki: https://docs.derivative.ca/Parameter_Mode

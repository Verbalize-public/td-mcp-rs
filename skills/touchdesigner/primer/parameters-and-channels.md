# Primer: parameters and channels

**Canonical:** `tdmcp://docs/primer/parameters-and-channels` · disk: `primer/parameters-and-channels.md`

## Four parameter modes

| Mode | Use when |
|------|----------|
| Constant | Fixed authored value |
| Expression | Python expression on the parameter |
| Export | CHOP channel drives the parameter |
| Bind | Two-way link between parameters |

Always `.eval()` when reading a parameter from Python unless you intentionally
inspect mode/raw storage. Operate cheatsheet: `tdmcp://docs/python-api`.

## Driving motion / look

Prefer CHOP → export/bind (or expressions) over per-frame scripts. For reusable
COMPs, expose a small custom-par control surface (`tdmcp://docs/custom-parameters`)
and wire data through In/Out ops (`tdmcp://docs/component-checklist`).

## Related

- `tdmcp://docs/python-api`
- `tdmcp://docs/custom-parameters`
- Wiki: https://docs.derivative.ca/Parameter_Mode

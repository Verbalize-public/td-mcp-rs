# Primer: parameters and channels

## Four parameter modes

| Mode | Use when |
|------|----------|
| Constant | Fixed authored value |
| Expression | Python expression on the parameter |
| Export | CHOP channel drives the parameter |
| Bind | Two-way link between parameters |

Always `.eval()` when reading a parameter from Python unless you intentionally
inspect mode/raw storage. Operate cheatsheet: [`python-api`](../reference/python-api.md).

## Driving motion / look

Prefer CHOP → export/bind (or expressions) over per-frame scripts. For reusable
COMPs, expose a small custom-par control surface ([`custom-parameters`](../reference/custom-parameters.md))
and wire data through In/Out ops ([`component-checklist`](../reference/component-checklist.md)).

## Related

- [`python-api`](../reference/python-api.md)
- [`custom-parameters`](../reference/custom-parameters.md)
- Wiki: https://docs.derivative.ca/Parameter_Mode

---

**Canonical:** [`primer/parameters-and-channels`](./parameters-and-channels.md)
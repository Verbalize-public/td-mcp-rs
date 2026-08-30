# Definition of Done (structural)

Observable runtime evidence only — never PASS from code, parameter values, or
docs alone. Look / FPS claims use [`look-grade`](./look-grade.md).

## Verdicts

| Verdict | Meaning |
|---------|---------|
| `PASS` | Evidence matches the claim |
| `FAIL` | Evidence contradicts the claim, or evidence is not good enough to trust a PASS |
| `BLOCKED` | Surface unreachable / unreadable (e.g. bridge timeout) — record diagnostics; do not upgrade to PASS |
| `SKIP` | No runtime surface for this claim, or deliberate cost-reduction already decided |

## Doubt → FAIL

If you are not sure the claim is true from **this turn's** live evidence, verdict
is **FAIL** — not PASS, not a soft skip.

Treat as insufficient (→ **FAIL**) when evidence is any of:

- Missing (never inspected / never captured for a claim that needs it)
- Ambiguous (two readings equally plausible; path unresolved)
- Stale (pre-mutation capture/inspect reused after a mutate that could change it)
- Unreadable (timeout, truncated beyond use, corrupt payload)
- Black / empty when a look surface was claimed (see look-grade)
- Inferred only from authored code, default params, memory, or docs

`SKIP` is only for claims with **no** runtime surface, or an explicit
cost-reduction decision already made. `BLOCKED` is for unreachable surfaces —
still not a PASS.

## Final grid pass

Once the network logic is complete and verified, do **one** layout pass:
reposition nodes so data flow reads left-to-right (or top-to-bottom) at a
glance, with zero overlapping operators. Depth:
[`network-design`](./network-design.md) (Layout section).

- **One pass only** — never reorganize iteratively after each mutation; it is
  a heavy cosmetic operation that burns tokens with no logic value.
- **Skip during complex multi-step plans** — defer the grid pass until the
  final step before claiming done.
- **Required before `PASS`** — a network left as spaghetti with overlapping
  nodes is not done.

## Structural checklist

- [ ] Final `inspect` on the touched network **parent** (COMP / container mutated
      under) — errors/warnings clean before claiming done
- [ ] Structure / children / params / errors / wires used `inspect` (not Python
      walks as the primary read)
- [ ] Non-trivial network (>3 nodes / COMP hub / branched) described in
      OpSketch before building or mutating ([`opsketch-notation`](./opsketch-notation.md))
- [ ] Any `execute_python` / expression / script preceded by
      [`python-api`](./python-api.md)
- [ ] Relative refs + In/Out rules held ([`network-design`](./network-design.md),
      [`component-checklist`](./component-checklist.md))
- [ ] Final grid pass: touched subtree reorganized — readable flow, zero
      overlapping nodes — one pass at the end, not iteratively
      ([`network-design`](./network-design.md) layout section)
- [ ] Custom COMP: About page populated, In/Out pins present, custom pars clean
      ([`component-checklist`](./component-checklist.md))
- [ ] Look claims: non-black `capture` (store-first, `maxSize: 256`) verified
      against [`look-grade`](./look-grade.md)
- [ ] Stop after 3 failed probes with no new evidence

## Related

- OpSketch language: [`opsketch-notation`](./opsketch-notation.md)
- Wiring notes: [`python-api`](./python-api.md)
- Look / FPS grading: [`look-grade`](./look-grade.md)
- Layout conventions: [`network-design`](./network-design.md)

---

**Canonical:** [`definition-of-done`](./definition-of-done.md)
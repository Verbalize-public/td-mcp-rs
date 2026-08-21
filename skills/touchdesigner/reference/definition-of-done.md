# Definition of Done (structural)

Observable runtime evidence only — never PASS from code, parameter values, or
docs alone. Look / FPS claims use `tdmcp://docs/look-grade`
(file: `look-grade.md`).

**Canonical:** `tdmcp://docs/definition-of-done` · disk: `reference/definition-of-done.md`

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

## Structural checklist

- [ ] Final `inspect` on the touched network **parent** (COMP / container mutated
      under) — errors/warnings clean before claiming done
- [ ] Structure / children / params / errors / wires used `inspect` (not Python
      walks as the primary read)
- [ ] Any `execute_python` / expression / script preceded by
      `tdmcp://docs/python-api`
- [ ] Relative refs + In/Out rules held (`tdmcp://docs/network-design`,
      `tdmcp://docs/component-checklist`)
- [ ] Stop after 3 failed probes with no new evidence

Depth: OpSketch language `tdmcp://docs/opsketch-notation`; wiring notes in
`tdmcp://docs/python-api`.

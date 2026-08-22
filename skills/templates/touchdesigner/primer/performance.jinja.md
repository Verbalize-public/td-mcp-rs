# Primer: performance

## Agent habits

1. Do not cook the whole project from Python “just to be sure”.
2. Prefer narrow `inspect` paths; avoid huge recursive walks.
3. After builds, check errors/warnings on the touched parent COMP.
4. FPS / smooth-motion claims need play-on evidence and
   {{ skill("look-grade") }} — not parameter values alone.
5. Cut CHOP fan-out that re-triggers expensive TOP/POP graphs every frame when a
   slower control rate would do.

## Performance Monitor

Use TD's Performance Monitor in the editor when diagnosing cook cost. Agents
should not claim FPS PASS without live observation (`capture` / timing evidence)
while the project is playing ({{ skill("play-state") }}).

## Related

- {{ skill("look-grade") }}
- {{ skill("primer/cook-and-families") }}
- {{ skill("tooling-concurrency") }}

---

**Canonical:** {{ skill("primer/performance") }}

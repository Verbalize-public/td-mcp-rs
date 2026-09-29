# Delta and guarded writes (TD)

Two separate problems that look alike and must not be solved with one call:
finding out what changed, and writing only if nothing did. The first is a
read. The second narrows a race it cannot close.

## What a delta is not

Inspecting twice and diffing the two results yourself looks equivalent and is
not. An `inspect` result carries no record of *when* it was read, so the
baseline cannot be shown to be a read of anything in particular. A delta is
only actionable if the caller can point at both moments.

`compare_nodes` reads the projection itself and returns a stamped
observation. That observation, retained, is the baseline.

## Take a baseline

```json
{"pid":123,"paths":["/project1/audio_gain"],"fields":["par.gain","comment"],"capture":true}
```

`capture: true` only reads. It returns an `observation` to keep, and nothing
about the scene changes.

Capture is opt-in on purpose. With no `capture` and no `baseline`, the call
**refuses** with `baseline_unavailable` — it does not return an empty diff. An
empty diff would say every field is unchanged when nothing was compared, and a
caller who believed it would write on top of a scene they had never looked at.

## Compare against it

```json
{"pid":123,"paths":["/project1/audio_gain"],"fields":["par.gain","comment"],"baseline":{"observedAtMs":1750000000000,"fields":["par.gain","comment"],"nodes":[{"path":"/project1/audio_gain","values":[]}]}}
```

Only the fields you name are compared. An unrequested field is not evidence of
anything, so it is absent from the result rather than reported unchanged. The
response reports `freshness` (how old the baseline was), the `changedFields`,
a per-node roster, and a fresh `observation` to use as the next baseline.

`atomic` and `exactlyOnce` are `false` in every response. Two reads and a
comparison are three moments, and a field can change between any two of them.

The result is a report about those reads, not a snapshot that is safe to write
against. It is the starting point for deciding, not the decision.

## What the comparison will not do for you

A daemon mutation counter advances when a call is *dispatched*, not when it
takes effect, so it is never consulted. Every field reported unchanged was
actually re-read. If a field could not be read, the node is marked
`readable: false` rather than silently counted as unchanged.

A baseline taken over a different field set, or missing a path you asked for,
is refused rather than partially compared.

Absent, `null`, `false`, `0` and `""` are five different states, and a change
between any two of them is reported as a change. `NaN` and `Infinity` come back
as an explicit `unrepresentable` marker rather than rounded, because a delta
computed against an approximation is not a delta.

## Guarded writes

A `mutate_nodes` step may carry `expect`, checked against a fresh read
immediately before the write:

```json
{"pid":123,"steps":[{"op":"set","path":"/project1/audio_gain","values":{"gain":0.4},"expect":{"fields":{"par.gain":0.5}}}]}
```

If the field has moved, the step is refused and **nothing is written**. The
response reports what was expected and what was observed, so you can decide
whether to re-plan or deliberately overwrite.

The guard is re-read at write time, not taken from the baseline you compared
against. A guard that reused a cached value would pass on evidence that no
longer holds, which is the same as having no guard.

Three limits worth stating before you rely on it:

- It **narrows** the race, it does not close it. Another script, a callback, or
  a user can still change the field between the check and the write.
- It **never retries**. A retried write against an unknown effect is how one
  intent becomes two mutations.
- A precondition that cannot be read **fails closed** (`precondition_unreadable`)
  and writes nothing. Confirming a write over a value nobody observed is the
  exact loss a guard exists to prevent.

`create` and `place` steps cannot be guarded — there is no prior value to
expect. A failing guard stops the later steps in that batch rather than
continuing past a refusal.

## The order that works

1. `compare_nodes` with `capture: true` when you need a baseline.
2. `inspect` for the detail behind a change, if the projection was not enough.
3. Mutate with `expect` set from what the comparison just observed.
4. `compare_nodes` again against the baseline to confirm the result.

Step 4 matters even when the write reported success. A reported success is
the call's account of what it did; the comparison is what is there.

## Related

- [`mutation-zones`](./mutation-zones.md) — choosing the subtree to change
- [`tooling-concurrency`](./tooling-concurrency.md) — serializing calls on one pid
- [`definition-of-done`](./definition-of-done.md) — claim-specific verification

**Canonical:** [`delta-guarded-writes`](./delta-guarded-writes.md)
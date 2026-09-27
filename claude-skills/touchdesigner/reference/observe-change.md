# Observe or change

Current-tool procedure. Apply the [`operate`](../SKILL.md). Substitute confirmed targets in examples.

## Pick the observation that answers the question

- Known parameter name: request that value directly. Do not perform a names-only preflight mechanically when the name is already known.
- Unknown parameter: discover names, then select the few relevant values. Do not dump all parameters to find one setting.
- Unknown topology: inspect the parent roster first, then the relevant consumers/children. A parent roster is not recursive health or complete wiring evidence.
- Need compiler evidence: request the supported compile/content surface only on affected consumers. Current `content` may include source text and force compilation; it is not a free generic “more detail” flag.

Default `inspect` returns nodes/errors/warnings, **not params**. A nonempty `include` is an allowlist. Names-only does not evaluate parameter values. Prefer exact `paramNames`; an empty selection is not the same as omitted selection.

Current example: read the known Gain value without unrelated sections:

```json
{"pid":123,"paths":["/project1/fx"],"include":["params"],"paramNames":["Gain"],"paramsMode":"values"}
```

Check node `ok`, `paramsSelection.missing/complete` and the parameter's `evaluation.available`. A present but unreadable value is not a valid null. Detailed menu/stored-value metadata is available; broader schema fields require supported tooling or a justified bounded Python probe—not invented inspect options.

## Change only after the relevant uncertainty is resolved

For a confirmed constant Gain in an authorized target:

```json
{"pid":123,"steps":[{"op":"set","path":"/project1/fx","values":{"Gain":0.8}}]}
```

For a driven parameter, preserve the intended expression/bind/export relationship unless changing it is authorized by the task. Ask a focused question only if the intended mode change is materially ambiguous.

- `values` writes parameters; `flags` accepts only the supported OP-flag allowlist. Layout properties such as `nodeX/nodeY` are neither. Use authorized Python when layout is actually required; do not invent a structured layout argument.
- Creating a COMP does not enter it. Use an explicit context and returned canonical paths; account for auto-renames and alias lifetime.
- Specify connector indices. Protect existing wiring with `onOccupied:"error"` unless replacement is intended.
- A batch is not a transaction. On failure, use [`recover`](./recover.md) before replaying anything.

## Verify the change, not the whole universe

Read back the relevant values/modes, affected destinations and diagnostic availability. For the example:

```json
{"pid":123,"paths":["/project1/fx"],"include":["params","errors","warnings"],"paramNames":["Gain"]}
```

Applied-step count alone does not prove the intended value, connection or appearance. Check returned paths and warnings. Follow roster pages when absence/completeness matters; a last page is not a complete roster and live offsets can move.

Read-only work stays read-only: report a stale comment or suspicious layout; do not fix it without scope. A scalar adjustment does not automatically authorize relayout, new controls, saving the original project or a packaging pass. Finish with the evidence appropriate to the requested claim, using [`verify-handoff`](./verify-handoff.md).

## Related

- [`operate`](../SKILL.md)
- [`recover`](./recover.md)
- [`definition-of-done`](./definition-of-done.md)
- [`mutation-zones`](./mutation-zones.md)

**Canonical:** [`observe-change`](./observe-change.md)
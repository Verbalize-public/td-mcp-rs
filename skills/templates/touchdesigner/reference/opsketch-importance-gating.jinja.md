# Importance gating

Compatibility entry for parameter selection in an OpSketch. The canonical rule
is **Parameter importance** in {{ skill("opsketch-notation") }}. Use it only when
a topology sketch serves the task, not as a required parameter-read pass.

## The rule

Show task-relevant observed values, driven modes, authored source references and
custom controls. Do not infer defaults from zero/false/empty values or omit a
load-bearing value to make a sketch shorter.

## Trivial types — never scan

Legacy anchor retained: the old unconditional exemption is withdrawn. Nulls,
Selects and boundary nodes often need only topology, but their relevant behavior
must still be inspected or shown when the task depends on it.

## Unknown-default bias

Unknown defaults remain unknown; use the canonical selection rule rather than
claiming a common-looking value is the factory default.

## Worked contrast

The planned `levelTOP` → `blurTOP` example is retained in
{{ skill("opsketch-notation") }} under Parameter importance.

## Definition of Done

The sketch explains the requested behavior without invented evidence; its scope
and planned/observed distinction are explicit.

## Related

- {{ skill("opsketch-notation") }}
- {{ skill("opsketch-examples") }}

**Canonical:** {{ skill("opsketch-notation") }}

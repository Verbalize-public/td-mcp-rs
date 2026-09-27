# Python execution contract

Current-tool procedure. Use [`operate`](../SKILL.md) and the actual runtime schema. This is the short execution contract; the detailed language/API reference remains available on demand.

## When Python is justified

Use a structured tool when it directly expresses the task. Python is appropriate for unsupported operations, custom numerical logic, or a coherent existing builder/test whose decomposition would add risk and calls. Do not write a broad network walk merely to discover names that `inspect` can return.

## Do not mix execution contexts

`execute_python` receives `script` and an explicit PID. Assign `result` inside the script to return data. `result`, `content`, `prompt`, `snippet` and `message` are not interchangeable tool-argument names. Use the complete current schema rather than a hand-maintained abbreviated list of optional fields.

The bridge provides `td`, `op`, `result`, `tdmcp_resolve` and documented conditional aliases. It does **not** provide a script-owner `me`, bare `parent`, bare operator classes or automatic stdlib imports. Import `math`, `json`, `sys`, etc. explicitly when needed. DAT scripts, parameter expressions and the textport have different contexts.

Use absolute OP paths or `tdmcp_resolve` for the intended `contextPath`. That context is a resolution helper, **not an authorization boundary or Python sandbox**. Respect the task's actual scene/filesystem scope.

## Resolve the uncertain symbol before another guess

Use current `api_help` class/classes queries and actual parameter names. If a needed member signature is not exposed, use an allowed, narrowly scoped documentation/member probe; do not invent an unsupported `api_help` query kind. Prefer exact observed symbols over memory of another DCC API.

Distinguish namespace/import errors, TD member errors and the script's own logic failures; `api_help` is not a stdlib import resolver. After an AttributeError or wrong-type result, do not try a sequence of plausible alternatives. Identify the object's actual class and the failing member/parameter; make one evidence-backed repair. Read the relevant detailed reference only for the unresolved operation.

## Keep execution and evidence bounded

- Check script syntax before sending when an allowed local check can do so; syntax validity is not TD API validity.
- Return the selected values needed for the next decision. Avoid printing the same JSON also assigned to `result`; keep relevant diagnostic logs when they matter.
- Convert unsupported result objects deliberately, without silently changing numeric meaning. JSON serialization failure can occur **after** scene edits.
- Do not block TD's main thread with sleeps while expecting frames/callbacks to advance. Use [`visual-timing`](./visual-timing.md).
- Use `try/finally` for temporary state you can actually restore. Restoring a play flag is not undoing accumulated simulation state or arbitrary script effects.
- An exception, timeout or failed nested `exec` is not proof nothing changed. Follow [`recover`](./recover.md).

Future script-source references, numerical tools and evidence readers are used only after their actual schemas/capabilities are available. Do not turn a release-plan name into a tool call.

## Related

- [`operate`](../SKILL.md)
- [`python-reference`](./python-reference.md)
- [`custom-parameters`](./custom-parameters.md)
- [`primer/scripting-surfaces`](../primer/scripting-surfaces.md)
- [`recover`](./recover.md)

**Canonical:** [`python-api`](./python-api.md)

## Compatibility reference index

Old section links remain navigation to the retained technical reference.

## Scope: TD scripts vs `execute_python`

See [`python-reference`](./python-reference.md) for this reference section.

## Global shortcuts

See [`python-reference`](./python-reference.md) for this reference section.

## Type objects — starter catalog

See [`python-reference`](./python-reference.md) for this reference section.

## Wiring (inputs / Connectors)

See [`python-reference`](./python-reference.md) for this reference section.

## OP — every node inherits this

See [`python-reference`](./python-reference.md) for this reference section.

## Par — parameter access

See [`python-reference`](./python-reference.md) for this reference section.

## TOP

See [`python-reference`](./python-reference.md) for this reference section.

## CHOP

See [`python-reference`](./python-reference.md) for this reference section.

## DAT — table & text

See [`python-reference`](./python-reference.md) for this reference section.

## POP / SOP (geometry)

See [`python-reference`](./python-reference.md) for this reference section.

## COMP

See [`python-reference`](./python-reference.md) for this reference section.

## Custom parameters (COMP-only)

See [`python-reference`](./python-reference.md) for this reference section.

## Pattern matching

See [`python-reference`](./python-reference.md) for this reference section.

## Gotchas

See [`python-reference`](./python-reference.md) for this reference section.
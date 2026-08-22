# Importance gating

## The rule

Show a parameter in a node's `{}` block only if at least one is true:

1. Its value differs from the operator type's factory default.
2. It's bound by expression, bind, or export (`=`/`~` prefix, or a wired export arrow).
3. It's a DAT's script/callback body (Script/Execute/Text DAT with authored code).
4. It's a custom parameter on a COMP (Component Editor page, not a built-in par).

If none apply, the node gets **no** `{}` block at all — not an empty `{}`.

## Trivial types — never scan

These always render as bare `<leaf> <opType> [<- inputs]`, even if one of their pars happens
to be non-default — the wiring pattern is the only fact that matters on these types:

| Type | Why exempt |
|------|-----------|
| `nullTOP`/`nullCHOP`/`nullSOP`/etc. | Passthrough by contract |
| `inCOMP` / `outCOMP` and other In/Out family ops | Boundary marker, not behavior |
| `mergeTOP`/`mergeCHOP`/etc. at default index | Pure combination, no tunable behavior |
| `selectCOMP`/`selectCHOP` with no custom pars beyond the select target | `{select:...}` is the only fact worth keeping |

## Unknown-default bias

Agents will not always recall an operator's factory default from memory. When unsure, use
this bias instead of spending an extra tool round-trip to look it up:

- **Omit** values that look canonical: `0`, `1`, `1.0`, `""`, `"Off"`, identity transform
  scale/rotation, default gray/white/black-adjacent colors.
- **Keep** anything that looks hand-authored: filenames/paths, non-round numbers, saturated
  colors, any expression or bind, any string reading like a name/label rather than a
  placeholder.
- If genuinely ambiguous and the parameter is load-bearing for the task at hand (e.g. the one
  value a look claim depends on), keep it — bias toward showing meaning over dropping it.

## Worked contrast

`level1`'s opacity is untouched (default `1.0`) — no `{}`. `blur1`'s size was changed from
its default `1.0` to `12.0` — shown:

```text
level1   levelTOP
blur1    blurTOP        <- level1  {size:12.0}
```

## Definition of Done

An OpSketch that shows `{}` on every node, or shows `{}` on zero nodes in a network that
clearly has custom work in it, has failed the gate — re-check against this rule before
returning it.

## Related

- {{ skill("opsketch-notation") }} — grammar
- {{ skill("opsketch-examples") }} — worked transcriptions


---

**Canonical:** {{ skill("opsketch-importance-gating") }} 

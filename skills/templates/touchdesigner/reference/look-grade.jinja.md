# Look / FPS grade

Operating-agent contract for visual and time-sliced claims. Structural verdicts:
{{ skill("definition-of-done") }}.

## Ownership

The agent that owns the user task grades look / FPS after `capture`. Do not
PASS a look claim from code, parameter values, or docs alone. If a helper
model only describes pixels, the operating agent still emits the final verdict.

## Capture (CONTRACT-aligned)

Image modes return structured `{ path, bytes, mimeType, … }` **and** MCP image
content when PNG is present (`imageBase64` stripped from structured after
promotion). Default `maxSize` is **256** (longer-side cap).

**Store-first (chat thrift):** prefer path + short note in returns; reuse the
same capture until a mutate that could change the look; do not re-inject huge
dumps. Store-first does **not** mean “chat never sees pixels” — the MCP image
attachment is valid evidence when the model can see it.

Modes (tool is self-describing): `top` / `preview` / `auto` / `chop_data` /
`pop_data` (+ preview aliases). Prefer `auto` unless you need a specific mode.

## Vision path

1. `capture` on the claimed surface (store-first path + image when PNG).
2. If the current model **cannot** see the image artifact, use a vision-capable
   helper with path + claim, then grade from that observation.
3. Operating agent still emits the final verdict.

Non-black requirement for image look claims: black / empty / unreadable →
**FAIL** (or **BLOCKED** if capture unreachable) — see definition-of-done
doubt rules.

## FPS / time-sliced claims

Require live evidence while the project is **playing**. If paused, fix play
state first ({{ skill("play-state") }}) before FAIL/PASS on motion or FPS.

## Related

- {{ skill("definition-of-done") }}
- {{ skill("play-state") }}
- {{ skill("tooling-concurrency") }}

---

**Canonical:** {{ skill("look-grade") }}

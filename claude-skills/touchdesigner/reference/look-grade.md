# Look / FPS grade

Operating-agent contract for visual and time-sliced claims. Structural verdicts:
[`definition-of-done`](./definition-of-done.md).

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

Judge against the requested output. Black and uniform-frame classifications
are observations, not automatic failures: masks, solid colors, and fades can
be intentional. Unexpected blank output needs investigation. If the image is
missing or unreadable, report the look as **unverified**, not successful.

## FPS / time-sliced claims

Require live evidence while the project is **playing**. If paused, fix play
state first ([`play-state`](./play-state.md)) before FAIL/PASS on motion or FPS.

## Related

- [`definition-of-done`](./definition-of-done.md)
- [`play-state`](./play-state.md)
- [`tooling-concurrency`](./tooling-concurrency.md)

---

**Canonical:** [`look-grade`](./look-grade.md)
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
promotion). Default `maxSize` is **512** (longer-side cap).

**Store-first (chat thrift):** prefer path + short note in returns; reuse the
same capture while relevant parameters, time, inputs and reset state remain
unchanged; do not re-inject huge
dumps. Store-first does **not** mean “chat never sees pixels” — the MCP image
attachment is valid evidence when the model can see it.

Modes (tool is self-describing): `top` / `preview` / `auto` / `chop_data` /
`chop_image` / `pop` (the last two are preview aliases). Prefer `auto` unless
you need a specific mode.

Send the target in the call itself, for example
`{"pid":123,"path":"/project1/fx/out1","mode":"top","maxSize":512}`,
replacing the example PID/path with the confirmed target and adding `daemonId`
when needed. On validation failure, compare the exact transmitted arguments
with `describe_tools`, correct the call, and distinguish client/schema rejection
from TD execution. Do not record a tool defect from an intended-but-unsent PID.

## Vision path

1. `capture` on the claimed surface (store-first path + image when PNG).
2. If the current model **cannot** see the image artifact, use a vision-capable
   helper with path + claim if one is available and authorized, then grade from
   that observation. Otherwise leave the visual claim unverified.
3. Operating agent still emits the final verdict.

Judge against the requested output. Black and uniform-frame classifications
are observations, not automatic failures: masks, solid colors, and fades can
be intentional. Unexpected blank output needs investigation. If the image is
missing or unreadable, report the look as **unverified**, not successful.

## FPS / time-sliced claims

FPS claims require live evidence while the project is **playing**. Check play
state first ({{ skill("play-state") }}).

For transitions and other stateful motion, follow {{ skill("reset-state") }}:
reset through the public signal, advance every intervening frame, and capture
initial/intermediate/final output after the required cooks. Controlled stepping
may leave transport paused between samples; verify actual advancement and
cooking. Record observed frames, offsets from reset, and play state with the
evidence. Independent captures on a freely playing timeline do not prove exact
sample spacing. Stepped samples do not establish real-time FPS.
Use {{ skill("timed-capture") }} for prepared sample schedules and video jobs;
check terminal state and actual offsets before grading their samples.
Movie FPS metadata describes playback rate, not measured live performance.

## Related

- {{ skill("definition-of-done") }}
- {{ skill("play-state") }}
- {{ skill("tooling-concurrency") }}

---

**Canonical:** {{ skill("look-grade") }}

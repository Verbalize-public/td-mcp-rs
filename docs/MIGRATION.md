# Migrating to v0.2.0

From `v0.1.4` to `v0.2.0`. This release is additive: every existing tool,
argument and response field still works, and nothing changes shape by
default. What follows is what is *new*, what a caller may now opt into, and
what a value that used to be ambiguous now says.

**Before upgrading**, read [Packaging and releases](DELIVERY.md) for the
installation behaviour. A `tdmcp-daemon install` preserves your
`config.toml` and any customized `template.toe`; `--force` refreshes managed
assets without touching either.

## Does anything I depend on change?

| | v0.1.4 | v0.2.0 |
| --- | --- | --- |
| Existing tool names and arguments | — | unchanged |
| `inspect` response with no `output` option | full legacy shape | **unchanged** |
| `config.toml` | — | unchanged; new keys are commented out and defaulted |
| `template.toe` | — | preserved across install and upgrade |

Nothing in this table is a code change, so nothing in it needs an action. It
is here because "the changelog is all additions" is a claim about this
release, and a caller upgrading a live configuration deserves to be told which
way to look if they are surprised.

One thing did grow rather than merely appear: the set of **bridged tools**
enqueued exclusive is now `execute_python`, `inspect`, `capture`, `record`,
`mutate_nodes`, `api_help`, `editor_context` and `palette_probe` — `record`
and `palette_probe` were added. If you run a palette probe or a timed
recording in the same session as another bridged call, expect
`tdmcp.mcp.session_busy` or `tdmcp.bridge.queue_busy` where a call used to
proceed. That is the same gate, not a new failure. A timed `capture`/`record`
job reserves TD transport across calls until cleanup, so a session with a live
timing job is not a session that will answer anything else.

## New tools

All four are additions to the catalog and do nothing until called.

- **`sample_data`** — exact numerical reads: a pixel value, a CHOP sample, a
  parameter's evaluated value. Replaces reading a number off a rendered image,
  which cannot distinguish a channel value from a preview. **A CHOP's
  `numpyArray` is channel-major** (`[chans, samples]`), and that was measured,
  not assumed: a `noiseCHOP` on 2025.32460 reports `[1, 600]`. The shape is
  cross-checked against the CHOP's own `numChans`/`numSamples` and a
  disagreement is refused, because guessing is what caused the bug. The
  per-sample indexer is the default path.
- **`script_source`** — named Python sources with immutable content
  references. Register once, run by `sourceId`; a re-registration reports the
  dependencies it did *not* apply rather than implying it did. Replaces
  re-sending the same script inline on every call.
- **`param_interface`** — export a control surface as a document, then apply
  one additively. Preserves a Python parameter's expression across a round
  trip, and refuses a menu whose items could not be set instead of reporting
  it as applied. Replaces hand-assembled `mutate_nodes` batches.
- **`project_snapshot`** — a save receipt that names what was written and
  where. The returned path is the only trustworthy identity: both save paths
  can write somewhere other than the obvious default, and a file that already
  exists is a silent in-place overwrite, so the receipt carries a content
  `sha256` of what was written. A component receipt reports its unsaved state
  as **`available: false` with a reason**: `COMP.dirty` exists, is a bool, and
  on 2025.32460 it never flips — it read False on a fresh COMP and stayed
  False after adding a page, a parameter, a value and a save. A flag that
  always reads False is worse than no flag, so it is not reported as an
  answer. The project-level `modified` list does name the component, but that
  is the *project's* unsaved state and a component save does not save the
  project. With `path` (a COMP to load into) it loads a `.tox` via `loadTox`,
  which is genuinely non-destructive; without `path` it reports what loading
  the file as the *project* would mean and loads nothing, because
  `project.load(path)` returns `None` — there is nothing to verify against, it
  destroys the open project, and it has no undo. A genuinely fresh load is
  `spawn_td`.

Each of these has a workflow scenario in `scripts/fixtures/guidance/` that
names the route it replaces and asserts that route is refused. If you upgrade
and find yourself wanting one of the old routes back, the scenario is the
document that explains why the new one exists.

The Palette library is new surface too. **`palette_index`** lists the
component libraries (the installed TouchDesigner ones and your own) with a
cheap line per entry — id, category, summary, whether it has a card — plus
`stats` counts. **`palette_probe`** loads a component into a *scratch* COMP,
digests its interface, and destroys it; the card you write afterwards is
authored by you from that digest, because nothing generates one. Probe in a
throwaway project, never in your work: a probe is the one operation here that
runs a stranger's startup code inside your TD. `mutate_nodes`' `place` step
drops a probed component in for real.

The default probe skips a seven-entry blacklist of built-in components whose
loaders open sockets or expect absent hardware, because **one wedging
component takes its whole batch with it**. Ask for one deliberately with
`palette_probe` `includeIgnored: true` when you actually need it.

`compare_nodes` is also newly exposed as a tool (it was previously reachable
only through `execute_python`). It compares a fresh read against a retained
baseline and returns exact per-field deltas. Three rules a caller can rely on:
a **missing baseline refuses** with `baseline_unavailable` rather than
returning an empty diff, because an empty diff says everything is unchanged
when nothing was compared; an unreadable node reports `readable: false` and is
never silently unchanged; and `absent`, `null`, `false`, `0` and `""` are five
different states, with a change between any two of them reported. Every
response carries `atomic: false` and `exactlyOnce: false`. It compares a
retained baseline against a *fresh* read — the fields reported unchanged were
actually re-read, and a daemon mutation counter is never used to decide that,
because it advances on dispatch and not on effect.

Guarded writes are classified rather than lumped in with errors. A
`mutate_nodes` step may carry `expect: {fields: {...}}`; the guard is re-read
at write time, so a field that does not hold the expected value is refused
with a distinct code from a precondition that could not be read at all, and
the second fails closed. `expect` must name at least one field.

## New optional arguments

- **`capture.timing`** and **`record.timing`** — a reset signal, a
  sequential-forward schedule, warmup and initialization frame counts, and a
  completion policy. The job owns the pid until cleanup. A project **save**
  over MCP is measured to end the stream without a response while the bridge
  stays connected and `fleet` keeps serving, so it gets a specific refusal
  rather than being presented as a call that failed cleanly; a **component**
  save is unaffected and verified working over the same transport.
- **`record.frames`** — record N consecutive frames from one TOP output.
  qtrle MOV, no audio.
- **`mutate_nodes` steps** may carry an `expect` guard, re-read at write time.
  There is no whole-scene atomicity and no exactly-once guarantee: a write
  refused by a guard changes nothing, and a write that succeeds is not
  replayed. The guard narrows the race; it does not close it.
- **`mutate_nodes` `place` step** — drop a Palette component in by `paletteId`
  or `toxPath`, wired in the same batch.
- **`describe_tools`** accepts exact `names`, an `includeSchema` projection,
  `includeExamples`, and `includeRevisions` (SHA256 catalog revisions). The
  default still returns every full schema, and standard MCP discovery remains
  complete. Revisions identify the catalog; they do **not** prove which bridge
  is loaded or that the scene is fresh, and a missing name is an explicit
  error rather than a silently shorter list.

## The version 2 output contract

`output: {contract: "v2"}` on `inspect` returns a typed envelope instead of
the legacy shape. Two things to know before opting in:

- **Omission never enables compact output.** `view` omitted requests the full
  representation. `view: "compact"` asks for the compact form and falls back
  to full when it would not be smaller or is not supported — a fallback is
  reported, never silent.
- **`retain: true` requires a reader.** The producer refuses retention when no
  permitted reader route exists, rather than retaining bytes nothing can
  fetch.

The compact view is lossless by construction and answers the same questions
(see [Testing](TESTING.md#the-four-scorecards-reported-separately)), but it is
strictly a per-request opt-in. No existing call changes.

## What a result now says that it used not to say

These are additions to a response, not changes to one. They exist because an
unlabelled value reads as a fact.

- **Any result carrying an image** gains an `admission` block with four
  separate numbers: `generated` (observed bridge-side), `emitted`,
  `admitted` (observed server-side, with the `maxImageBytes` that decided it),
  and `includedInModelInput` — which is **`unavailable`**, always. The server
  cannot observe what a client's model was given. "The server sent an image" is
  not evidence that one was inspected.
- **A withheld image is never dropped.** Above
  `[bridge] admission_max_image_bytes` (default 8 MiB of *encoded* image) the
  attachment is withheld and the bytes stay in
  `structuredContent.imageBase64`, with the verdict in the block above.
- **A capture over 256 KiB of text** no longer repeats `structuredContent`
  verbatim in `content[]`. It is replaced by a short pointer naming the field
  and the size — never by an empty content array. A client that only renders
  `content[]` should read `structuredContent`.
- **A capture reports its own geometry**: source dimensions, delivered
  dimensions, `transformed`, and the `captureRoute`. Downscaling averages, so
  a larger source can produce a *smaller* image; the delivered dimensions are
  what the cost depends on.
- **Receipts** exist for real bridge dispatches, and a client can read back
  its own and account for unobserved work. A daemon's mutation counter is not
  evidence that the scene is unchanged.

## New configuration

Every key is commented out in the shipped `default.toml` and has a default,
so no edit is required. Set one only if the default does not fit.

| Key | Default | What it does |
| --- | --- | --- |
| `[bridge] admission_max_image_bytes` | `8388608` | Bytes of encoded image admitted into a result's attachment list. `0` is refused. |
| `[palette] user_root`, `store_dir`, `ignore` | auto / data dir / 7 id globs | Palette component library. `ignore` is a **probe blacklist**: a blacklisted component's startup scripts run during a probe. |
| Wine keys | auto-detected | `wine_exe`, `wine_prefix`. Linux only; set only if auto-detection misses. |

`ignore` already ships the seven-entry blacklist described above. A blacklisted
entry stays out of an index scan entirely, and asking for it by exact id is
how you get past the blacklist.

Dashboard saves now apply federation, call timeouts and keep-alive
immediately; listener, process and manual-edit settings still need a daemon
restart. That split is a change from v0.1.4, where every change needed a
restart.

## Evidence and retention

A crash-journaled evidence store retains captured projections, scoped to the
client that asked. Only `inspect` captures today; there is no reader route
exposed to a caller and no UI to find a retained observation. If you were
expecting a per-call receipt for another tool, it is not there yet — see
[Open work](OPEN_WORK.md).

Separately, every real bridge dispatch gets a **dispatch receipt**, readable at
`GET /admin/operations` and resolvable at `POST /admin/operations/resolve`.
Both are PSK-authenticated and loopback-only, and the list is scoped to the
caller's `x-tdmcp-client` principal. A dispatch whose effect the daemon cannot
observe stays `unknown_effect` — the caller records what it saw
(`observedBy: "caller"`), and the receipt's `state` and `effect` are **not**
changed — `unknown_effect` stays unknown, because a caller's assertion is not
an observation. Resolving an already-observed receipt is a conflict, and
nothing is re-sent. This is the route that makes "the daemon's counter did not
move" and "the scene did not change" separate statements. A daemon that
cannot open a journal answers both routes `503` with a reason rather than
serving an empty list that reads as "nothing happened".

## What this release does not claim

- No model token A/B significance, and no agent task-acceptance measurement.
  The compact codec is measured for **token count under named tokenizers** and
  for **decision fidelity against known ground truth**; a model was not
  benchmarked.
- No native Windows or macOS qualification from a Linux host. Those builds
  run in CI; see [Packaging and releases](DELIVERY.md).
- **These installers are unsigned.** Windows publisher signing and macOS
  Developer ID notarization require credentials this build does not have, so
  the installers you get are unsigned and nothing here should be read as
  saying otherwise.
- The admission default is frozen as "does not withhold a capture anyone has
  taken" — a floor on nine measured PNG captures, not a tuned value for HDR,
  EXR or photographic content.
- TouchDesigner under Wine is **not** a qualification target. The live
  acceptance on the build host ran against TouchDesigner 2025.32460 under Wine
  and passed — handshake, inspect, create, guarded write, capture and graceful
  kill — but that is a convenience for the tests, not a support statement.
  Native Windows and macOS remain unqualified here, and the Wine path depends
  on a `wine_exe` launcher whose graphics and licensing setup is local to this
  machine.

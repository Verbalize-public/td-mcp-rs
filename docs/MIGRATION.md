# Migrating to v0.2.0

Upgrading from v0.1.4? Install the new release, reconnect your assistant, and
check the loaded bridge before trying the new tools. Configuration and a
customized `template.toe` are preserved. See [Installation](INSTALL.md#updates)
for the update procedure; `install --force` refreshes managed assets for a
same-version build without resetting settings.

For a new installation, use the [quick start](QUICKSTART.md).

## Does anything I depend on change?

| Existing setup | Upgrade behavior |
| --- | --- |
| Tool names and existing arguments | Retained; new tools and optional fields are added |
| `inspect` without an `output` option | Keeps the full legacy response shape |
| Existing `config.toml` | Preserved; added settings have defaults |
| Customized starter project | Preserved |
| Concurrent calls to one TD process | Still subject to session/PID gates; timed jobs reserve the process across calls |

Treat `tdmcp.mcp.session_busy`, `tdmcp.bridge.queue_busy`, and
`tdmcp.timing.busy` as different contention reports. A timed job allows its
status/cancel calls while reserving TD transport until cleanup. Fleet remains
available; its task list is not a timed-job progress display.

## New tools

| Tool | Use it for |
| --- | --- |
| `sample_data` | TOP pixel values, CHOP samples, and SOP point positions, with explicit shape and availability reporting |
| `script_source` | Register Python source once, then run it by content ID with dependency checks |
| `param_interface` | Export a COMP's control surface or apply one additively |
| `project_snapshot` | Save a component with a file receipt, inspect unsaved state, or load a component |
| `compare_nodes` | Compare a fresh read of selected fields with a captured baseline |
| `palette_index` / `palette_probe` | Catalogue component libraries and inspect components in a scratch TD container |
| `record` | Record consecutive frames from an explicit TOP output |

Use `describe_tools` for exact current schemas and the
[recipes](RECIPES.md) for examples of what to ask.

### Numerical reads and reusable code

A PNG preview cannot establish a float value or HDR range. `sample_data`
reports the source format and distinguishes exact reads from summaries.
TOP pixels are requested as `[x, y]`; arrays report their ordering. CHOP reads
use the per-sample indexer by default. The recorded `noiseCHOP` observation on
TD 2025.32460 was channel-major (`[1, 600]`); shape checks guard the optional
array route. Non-finite values need explicit reporting because JSON has no NaN.

`script_source` registration never runs code. Running a source uses the Python
execution path and checks declared dependencies again. A dependency that now
resolves to a different node is reported. Re-registering a source reports
which dependencies were not applied. Deleting a source entry does not undo
an earlier execution.

### Interfaces and files

`param_interface` adds missing controls and removes nothing. Incompatible
existing types are skipped with a reason; outcomes are reported per entry.
This supports reviewing a panel interface separately from its current values.

`project_snapshot` component receipts report the actual written path and
SHA-256, separately from the requested target. Existing destinations are
refused by default; replacing one must be explicit. A component's unsaved
state is reported unavailable rather than inferred from an unreliable dirty
flag. Project unsaved state and component saves are different things.

Whole-project save is exposed by the implementation, but recorded live work
found that it could end the MCP stream without a response. Do not treat a
missing reply as a refused or cancelled save. For everyday use, save `.toe`
files through TD's UI and check any destination before retrying a tool save.

A load request with a host COMP path loads a `.tox` into that COMP. Without
that path, it reports the implications of replacing the project and loads
nothing. Use `spawn_td` to open a separate project in a fresh process.

### Comparisons, guards, and Palette probes

`compare_nodes` refuses a missing baseline instead of returning an empty diff.
Unreadable nodes are marked unreadable. Absent fields, `null`, `false`, `0`,
and `""` remain distinct. Results carry `atomic: false` and
`exactlyOnce: false`: fields can change between reading and writing.

A `mutate_nodes` step can carry `expect: {fields: {...}}`. The expectation
must name at least one field and is checked at write time. A mismatch and an
unreadable precondition have different diagnostics; both refuse the guarded
write. This narrows a race without making a batch atomic.

Palette probes load real components, including their startup code. Use a
scratch project. The default blacklist skips known troublesome components;
explicit IDs or `includeIgnored: true` can override it. Catalogue presence
is not permission to load an ignored component. The assistant authors cards
from probe results; `mutate_nodes`' `place` step places a component for use.

## New optional arguments

- `capture.timing` and `record.timing`: reset, initialization/warmup, sequential
  stepping, and completion policy. See the [timing contract](CONTRACT.md#timing-metadata-and-deferred-capturerecord-jobs).
- `record.frames`: consecutive frame count; current output is qtrle MOV without audio.
- `mutate_nodes` step `expect`: guarded writes; `place`: Palette or `.tox` placement.
- `describe_tools`: exact `names`, `includeSchema`, `includeExamples`, and
  `includeRevisions`. Revisions identify catalogue content, not the loaded
  bridge or current scene. Missing names produce an explicit error.

## The version 2 output contract

`output: {contract: "v2"}` on `inspect` requests a typed envelope. Compact
output is a separate opt-in: omit `view` for full output, or use
`view: "compact"`. If compact output is unsupported or no smaller, the
response reports a fallback to full.

Retention is also explicit. Check its reported outcome and the
[evidence limits below](#evidence-and-retention); requesting retention does
not establish that a caller can later retrieve it through a public tool.

The compact codec is tested for fidelity against known observations. This
measures documents, not model quality or task success. See
[Testing](TESTING.md#the-four-scorecards-reported-separately).

## What a result now says that it used not to say

- Image admission distinguishes generated, emitted, and admitted content.
  `includedInModelInput` is always `unavailable`: the server cannot know what
  the assistant's model received or inspected.
- Above the configured image limit, an attachment is withheld and reported;
  the encoded bytes remain in `structuredContent.imageBase64`.
- Capture text above 256 KiB uses a short pointer instead of duplicating the
  structured payload in `content[]`. Clients must read `structuredContent`.
- Capture geometry identifies source and delivered dimensions, transformation,
  and capture route.
- Dispatch receipts record real bridge dispatches. A mutation counter tracks
  dispatch, not proof that the scene changed or stayed unchanged.

## New configuration

| Setting | Default | Purpose |
| --- | --- | --- |
| `bridge.admission_max_image_bytes` | `8388608` | Maximum encoded image bytes admitted as an attachment; zero is refused |
| `palette.user_root`, `store_dir`, `ignore` | Auto/data directory/built-in exclusions | Component collection and probe exclusions |
| `official_tools.wine_exe`, `wine_prefix` | Auto-detected | Linux Wine runner and prefix |

Ignored Palette entries can remain indexed; default probing excludes them.
Dashboard saves apply federation, call timeouts, and keep-alive immediately.
Listener/process changes and manual file edits require restart. See
[Configuration](CONFIG.md#when-changes-take-effect).

## Evidence and retention

The evidence store retains `inspect` projections with owner scoping, quotas,
and leases. Store-layer reads are tested, but there is no caller-facing paging
tool/HTTP reader or UI for finding a retained observation. This is separate
from dispatch receipts. See [Current limitations](OPEN_WORK.md).

Dispatch receipts are available through `GET /admin/operations` and
`POST /admin/operations/resolve`, with loopback/authentication restrictions and
scope derived from `x-tdmcp-client`. Recording the caller's account of an
unknown effect does not turn it into a daemon observation: `unknown_effect`
stays unknown. Resolution never resends work. Already-observed receipts
conflict, and an unavailable journal returns `503` with a reason.

Receipts whose callers stop waiting stay `wait_expired` with an `unknown`
effect. They release their admission slots immediately and use the same bounded
history retention as terminal receipts (24 hours by default, also subject to
row limits). This does not cancel, retry, or establish the effect of the work.
Only active `admitted` / `dispatched` calls consume the 64-per-owner and
512-global admission limits. Reading or resolving a receipt does not stop work.
Clients without `x-tdmcp-client` share the `local:anonymous` principal; send a
stable client claim to keep task histories and admission windows separate.

The operations list retains `count` as the number of rows in the returned page
and exposes the same value as `pageCount`. `totalCount`, `inFlight`, and
`unresolved` count the owner's entire retained history, independent of the page
limit. Unresolved includes dispatched, wait-expired, and unknown-effect receipts
without a caller resolution. Resolving a receipt records the caller's account
and removes it from `unresolved`, while its state and effect stay unchanged.

## What this release does not claim

- Document/token measurements do not establish model token A/B significance
  or agent task-acceptance rates.
- Nine recorded PNG captures fit the default admission bound; that sample
  does not qualify HDR/EXR, photographic content, or every assistant client.
- Recorded live acceptance on TD 2025.32460 under Wine covers the measured
  scenarios, not every runner/GPU or native Windows/macOS behavior. A later
  launch failure on that host remained an unresolved environment issue.
- Windows publisher signing and macOS Developer ID/notarization require
  publisher credentials. Ad-hoc signing is not notarization. Check the actual
  release artifacts and [packaging procedure](DELIVERY.md).

Current behavior and known gaps are maintained in [Contract](CONTRACT.md) and
[Current limitations](OPEN_WORK.md); historical observations are not a promise
that a live TD session can be reproduced on any machine.

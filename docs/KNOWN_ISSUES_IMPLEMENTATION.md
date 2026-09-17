# Known-issues refactor — implementation ledger

Implemented from [the audited plan](KNOWN_ISSUES_REFACTOR_PLAN.md), with
[scoped live validation](KNOWN_ISSUES_LIVE_VALIDATION.md). Original field notes
remain preserved in [KNOWN_ISSUES.md](KNOWN_ISSUES.md).

**Status: implementation and live acceptance completed on the available TD
2025.32460 Wine/Linux installation.** This is not a claim that every historical
2025.33070 symptom or every platform has been reproduced or fixed upstream.

## Execution route

The initial harness profile could not provide TD operations. After the user
explicitly authorized manual stdio, `scripts/mcp_stdio_probe.py` drove the real MCP
initialize/notification/tools-call protocol through the installed executable.
No harness-profile modification was needed. The client owns only its proxy
subprocess, never retries a timed-out mutation, and preserves full tool results.
`scripts/live_evidence.py` records exact requests/results and extracts images.

## Implemented changes

- Fixed root argument spans (`..pid` → `pid`), with capture target validation and
  real MCP transport regressions. Valid numeric/string PIDs also captured live.
- Added explicit occupied-input protection and summary connector observations.
  Default replacement semantics remain compatible, with warnings. Identical safe
  connections are no-ops using native owner-path/output-index identity, not Python
  wrapper equality. Older bridges reject the private checked-connect operation
  rather than silently ignoring the safety policy.
- Extended create/place aliases to descendants using longest path-segment
  matching; fixed context, partial-apply and skipped-step behavior are retained.
- Added bounded menu/stored-value metadata and evaluation availability. Invalid
  built-in closed `Menu` tokens/indices are rejected before assignment; custom,
  source-driven, StrMenu, unreadable and oversized menus remain conservatively
  unvalidated. No auto-case correction or whole-batch rollback is promised.
- Replaced false-positive shader success with affirmative bounded evidence.
  Empty, foreign, incomplete and truncated logs are not success.
- Discovered and implemented the actual GLSL POP compile-log surface: an existing
  bound non-passive general Info DAT. Logs include source DAT paths and lines.
  Missing/unverified observers remain unsupported; the bridge creates no nodes
  during reads. Direct TOP/MAT compileResult support is retained.
- Preserved availability instead of presenting failed reads as empty/null/clean.
  Compile observation runs before requested operator messages so fresh compiler
  results are not paired with stale earlier errors.
- Fixed MCP promotion of new mutation codes and preservation of advertised
  shaderErrors/shaderNotes on both success and partial failure.
- Corrected operating guidance and capture vocabulary, added schema-backed
  vocabulary/fixture tests, and regenerated checked-in skills while preserving
  the pre-existing concurrent edits.

## Plan disposition

| Stage | Outcome |
| --- | --- |
| P0 evidence | Source baseline, exact live requests/results, images, native module identity and artifacts recorded. Original private incident project/captures were not imported. |
| P1 capture/diagnostics | Unit, real-transport and native TD acceptance passed, including malformed arguments and shader failures/recovery. |
| P2 mutations | Safe rejection, intentional replacement, native no-op identity, descendant collision handling and fixed context validated. |
| P3 parameters | Built-in closed-menu enforcement and open-menu preservation validated live. Generic GLSL parsing and speculative mixed-input rules were rejected in favor of real compiler evidence. |
| P4 TD matrix | D1–D10 investigated with bounded experiments. Results are build/configuration-scoped; unsupported generalizations were not turned into runtime restrictions. |
| P5 guidance/fixtures | Native render, feedback, trail and trigger fixtures, working Reset/Intensity/Scale controls, owner-relative references, exported tox and independently loaded project delivered. No inert historical Trail control was copied. |
| P6 deployment | Daemon rebuilt/force-installed/restarted, assets refreshed, native bridge behavior exercised, exported component loaded in a blank project, saved project reopened in a fresh process. |

The original T3 duplication and blanket primitive/alpha/temporal claims did not
justify the proposed restrictions on the tested build. The live report records
what did and did not reproduce. Noise normalization was disabled by its current
constraint mode; active constrained normalization is not generalized from that
measurement. No unmeasured performance claim is made.

## Verification and assets

- Final full locked Rust workspace tests, clippy with warnings denied and format
  checks passed. Source edits include meaningful success and failure regressions.
- Bridge suite: **458 passed, 29 subtests passed**.
- Script suite: **11 passed**. Schema/catalog completeness, fixture argument
  shapes, generated skill parity and documentation links passed.
- Package smoke checks repeat installation, preserved config, assets/resources,
  real MCP startup and clean shutdown in an isolated directory.
- No bootstrap Python source changed. Its tox drift guard passed. No bootstrap
  repack or false hash stamp was performed; the installer refreshed managed
  bridge/catalog/skills/bootstrap copies. The regression fixture tox was genuinely
  saved by TD.
- Local artifacts and evidence live under
  `td-sandbox/known-issues-refactor.x5LYaP/`; final hashes are in its manifest.
  The directory is ignored, not published.
- No commit or release was created. Existing user changes were preserved.

## Runtime left for the user

Owned audit PID 378329 and blank-load PID 500781 were saved and gracefully closed.
The saved project was reopened as PID **507693**, with fixture root
`/project1/loaded_fixture`, and is left **paused**. Its GLSL POP reports compiled
through the verified Info DAT, recursive errors/warnings are empty, and reset
then stepping produced viewed geometry with repeatable point counts.

TD reported the reopened project name `verified_roundtrip.1.toe`; the request
used the saved base filename. Both actual saved filenames are retained, rather
than conflating the requested path with the reported project identity. The
original incident build 2025.33070 and native Windows/macOS remain unverified.

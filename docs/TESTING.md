# Testing

Automated tests do not require TouchDesigner. Live verification is separate;
a passing build is not evidence that a TD network works.

## Local gate

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
python -m pytest bridge/tests
```

Install test dependencies in an activated virtual environment with
`python -m pip install -r scripts/requirements-test.txt`. `scripts/check.sh` and
`scripts/check.ps1` wrap the local checks. The JSON Schema validator is test-only;
no project script is executed by documentation validation.

## Where tests live

| Layer | Location | Coverage |
| --- | --- | --- |
| Rust unit tests | Each crate | Config validation, queues, diagnostics, rendering, project I/O |
| Daemon integration | `crates/tdmcp-daemon/tests/` | Real listeners/processes with fake TD peers |
| Python bridge | `bridge/tests/` | TD dispatch and result shaping with fake operators |
| Package smoke | `scripts/package_smoke.py` | Repeat installation, config preservation, embedded assets, real MCP, shutdown |
| Live TD | [Dev environment](DEV_ENV.md), [E2E checklist](E2E_CHECKLIST.md) | Actual TD behavior and rendered output |

The concurrency, multi-client, stdio recovery, federation, and restart tests
protect against hangs, PID reuse, queue starvation, stale sessions, and
listener races. Keep these behavioral tests when refactoring internals.
Tests must use temporary paths and injected discovery; never depend on or
modify the developer's real Palette or projects.
Child-process tests should use `tdmcp_test_support::unique_test_port` and verify
the responding PID during readiness. A successful health response alone can
come from another test's daemon after a port-selection race.

Against a scratch TD PID, [live federation smoke](../scripts/live_federation_smoke.py)
checks live settings, rejected keys, remote calls and images.
[Live bridge limits](../scripts/live_bridge_limits_smoke.py) checks oversized
and invalid replies without disconnecting. Both accept `PID /path/to/output`,
remove their temporary operators, and must not target a production session.

## Asset drift checks

Changing skills requires rendering `claude-skills/` in the same change.
Changing either bootstrap Python source requires repacking the embedded tox.
The installer tests check both; do not bypass them:

```sh
cargo run -p tdmcp-daemon -- skills render --dest claude-skills
git add claude-skills
```

See [skill authoring](../skills/README.md) and
[bootstrap packing](../scripts/pack_bootstrap_tox.md).

## Release measurements

Two offline measurements read the document pairs `TDMCP_TOKEN_DUMP` writes from
`crates/tdmcp-mcp/tests/inspect_corpus_replay.rs`. They answer different
questions and are not substitutes.

```sh
TDMCP_TOKEN_DUMP=/tmp/dump.json \
  cargo test --offline --locked -p tdmcp-mcp --test inspect_corpus_replay

.venv/bin/python scripts/measure_tokens.py /tmp/dump.json               # COST
.venv/bin/python scripts/measure_decision_fidelity.py /tmp/dump.json   # FIDELITY
```

`measure_tokens.py` reports the saving in **tokens**, naming and proving the
tokenizer. `measure_decision_fidelity.py` asks whether the compact document
still answers the questions the full one answered — whether a factored-out
default is recoverable rather than reading as absent, whether `null`/`false`/
`0`/`""` survive, whether a per-node failure stays visible. Both refuse to
report a number they did not earn: the first when an encoding will not prove
itself, the second when there is no compact document, when the dump's rows
disagree with each other, or when the codec version is not one it was written
against.

A smaller representation that increases mistakes does not ship by default, so
the fidelity script's exit code is a **release gate**, not a report. Neither
script measures model behaviour; both measure documents, and neither is
evidence about what any particular model would conclude from them.

The replay writes its dump for the public fixture set (`scripts/fixtures/inspect/`)
as well as for a recorded corpus, so a reviewer without the private recorded
payloads can still reproduce every figure. Point `TDMCP_INSPECT_CORPUS` at a
recorded directory to repeat the same measurement over real data.

### Workflow value, measured apart from encoding

A tool that costs schema bytes to make available is not paid for by a smaller
response document. `scripts/measure_workflow_value.py` keeps the two apart by
charging each tool to the sessions that would have to carry its schema, and it
reports four separate scorecards — `numeric`, `source`, `interface`,
`checkpoint` — rather than one blended row.

```sh
.venv/bin/python scripts/measure_workflow_value.py
```

Each scorecard answers two different questions and keeps them apart. It is
**measured** only when a scenario *requires* the tool, the oracle was
*executed* to an accepted session, and the route the workflow replaces was
*refused* — sufficiency and necessity are separate claims, and a tool whose
old route still works has demonstrated neither use. The cost half reports
`schemaBytesAddedPerEligibleSession`, the price of making the tool reachable in
the sessions that do not already allow it, over `eligibleSessions`. A workflow
with no such scenario is **unmeasured**, and the script exits non-zero saying
so: unknown is not zero, and four empty scorecards print as four rows of zeros.

Every figure comes from playing the fixtures through `guidance_eval.py`, which
is a deterministic mock that never connects to TouchDesigner. It measures
whether the declared route is sufficient and the replaced route is closed, not
what a model would do. `replacedRoutesProbed` is the evidence;
`replacedRouteRefused` is a summary of it.

### A measured default, and what the measurement is not

`scripts/fixtures/admission/live-capture-bytes.json` records what real captures
actually cost: nine PNG captures taken from a task-owned TD over a resolution
ladder, each returned its true encoded size. The default admission bound
admits all nine (largest 440,234 bytes, ~5% of the 8 MiB bound), and
`admission::tests::the_default_bound_admits_every_capture_we_have_measured`
fails if the bound ever tightens below a capture someone has taken.

Read the fixture's `note` before citing the number. It is one noise source in
PNG: it says nothing about HDR/EXR, photographic content, or what a particular
client's model will accept. It is a floor on the evidence, not a distribution,
and it is published so the default can be argued against numbers rather than
against recollection.

## Release and CI

CI is manual on ordinary branches, with optional native Windows/macOS tests.
A weekly dependency check catches advisories without rebuilding every platform.
Install cargo-deny and run `python scripts/check_dependencies.py` to reproduce
it locally. Each release target is checked separately: a merged target graph
can include mutually exclusive dependencies that no shipped binary uses.
Every release runs the Linux quality gate, native platform tests, packaging,
and the isolated package smoke on each archive.

Run the package smoke locally:

```sh
python scripts/package_smoke.py target/debug/tdmcp-daemon
```

GUI fixtures (`cargo run -p tdmcp-gui --features preview --example dashboard_preview`)
help inspect layout, but do not replace clicking through the running dashboard.

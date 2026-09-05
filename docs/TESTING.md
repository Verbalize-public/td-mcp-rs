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

Install pytest in a virtual environment. `scripts/check.sh` and
`scripts/check.ps1` wrap the local checks.

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

# Dev fixtures (live TD harness)

Interactive dual-MCP harness for testing the TouchDesigner side of td-mcp-rs.
Agent runbook: [`docs/DEV_ENV.md`](../../docs/DEV_ENV.md). Pack recipe:
[`scripts/pack_e2e_kit.md`](../../scripts/pack_e2e_kit.md).

## Contents

| Path | Role |
| --- | --- |
| `e2e_kit.tox` | **Committed** baseline COMP (`e2e_kit`): non-black `probe` TOP → `out1`, empty `zone` shell |
| `session/` | **Gitignored** interactive snapshots (`latest.tox` + `latest.json`) |

Bootstrap (`tdmcp_rs`) is **not** baked into the kit — drop
`%LOCALAPPDATA%/tdmcp-rs/bootstrap.tox` (or data-dir equivalent) separately so
bridge updates do not require re-packing the kit.

## Regenerate baseline

1. Owned host (never lab `:9981`) — see `DEV_ENV.md`.
2. Run the script in `scripts/pack_e2e_kit.md` via classic TD MCP
   `execute_python_script`.
3. Commit the new `e2e_kit.tox` only when intentionally refreshing the baseline.

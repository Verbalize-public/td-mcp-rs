# Live verify audit — serde skip-flags + inspect child-roster omit

**Date:** 2026-07-31  
**Surface:** Cursor MCP `user-tdmcp-rs` → `tdmcp-daemon` + live TD  
**Pid:** `15448` (`_agent_tdmcprs_dev.4.toe`)  
**Scratch zone:** `/project1/serde_inspect_audit`  
**Unit gate:** `cargo test --workspace` + `cargo clippy --workspace -D warnings` + `pytest bridge/tests/test_inspect_summary.py` green before live.

## Summary table

| # | Tool call | Expected | Observed | Verdict |
| --- | --- | --- | --- | --- |
| 1 | `fleet` `{}` | Healthy process; omit `windowStatus` / `tasks` / `resurrected` / `lastDisconnectAt` / `cancelledTasks`; no null leaks | `pid`/`bridge`/`title`/`toePath` only | **pass** |
| 2 | `fleet` `{include:["tasks","cancelled"]}` | Observe empty-section shape | `tasks: []` present; `cancelledTasks` absent | **inconsistent** (documented asymmetry, not a contract break) |
| 3 | `mutate_nodes` create zone + 2 `nullTOP` | `ok`, applied 3 | applied 3, all step ok | **pass** |
| 4 | `inspect` zone `include:[]` | Roster present, `childCount`/`childrenReturned`=2 | Matches | **pass** |
| 5 | `inspect` zone `include:["errors"]` | `errors` present; no child roster keys | `errors:[]`; no `children`/`childCount`/`childrenReturned` | **pass** |
| 6 | `inspect` zone `include:["warnings"]` | `warnings` present; no child roster keys | Matches | **pass** |
| 7 | `inspect` zone `include:["nodes"]` | Roster present; no errors/warnings | Matches | **pass** |
| 8 | `inspect` node with structural message `include:["warnings"]` (see note) | Non-empty section; no child roster keys | Non-empty `warnings`; no roster keys | **pass** (adapted: TD emits **warnings**, not `errors`, for invalid select / missing movie) |
| 9 | Handshake / `ProcessAttrs` / `ProcessFingerprint` omit | Live-unobservable | Unit-only | **n/a (coverage gap)** |

## Concrete errors

None. No contract violations on the live path for Part B (inspect roster omit) or for `FleetProcess` omission on the default `fleet` call.

## Inconsistencies

1. **`fleet` requested-empty `tasks` vs omitted `cancelledTasks`** — **resolved**  
   Idle + `include: ["tasks"]` now omits `tasks` (same omit-empty rule as `cancelledTasks`). See CONTRACT `fleet` / `include`.

2. **Default `inspect` still emits empty `errors` / `warnings` arrays**  
   `include: []` → `"errors": []`, `"warnings": []`. Roster omission when not requested uses key absence; default include keeps empty arrays for errors/warnings. Intentional (sections *are* loaded), but different from “omit empty” serde style on Rust structs.

3. **TD structural messages often land in `warnings`, not `errors`**  
   Invalid `selectTOP` path and missing `moviefileinTOP` file both produced non-empty `warnings` and empty `errors` after force-cook. Check 8 could not get a non-empty `errors` array with these fixtures; omission still verified on non-empty `warnings`.

4. **Coverage gap (Part A IPC types)**  
   `HandshakeResponse.minDaemon`, `ProcessAttrs`, `ProcessFingerprint` never appear on MCP tool or `/admin/*` JSON. Live verify cannot prove those `skip_serializing_if` fixes; they remain unit/compile-time hygiene.

5. **`CatalogEntry` serialize skip**  
   Same as (4): YAML load → copy into `DiagnosticItem` (already correctly annotated). Not live-auditable via tools.

## Axes of improvement

1. **Align `fleet` include empty-section policy** — **done** (omit-empty `tasks`).
2. **Admin/debug handshake dump** — optional `/admin/handshake` or last-handshake echo would make `minDaemon` omission live-auditable.
3. **`include` typo feedback** — **clarified**: MCP serde already rejects unknown enum variants (unit-tested); silent ignore only applies to direct Python-bridge bypass.
4. **Same-version bridge extract** — **done**: `tdmcp-daemon install --force` / `ensure --force`.
5. **DoD fixtures for `errors` vs `warnings`** — documented in CONTRACT; many “broken” setups are warnings-only in current TD.

## Raw responses

### Check 1 — `fleet` `{}`

```json
{"processes":[{"bridge":"connected","pid":15448,"title":"_agent_tdmcprs_dev.4.toe","toePath":"C:/Users/corbe/Documents/Derivative/Projects/td-sandbox/toe/_agent_tdmcprs_dev\\_agent_tdmcprs_dev.4.toe"}]}
```

### Check 2 — `fleet` `{include:["tasks","cancelled"]}`

```json
{"processes":[{"bridge":"connected","pid":15448,"tasks":[],"title":"_agent_tdmcprs_dev.4.toe","toePath":"C:/Users/corbe/Documents/Derivative/Projects/td-sandbox/toe/_agent_tdmcprs_dev\\_agent_tdmcprs_dev.4.toe"}]}
```

### Check 3 — `mutate_nodes` create scratch

```json
{"applied":3,"failedAt":null,"ok":true,"steps":[{"ok":true,"path":"/project1/serde_inspect_audit"},{"ok":true,"path":"/project1/serde_inspect_audit/null_a"},{"ok":true,"path":"/project1/serde_inspect_audit/null_b"}]}
```

### Check 4 — `inspect` `include:[]`

```json
{"node":{"childCount":2,"children":[{"name":"null_a","opType":"nullTOP"},{"name":"null_b","opType":"nullTOP"}],"childrenReturned":2,"errors":[],"family":"COMP","opType":"baseCOMP","path":"/project1/serde_inspect_audit","warnings":[]},"ok":true}
```

### Check 5 — `inspect` `include:["errors"]`

```json
{"node":{"errors":[],"family":"COMP","opType":"baseCOMP","path":"/project1/serde_inspect_audit"},"ok":true}
```

### Check 6 — `inspect` `include:["warnings"]`

```json
{"node":{"family":"COMP","opType":"baseCOMP","path":"/project1/serde_inspect_audit","warnings":[]},"ok":true}
```

### Check 7 — `inspect` `include:["nodes"]`

```json
{"node":{"childCount":2,"children":[{"name":"null_a","opType":"nullTOP"},{"name":"null_b","opType":"nullTOP"}],"childrenReturned":2,"family":"COMP","opType":"baseCOMP","path":"/project1/serde_inspect_audit"},"ok":true}
```

### Check 8 — structural message, no roster keys

`selectTOP` with invalid `top` path — `include:["warnings"]`:

```json
{"node":{"family":"TOP","opType":"selectTOP","path":"/project1/serde_inspect_audit/bad_sel","warnings":["Warning: Invalid path for node \"/project1/serde_inspect_audit/does_not_exist\" referenced by parameter \"TOP\" (/project1/serde_inspect_audit/bad_sel)"]},"ok":true}
```

Attempted `moviefileinTOP` missing file with `include:["errors"]` still returned `errors:[]` (TD warning only after cook). Supporting probe:

```json
{"logs":"","ok":true,"result":{"errors":"","warnings":"Warning: Failed to open file. (/project1/serde_inspect_audit/bad_movie)"}}
```

### Check 9 — coverage gap

Not exercised live. Fixed in:

- `crates/tdmcp-ipc/src/handshake.rs`
- `crates/tdmcp-core/src/registry.rs`
- `crates/tdmcp-core/src/fingerprint.rs`
- `crates/tdmcp-diagnostics/src/catalog.rs`

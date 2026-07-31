# Live verify audit — fleet omit-empty + install --force + include/empty-errors

**Date:** 2026-07-31 (~22:26)  
**Surface:** Cursor MCP `user-tdmcp-rs` + HTTP `:9860` + CLI `target/dist/tdmcp-daemon.exe`  
**Pid:** `15448` (`_agent_tdmcprs_dev.4.toe`)  
**Daemon:** health ok, admin `version: 0.1.0`, dist mtime `22:18:18`, process start ~`22:21`  
**Scratch zone:** `/project1/serde_inspect_audit` (from prior session)

## Summary table

| # | Call | Expected | Observed | Verdict |
| --- | --- | --- | --- | --- |
| L1 | `fleet` `{}` | No `tasks` / `cancelledTasks` / `resurrected` / `windowStatus` | Only `pid`, `bridge`, `title`, `toePath` | **pass** |
| L2 | `fleet` `{include:["tasks","cancelled"]}` idle | Omit empty `tasks` + empty `cancelledTasks` | Same shape as L1 (no those keys) | **pass** |
| L3 | `fleet` `{include:["typo"]}` | Reject unknown include | `-32602` unknown variant (was wrapped as `-32603`; **fixed** via stdio proxy passthrough) | **pass** |
| L4 | `inspect` `{include:["typo"]}` | Reject | Same pattern; expected enum listed | **pass** |
| L5 | `inspect` zone `include:[]` | Roster + empty `errors`/`warnings` | `childCount:4`, `errors:[]`, `warnings:[]`, `ok:true` | **pass** |
| L6 | `inspect` zone `include:["errors"]` | `errors` present; no roster keys | Matches; tool success with `errors:[]` | **pass** |
| L7 | `inspect` zone `include:["warnings"]` | `warnings` present; no roster | Matches | **pass** |
| L8 | `inspect` zone `include:["nodes"]` | Roster; no errors/warnings | Matches | **pass** |
| L9 | `inspect` `bad_sel` `include:["warnings"]` | Non-empty warnings; no roster | Warning string present; no children keys | **pass** |
| L10 | `inspect` `null_a` `include:["params"]` | Params; no roster | Matches | **pass** |
| L11 | `fleet` `{include:["cancelled"]}` | No-op gate; omit empty stack | Identity-only row (no `cancelledTasks`) | **pass** / noted |
| L12 | CLI `install --force` | Re-extract + stamp rewrite; daemon stays healthy | `installed 0.1.0 → …`; bridge mtime updated; health still ok | **pass** |
| L13 | `execute_python` short | Success | `{ok:true, result:"ok"}` | **pass** |
| — | Cursor `GetMcpTools` fleet schema | New include docstring | **Stale** old text (`cancelled always when non-empty`) | **inconsistent** |
| — | HTTP `GET /mcp/tools/list` fleet schema | New include docstring | **New** text (`tasks` omitted when empty) | **pass** (daemon correct) |

## Concrete errors

None that break the shipped contract for these features.

## Inconsistencies

1. **Cursor MCP schema cache vs live daemon**  
   `GetMcpTools` for `fleet.include.description` still shows the pre-change string. HTTP `/mcp/tools/list` on the same daemon shows the updated docstring. Tool **behavior** matches the new binary; agents that trust Cursor’s cached `inputSchema` text can be misled until the MCP client session is fully respawned / cache cleared.

2. **Invalid-args error code wrapping** — **resolved**  
   Stdio proxy now forwards `ServiceError::McpError` unchanged (`ErrorData::invalid_params` / `-32602`) instead of remapping every upstream error to `internal_error` (`-32603`).

3. **`FleetInclude::Cancelled` is still a no-op gate**  
   `include:["cancelled"]` does not change emission vs default when the stack is empty (and when non-empty, cancelled tasks appear regardless of include — CONTRACT documents this). The enum value looks like a section toggle but is not one.

4. **`install --force` refreshes disk, not in-TD Python**  
   Force extract succeeded while the daemon stayed up; TD may still be running the previously loaded bridge package until tox reload / re-handshake. Force alone is not a full “hot reload bridge in live TD” button.

5. **Default inspect still emits empty `errors`/`warnings`**  
   Unchanged by design (section loaded). Asymmetric vs omit-empty fleet `tasks` and vs roster omission when `nodes` excluded — documented, still surprising at a glance.

6. **Non-empty `tasks` not live-exercised this pass**  
   Idle omit-empty verified; non-empty snapshot only unit-covered (Cursor serializes tool calls, hard to observe in-flight queue without a concurrent probe).

## Axes of improvement

1. **Respawn / invalidate Cursor tool schema** after daemon binary upgrade (or document that agents should prefer `/mcp/tools/list` / `describe_tools` when schema text matters).
2. **Surface invalid `include` as clean `-32602`** — **done** (stdio proxy MCP error passthrough).
3. **Either gate `cancelled` on include or drop it from the enum** until it does something — reduce fake affordances (`popups` similarly deferred).
4. **Document / automate post-`install --force` TD bridge reload** (destroy+loadTox or re-handshake cue) in RUNBOOK/DEV_ENV.
5. **Optional live gate for non-empty `tasks`:** admin `/admin/fleet` while an exclusive script sleeps, or a tiny `fleet` integration test already covering serialize shape (unit exists — promote to checklist S-row).

## Raw responses

### L1 — `fleet` `{}`

```json
{"processes":[{"bridge":"connected","pid":15448,"title":"_agent_tdmcprs_dev.4.toe","toePath":"C:/Users/corbe/Documents/Derivative/Projects/td-sandbox/toe/_agent_tdmcprs_dev\\_agent_tdmcprs_dev.4.toe"}]}
```

### L2 — `fleet` `{include:["tasks","cancelled"]}`

```json
{"processes":[{"bridge":"connected","pid":15448,"title":"_agent_tdmcprs_dev.4.toe","toePath":"C:/Users/corbe/Documents/Derivative/Projects/td-sandbox/toe/_agent_tdmcprs_dev\\_agent_tdmcprs_dev.4.toe"}]}
```

### L3 — `fleet` `{include:["typo"]}`

```text
MCP error -32603: Mcp error: -32602: unknown variant `typo`, expected one of `process`, `bridge`, `tasks`, `cancelled`, `popups`
```

### L4 — `inspect` `{include:["typo"]}`

```text
MCP error -32603: Mcp error: -32602: unknown variant `typo`, expected one of `nodes`, `params`, `errors`, `warnings`
```

### L5 — `inspect` default

```json
{"node":{"childCount":4,"children":[{"name":"null_a","opType":"nullTOP"},{"name":"null_b","opType":"nullTOP"},{"name":"bad_sel","opType":"selectTOP"},{"name":"bad_movie","opType":"moviefileinTOP"}],"childrenReturned":4,"errors":[],"family":"COMP","opType":"baseCOMP","path":"/project1/serde_inspect_audit","warnings":[]},"ok":true}
```

### L6 — `inspect` `include:["errors"]`

```json
{"node":{"errors":[],"family":"COMP","opType":"baseCOMP","path":"/project1/serde_inspect_audit"},"ok":true}
```

### L7 — `inspect` `include:["warnings"]`

```json
{"node":{"family":"COMP","opType":"baseCOMP","path":"/project1/serde_inspect_audit","warnings":[]},"ok":true}
```

### L8 — `inspect` `include:["nodes"]`

```json
{"node":{"childCount":4,"children":[{"name":"null_a","opType":"nullTOP"},{"name":"null_b","opType":"nullTOP"},{"name":"bad_sel","opType":"selectTOP"},{"name":"bad_movie","opType":"moviefileinTOP"}],"childrenReturned":4,"family":"COMP","opType":"baseCOMP","path":"/project1/serde_inspect_audit"},"ok":true}
```

### L9 — `bad_sel` warnings-only

```json
{"node":{"family":"TOP","opType":"selectTOP","path":"/project1/serde_inspect_audit/bad_sel","warnings":["Warning: Invalid path for node \"/project1/serde_inspect_audit/does_not_exist\" referenced by parameter \"TOP\" (/project1/serde_inspect_audit/bad_sel)"]},"ok":true}
```

### L10 — `null_a` params-only (roster keys absent; params truncated in prose)

```json
{"node":{"family":"TOP","opType":"nullTOP","params":[{"mode":"CONSTANT","name":"pageindex","val":0},{"mode":"CONSTANT","name":"resolutionw","val":256}],"path":"/project1/serde_inspect_audit/null_a"},"ok":true}
```

*(Full params list returned live; shortened here for readability — no `children`/`childCount`.)*

### L11 — `fleet` `{include:["cancelled"]}`

```json
{"processes":[{"bridge":"connected","pid":15448,"title":"_agent_tdmcprs_dev.4.toe","toePath":"C:/Users/corbe/Documents/Derivative/Projects/td-sandbox/toe/_agent_tdmcprs_dev\\_agent_tdmcprs_dev.4.toe"}]}
```

### L12 — `tdmcp-daemon install --force`

```text
installed 0.1.0 → C:\Users\corbe\AppData\Local\tdmcp-rs
bridge/manifest.json LastWriteTime: 2026-07-31 22:26:29
health: {"ok":true}
```

### Schema probe

- Cursor GetMcpTools: `cancelled always when non-empty` (**stale**)  
- HTTP `/mcp/tools/list`: `` `tasks` omitted when empty; cancelled stack always when non-empty. `` (**current**)

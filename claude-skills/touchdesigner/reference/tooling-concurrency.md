# Tooling concurrency (td-mcp-rs)

Sequential bridged tools against one TD `pid` — HARD RULE for agents.

## Bridged vs exempt

| Kind | Tools | Rule |
|------|-------|------|
| **Bridged** | `execute_python`, `inspect`, `capture`, `mutate_nodes`, `api_help`, `editor_context` | At most **one** in-flight per `(mcp_session, daemon_scope, pid)` — `daemon_scope` is `local` or remote `daemonId` when federated |
| **Exempt** | `fleet`, `describe_tools` | Safe during an in-flight bridged call |

## What to do

1. Call bridged tools **one at a time**; wait for each result before the next.
2. On `tdmcp.mcp.session_busy` ("chill down") or `tdmcp.bridge.queue_busy`: wait
   for in-flight work, then **retry** — do not disconnect, restart the daemon,
   or drop the tox.

## Daemon gates (summary)

- **Session chill:** `(mcp_session_id, daemon_scope, pid)` — one in-flight bridged tool (local or proxied).
- **Federation:** pass optional `daemonId` on pid tools when the master aggregates multiple daemons; ambiguous pid → `tdmcp.federation.ambiguous_pid`.
- **Pid exclusive:** per-pid task queue rejects enqueue if non-empty (on the daemon that owns the pid)
  (`tdmcp.bridge.queue_busy`).

## Related

- Operate umbrella: [`operate`](../SKILL.md)
- Play / pause stalls: [`play-state`](./play-state.md)

---

**Canonical:** [`tooling-concurrency`](./tooling-concurrency.md)
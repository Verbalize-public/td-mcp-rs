# Tooling concurrency (td-mcp-rs)

Sequential bridged tools against one TD `pid` — HARD RULE for agents.

**Canonical:** {{ skill("tooling-concurrency") }} 

## Bridged vs exempt

| Kind | Tools | Rule |
|------|-------|------|
| **Bridged** | `execute_python`, `inspect`, `capture`, `mutate_nodes`, `api_help`, `editor_context` | At most **one** in-flight per `(mcp_session, pid)` |
| **Exempt** | `fleet`, `describe_tools` | Safe during an in-flight bridged call |

## What to do

1. Call bridged tools **one at a time**; wait for each result before the next.
2. On `tdmcp.mcp.session_busy` ("chill down") or `tdmcp.bridge.queue_busy`: wait
   for in-flight work, then **retry** — do not disconnect, restart the daemon,
   or drop the tox.

## Daemon gates (summary)

- **Session chill:** `(mcp_session_id, pid)` — one in-flight bridged tool.
- **Pid exclusive:** per-pid task queue rejects enqueue if non-empty
  (`tdmcp.bridge.queue_busy`).

## Related

- Operate umbrella: {{ skill("operate") }}
- Play / pause stalls: {{ skill("play-state") }}

# Tooling concurrency (td-mcp-rs)

Sequential bridged tools against one TD `pid` — HARD RULE for agents.

## Bridged vs exempt

| Kind | Tools | Rule |
|------|-------|------|
| **Bridged** | `execute_python`, `inspect`, `capture`, `record`, `mutate_nodes`, `api_help`, `editor_context` | At most **one** in-flight per `(mcp_session, daemon_scope, pid)` — `daemon_scope` is `local` or remote `daemonId` when federated |
| **Exempt** | `fleet`, `describe_tools` | Safe during an in-flight bridged call |

## What to do

1. Call bridged tools **one at a time**; wait for each result before the next.
2. On `tdmcp.mcp.session_busy` ("chill down") or `tdmcp.bridge.queue_busy`: wait
   for in-flight work, then **retry** — do not disconnect, restart the daemon,
   or drop the tox.
3. A timed capture/record job additionally reserves the TD transport between
   calls until cleanup. On `tdmcp.timing.busy`, use the named job's status/cancel
   actions, not other bridged tools. See {{ skill("timed-capture") }}.

## Daemon gates (summary)

Coordinate all agents using the same actual daemon/PID, even across MCP
sessions. Session gates are not ownership locks between collaborators.
Assign one caller to live inspection, mutation and Palette probing on that
process; helpers can analyze collected digests in parallel. Separate owned TD
processes permit independent live work when resources allow it. Do not assume
that launching more agents creates more TD capacity.

A timeout does not prove cancellation or absence of a side effect. Preserve
the exact request and response, check process/bridge state with `fleet`, and
reconcile the affected state once responsive before retrying a mutation.
For offline Palette writes, read the card back before retrying. If the TD main
thread appears wedged, stop dispatching live work and use the lifecycle
recovery procedure; do not restart a user's instance to clear a queue.

- **Session chill:** `(mcp_session_id, daemon_scope, pid)` — one in-flight bridged tool (local or proxied).
- **Federation:** pass optional `daemonId` on pid tools when the master aggregates multiple daemons; ambiguous pid → `tdmcp.federation.ambiguous_pid`.
- **Pid exclusive:** per-pid task queue rejects enqueue if non-empty (on the daemon that owns the pid)
  (`tdmcp.bridge.queue_busy`).

## Related

- Operate umbrella: {{ skill("operate") }}
- Play / pause stalls: {{ skill("play-state") }}

---

**Canonical:** {{ skill("tooling-concurrency") }}

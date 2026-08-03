# TODOLIST

## Tighten fuzzy near-miss suggestions (diagnostics)

`bridge/tdmcp_bridge/__init__.py` → `_suggest_names` uses `difflib.get_close_matches(..., cutoff=0.5)` over `dir(td)`. Shared family suffixes (`TOP`/`CHOP`/…) inflate ratios, so garbage inputs (`fooTOP`, `xyzTOP`, `geo`) get confident wrong `tdmcp.op.similar_type` / `tdmcp.par.similar_name` lints.

**Fix direction:** score stems (strip family), require same family when present, stem/prefix gates for bare names, length/edit budget, filter `list_op_type_names` to real op classes; keep casefold + real typos (`hsvAdjustTOP`, `noizeTOP`, `satmult`). Extend `bridge/tests/test_mutate.py`.


# Per proejct components blacklist (configurable from the bridge operator)
The goal is to give an easy way for the user to "black list" component from the mcp entierly meaning:
- It wont appear in inspect
- Mutation on it are rejected
- If the mcp use python script to edit/discover it, it will success however (stated issue)


# Large script cause the bridge to disconnect
Disconnect behavior (what triggered it)
Most likely cause: large execute_python batches during custom-page / bulk setup → bridge session dropped mid-flight.

Evidence:

Fleet cancelledTasks showed PythonEval with reason: bridge_lost (not a cook error on the pack).
TD pid 29616 stayed alive; only the in-file bridge lost the daemon handshake.
Visual TOPs had already been created but left unwired — classic “batch died mid-mutate” pattern.
Daemon itself was fine (fleet empty ≠ daemon down); reconnect revived pid 29616 on .3.toe.
Practical takeaway: prefer mutate_nodes for create/wire/set; keep execute_python small (custom pages only) so a single eval can’t take the bridge down.

# Various tools should emit lint when FPS drop
Eg: `Warning: FPS drop: 30, last healty before: unknown`, `Warning: FPS drop: 30, last healty before: "script_execute([preview script]) at [HH:MM:SS]` emited when calling inspect tool/fps are not healty


# Force gpu noise if resoluton > 64px (almost laways)

# Error on custom component are not always seen (e.g error from the parameter customisation)
# TODOLIST

## Tighten fuzzy near-miss suggestions (diagnostics)

`bridge/tdmcp_bridge/__init__.py` → `_suggest_names` uses `difflib.get_close_matches(..., cutoff=0.5)` over `dir(td)`. Shared family suffixes (`TOP`/`CHOP`/…) inflate ratios, so garbage inputs (`fooTOP`, `xyzTOP`, `geo`) get confident wrong `tdmcp.op.similar_type` / `tdmcp.par.similar_name` lints.

**Fix direction:** score stems (strip family), require same family when present, stem/prefix gates for bare names, length/edit budget, filter `list_op_type_names` to real op classes; keep casefold + real typos (`hsvAdjustTOP`, `noizeTOP`, `satmult`). Extend `bridge/tests/test_mutate.py`.


# Per proejct components blacklist (configurable from the bridge operator)
The goal is to give an easy way for the user to "black list" component from the mcp entierly meaning:
- It wont appear in inspect
- Mutation on it are rejected
- If the mcp use python script to edit/discover it, it will success however (stated issue)


# Large script cause the bridge to disconnect — DONE
Fixed: progress-based mid-frame reads (IDLE_DEAD silence, not 1s poll), Windows
WriteFile loop, execute_python script/result 1 MiB soft caps
(`tdmcp.script.too_large` / `tdmcp.script.result_too_large`), serve_queued closes
stream on exit. Hygiene: prefer mutate_nodes for create/wire/set; keep
execute_python small for custom pages.

# Various tools should emit lint when FPS drop
Eg: `Warning: FPS drop: 30, last healty before: unknown`, `Warning: FPS drop: 30, last healty before: "script_execute([preview script]) at [HH:MM:SS]` emited when calling inspect tool/fps are not healty


# Force gpu noise if resoluton > 64px (almost laways)

# Error on custom component are not always seen (e.g error from the parameter customisation)
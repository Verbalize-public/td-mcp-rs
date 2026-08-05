# TODOLIST

# Per proejct components blacklist (configurable from the bridge operator)
The goal is to give an easy way for the user to "black list" component from the mcp entierly meaning:
- It wont appear in inspect
- Mutation on it are rejected
- If the mcp use python script to edit/discover it, it will success however (stated issue)

# Various tools should emit lint when FPS drop
Eg: `Warning: FPS drop: 30, last healty before: unknown`, `Warning: FPS drop: 30, last healty before: "script_execute([preview script]) at [HH:MM:SS]` emited when calling inspect tool/fps are not healty


# Force gpu noise if resoluton > 64px (almost laways)

# Error on custom component are not always seen (e.g error from the parameter customisation)
# Partially addressed: inspect enableExpr enrichment (parmExprIssues) when TD emits
# enable-parm warnings. Remaining: other custom-par / Component Editor error surfaces.

# Comment support (the agent could be much faster by storing comment/reading theme)


# Inspect tool throw exception on 'deactivated operator'

# Annotation aware -> understand which node is under annotation and natturaly make it available to agent




# Scripting doc (python skills) isn't clear enough or there is maybe contradiction, this is the begining of a "failed" tool call to study

iring isn't surfaced by inspect (no wires include), so I'll complement with one Python read for incident inputs, and batch-inspect all 39 children for params/errors in parallel:

S102 — intentional TD script surface","lineno":181,"name":"handle_execute_python"},{"filename":"<string>","line":"        src = ic.op","lineno":5,"name":"<module>"}],"message":"'td.Connector' object has no attribute 'op'","raw":"Traceback (most recent call last):\n  File \"C:\\Users\\corbe\\AppData\\Local\\tdmcp-rs\\bridge\\tdmcp_bridge\\execute.py\", line 181, in handle_execute_python\n    exec(script, local_vars, local_vars)  # noqa: S102 — intentional TD script surface\n    ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^\n  File \"<string>\", line 5, in <module>\nAttributeError: 'td.Connector' object has no attribute 'op'\n","syntax":null,"type":"AttributeError"},"layer":"script","message":"'td.Connector' object has no attribute 'op'","mitigation":["Fix using span + references","Prefer mutate_nodes for simple create/set/delete/connect/disconnect"],"rawTraceback":"Traceback (most recent call last):\n  File \"C:\\Users\\corbe\\AppData\\Local\\tdmcp-rs\\bridge\\tdmcp_bridge\\execute.py\", line 181, in handle_execute_python\n    exec(script, local_vars, local_vars)  # noqa: S102 — intentional TD script surface\n    ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^\n  File \"<string>\", line 5, in <module>\nAttributeError: 'td.Connector' object has no attribute 'op'\n","references":[{"id":"corpora/td-software/distilled/python-td-module/README.md","kind":"corpus"}],"severity":"error","span":{"line":5,"snippet":"        src = ic.op","tool":"execute_python"}}],"ok":false,"summary":"'td.Connector' object has no attribute 'op'"}
The wiring read needs the correct TD API — per skill hard rule I must check the Python cheatsheet before writing TD Python. Reading it + querying the Connector API:

The Connector API uses .inOP/.outOP. Re-running the wiring read:

inOP returned None everywhere — the documented n.inputs list is the reliable rea
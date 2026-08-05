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

# Inspect does not surface wires (no wires include)
# Agent mitigation (live-verified): creative-corpus `python-api.md` Wiring —
# prefer `n.inputs` / `n.outputs`; Connector has no `.op`; `inOP`/`outOP` often None.

# Allow ignoring PID when there is only one td instance (default = the single instance if there is only one otw flee needed)

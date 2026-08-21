# TODOLIST

# Sequenced tool call -> allo the agent to perform perflectly timed inspect/execute python sequence

# Force gpu noise if resoluton > 64px (almost laways) -> via skill

# Comment support (the agent could be much faster by storing comment/reading theme)

# Inspect tool throw exception on 'deactivated operator'

# Annotation aware -> understand which node is under annotation and natturaly make it available to agent

# Allow ignoring PID when there is only one td instance (default = the single instance if there is only one otw flee needed)


# Pallet/components gallery awarness (what are the component we can use, in project, from plaette ect, cached  "compressed card" of the components ect)


# Imorove diagnostic overall (deepen) + add reference to appropriate mcp documentation or give proper tool call to reach appropriate touchdesgienr doc via the mcp doc tool

# We should trim stack trace/remove the wraper code when throwing error its just consume token for nothing/is always the same
I want to improve the diagnostic feature of td-mcp-rs

# Goals
- Use the mcp live and figureout what diagnosdtic could be added/improved or should be removed ect
- Improve the overall diagnostic doc (streamline, make clearer sentence, roganise ect)
- **IMPORTANT** Add when possible reference to the documentation (mcp resources or give the appropriate tool call to obtain the proper doc from the mcp live td doc tool)

# Acceptance
- Once done you must test/trigger all diagnostic and make sure everything is revalent (no broken like or bqad tool call hint ect)
- Iterate as much as needed  (after an iteratio we always do the full loop again including diagnostic ect)

# Guidance
- Be carefull when updating the install, if you are stunt handoff to the user dont try alambigate things i will restart mcp ect as needed (sometime need harness restart)


# GUI update
- in the settings page, the back, save ect button must be at the top of the view/stick to the bottom  (like display fixed)/not scroll + make a visual effort so buttons are pretty (maybe icon)
- Still in the settings page, make sure various input fields take the full avail width or use a kind of justify space betwen  (eg: for checkbox) so its not ugly
- Merge the `skills`/`tox` button into a single folder icon button

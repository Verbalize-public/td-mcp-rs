# Recipes — what to say

There is no new interface to learn. You talk to your assistant, and it
operates TouchDesigner. These are prompts that work, ordered from *"just look
at this"* to *"build the whole thing"*.

**Contents**

- [Before you start](#before-you-start)
- [Level 1 · Look and explain](#level-1--look-and-explain)
- [Level 2 · Small precise edits](#level-2--small-precise-edits)
- [Level 3 · Delegate a chunk](#level-3--delegate-a-chunk)
- [Level 4 · End to end](#level-4--end-to-end)
- [Teach it your Palette](#teach-it-your-palette)
- [Debugging](#debugging)
- [Project and file wrangling](#project-and-file-wrangling)
- [Across machines](#across-machines)
- [Making it check its work](#making-it-check-its-work)

---

## Before you start

**You don't need to name the tools.** *"Why is this black?"* is a fine prompt.
Naming them (*"call `inspect` on `/project1`"*) only helps when the assistant
has wandered off.

**"This node" works.** The assistant can see which network pane you have open
and what's selected, so *"the node I've got selected"* or *"the one I'm
looking at"* resolves correctly.

**Ask it to look before it claims.** Add *"…then screenshot it and tell me
what you see."* Without that, an assistant will tell you a black render looks
great.

**Give it somewhere to work.** *"Build it in a new COMP called `bloom_v2`"* is
better than turning it loose in your root network, and makes the result
trivial to delete.

**Save first, the first few times.** It operates your live project. Nothing
here is destructive by design, but you'll experiment more freely with a save
behind you.

---

## Level 1 · Look and explain

Nothing is modified.

> *"What's in this project? Give me the structure."*

> *"Walk me through what this network does, node by node."*

> *"What is this feedback loop doing, and why does it need the Level TOP?"*

> *"Which operators are erroring or warning right now, and what do the errors
> mean?"*

> *"Show me the parameters on the node I have selected, and explain which ones
> matter."*

> *"Screenshot the output of `render1` and describe what you see."*

> *"What's the difference between a POP and a SOP, and which should I use
> here?"*

> *"Read the GLSL in this shader DAT and explain what each block does."*

The answers come from your actual network — names, wiring, parameter values,
error text, DAT bodies — cross-referenced against a built-in TouchDesigner
manual (operator families, cooking, GLSL ground truth, network conventions).

---

## Level 2 · Small precise edits

Exactly what you said, no improvising. Faster than reaching for the mouse.

> *"Add a Level TOP after `noise1`, set gamma to 0.8, wire it into `out1`."*

> *"Set the resolution on every TOP in this COMP to 1920×1080."*

> *"Give this COMP a custom page called `Look` with sliders for gain,
> saturation and blur, and bind them to the right parameters."*

> *"Rename these nodes to something sane and lay them out left to right."*

> *"Duplicate this chain three times, one per scene, and offset the noise seed
> on each."*

> *"Add a comment to each of these nodes saying what it's for."*

> *"Turn this hard-coded 0.5 into a reference to the slider I just added."*

Edits go through in one ordered batch. If step four fails you're told which
step and why, so you never end up with half a network and no explanation.

---

## Level 3 · Delegate a chunk

You describe the outcome; it picks the operators.

> *"Build an audio-reactive particle system in a new COMP. Use something from
> the Palette rather than hand-rolling it."*

> *"Make me a feedback-based trails effect I can drive from one 'amount'
> parameter."*

> *"Port this Shadertoy to a GLSL TOP and get it compiling: <paste the code>"*

> *"Set up a Kinect input chain with smoothing and a deadzone, exposed as
> custom parameters."*

> *"Take this static composite and make it react to the audio input — put the
> analysis in its own COMP so I can reuse it."*

> *"Build a 3-scene switcher with a crossfader and a preview of each scene."*

Worth adding to any of these:

> *"…then screenshot the result. If it doesn't look like what I asked for, fix
> it and screenshot again."*

Let that build-look-correct loop run twice before you step in.

---

## Level 4 · End to end

Nothing open, nothing set up.

> *"Start TouchDesigner on a new project, build me a generative visual driven
> by microphone input, and show me what it looks like."*

> *"Open `show_v3.toe`, find whatever is causing the frame drops, fix it, save
> as `show_v4.toe` and tell me what you changed."*

> *"Create a reusable `.tox` for our standard output chain — colour
> correction, LUT, safe-area overlay — with custom parameters for everything,
> then save it to the shared folder."*

> *"Spin up TouchDesigner, load each of these four project files in turn,
> check them for errors, and give me a one-line verdict on each."*

For this level, point `[project] template_path` in `config.toml` at a template
`.toe` that already contains the bridge — then *"make a new project"* gives
the assistant a project it can immediately work in. See
[`CONFIG.md`](CONFIG.md#common-settings).

---

## Teach it your Palette

TouchDesigner ships hundreds of finished components, and you probably have a
folder of your own. An assistant that knows about them stops rebuilding
`particlesGpu` from scratch.

**One time, to build the catalogue:**

> *"Scan my palette and tell me what's in it."*

> *"Learn my palette — start with the TOP and CHOP categories, and write up
> what each component does."*

It indexes the components, opens them safely in a scratch container to read
their real inputs, outputs and custom parameters, then writes a description of
each. This happens a slice at a time, on your machine only.

**Afterwards:**

> *"Is there a Palette component for this? Use it instead of building one."*

> *"Place `particlesGpu` in this COMP, wire my audio analysis into the birth
> rate, and expose the count as a custom parameter."*

> *"What's in my palette that deals with projection mapping?"*

**If a component misbehaves** — some open network sockets or expect hardware
that isn't there, and loading one can hang TouchDesigner:

> *"Blacklist that component so you never load it again."*

A handful of known-problematic ones are excluded by default. The list is yours
to edit; see [`CONFIG.md`](CONFIG.md#common-settings).

---

## Debugging

> *"This TOP is black. Find out why."*

> *"The project cooks at 30fps and should be at 60. What's expensive?"*

> *"This GLSL won't compile — here's the error, fix it."*

> *"Something's wrong with my audio chain, nothing reacts. Trace it from the
> input to the output and tell me where the signal dies."*

> *"Why does this only break when I load the project fresh?"*

Let it look rather than reason from memory:

> *"Don't guess — inspect the chain, read the actual parameter values, and
> capture the output at each stage."*

It can screenshot intermediate nodes, read raw CHOP channel values, and follow
GLSL DAT references, so *"the signal is zero after `math1`"* is something it
can establish rather than assume.

---

## Project and file wrangling

These work on closed project files, with TouchDesigner not running.

> *"Install the tdmcp bridge into every `.toe` in this folder."*

> *"Unpack `show.toe` so I can see it as files, and tell me what components
> it's made of."*

> *"Check these project files for problems before I take them to the venue."*

> *"Which TouchDesigner versions do I have installed, and which one is
> usable?"*

And on running instances:

> *"Is there a dialog blocking TouchDesigner right now? Dismiss it."*

> *"Close the TouchDesigner running `test.toe`, leave the other one alone."*

Everything is addressed by process id, so *"the other one"* is unambiguous
even with four instances open.

---

## Across machines

With [federation](FEDERATION.md) set up:

> *"What's running across the studio?"*

> *"On studio-b, capture the main output and tell me if it matches what I've
> got here."*

> *"Push this fix to every machine running the show project."*

> *"Kill TouchDesigner on all the render nodes, then relaunch them on
> `show_v4.toe` and confirm all four came back."*

> *"Studio-c is behaving differently from the others. Compare its project
> against studio-b's and tell me what differs."*

---

## Making it check its work

| Say this | Because |
| --- | --- |
| *"Screenshot it and tell me what you see."* | Perception is explicit — without asking, it may never look. |
| *"Don't guess — inspect it first."* | Redirects from memory to your actual network. |
| *"Show me the parameter values you read."* | Turns a claim into evidence. |
| *"Build it in a new COMP called `x`."* | Contains the damage. |
| *"What did you change? List it."* | A review pass before you accept anything. |
| *"That's still wrong — look again and fix it."* | Two correction rounds is normal. |

**When it goes off the rails:** stop it, undo in TouchDesigner
(<kbd>Ctrl/Cmd</kbd>+<kbd>Z</kbd> works — these are ordinary operator
changes), and be more specific about where it may build. Deleting the COMP it
was told to work in resets everything.

**When it won't use the tools at all:** say so directly — *"use the td-mcp-rs
tools, start with `fleet`"*. On Claude Code, install the plugin rather than the
bare MCP server; the bundled skill pack is what makes tool use automatic.

---

## See also

- [`INSTALL.md`](INSTALL.md) — setup and troubleshooting
- [`FEDERATION.md`](FEDERATION.md) — multi-machine fleets
- [`CONTRACT.md`](CONTRACT.md) — what every tool does, precisely
- [`../README.md`](../README.md) — the overview

# td-mcp-rs

**Give your AI assistant a connection to TouchDesigner.** Ask it to explore a
network, build an effect, fix a shader, or show you what changed. It can read
and edit the actual project, capture its output, and work across several TD
instances or computers.

> “Build a feedback trails effect in a new COMP. Give me one slider for the
> trail length, then capture the output so we can see how it looks.”

You bring the visual idea. The assistant can handle the wiring, parameter
edits, and investigation that go with it. You still get an ordinary TD network
to inspect and keep working on.

[Start here](docs/QUICKSTART.md) · [Things to try](docs/RECIPES.md) ·
[Download](https://github.com/Verbalize-public/td-mcp-rs/releases) ·
[Documentation map](docs/README.md)

## How it fits into your workflow

Three pieces connect your conversation to your project:

```text
Your assistant → td-mcp-rs on your computer → a bridge COMP inside TouchDesigner
```

**MCP** is the protocol your assistant uses to call tools. The **daemon** is
our small local application: it handles those calls and provides a tray
and dashboard. The **bridge** is the component you drop into a TD project
so the daemon can work with it. You don't need to write Python to use them.

One daemon can serve multiple assistants and TD processes. Each project is
addressed explicitly, so your experiment and your show file can stay separate.
The assistant should confirm which one you mean before editing.

## Get started

1. Install the release for your computer and launch td-mcp-rs.
2. Connect your MCP-compatible assistant. Claude Code has a bundled plugin;
   other clients use the installed executable with the `mcp` argument.
3. Open the dashboard, choose **Reveal .tox**, and drag `bootstrap.tox` into
   your TD project. Save the project with its bridge.
4. Ask: “List my TouchDesigner instances and inspect `/project1` in this project.”

The [quick start](docs/QUICKSTART.md) walks through each step, including what
success looks like. Windows and macOS have installers; Linux runs the daemon
natively and needs an existing [Wine/TouchDesigner setup](docs/LINUX_SUPPORT.md).
TouchDesigner itself is installed separately.

## What you can make it do

### Get past “why is this black?”

Inspect operators, wiring, parameters, errors, shader code, and the nodes
selected in your editor. Capture intermediate outputs, read CHOP samples or
TOP pixel values or SOP point positions, and follow a signal until it stops doing what you expected.
Live Python API help lets the assistant check the TD version you're running.

> “Trace this render chain. Show me where the image disappears and explain
> the fix before changing anything.”

### Build, tweak, and reuse

Create and connect operators, edit expressions, run Python, and build custom
parameter pages. Discover Palette components and reuse your own `.tox`
library. Export a component's control interface or register a Python script
for repeated runs. Compare selected fields against an earlier observation
when you need to know what changed.

Edits run in order. If one fails, earlier changes may remain; the result tells
the assistant where to investigate. Save before experimenting, and give new
ideas their own COMP. A tidy sandbox beats a root network full of `null47`.

### Look at the right moment

A feedback effect needs time to develop. Timed capture can trigger its reset,
allow initialization and warmup, then advance **one frame at a time** and
capture chosen offsets. Intervening frames are processed too. Pair captures
with parameter/error inspection to check appearance and state together.

> “Reset this effect, warm it up for 10 frames, then show me offsets 0, 30,
> and 60. Leave the timeline paused when you're done.”

You can also record consecutive frames from a TOP. Timing jobs report status
and support cancellation; they reserve that TD instance while running.
Recording currently produces silent qtrle MOV files, with size and duration
bounds. See [timing recipes](docs/RECIPES.md#look-at-specific-frames) for
controls, limits, and the current platform acceptance scope.

### Keep several projects in view

List running instances, open a project in a new process, create one from a
starter template, and stop a specific instance. With
[federation](docs/FEDERATION.md), the same assistant can inspect and operate
TD on other computers through one coordinator.

> “Compare the output on studio-b with the rehearsal project on this computer.
> Tell me which project and machine each image came from.”

Federation routes calls; it does not synchronize playback, copy your media,
or provide automatic show-machine failover.

### Prepare files before opening TD

Unpack `.toe` and `.tox` files using Derivative's official tools, inspect or
edit the expanded files, check their structure, and repack them. Install or
update the bridge in a closed project, with a backup by default. This is
useful when a folder of older projects needs connecting to your assistant.

Offline checks cannot tell you whether a shader renders correctly. Open a
copy in TD for that final check. See [project and file recipes](docs/RECIPES.md#project-and-file-wrangling).

### Recover a connection without losing the plot

The assistant's local connection can reconnect and, when needed, ask the
daemon to start again. Bridge connections have their own recovery path and
report disconnections. Failed tool calls are surfaced, not silently replayed:
a lost reply does not prove an edit never happened.

The [recovery guide](docs/OBSERVABILITY.md#when-a-connection-drops) explains
what to check before continuing. Recovery restores communication; it doesn't
undo edits or resume a timing job invalidated by bridge reload.

## Dashboard, privacy, and control

The dashboard brings together instances, federation, Palette, logs, and
settings. Configuration and customized starter templates survive upgrades.
Most local use needs no network configuration.

The default listener is local-only. For federation, use trusted computers,
access keys, and a trusted LAN or VPN: remote tools can run Python and access
files. Plain HTTP does not encrypt traffic. The daemon has no hosted service;
anything returned to your assistant follows that assistant's data policies.

## For developers

Rust 1.92+; Python 3 and the test dependencies for bridge/guidance checks.

```sh
cargo build --workspace
scripts/check.sh
```

On Windows, use `scripts/check.ps1`. Start with [Development](docs/DEV_ENV.md),
[Architecture](ARCHITECTURE.md), and [Testing](docs/TESTING.md). The
[tool contract](docs/CONTRACT.md) holds exact request/result details;
[current limitations](docs/OPEN_WORK.md) separates shipped behavior from gaps.

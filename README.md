# td-mcp-rs

Control TouchDesigner from an MCP-compatible AI assistant. Inspect networks,
edit operators, run Python, capture output, and manage projects through a
small bridge component inside TD.

The daemon runs locally. One daemon can serve several assistants and TD
processes; federation extends this to other computers.

## Get started

1. Download the appropriate installer or archive from
   [Releases](https://github.com/Verbalize-public/td-mcp-rs/releases).
   Windows uses the x64 installer; macOS has Apple Silicon and Intel DMGs;
   Linux uses the x86_64 archive and requires a working Wine/TD installation.
2. Start the daemon. For an extracted binary, run
   `tdmcp-daemon install`, then `tdmcp-daemon ensure`.
3. Add a **stdio MCP server** to your assistant: command = the absolute path
   to `tdmcp-daemon` (Windows: `tdmcp-daemon.exe`), arguments = `["mcp"]`.
4. Open TD, drag the dashboard's **Reveal .tox** file into your project,
   and save. Ask your assistant: “List the running TouchDesigner processes
   and inspect /project1.”

See [Installation](docs/INSTALL.md) for paths, bridge installation, and
troubleshooting. The [Claude Code plugin](docs/CLAUDE_CODE_PLUGIN.md) bundles
the MCP registration and operating skills.

## What you can do

- Inspect operators, parameters, connections, errors, and live Python help.
- Create and edit networks; capture TOPs and operator viewers.
- Start/stop TD, create projects, and install the bridge into closed projects.
- Discover, inspect, and reuse Palette components.
- Control TD on several computers through [Federation](docs/FEDERATION.md).

Try: “Find the broken connection in this network,” “Build a feedback effect
under /project1/visuals,” or “Capture the final output and check its errors.”
More examples: [Recipes](docs/RECIPES.md).
Tool arguments and diagnostics: [Contract](docs/CONTRACT.md).

## Dashboard and settings

The tray opens a dashboard with Overview, Federation, Palette, Logs, and
Settings. Federation roles, call timeouts, and keep-alive apply on Save.
Listener addresses, authentication, and process-level settings require a
restart; the dashboard tells you when one is pending.

Settings and customized project templates survive installation and upgrades.
See [Configuration](docs/CONFIG.md).

## Security

The default listener is local-only. Enabling network sharing exposes powerful
tools, including Python execution and filesystem access. Use a trusted LAN
or VPN and an access key; do not expose an unauthenticated daemon publicly.
HTTP alone does not encrypt traffic.

The daemon has no hosted service, but data returned to your assistant follows
that assistant's privacy and retention policies. Federation sends requests
and results between the configured computers.

## Develop

Rust 1.92 or newer; Python 3 with pytest for bridge tests.

```sh
cargo build --workspace
scripts/check.sh
```

On Windows use `scripts/check.ps1`. See [Development](docs/DEV_ENV.md),
[Architecture](ARCHITECTURE.md), [Testing](docs/TESTING.md), and
[Release packaging](docs/DELIVERY.md).

[Current limitations](docs/OPEN_WORK.md) lists known gaps, not a promised roadmap.

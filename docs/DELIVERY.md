# Delivery

## Artifacts

| Artifact | Role |
| --- | --- |
| `tdmcp-daemon` binary | Control plane + MCP + admin API + (default) in-process tray UI |
| `bridge/` | Python package + `manifest.json` beside install/data dir |
| `diagnostics/catalog.yaml` | Diagnostic catalog |
| `skills/` | Agent operate pack (Jinja templates under `templates/touchdesigner/`); served as MCP `tdmcp://docs/*` resources and exported via `tdmcp-daemon skills render` |
| bootstrap `.tox` | Tiny TD dialer COMP `tdmcp_rs` (handshake → FS load of `bridge/`). Embedded in the daemon; extracted to `{dataDir}/bootstrap.tox`. Rebuild recipe: [`scripts/pack_bootstrap_tox.md`](../scripts/pack_bootstrap_tox.md) |
| `.claude-plugin/` + `claude-skills/` | Claude Code plugin: MCP server registration + a checked-in filesystem-mode render of `skills/`. See [`CLAUDE_CODE_PLUGIN.md`](CLAUDE_CODE_PLUGIN.md) |

The tray dashboard lives in the `tdmcp-gui` **library** crate, linked into
`tdmcp-daemon` when the default `gui` Cargo feature is enabled. There is no
separate `tdmcp-gui` binary.

## Config

Source of truth: TOML file owned by crate `tdmcp-config` (see
[`docs/CONFIG.md`](CONFIG.md)).

**CLI args / env (`TDMCP_*`) > config.toml > built-in defaults.**

| Kind | Default path |
| --- | --- |
| Config file | `%APPDATA%/tdmcp-rs/config.toml` (Windows); Application Support / XDG config elsewhere |
| Data dir | `%LOCALAPPDATA%/tdmcp-rs/` (Windows); Application Support / XDG data elsewhere |

Notable `[daemon]` fields: `keep_alive`, `always_on`, `show_tray`.
`install` always resets `config.toml` to the embedded template; `start` /
`ensure` / `mcp` only create-if-missing.

## GUI feature

| Build / runtime | Behavior |
| --- | --- |
| Default (`cargo build -p tdmcp-daemon`) | `gui` feature on; `start` shows tray + toast (dashboard hidden until opened); gear opens Settings |
| `--no-default-features` | Headless binary (no egui/tray linked) |
| `--no-gui` / `TDMCP_NO_GUI=1` / `show_tray = false` | Headless even when `gui` is compiled in |

## Auto-upsert — Shipped

Cursor registers `tdmcp-daemon` with `args: ["mcp"]`. On MCP connect:

1. `mcp` calls `ensure` — health check, lockfile, detached spawn if needed, poll
   until healthy.
2. Stdio MCP proxy forwards tool requests to `http://127.0.0.1:{port}/mcp/rpc`.
3. Stale lockfile (pid dead) → reclaim.
4. Detached `start` (default) brings up the in-process tray with the daemon.

The long-lived HTTP daemon survives MCP client restarts; only the stdio proxy
process is respawned.

## Singleton

One owner per port: `daemon.lock` + TCP bind. Second healthy `start` refuses.
`/admin/restart` clears the lock, spawns a replacement, then exits; the new
process retries bind for a few seconds.

## Assets

Bridge, diagnostic catalog, bootstrap `.tox`, and the **skills/** operate pack are
embedded in the `tdmcp-daemon` binary. `install`, `ensure`, `start`, and `mcp`
extract them into the data dir on first use (no separate asset bundle required
for dev builds). Skills also surface as MCP resources (`tdmcp://docs/*`); see
[`../skills/README.md`](../skills/README.md).

## Packaging & Release

Two delivery paths coexist by design:

| Path | Audience | Binary lands at |
| --- | --- | --- |
| Installer — `tdmcp-rs-*-x64-setup.exe` / `.dmg` | End users | `%LOCALAPPDATA%\Programs\tdmcp-rs\` · `/Applications/tdmcp.app` |
| Dev flow — `tdmcp-daemon install` | Development | `{data_dir}/bin/` |

Both keep working because `ensure`, MCP-client upsert, and OS autostart bind to
the **running exe's own path**; config/data dirs are shared and untouched by
uninstalls.

### Release pipeline (tag-driven, zero manual build steps)

1. **Cut**: `cargo run -p xtask -- release patch|min|major [--dry-run]` — bumps
   `[workspace.package] version`, regenerates `Cargo.lock`, prepends a grouped
   CHANGELOG section built from conventional commits since the last `v*` tag,
   commits `chore(release): vX.Y.Z`, creates annotated tag `vX.Y.Z`. It never
   pushes; `--dry-run` prints everything harmlessly.
2. **Ship**: push the branch + tag. `.github/workflows/release.yml` then:
   asserts tag == workspace version → builds all targets via
   `cargo run -p xtask -- package --target …` (the exact command a dev laptop
   runs) → smoke-tests each archive → attaches platform artifacts.
3. **Artifacts**: `tdmcp-rs-{version}-{target}.zip|.tar.gz` +
   `SHA256SUMS.txt` + `tdmcp-rs-{version}-x64-setup.exe` (Inno Setup 6,
   per-user) + `tdmcp-rs-{version}-{aarch64|x86_64}.dmg` (`.app`
   `LSUIElement` bundle inside a UDZO DMG).

### CI layout

| Workflow | Trigger | What |
| --- | --- | --- |
| `ci.yml` | every push (any branch) | Windows gate: fmt/clippy/tests/pytest |
| `ci.yml` | daily cron + dispatch + main pushes | macOS `cargo test --workspace` (includes `tdmcp-dialogs` compile) + pytest |
| `ci.yml` | dispatch | MSRV 1.88 check |
| `ci.yml` | daily cron | `cargo deny check` + `cargo audit` |
| `release.yml` | tag `v*` | full pipeline above |

Artifact attestations are public-repo-only on this plan; the step is gated on
`github.repository_visibility == 'public'` and self-enables if the repo goes
public.

### Signing status (v1: unsigned, wiring ready)

Windows SmartScreen shows *"More info → Run anyway"*; macOS blocks downloaded,
ad-hoc-signed apps until the user allows them (System Settings ▸ Privacy &
Security, or `xattr -cr <app>` in Terminal). Wire-in points when certs arrive:
`signtool` on the setup exe (Windows job), `APPLE_DEVELOPER_ID_IDENTITY` /
`APPLE_NOTARY_PROFILE` secrets consumed by `make_app.sh` — both steps already
exist behind conditionals.

### Local commands

```text
cargo run -p xtask -- package [--target <triple>] [--out dir]   # named archive(s) + SHA256SUMS
cargo run -p xtask -- release minor --dry-run                   # rehearse a cut
ISCC.exe /DVersion=vX.Y.Z packaging/windows/installer.iss       # local installer (needs Inno Setup)
packaging/macos/make_app.sh <bin> <version> <triple> <out-dir>  # .app + dmg (macOS only)
```

### Dev install flow (unchanged)

`cargo run -p xtask -- dist` still produces the plain exe tree in
`target/dist/` (kill-daemons first so Cursor `mcp` shims don't lock it;
always rebuilds with `--features gui`). `target/release/tdmcp-daemon install`
copies into `{data_dir}/bin/` with rename-aside swap; stop the daemon before
re-running `install` when it runs from that installed location.

# Dashboard code map

The GUI is the `tdmcp-gui` library linked into the daemon, not another binary.
It uses egui/eframe for the dashboard and tray popup. HTTP owns daemon state;
the UI renders snapshots and submits validated settings patches.

| Module | Responsibility |
| --- | --- |
| `app.rs` | Shared view state, settings drafts, polling, lifecycle actions |
| `background.rs` | Single-flight background jobs for bounded HTTP work |
| `http.rs` | Admin requests, status snapshots, subnet discovery |
| `wire.rs` | Admin response DTOs |
| `dashboard.rs`, `dashboard/nav.rs` | Window shell and navigation |
| `dashboard/overview.rs`, `dashboard/fleet.rs` | Local/remote TD and client sessions |
| `dashboard/settings.rs` | Settings and Federation pages, shared save/discard actions |
| `federation.rs` | Join flow, discovery results, remote computer settings |
| `dashboard/logs.rs` | Filtered log tail |
| `palette.rs`, `dashboard/palette.rs` | Palette state and presentation |
| `popup.rs`, `tray.rs`, `platform.rs` | Tray, popup, notifications and OS actions |
| `theme.rs`, `dashboard/widgets.rs` | Reusable colors, typography and controls |
| `preview.rs` | Feature-gated fixture screens |

All paths above are relative to `crates/tdmcp-gui/src/`.

Keep network polling and multi-request flows off the render thread. Check
HTTP status and application errors before reporting success. Draft values are
not effective runtime values: use the loaded authentication snapshot for
requests and the daemon's restart-required list for saved settings.
Narrow windows use a page selector and Actions menu instead of the sidebar;
form rows stack labels above controls. Do not rely on native minimum sizes:
tiling window managers can ignore them.

## Verify a UI change

Run GUI unit tests and the preview harness:

```sh
cargo test -p tdmcp-gui
cargo run -p tdmcp-gui --features preview --example dashboard_preview
```

Inspect the changed screen at the minimum window size, then exercise it against
a real daemon, including an unreachable host and a rejected key. Fixture
screens prove layout only; actual connection success comes from the admin/MCP
state. See [Development](DEV_ENV.md) for live TD checks.

# Popups — detect and clear OS dialogs blocking TouchDesigner

A modal popup wedges TD's main thread: every bridged call stalls until its
budget expires while `ping` still answers. td-mcp-rs detects popups
daemon-side and fails calls fast instead of letting them time out blind.

## Triage flow

1. `fleet` `include:["popups"]` → rows carry `popups[]` and `windowStatus`
   (`blocked_by_modal_window` / `not_responding` / `responsive`).
2. `dialogs` `action:"list"` on the pid → full popup details
   (severity, message, buttons).
3. Dismiss deliberately: `action:"dismiss"`, `id`, optional `button`.
4. Verify-gone is built into dismiss (`stillOpen` empty ⇒ gone).

## Severity

| Severity | Examples | Policy |
| --- | --- | ---|
| hard | unexpected node name duplication, THREAD CONFLICT, cross-thread reference | surface loudly; fix the cause, never click through |
| soft | "Backwards Compatiblity Issue" (TD's own typo) | usually safe after reading; still explicit |
| unknown | unclassified | treat as soft-until-read |

## macOS

Everything runs on the **Accessibility** TCC grant alone — Screen Recording is
not needed. Enumeration, titles and dialog classification come from the
Accessibility API; `list` reports `accessibilityGranted`, and when it is false
also a `permissionHint`. Without the grant you get nothing at all (not even
titles), and `tdmcp.dialog.permission_denied` is the code `describe`/`dismiss`
return if you try anyway.

Two real limits on TouchDesigner's **own** dialogs — TD draws its UI itself, so
it publishes almost nothing to the accessibility tree:

- **Button labels and body text are usually unreadable.** `describe` reports the
  dialog, its title and its severity, but often zero buttons. Native dialogs TD
  opens (file pickers, system alerts) are fully readable — this limit is
  specific to TD-drawn windows.
- **Some TD dialogs cannot be dismissed programmatically** (`THREAD CONFLICT` is
  the known case: no cancel button, no labelled button, no close widget).
  `dismiss` returns `tdmcp.dialog.dismiss_failed` rather than faking success.
  Treat these as detect-and-surface: tell the user, let them click.

If TD wedges hard enough to stop answering accessibility queries altogether,
detection degrades rather than going blind: popups are still reported from the
window server, with the title `(untitled - app not answering accessibility)`.
That is a strong signal the main thread is stuck, not merely busy.

## Safety rails

- Main chrome is protected (`tdmcp.dialog.chrome_protected`).
- Save-prompts are never auto-answered — decide about unsaved work first.
- Interception (`tdmcp.dialog.blocking`) fires before enqueue, so a wedged TD
  costs milliseconds, not budget timeouts.
- Spawned processes are watched from t=0 — startup modals (version/compat/
  licence) are visible even before any handshake.

## Related

- Startup modals after spawn: [`lifecycle`](./lifecycle.md)
- Busy vs blocked bridged calls: [`tooling-concurrency`](./tooling-concurrency.md)
- Stalled cooking that is *not* a popup: [`play-state`](./play-state.md)

---

**Canonical:** [`popups`](./popups.md)
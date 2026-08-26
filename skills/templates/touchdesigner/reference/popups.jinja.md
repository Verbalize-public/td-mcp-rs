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

## Safety rails

- Main chrome is protected (`tdmcp.dialog.chrome_protected`).
- Save-prompts are never auto-answered — decide about unsaved work first.
- Interception (`tdmcp.dialog.blocking`) fires before enqueue, so a wedged TD
  costs milliseconds, not budget timeouts.
- Spawned processes are watched from t=0 — startup modals (version/compat/
  licence) are visible even before any handshake.

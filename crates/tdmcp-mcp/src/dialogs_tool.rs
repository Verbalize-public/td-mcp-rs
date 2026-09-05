//! `dialogs` — list/describe/dismiss OS popups owned by a TD pid.
//!
//! Local tool (no bridge dispatch; session-gate exempt). Requires the daemon
//! dialogs backend; otherwise fails with `tdmcp.dialog.unsupported`.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};
use tdmcp_core::Pid;

/// Args for `dialogs`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DialogsParams {
    /// Target pid.
    pub pid: Pid,
    /// What to do.
    pub action: DialogsAction,
    /// Popup id (from `list`) — required by describe/dismiss.
    #[serde(default)]
    pub id: Option<String>,
    /// Button label or ctrl id for dismiss; default button when omitted.
    #[serde(default)]
    pub button: Option<String>,
}

/// Sub-actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum DialogsAction {
    /// Snapshot popups + window status.
    List,
    /// Full content for one popup id.
    Describe,
    /// Run the dismiss ladder (verify-gone included).
    Dismiss,
}

/// Execute against an explicit shared state (test seam).
pub fn run_with(
    shared: &crate::dialogs::DialogsShared,
    params: &DialogsParams,
) -> Result<Value, (&'static str, String)> {
    match params.action {
        DialogsAction::List => {
            if !shared.source.supports_dialogs() {
                return Err(dialog_err(tdmcp_core::DialogError::Unsupported));
            }
            let snap = shared.source.snapshot(params.pid.get());
            // Only the macOS block below mutates this.
            #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
            let mut out = json!({
                "ok": true,
                "pid": params.pid,
                "windowStatus": snap.window_status,
                "popups": snap.popups,
            });
            #[cfg(target_os = "macos")]
            {
                out["accessibilityGranted"] =
                    json!(tdmcp_dialogs::sys::macos::accessibility_trusted());
                if out["accessibilityGranted"] == json!(false) {
                    out["permissionHint"] = json!(
                        "Grant Accessibility for tdmcp-daemon in System Settings → Privacy & Security → Accessibility"
                    );
                }
            }
            Ok(out)
        }
        DialogsAction::Describe => {
            let id = params
                .id
                .as_deref()
                .ok_or(("tdmcp.args.missing_field", "describe requires `id`".into()))?;
            let popup = shared
                .source
                .describe(params.pid.get(), id)
                .map_err(dialog_err)?;
            Ok(json!({ "ok": true, "pid": params.pid, "popup": popup }))
        }
        DialogsAction::Dismiss => {
            let id = params
                .id
                .as_deref()
                .ok_or(("tdmcp.args.missing_field", "dismiss requires `id`".into()))?;
            let outcome = shared
                .source
                .dismiss(params.pid.get(), id, params.button.as_deref())
                .map_err(dialog_err)?;
            Ok(json!({
                "ok": true,
                "pid": params.pid,
                "dismissed": outcome.dismissed,
                "via": outcome.via,
                "stillOpen": outcome.still_open,
            }))
        }
    }
}

/// Execute against the installed backend.
pub fn run(params: DialogsParams) -> Result<Value, (&'static str, String)> {
    let Some(shared) = crate::dialogs::get() else {
        return Err((
            "tdmcp.dialog.unsupported",
            "dialogs backend not installed".into(),
        ));
    };
    run_with(shared, &params)
}

fn dialog_err(e: tdmcp_core::DialogError) -> (&'static str, String) {
    use tdmcp_core::DialogError::*;
    match e {
        Unsupported => ("tdmcp.dialog.unsupported", "no dialogs backend".into()),
        NotFound { id } => ("tdmcp.dialog.not_found", format!("popup {id} not found")),
        DismissFailed { id } => (
            "tdmcp.dialog.dismiss_failed",
            format!("popup {id} still open after ladder"),
        ),
        ChromeProtected { id } => (
            "tdmcp.dialog.chrome_protected",
            format!("{id} is protected main chrome"),
        ),
        PermissionDenied => (
            "tdmcp.dialog.permission_denied",
            "macOS Accessibility permission required for describe/dismiss (System Settings → Privacy & Security → Accessibility)".into(),
        ),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;
    use tdmcp_core::{DialogError, DialogSnapshot, DialogSource, DismissOutcome, PopupInfo};

    /// A backend that can't answer dialogs at all (mirrors `NullDialogSource`
    /// / the Linux backend) — `list` must report `unsupported`, never a
    /// silently-empty snapshot.
    struct UnsupportedSource;

    impl DialogSource for UnsupportedSource {
        fn snapshot(&self, _pid: u32) -> DialogSnapshot {
            DialogSnapshot::default()
        }

        fn describe(&self, _pid: u32, _id: &str) -> Result<PopupInfo, DialogError> {
            Err(DialogError::Unsupported)
        }

        fn dismiss(
            &self,
            _pid: u32,
            _id: &str,
            _button: Option<&str>,
        ) -> Result<DismissOutcome, DialogError> {
            Err(DialogError::Unsupported)
        }

        fn supports_dialogs(&self) -> bool {
            false
        }
    }

    #[test]
    fn list_action_reports_unsupported_when_backend_lacks_dialogs() {
        let shared = crate::dialogs::DialogsShared {
            source: std::sync::Arc::new(UnsupportedSource),
            snapshots: std::sync::Mutex::new(std::collections::HashMap::new()),
            intercept: false,
        };
        let params = DialogsParams {
            pid: tdmcp_core::Pid(42),
            action: DialogsAction::List,
            id: None,
            button: None,
        };
        let err = run_with(&shared, &params).unwrap_err();
        assert_eq!(err.0, "tdmcp.dialog.unsupported");
    }
}

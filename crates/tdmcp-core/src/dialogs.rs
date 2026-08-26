//! OS-dialog domain types + the platform seam for popup detection/dismissal.
//!
//! Pure data + trait only (crate stays zero-I/O). Platform backends live in
//! `tdmcp-dialogs` (Windows user32/UIA now, macOS later); `NullDialogSource`
//! covers non-Windows targets and tests. Full mechanics: `docs/DIALOGS.md`.

use serde::{Deserialize, Serialize};

/// Why a dialog matters (ported POC severity regexes classify into these).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DialogSeverity {
    /// Blocks real work loudly (node-name duplication, thread conflict).
    Hard,
    /// Informational / version-compat chatter ("Backwards Compatiblity Issue").
    Soft,
    /// Unclassified — surface, never auto-act.
    Unknown,
}

/// Coarse window kind from class/style classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PopupKind {
    /// Classic message box (`#32770` family).
    MessageBox,
    /// OS file dialog.
    FileDialog,
    /// Qt-hosted or otherwise custom chrome.
    Custom,
    /// Detected but unclassifiable.
    Unknown,
}

/// One clickable button inside a popup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PopupButton {
    /// Stable control id (ctrl id or hwnd-derived).
    pub id: String,
    /// Visible label ("OK", "Save"...). Empty when unreadable.
    pub label: String,
    /// Default button per `BS_DEFPUSHBUTTON` / UIA default flag.
    pub is_default: bool,
}

/// One detected popup window owned by a TD pid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PopupInfo {
    /// Stable id while the window lives (hwnd-derived; re-verified before acting).
    pub id: String,
    /// Window title.
    pub title: String,
    /// Win32 class name when available (`#32770`, Qt classes...).
    pub class: Option<String>,
    /// Coarse kind.
    pub kind: PopupKind,
    /// Severity classification.
    pub severity: DialogSeverity,
    /// Message text when extractable (Qt-hosted dialogs may yield none via
    /// classic controls; UIA fills what it can).
    pub message: Option<String>,
    /// Buttons with labels + default flag; empty when unreadable.
    pub buttons: Vec<PopupButton>,
    /// Main editor chrome — never dismissable through the seam.
    pub is_main_chrome: bool,
}

/// Main-window responsiveness hint (fills reserved `window_status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowStatus {
    /// No owned modal and the main window answers probes.
    Responsive,
    /// An owned popup is open — bridged main-thread work is likely wedged.
    BlockedByModalWindow,
    /// Main window fails hang probes.
    NotResponding,
}

impl WindowStatus {
    /// Wire-friendly string (registry stores `Option<String>`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            WindowStatus::Responsive => "responsive",
            WindowStatus::BlockedByModalWindow => "blocked_by_modal_window",
            WindowStatus::NotResponding => "not_responding",
        }
    }
}

/// Point-in-time view of one pid's popups + main-window state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DialogSnapshot {
    /// Popups currently visible (cap applied by backends, oldest dropped).
    pub popups: Vec<PopupInfo>,
    /// Responsiveness of the main window.
    pub window_status: Option<WindowStatus>,
}

/// Seam failures (map to `tdmcp.dialog.*` codes at the MCP layer).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogError {
    /// No platform backend (non-Windows target / feature off).
    Unsupported,
    /// Stale or unknown popup id (hwnd reuse race re-verifies before acting).
    NotFound {
        /// Offending popup id.
        id: String,
    },
    /// Dismiss attempted but window persisted past verification.
    DismissFailed {
        /// Popup id that stayed open.
        id: String,
    },
    /// Target is main chrome — protected by policy.
    ChromeProtected {
        /// Protected target id.
        id: String,
    },
    /// macOS Accessibility permission missing (TCC).
    PermissionDenied,
}

impl core::fmt::Display for DialogError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            DialogError::Unsupported => write!(f, "dialogs unsupported on this platform"),
            DialogError::NotFound { id } => write!(f, "popup id not found: {id}"),
            DialogError::DismissFailed { id } => write!(f, "dismiss failed, still open: {id}"),
            DialogError::ChromeProtected { id } => {
                write!(f, "target is protected main chrome: {id}")
            }
            DialogError::PermissionDenied => {
                write!(f, "accessibility permission denied (macOS TCC)")
            }
        }
    }
}

impl std::error::Error for DialogError {}

/// Result of one dismissal attempt. Never ok-fakes success: `still_open`
/// carries ids that survived verification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DismissOutcome {
    /// Whether every targeted popup verified gone.
    pub dismissed: bool,
    /// How it was dismissed ("button:OK", "close") when successful.
    pub via: Option<String>,
    /// Ids still open after the ladder + verify-gone loop.
    pub still_open: Vec<String>,
}

/// Platform seam: enumerate/describe/dismiss popups of a registered TD pid.
///
/// Justified single seam despite the constitution's single-impl-trait stance:
/// it carries [`NullDialogSource`] for non-Windows/test targets plus the future
/// macOS backend behind one narrow surface (see DIALOGS.md §5.1).
pub trait DialogSource: Send + Sync {
    /// Cheap snapshot for the watcher/poll path (user32-only on Windows).
    fn snapshot(&self, pid: u32) -> DialogSnapshot;

    /// Full content for one popup (buttons/message fill-in; cached upstream).
    ///
    /// # Errors
    /// [`DialogError::Unsupported`] / [`DialogError::NotFound`].
    fn describe(&self, pid: u32, id: &str) -> Result<PopupInfo, DialogError>;

    /// Run the dismiss ladder for one popup; verifies gone afterwards.
    ///
    /// # Errors
    /// [`DialogError::Unsupported`] / [`DialogError::NotFound`] /
    /// [`DialogError::ChromeProtected`] / [`DialogError::DismissFailed`].
    fn dismiss(
        &self,
        pid: u32,
        id: &str,
        button: Option<&str>,
    ) -> Result<DismissOutcome, DialogError>;

    /// Image basename of `pid` (`TouchDesigner.exe`) for kill-pid checks.
    /// Default: unknown (Null backend).
    fn process_image_name(&self, _pid: u32) -> Option<String> {
        None
    }

    /// Cheap liveness probe. Default: unknown => false.
    fn process_alive(&self, _pid: u32) -> bool {
        false
    }
}

/// No-op backend: non-Windows targets and tests. Empty snapshots, unsupported
/// describe/dismiss — detection must fail open without making calls worse.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullDialogSource;

impl DialogSource for NullDialogSource {
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
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;

    #[test]
    fn popup_info_serializes_camel_case() {
        let info = PopupInfo {
            id: "78910".into(),
            title: "Backwards Compatiblity Issue".into(),
            class: Some("#32770".into()),
            kind: PopupKind::MessageBox,
            severity: DialogSeverity::Soft,
            message: Some("saved by an older build".into()),
            buttons: vec![PopupButton {
                id: "1".into(),
                label: "OK".into(),
                is_default: true,
            }],
            is_main_chrome: false,
        };
        let v = serde_json::to_value(&info).unwrap();
        assert_eq!(v["isMainChrome"], false);
        assert_eq!(v["severity"], "soft");
        assert_eq!(v["kind"], "message_box");
        assert_eq!(v["buttons"][0]["isDefault"], true);
        let back: PopupInfo = serde_json::from_value(v).unwrap();
        assert_eq!(back, info);
    }

    #[test]
    fn window_status_strings_match_registry_wire() {
        assert_eq!(
            WindowStatus::BlockedByModalWindow.as_str(),
            "blocked_by_modal_window"
        );
        let s = serde_json::to_string(&WindowStatus::NotResponding).unwrap();
        assert_eq!(s, r#""not_responding""#);
    }

    #[test]
    fn null_source_is_empty_and_unsupported() {
        let src = NullDialogSource;
        let snap = src.snapshot(42);
        assert!(snap.popups.is_empty());
        assert!(snap.window_status.is_none());
        assert!(matches!(
            src.describe(42, "1"),
            Err(DialogError::Unsupported)
        ));
        assert!(matches!(
            src.dismiss(42, "1", None),
            Err(DialogError::Unsupported)
        ));
    }
}

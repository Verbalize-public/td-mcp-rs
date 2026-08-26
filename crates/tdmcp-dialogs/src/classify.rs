//! Portable classification: severity matching, chrome guard, kind detection,
//! content composition. No OS calls — fully unit-tested.

use tdmcp_core::{DialogSeverity, PopupButton, PopupInfo, PopupKind};

use crate::sys::{SysControl, SysWindow};

/// Hard-severity markers (ported from the JS POC regexes; matched
/// case-insensitively). TD's own typo "Compatiblity" is kept verbatim for the
/// soft marker.
pub const HARD_MARKERS: [&str; 2] = [
    "thread conflict",
    "cannot be referenced from separate threads",
];
/// Soft-severity marker (verbatim TD typo).
pub const SOFT_MARKER: &str = "backwards compatiblity issue";

/// `unexpected node [name] <ws>duplicat...` — hand-rolled equivalent of the
/// POC's hard regex alternative.
fn unexpected_node_duplication(hay: &str) -> bool {
    let mut rest = hay;
    while let Some(i) = rest.find("unexpected node") {
        let after = &rest[i + "unexpected node".len()..];
        let after = after.strip_prefix(" name").unwrap_or(after);
        if after.trim_start().starts_with("duplicat") {
            return true;
        }
        rest = after;
    }
    false
}

/// Classify severity over title+message. Unknown when nothing matches.
#[must_use]
pub fn severity(title: &str, message: Option<&str>) -> DialogSeverity {
    let hay = format!("{}\n{}", title, message.unwrap_or("")).to_lowercase();
    if hay.contains("thread conflict")
        || hay.contains("cannot be referenced from separate threads")
        || unexpected_node_duplication(&hay)
    {
        DialogSeverity::Hard
    } else if hay.contains(SOFT_MARKER) {
        DialogSeverity::Soft
    } else {
        DialogSeverity::Unknown
    }
}

/// Chrome guard (POC rule): empty titles and main-editor chrome
/// (`TouchDesigner <build>: <path>`) are never dismissable targets.
#[must_use]
pub fn is_chrome_title(title: &str) -> bool {
    let lower = title.to_lowercase();
    let trimmed = lower.trim();
    if trimmed.is_empty() {
        return true;
    }
    trimmed == "touchdesigner" || trimmed.starts_with("touchdesigner ")
}

/// System helper windows that are always visible on some setups and must
/// never count as popups (live-recorded 2026-08-26: ConsoleWindowClass +
/// IME helpers tripped the interception gate as three phantom "modals").
pub const SYSTEM_HELPER_CLASSES: [&str; 4] =
    ["MSCTFIME UI", "Default IME", "IME", "ConsoleWindowClass"];

/// True when the class is a known system helper window.
#[must_use]
pub fn is_system_helper(class: &str) -> bool {
    SYSTEM_HELPER_CLASSES
        .iter()
        .any(|c| class.eq_ignore_ascii_case(c))
}

/// Main-window candidate heuristic: visible chrome window (hang probes).
#[must_use]
pub fn is_main_candidate(win: &SysWindow) -> bool {
    win.visible && !win.title.is_empty() && is_chrome_title(&win.title)
}

/// Coarse kind from window class.
#[must_use]
pub fn kind_for_class(class: &str) -> PopupKind {
    match class {
        "#32770" => PopupKind::MessageBox,
        "" => PopupKind::Unknown,
        _ => PopupKind::Custom,
    }
}

/// Snapshot-level popup: identity + severity only (no content walk).
#[must_use]
pub fn popup_from_window(win: &SysWindow) -> PopupInfo {
    PopupInfo {
        id: win.id.clone(),
        title: win.title.clone(),
        class: Some(win.class.clone()),
        kind: kind_for_class(&win.class),
        severity: severity(&win.title, None),
        message: None,
        buttons: Vec::new(),
        is_main_chrome: false,
    }
}

/// Describe-level base before content fill-in (id known, rest pending).
#[must_use]
pub fn popup_from_stub(id: &str) -> PopupInfo {
    PopupInfo {
        id: id.to_string(),
        title: String::new(),
        class: None,
        kind: PopupKind::Unknown,
        severity: DialogSeverity::Unknown,
        message: None,
        buttons: Vec::new(),
        is_main_chrome: false,
    }
}

/// Compose full content from child controls: buttons from Button class,
/// message from the longest Static text (improves on the POC's lossy join).
#[must_use]
pub fn fill_content(mut base: PopupInfo, children: &[SysControl]) -> PopupInfo {
    let mut message_len = 0usize;
    for c in children {
        if c.class.eq_ignore_ascii_case("Button") {
            base.buttons.push(PopupButton {
                id: c
                    .ctrl_id
                    .map(|i| i.to_string())
                    .unwrap_or_else(|| c.id.clone()),
                label: c.label.clone(),
                is_default: c.is_default,
            });
        } else if c.class.eq_ignore_ascii_case("Static") && c.label.chars().count() > message_len {
            message_len = c.label.chars().count();
            base.message = Some(c.label.clone());
        }
    }
    base.severity = severity(&base.title, base.message.as_deref());
    base.kind = base
        .class
        .as_deref()
        .map(kind_for_class)
        .unwrap_or(PopupKind::Unknown);
    base
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;

    #[test]
    fn hard_severity_variants() {
        assert_eq!(
            severity("Unexpected node name duplicated", None),
            DialogSeverity::Hard
        );
        assert_eq!(
            severity("Error", Some("THREAD CONFLICT detected")),
            DialogSeverity::Hard
        );
        assert_eq!(
            severity("", Some("op cannot be referenced from separate threads")),
            DialogSeverity::Hard
        );
    }

    #[test]
    fn soft_severity_td_typo_verbatim() {
        assert_eq!(
            severity("Backwards Compatiblity Issue", None),
            DialogSeverity::Soft
        );
    }

    #[test]
    fn unknown_when_no_match() {
        assert_eq!(
            severity("Some dialog", Some("plain text")),
            DialogSeverity::Unknown
        );
    }

    #[test]
    fn chrome_guard_blocks_main_and_empty() {
        assert!(is_chrome_title(""));
        assert!(is_chrome_title("TouchDesigner"));
        assert!(is_chrome_title("TouchDesigner 2025.32460: C:/proj/x.toe"));
        assert!(!is_chrome_title("Backwards Compatiblity Issue"));
    }

    #[test]
    fn system_helper_classes_filtered() {
        assert!(is_system_helper("MSCTFIME UI"));
        assert!(is_system_helper("default ime"));
        assert!(is_system_helper("ConsoleWindowClass"));
        assert!(!is_system_helper("#32770"));
        assert!(!is_system_helper("Qt5152QWindowIcon"));
    }
    #[test]
    fn fill_content_picks_longest_static_and_buttons() {
        let base = popup_from_stub("1");
        let kids = vec![
            SysControl {
                id: "s1".into(),
                class: "Static".into(),
                label: "short".into(),
                ctrl_id: None,
                is_default: false,
            },
            SysControl {
                id: "s2".into(),
                class: "Static".into(),
                label: "the long message".into(),
                ctrl_id: None,
                is_default: false,
            },
            SysControl {
                id: "b".into(),
                class: "Button".into(),
                label: "OK".into(),
                ctrl_id: Some(1),
                is_default: true,
            },
        ];
        let full = fill_content(base, &kids);
        assert_eq!(full.message.as_deref(), Some("the long message"));
        assert_eq!(full.buttons.len(), 1);
        assert_eq!(full.buttons[0].id, "1");
        assert!(full.buttons[0].is_default);
    }

    #[test]
    fn kind_from_class() {
        assert_eq!(kind_for_class("#32770"), PopupKind::MessageBox);
        assert_eq!(kind_for_class(""), PopupKind::Unknown);
        assert_eq!(kind_for_class("Qt5152QWindowIcon"), PopupKind::Custom);
    }
}

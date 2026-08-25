//! Dismiss ladder selection (portable, pure): explicit button → default button
//! → WM_CLOSE fallback. Execution + verify-gone live with the backend.

use crate::sys::SysControl;

/// What the ladder decided to do first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LadderStep {
    /// Click the button control (ctrl id, human label for `via` reporting).
    Click(i32, String),
    /// Post WM_CLOSE to the dialog itself.
    Close,
}

/// Pick the first ladder step for a dismissal request.
///
/// - explicit `button` matches ctrl id (`"1"`) or case-insensitive label ("OK")
/// - else the flagged default button
/// - else any single button
/// - else close
#[must_use]
pub fn plan_ladder(button: Option<&str>, children: &[SysControl]) -> LadderStep {
    let buttons: Vec<&SysControl> = children
        .iter()
        .filter(|c| c.class.eq_ignore_ascii_case("Button"))
        .collect();
    if let Some(want) = button {
        let want_lower = want.trim().to_lowercase();
        let hit = buttons.iter().find(|b| {
            b.ctrl_id.map(|i| i.to_string()) == Some(want_lower.clone())
                || b.label.to_lowercase() == want_lower
                || b.id == want
        });
        if let Some(b) = hit {
            if let Some(ctrl_id) = b.ctrl_id {
                return LadderStep::Click(ctrl_id, b.label.clone());
            }
        }
    }
    if let Some(def) = buttons.iter().find(|b| b.is_default) {
        if let Some(ctrl_id) = def.ctrl_id {
            return LadderStep::Click(ctrl_id, def.label.clone());
        }
    }
    if let Some(any) = buttons.first() {
        if let Some(ctrl_id) = any.ctrl_id {
            return LadderStep::Click(ctrl_id, any.label.clone());
        }
    }
    LadderStep::Close
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;

    fn btn(id: &str, label: &str, ctrl: i32, def: bool) -> SysControl {
        SysControl {
            id: id.into(),
            class: "Button".into(),
            label: label.into(),
            ctrl_id: Some(ctrl),
            is_default: def,
        }
    }

    fn stat(label: &str) -> SysControl {
        SysControl {
            id: "s".into(),
            class: "Static".into(),
            label: label.into(),
            ctrl_id: None,
            is_default: false,
        }
    }

    #[test]
    fn explicit_label_beats_default() {
        let kids = [
            stat("msg"),
            btn("a", "Save", 6, false),
            btn("b", "OK", 1, true),
        ];
        assert_eq!(
            plan_ladder(Some("save"), &kids),
            LadderStep::Click(6, "Save".into())
        );
    }

    #[test]
    fn default_button_without_explicit() {
        let kids = [btn("a", "Save", 6, false), btn("b", "OK", 1, true)];
        assert_eq!(plan_ladder(None, &kids), LadderStep::Click(1, "OK".into()));
    }

    #[test]
    fn falls_back_to_close_without_buttons() {
        assert_eq!(plan_ladder(None, &[stat("m")]), LadderStep::Close);
        assert_eq!(plan_ladder(None, &[]), LadderStep::Close);
    }

    #[test]
    fn unknown_explicit_falls_through_to_default() {
        let kids = [btn("a", "Cancel", 2, false), btn("b", "Yes", 6, true)];
        assert_eq!(
            plan_ladder(Some("nonexistent"), &kids),
            LadderStep::Click(6, "Yes".into())
        );
    }
}

//! Platform facade: shared window/control shapes + cfg dispatch to backends.
//!
//! Backends implement ONLY the functions below; all domain logic lives above
//! (classify/policy/lib). Adding macOS = implementing this surface there.

#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(all(not(windows), not(target_os = "macos")))]
pub mod stub;
#[cfg(windows)]
pub mod windows;

/// One visible top-level window of a pid (snapshot-level data).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SysWindow {
    /// Owning process id.
    pub pid: u32,
    /// Stable opaque id (hwnd-derived on Windows).
    pub id: String,
    /// Win32 class name.
    pub class: String,
    /// Window title.
    pub title: String,
    /// Visibility flag.
    pub visible: bool,
    /// Raw GWL_STYLE value (corroboration only).
    pub styles: isize,
    /// Raw GWL_EXSTYLE value.
    pub ex_styles: isize,
}

/// One child control of a dialog (content-level data).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SysControl {
    /// Stable control id (hwnd-derived).
    pub id: String,
    /// Control class ("Button"/"Static"/Qt class...).
    pub class: String,
    /// Visible label text ("" when unreadable).
    pub label: String,
    /// Dialog ctrl id when classic controls expose one.
    pub ctrl_id: Option<i32>,
    /// Default-pushbutton flag from style bits.
    pub is_default: bool,
}

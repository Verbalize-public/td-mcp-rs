//! Wine invocation (Linux only): TouchDesigner ships no Linux build, so every
//! official `.exe` (TouchDesigner itself, `toeexpand`, `toecollapse`) has to
//! run under Wine. Kept general on purpose — no assumption about *which*
//! Wine build or prefix layout the user has:
//!
//! - The Wine binary is `TDMCP_WINE_EXE` (from `[official_tools] wine_exe`,
//!   promoted to env by `tdmcp_config::load`), default `"wine"` — a path or a
//!   bare name works, so a Lutris/Bottles/CrossOver wrapper script is a valid
//!   override.
//! - The prefix is derived from the resolved exe path's `drive_c` ancestor,
//!   not guessed — every Wine-compatible layout (plain Wine, Proton, Lutris,
//!   Bottles, CrossOver) uses that convention, so this works with zero config
//!   for any of them. An explicit `TDMCP_WINE_PREFIX` (from
//!   `[official_tools] wine_prefix`, promoted to env by `tdmcp_config::load`)
//!   wins outright — for layouts with no `drive_c` ancestor (e.g. the exe
//!   living outside its prefix, as the AUR touchdesigner-linux package keeps
//!   it under `/opt/touchdesigner/td`) it is the *only* way to pin the prefix.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::resolve::ENV_WINE_PREFIX;

/// True when `program` is a Windows PE binary that cannot run natively here.
#[must_use]
pub fn needs_wine(program: &Path) -> bool {
    cfg!(all(unix, not(target_os = "macos")))
        && program
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("exe"))
}

/// The Wine prefix to run `exe` under: an explicit `TDMCP_WINE_PREFIX` env
/// (from `[official_tools] wine_prefix`, promoted by `tdmcp_config::load`)
/// wins outright; otherwise `exe`'s `drive_c` ancestor decides. `None` when no
/// override exists and the exe is not under a `drive_c` layout — Wine then
/// falls back to its own default prefix.
#[must_use]
pub fn prefix_for(exe: &Path) -> Option<PathBuf> {
    let env_override = std::env::var(ENV_WINE_PREFIX)
        .ok()
        .filter(|v| !v.is_empty());
    prefix_for_with(env_override.as_deref(), exe)
}

/// [`prefix_for`] minus the process-env read — callers (and tests) pass the
/// override directly, so tests never mutate shared env state.
#[must_use]
fn prefix_for_with(env_override: Option<&str>, exe: &Path) -> Option<PathBuf> {
    if let Some(p) = env_override {
        return Some(PathBuf::from(p));
    }
    let mut cur = exe.parent();
    while let Some(dir) = cur {
        if dir.file_name().and_then(OsStr::to_str) == Some("drive_c") {
            return dir.parent().map(Path::to_path_buf);
        }
        cur = dir.parent();
    }
    None
}

/// Wine binary name/path: `TDMCP_WINE_EXE` env (config-promoted), else `"wine"`.
#[must_use]
pub fn wine_exe() -> String {
    std::env::var("TDMCP_WINE_EXE")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "wine".to_string())
}

/// Build the `Command` to run `program`, wrapping it through Wine when
/// needed (native platforms and non-`.exe` programs pass through untouched).
///
/// `export_linux_pid` additionally has the child export its own real Linux
/// pid as `TDMCP_LINUX_PID` before exec'ing Wine. Wine's own `os.getpid()` —
/// what TD's (win32) Python reports at bridge handshake — is a virtual NT
/// pid, not this host's pid, so without this the daemon's registry (keyed by
/// the real launcher pid from `spawn_td`) never matches the handshake and
/// the spawn wait always reports a timeout even on a fully successful
/// connect. The value can't be passed as an ordinary env var — this process
/// doesn't know the child's pid until after spawning it — so a tiny shell
/// wrapper captures its own pid (`$$`) and `exec`s Wine in place, which
/// preserves that pid for the lifetime of the process. Only meaningful for
/// `spawn_td` (the live bridge target); offline tool runs (`toeexpand` /
/// `toecollapse`) never set this.
#[must_use]
pub fn command_for(program: &Path, export_linux_pid: bool) -> Command {
    if !needs_wine(program) {
        return Command::new(program);
    }
    let wine_bin = wine_exe();
    let prefix = prefix_for(program);
    let mut cmd = if export_linux_pid {
        let mut c = Command::new("sh");
        c.arg("-c")
            .arg(r#"export TDMCP_LINUX_PID="$$"; exec "$0" "$@""#)
            .arg(&wine_bin)
            .arg(program);
        c
    } else {
        let mut c = Command::new(&wine_bin);
        c.arg(program);
        c
    };
    if let Some(p) = prefix {
        cmd.env("WINEPREFIX", p);
    }
    cmd
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "unit tests")]
mod tests {
    use super::*;

    #[test]
    fn prefix_for_finds_drive_c_ancestor() {
        let exe = Path::new("/home/u/.wine/drive_c/Program Files/Derivative/TouchDesigner.2025/bin/TouchDesigner.exe");
        assert_eq!(
            prefix_for_with(None, exe),
            Some(PathBuf::from("/home/u/.wine"))
        );
    }

    #[test]
    fn prefix_for_none_without_drive_c() {
        assert_eq!(
            prefix_for_with(None, Path::new("/opt/custom/TouchDesigner.exe")),
            None
        );
    }

    #[test]
    fn explicit_wine_prefix_wins_over_drive_c_derivation() {
        // The AUR-style case: exe outside any drive_c layout, prefix pinned
        // via config — the override must decide, not the (missing) ancestor.
        let pinned = Some("/home/u/.local/share/touchdesigner-linux/prefix");
        assert_eq!(
            prefix_for_with(
                pinned,
                Path::new("/opt/touchdesigner/td/bin/TouchDesigner.exe")
            ),
            Some(PathBuf::from(
                "/home/u/.local/share/touchdesigner-linux/prefix"
            ))
        );
        // …and it beats derivation even when a drive_c ancestor exists.
        assert_eq!(
            prefix_for_with(
                pinned,
                Path::new(
                    "/home/u/.wine/drive_c/Program Files/Derivative/TD/bin/TouchDesigner.exe"
                )
            ),
            Some(PathBuf::from(
                "/home/u/.local/share/touchdesigner-linux/prefix"
            ))
        );
    }

    #[test]
    fn prefix_for_reads_tdmcp_wine_prefix_env() {
        // Sole test touching this env var (the others go through
        // `prefix_for_with`), so parallel tests can never race it.
        std::env::set_var(ENV_WINE_PREFIX, "/tmp/pinned-prefix");
        assert_eq!(
            prefix_for(Path::new("/opt/custom/TouchDesigner.exe")),
            Some(PathBuf::from("/tmp/pinned-prefix"))
        );
        std::env::remove_var(ENV_WINE_PREFIX);
    }

    #[test]
    fn wine_exe_defaults_to_wine() {
        std::env::remove_var("TDMCP_WINE_EXE");
        assert_eq!(wine_exe(), "wine");
    }
}

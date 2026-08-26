//! Official-tool resolution: explicit config pair → env → Program Files scan.
//!
//! Mirrors opendesigner's proven chain with our own env namespace
//! (`TDMCP_TOEEXPAND` / `TDMCP_TOECOLLAPSE` / `TDMCP_TOUCHDESIGNER_EXE`).
//! V2-0 probe law: a candidate install counts **only if the needed tool files
//! actually exist** — stub installs (versioned dirs without bin tools) must be
//! skipped, never trusted by directory presence.

use std::path::{Path, PathBuf};

use crate::error::ProjectIoError;

/// A resolved official-tools pair (both always present after resolution).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfficialTools {
    /// Absolute path to toeexpand.
    pub expand: PathBuf,
    /// Absolute path to toecollapse.
    pub collapse: PathBuf,
}

/// Per-install inventory beside a TouchDesigner.exe (`td_installs` row source).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallInfo {
    /// TouchDesigner.exe path probed.
    pub exe: PathBuf,
    /// Versioned install root (`...\Derivative\TouchDesigner.<v>`).
    pub root: PathBuf,
    /// toeexpand path when present.
    pub toeexpand: Option<PathBuf>,
    /// toecollapse path when present.
    pub toecollapse: Option<PathBuf>,
    /// Bundled python.exe when present.
    pub python: Option<PathBuf>,
}

fn beside(exe: &Path, name: &str) -> Option<PathBuf> {
    let bin = exe.parent()?;
    for candidate in [format!("{name}.exe"), name.to_string()] {
        let p = bin.join(candidate);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

/// Versioned install root for a TouchDesigner binary path.
#[must_use]
pub fn install_root_from_exe(exe: &Path) -> PathBuf {
    // No `.app/` string pre-check: `Path::join` uses the HOST separator, so a
    // macOS bundle path built on Windows reads `...app\Contents` and the check
    // misses. Walking extensions is separator-agnostic, and no non-bundle
    // install dir is named `*.app`, so the walk simply falls through.
    let mut current = exe.to_path_buf();
    while current.parent().is_some() {
        if current.extension().is_some_and(|e| e == "app") {
            return current;
        }
        current = current.parent().map(Path::to_path_buf).unwrap_or_default();
    }
    exe.parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_default()
}

/// Inspect one candidate exe: which official tools actually exist beside it.
#[must_use]
pub fn inspect_install(exe: &Path) -> InstallInfo {
    InstallInfo {
        exe: exe.to_path_buf(),
        root: install_root_from_exe(exe),
        toeexpand: beside(exe, TOOL_NAMES[0]),
        toecollapse: beside(exe, TOOL_NAMES[1]),
        python: beside(exe, "python"),
    }
}

/// Where tools may come from, mapped from `[official_tools]` config by the caller.
#[derive(Debug, Default, Clone)]
pub struct ToolSource {
    /// Explicit expand tool path (must be paired with collapse).
    pub expand: Option<PathBuf>,
    /// Explicit collapse tool path (must be paired with expand).
    pub collapse: Option<PathBuf>,
    /// Explicit TouchDesigner.exe; tools are expected beside it in the same bin dir.
    pub td_exe: Option<PathBuf>,
}

/// Env-var lookup seam (`std::env::var` in production, scripted in tests).
pub type EnvLookup<'a> = &'a dyn Fn(&str) -> Option<String>;

/// Reads from real process environment.
pub fn std_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

const ENV_EXPAND: &str = "TDMCP_TOEEXPAND";
const ENV_COLLAPSE: &str = "TDMCP_TOECOLLAPSE";
const ENV_TD_EXE: &str = "TDMCP_TOUCHDESIGNER_EXE";
const TOOL_NAMES: [&str; 2] = ["toeexpand", "toecollapse"];

fn validate_pair(expand: &Path, collapse: &Path) -> Option<OfficialTools> {
    if expand.is_file() && collapse.is_file() {
        Some(OfficialTools {
            expand: expand.to_path_buf(),
            collapse: collapse.to_path_buf(),
        })
    } else {
        None
    }
}

fn tools_beside(td_exe: &Path, name: &str) -> Option<PathBuf> {
    let bin = td_exe.parent()?;
    for candidate in [format!("{name}.exe"), name.to_string()] {
        let p = bin.join(candidate);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

fn scan_searched_log(roots: &[PathBuf], searched: &mut Vec<String>) {
    for root in roots {
        searched.push(format!("scan:{root:?}"));
    }
}

/// Resolve the official tools pair.
///
/// Order: explicit config pair (XOR violation is an error) → env pair → env/scan
/// `TouchDesigner.exe` candidates (newest version dir first). Every candidate is
/// validated by actual tool-file existence.
pub fn resolve_tools(
    src: &ToolSource,
    env: EnvLookup<'_>,
) -> Result<OfficialTools, ProjectIoError> {
    // 1. explicit config pair (XOR rule)
    match (&src.expand, &src.collapse) {
        (Some(e), Some(c)) => {
            if let Some(t) = validate_pair(e, c) {
                return Ok(t);
            }
            return Err(ProjectIoError::ToolMissing {
                tool: format!("{}+{}", TOOL_NAMES[0], TOOL_NAMES[1]),
                searched: vec![format!("config:{e:?}"), format!("config:{c:?}")],
            });
        }
        (Some(_), None) | (None, Some(_)) => return Err(ProjectIoError::ToolPairPartial),
        (None, None) => {}
    }

    let mut searched: Vec<String> = Vec::new();

    // 2. env pair
    if let (Some(e), Some(c)) = (env(ENV_EXPAND), env(ENV_COLLAPSE)) {
        let (e, c) = (PathBuf::from(&e), PathBuf::from(&c));
        if let Some(t) = validate_pair(&e, &c) {
            return Ok(t);
        }
        searched.push(format!("env:{e:?}"));
        searched.push(format!("env:{c:?}"));
    } else {
        if let Some(v) = env(ENV_EXPAND) {
            searched.push(format!("env:{v} (pair incomplete)"));
        }
        if let Some(v) = env(ENV_COLLAPSE) {
            searched.push(format!("env:{v} (pair incomplete)"));
        }
    }

    // 3. td_exe candidates: config → env → scan roots
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(exe) = &src.td_exe {
        candidates.push(exe.clone());
        searched.push(format!("config:{exe:?}"));
    }
    if let Some(v) = env(ENV_TD_EXE) {
        candidates.push(PathBuf::from(&v));
        searched.push(format!("env:{v}"));
    }
    let roots = default_scan_roots(env);
    scan_searched_log(&roots, &mut searched);
    for root in &roots {
        candidates.extend(scan_install_exes(root));
    }

    for exe in &candidates {
        if !exe.is_file() {
            continue;
        }
        if let (Some(e), Some(c)) = (
            tools_beside(exe, TOOL_NAMES[0]),
            tools_beside(exe, TOOL_NAMES[1]),
        ) {
            if let Some(t) = validate_pair(&e, &c) {
                tracing::info!(
                    expand = %t.expand.display(),
                    collapse = %t.collapse.display(),
                    "resolved official tools"
                );
                return Ok(t);
            }
        }
    }

    Err(ProjectIoError::ToolMissing {
        tool: TOOL_NAMES[0].to_string(),
        searched,
    })
}

/// Default scan roots for TouchDesigner installs on this platform.
pub fn default_scan_roots(env: EnvLookup<'_>) -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        windows_scan_roots(env)
    }
    #[cfg(target_os = "macos")]
    {
        let _ = env;
        macos_scan_roots()
    }
    #[cfg(all(not(windows), not(target_os = "macos")))]
    {
        let _ = env;
        Vec::new()
    }
}

#[cfg(windows)]
fn windows_scan_roots(env: EnvLookup<'_>) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for var in ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"] {
        if let Some(base) = env(var) {
            let root = PathBuf::from(base).join("Derivative");
            let key = root.to_string_lossy().to_lowercase();
            if roots
                .iter()
                .any(|r: &PathBuf| r.to_string_lossy().to_lowercase() == key)
            {
                continue;
            }
            roots.push(root);
        }
    }
    roots
}

#[cfg(target_os = "macos")]
fn macos_scan_roots() -> Vec<PathBuf> {
    let mut roots = vec![PathBuf::from("/Applications")];
    if let Some(home) = std::env::var_os("HOME") {
        roots.push(PathBuf::from(home).join("Applications"));
    }
    roots
}

/// Enumerate TouchDesigner executables under `root`, newest install first.
pub fn scan_install_exes(root: &Path) -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        windows_scan_install_exes(root)
    }
    #[cfg(target_os = "macos")]
    {
        macos_scan_install_exes(root)
    }
    #[cfg(all(not(windows), not(target_os = "macos")))]
    {
        let _ = root;
        Vec::new()
    }
}

#[cfg(windows)]
fn windows_scan_install_exes(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_dir()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("TouchDesigner"))
        })
        .collect();
    dirs.sort_unstable_by(|a, b| b.cmp(a));
    dirs.into_iter()
        .map(|d| d.join("bin").join("TouchDesigner.exe"))
        .collect()
}

#[cfg(target_os = "macos")]
fn macos_scan_install_exes(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut apps: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_dir()
                && p.extension().is_some_and(|e| e == "app")
                && p.file_stem()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("TouchDesigner"))
        })
        .collect();
    apps.sort_unstable_by(|a, b| b.cmp(a));
    apps.into_iter()
        .map(|app| app.join("Contents/MacOS/TouchDesigner"))
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;
    use std::fs;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[cfg(windows)]
    fn env_map<'a>(map: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name: &str| {
            map.iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| (*v).to_string())
        }
    }

    #[cfg(windows)]
    fn fake_install(root: &Path, version: &str, with_tools: bool) -> PathBuf {
        // Layout mirrors reality: <ProgramFiles>/Derivative/TouchDesigner.<v>/bin
        let dir = root
            .join("Derivative")
            .join(format!("TouchDesigner.{version}"))
            .join("bin");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("TouchDesigner.exe"), b"exe").unwrap();
        if with_tools {
            fs::write(dir.join("toeexpand.exe"), b"e").unwrap();
            fs::write(dir.join("toecollapse.exe"), b"c").unwrap();
        }
        dir.join("TouchDesigner.exe")
    }

    #[test]
    fn explicit_pair_wins_and_validates_files() {
        let tmp = tempfile::tempdir().unwrap();
        let e = tmp.path().join("e.exe");
        let c = tmp.path().join("c.exe");
        fs::write(&e, b"x").unwrap();
        fs::write(&c, b"x").unwrap();
        let src = ToolSource {
            expand: Some(e.clone()),
            collapse: Some(c.clone()),
            td_exe: None,
        };
        let t = resolve_tools(&src, &no_env).unwrap();
        assert_eq!(t.expand, e);
        assert_eq!(t.collapse, c);
    }

    #[test]
    fn partial_explicit_pair_is_rejected() {
        let src = ToolSource {
            expand: Some(PathBuf::from("only-expand")),
            collapse: None,
            td_exe: None,
        };
        assert!(matches!(
            resolve_tools(&src, &no_env),
            Err(ProjectIoError::ToolPairPartial)
        ));
    }

    #[cfg(windows)]
    #[test]
    fn env_pair_beats_scan() {
        let tmp = tempfile::tempdir().unwrap();
        fake_install(tmp.path(), "2099.99999", true);
        let e = tmp.path().join("env_e");
        let c = tmp.path().join("env_c");
        fs::write(&e, b"x").unwrap();
        fs::write(&c, b"x").unwrap();
        let binding = [
            ("TDMCP_TOEEXPAND", e.to_str().unwrap()),
            ("TDMCP_TOECOLLAPSE", c.to_str().unwrap()),
        ];
        let envf = env_map(&binding);
        // scan would find the 2099 install; env pair must win.
        let t = resolve_tools(&ToolSource::default(), &envf).unwrap();
        assert_eq!(t.expand, e);
    }

    #[cfg(windows)]
    #[test]
    fn scan_skips_stub_installs_and_prefers_newest_complete() {
        let pf = tempfile::tempdir().unwrap();
        let old = fake_install(pf.path(), "2025.32460", true);
        let stub = fake_install(pf.path(), "2025.33070", false); // probe lesson: no tools
        let binding = [("ProgramFiles", pf.path().to_str().unwrap())];
        let envf = env_map(&binding);
        let t = resolve_tools(&ToolSource::default(), &envf).unwrap();
        assert_eq!(t.expand.parent().unwrap(), old.parent().unwrap());
        assert_ne!(t.expand.parent().unwrap(), stub.parent().unwrap());
    }

    #[cfg(windows)]
    #[test]
    fn missing_everywhere_is_typed_tool_missing() {
        let empty = tempfile::tempdir().unwrap();
        let binding = [("ProgramFiles", empty.path().to_str().unwrap())];
        let envf = env_map(&binding);
        let err = resolve_tools(&ToolSource::default(), &envf).unwrap_err();
        assert!(matches!(err, ProjectIoError::ToolMissing { .. }));
    }

    #[cfg(windows)]
    #[test]
    fn scan_roots_dedup_aliasing_program_files() {
        let pf = tempfile::tempdir().unwrap();
        let binding = [
            ("ProgramFiles", pf.path().to_str().unwrap()),
            ("ProgramW6432", pf.path().to_str().unwrap()),
        ];
        let envf = env_map(&binding);
        assert_eq!(default_scan_roots(&envf).len(), 1);
    }

    #[test]
    fn install_root_from_app_bundle() {
        let app = PathBuf::from("/Applications/TouchDesigner.2025.32460.app");
        let exe = app.join("Contents/MacOS/TouchDesigner");
        assert_eq!(install_root_from_exe(&exe), app);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_scan_finds_newest_app_first() {
        let apps = tempfile::tempdir().unwrap();
        for (name, ver) in [
            ("TouchDesigner.2025.32460.app", "a"),
            ("TouchDesigner.2025.33070.app", "b"),
        ] {
            let macos = apps.path().join(name).join("Contents/MacOS");
            fs::create_dir_all(&macos).unwrap();
            fs::write(macos.join("TouchDesigner"), ver.as_bytes()).unwrap();
            fs::write(macos.join("toeexpand"), b"e").unwrap();
            fs::write(macos.join("toecollapse"), b"c").unwrap();
        }
        let exes = macos_scan_install_exes(apps.path());
        assert_eq!(exes.len(), 2);
        assert!(exes[0].to_string_lossy().contains("33070"));
        let info = inspect_install(&exes[0]);
        assert_eq!(
            info.root.file_name().and_then(|n| n.to_str()),
            Some("TouchDesigner.2025.33070.app")
        );
        assert!(info.toeexpand.is_some());
    }
}

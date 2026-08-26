//! `.toc` parsing + expand-dir sanity. Strict per V2-0 laws: LF-only, no BOM,
//! entries are root-relative with forward slashes, no escapes.

use std::path::{Path, PathBuf};

use crate::error::ProjectIoError;

/// Parse a `.toc` file into its ordered entries.
///
/// Rejects CRLF (silent-0-byte collapse law) and BOMs outright — a toc that
/// would poison `toecollapse` must never pass validation here.
pub fn parse(path: &Path) -> Result<Vec<String>, ProjectIoError> {
    let bytes = std::fs::read(path).map_err(|source| ProjectIoError::Fs {
        path: path.to_path_buf(),
        source,
    })?;
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return Err(ProjectIoError::TocInvalid {
            path: path.to_path_buf(),
            reason: "UTF-8 BOM not allowed".into(),
        });
    }
    if bytes.contains(&b'\r') {
        return Err(ProjectIoError::TocInvalid {
            path: path.to_path_buf(),
            reason: "CR found - toc must be strict LF".into(),
        });
    }
    let text = String::from_utf8_lossy(&bytes);
    let mut entries = Vec::new();
    for line in text.split('\n') {
        let line = line.strip_suffix('\n').unwrap_or(line);
        if line.is_empty() {
            continue;
        }
        entries.push(line.to_string());
    }
    Ok(entries)
}

/// Validate entries against their expand root: no absolute paths, no `..`
/// climbs, no backslashes. Returns [`ProjectIoError::TocEscape`] on violation.
pub fn validate_entries(root: &Path, entries: &[String]) -> Result<(), ProjectIoError> {
    for entry in entries {
        if entry.starts_with('\\')
            || entry.starts_with('/')
            || entry.contains('\\')
            || entry.contains("..")
            || entry.contains(':')
        {
            return Err(ProjectIoError::TocEscape {
                entry: entry.clone(),
            });
        }
        let joined = root.join(entry.replace('/', "\\"));
        // Path must stay inside root (redundant after the checks above; belt+braces).
        if !joined.starts_with(root) {
            return Err(ProjectIoError::TocEscape {
                entry: entry.clone(),
            });
        }
    }
    Ok(())
}

/// Expand-dir sanity: `.toc` sibling exists and parses, dir non-empty.
pub fn check_expand_dir(dir: &Path) -> Result<Vec<String>, ProjectIoError> {
    let toc = toc_path_for(dir);
    if !dir.is_dir() {
        return Err(ProjectIoError::SrcNotExpandDir {
            dir: dir.to_path_buf(),
            reason: "not a directory".into(),
        });
    }
    let entries = parse(&toc)?;
    if entries.is_empty() {
        return Err(ProjectIoError::SrcNotExpandDir {
            dir: dir.to_path_buf(),
            reason: "empty toc".into(),
        });
    }
    validate_entries(dir.parent().unwrap_or(dir), &entries)?;
    Ok(entries)
}

/// Sibling `.toc` path for an expand dir — official layout strips `.dir`:
/// `<name>.toe.dir` → `<name>.toe.toc` (V2-0 probe evidence).
#[must_use]
pub fn toc_path_for(dir: &Path) -> PathBuf {
    let s = dir.as_os_str().to_string_lossy();
    let base = s.strip_suffix(".dir").unwrap_or(&s);
    PathBuf::from(format!("{base}.toc"))
}

/// Read the TD build string from an expand dir's `.build`
/// (`build <version>` line, e.g. `2025.32460`). None when absent/unparsable.
#[must_use]
pub fn read_build(dir: &Path) -> Option<String> {
    let bytes = std::fs::read(dir.join(".build")).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("build ") {
            return Some(rest.trim().to_string());
        }
    }
    None
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn parses_strict_lf_entries() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.toc");
        fs::write(&p, b".build\nproject1.n\nlocal/maps.n\n").unwrap();
        let e = parse(&p).unwrap();
        assert_eq!(e, vec![".build", "project1.n", "local/maps.n"]);
    }

    #[test]
    fn rejects_crlf_and_bom() {
        let dir = tempfile::tempdir().unwrap();
        let crlf = dir.path().join("crlf.toc");
        fs::write(&crlf, b"a\nb\r\n").unwrap();
        assert!(matches!(
            parse(&crlf),
            Err(ProjectIoError::TocInvalid { .. })
        ));
        let bom = dir.path().join("bom.toc");
        fs::write(&bom, [0xEF, 0xBB, 0xBF, b'a', b'\n']).unwrap();
        assert!(matches!(
            parse(&bom),
            Err(ProjectIoError::TocInvalid { .. })
        ));
    }

    #[test]
    fn escape_attempts_rejected() {
        let root = tempfile::tempdir().unwrap();
        assert!(matches!(
            validate_entries(root.path(), &["../evil".into()]),
            Err(ProjectIoError::TocEscape { .. })
        ));
        assert!(matches!(
            validate_entries(root.path(), &["C:/abs/path.n".into()]),
            Err(ProjectIoError::TocEscape { .. })
        ));
        assert!(matches!(
            validate_entries(root.path(), &["a\\b.n".into()]),
            Err(ProjectIoError::TocEscape { .. })
        ));
        assert!(validate_entries(root.path(), &["ok.n".into(), "sub/dir/x.text".into()]).is_ok());
    }

    #[test]
    fn check_expand_dir_requires_toc_and_content() {
        let dir = tempfile::tempdir().unwrap();
        let ed = dir.path().join("p.toe.dir");
        fs::create_dir_all(ed.join("project1")).unwrap();
        // missing toc
        assert!(matches!(
            check_expand_dir(&ed),
            Err(ProjectIoError::SrcNotExpandDir { .. }) | Err(ProjectIoError::Fs { .. })
        ));
        fs::write(toc_path_for(&ed), b".build\nproject1.n\n").unwrap();
        fs::write(ed.join(".build"), b"version 099\n").unwrap();
        assert!(check_expand_dir(&ed).is_ok());
    }
}

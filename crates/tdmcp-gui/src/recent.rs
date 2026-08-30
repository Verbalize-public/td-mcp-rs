//! Recent-projects LRU list (≤16, deduped) persisted as JSON beside `data_dir`.
//! Keeps config.toml clean — recents are app state, not config.

use std::path::{Path, PathBuf};

const MAX: usize = 16;
const FILE: &str = "recent_projects.json";

fn file_path(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE)
}

#[allow(dead_code)]
pub(crate) fn load(data_dir: &Path) -> Vec<PathBuf> {
    let p = file_path(data_dir);
    let Ok(text) = std::fs::read_to_string(&p) else {
        return Vec::new();
    };
    let Ok(arr) = serde_json::from_str::<Vec<String>>(&text) else {
        return Vec::new();
    };
    arr.into_iter()
        .map(PathBuf::from)
        .filter(|p| {
            let s = p.to_string_lossy();
            !s.trim().is_empty()
        })
        .take(MAX)
        .collect()
}

pub(crate) fn save(data_dir: &Path, list: &[PathBuf]) {
    let p = file_path(data_dir);
    let arr: Vec<String> = list.iter().map(|p| p.display().to_string()).collect();
    if let Ok(text) = serde_json::to_string_pretty(&arr) {
        let _ = std::fs::write(p, text);
    }
}

pub(crate) fn push(list: &mut Vec<PathBuf>, path: PathBuf) {
    let norm = normalize(&path);
    list.retain(|p| normalize(p) != norm);
    list.insert(0, path);
    if list.len() > MAX {
        list.truncate(MAX);
    }
}

fn normalize(p: &Path) -> String {
    // Case-insensitive dedup on macOS/Windows, case-sensitive elsewhere.
    let s = p.to_string_lossy().to_string();
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        s.to_ascii_lowercase()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_deduplicates_and_caps() {
        let mut list = Vec::new();
        push(&mut list, PathBuf::from("/a/b.toe"));
        push(&mut list, PathBuf::from("/a/c.toe"));
        push(&mut list, PathBuf::from("/a/b.toe"));
        assert_eq!(list[0], PathBuf::from("/a/b.toe"));
        assert_eq!(list.len(), 2);
        for i in 0..20 {
            push(&mut list, PathBuf::from(format!("/tmp/{i}.toe")));
        }
        assert_eq!(list.len(), MAX);
        assert_eq!(list[0], PathBuf::from("/tmp/19.toe"));
    }
}

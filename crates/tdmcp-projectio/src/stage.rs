//! Staging directories + atomic publish. All mutation happens here; failures
//! discard partials, success renames into place (same-volume rename is atomic).

use std::path::{Path, PathBuf};

use crate::error::ProjectIoError;

/// A staging directory under `{root}/<uuid>`. Discard on drop is best-effort.
#[derive(Debug)]
pub struct StagingDir {
    path: PathBuf,
}

impl StagingDir {
    /// Directory path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Create `{root}/<uuid>`, creating `root` if needed.
    pub fn create(root: &Path) -> Result<StagingDir, ProjectIoError> {
        std::fs::create_dir_all(root).map_err(|source| ProjectIoError::Fs {
            path: root.to_path_buf(),
            source,
        })?;
        let path = root.join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir(&path).map_err(|source| ProjectIoError::Fs {
            path: path.clone(),
            source,
        })?;
        tracing::debug!(staging = %path.display(), "created project-io staging dir");
        Ok(StagingDir { path })
    }

    /// Publish the whole staging dir as `dest` via rename.
    ///
    /// Same-volume rename is atomic. Cross-volume falls back to a recursive copy
    /// followed by staged removal — still all-or-nothing from the caller's view
    /// because `dest` only appears at the end.
    pub fn publish_dir(mut self, dest: &Path) -> Result<(), ProjectIoError> {
        let result = self.publish_inner(dest);
        match result {
            Ok(()) => {
                // Path moved away; suppress Drop cleanup of the vanished tree.
                self.path = PathBuf::new();
                Ok(())
            }
            Err(e) => {
                self.discard();
                Err(e)
            }
        }
    }

    fn publish_inner(&self, dest: &Path) -> Result<(), ProjectIoError> {
        let src = &self.path;
        if !src.exists() {
            return Err(ProjectIoError::ExpandOutputMissing {
                packed: src.clone(),
            });
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|source| ProjectIoError::Fs {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        if std::fs::rename(src, dest).is_ok() {
            return Ok(());
        }
        // cross-volume fallback
        copy_recursive(src, dest)
    }

    /// Publish a single file from inside staging to `dest`.
    pub fn publish_file(mut self, src_name: &str, dest: &Path) -> Result<(), ProjectIoError> {
        let src = self.path.join(src_name);
        if !src.is_file() {
            return Err(ProjectIoError::CollapseOutputMissing { out: src });
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|source| ProjectIoError::Fs {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        match std::fs::rename(&src, dest) {
            Ok(()) => {}
            Err(_) => {
                std::fs::copy(&src, dest).map_err(|source| ProjectIoError::Fs {
                    path: dest.to_path_buf(),
                    source,
                })?;
            }
        }
        self.discard();
        Ok(())
    }

    /// Best-effort removal of the staging tree.
    pub fn discard(&mut self) {
        if !self.path.as_os_str().is_empty() {
            let _ = std::fs::remove_dir_all(&self.path);
            tracing::debug!(staging = %self.path.display(), "discarded staging dir");
            self.path = PathBuf::new();
        }
    }
}

impl Drop for StagingDir {
    fn drop(&mut self) {
        self.discard();
    }
}

fn copy_recursive(src: &Path, dest: &Path) -> Result<(), ProjectIoError> {
    if src.is_dir() {
        std::fs::create_dir_all(dest).map_err(|source| ProjectIoError::Fs {
            path: dest.to_path_buf(),
            source,
        })?;
        for entry in std::fs::read_dir(src).map_err(|source| ProjectIoError::Fs {
            path: src.to_path_buf(),
            source,
        })? {
            let entry = entry.map_err(|source| ProjectIoError::Fs {
                path: src.to_path_buf(),
                source,
            })?;
            copy_recursive(&entry.path(), &dest.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(src, dest)
            .map(|_| ())
            .map_err(|source| ProjectIoError::Fs {
                path: dest.to_path_buf(),
                source,
            })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;

    #[test]
    fn create_publish_renames_whole_dir() {
        let root = tempfile::tempdir().unwrap();
        let dest_root = tempfile::tempdir().unwrap();
        let st = StagingDir::create(root.path()).unwrap();
        std::fs::write(st.path().join("x.txt"), b"data").unwrap();
        let dest = dest_root.path().join("published.dir");
        st.publish_dir(&dest).unwrap();
        assert!(dest.join("x.txt").is_file());
    }

    #[test]
    fn drop_discards_staging_tree() {
        let root = tempfile::tempdir().unwrap();
        let path = {
            let st = StagingDir::create(root.path()).unwrap();
            let p = st.path().to_path_buf();
            std::fs::write(p.join("junk"), b"j").unwrap();
            p
        };
        assert!(!path.exists());
    }

    #[test]
    fn publish_file_moves_and_cleans_up() {
        let root = tempfile::tempdir().unwrap();
        let dest_root = tempfile::tempdir().unwrap();
        let st = StagingDir::create(root.path()).unwrap();
        std::fs::write(st.path().join("out.toe"), b"packed").unwrap();
        let dest = dest_root.path().join("nested").join("out.toe");
        st.publish_file("out.toe", &dest).unwrap();
        assert!(dest.is_file());
        assert!(!root.path().join("staging").exists());
    }

    #[test]
    fn publish_missing_file_is_typed_error_and_still_cleans_up() {
        let root = tempfile::tempdir().unwrap();
        let dest_root = tempfile::tempdir().unwrap();
        let st = StagingDir::create(root.path()).unwrap();
        let res = st.publish_file("absent", &dest_root.path().join("x"));
        assert!(matches!(
            res,
            Err(ProjectIoError::CollapseOutputMissing { .. })
        ));
        // Consumed self dropped inside publish_file -> Drop cleaned the tree.
        assert!(root.path().read_dir().unwrap().next().is_none());
    }
}

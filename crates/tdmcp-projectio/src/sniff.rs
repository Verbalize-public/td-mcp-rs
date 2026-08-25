//! Cheap packed-format pre-checks (magic sniffing).
//!
//! Observed layout (opendesigner `official.rs` + V2-0 probes):
//! - `.toe`: magic bytes `b"10"` at offset 0, u32be payload length at 2.
//! - `.tox`: u32be prefix (observed value 1), then the same `b"10"` magic at 4.
//!
//! This is a cheap "is this really a packed project" gate, not a parser.

use std::path::Path;

use crate::error::ProjectIoError;

/// Which packed container was detected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackedKind {
    /// TouchDesigner project file.
    Toe,
    /// TouchDesigner component file.
    Tox,
}

const TOE_MAGIC: [u8; 2] = *b"10";

fn has_magic(bytes: &[u8]) -> bool {
    bytes.len() >= 2 && bytes[0] == TOE_MAGIC[0] && bytes[1] == TOE_MAGIC[1]
}

/// Sniff a packed file's kind by its magic bytes.
///
/// Reads at most the first 8 bytes; any short read / garbage maps to
/// [`ProjectIoError::NotPackedFormat`].
pub fn sniff_packed(path: &Path) -> Result<PackedKind, ProjectIoError> {
    let file = std::fs::File::open(path).map_err(|source| ProjectIoError::Fs {
        path: path.to_path_buf(),
        source,
    })?;
    use std::io::Read;
    let mut head = [0u8; 8];
    let mut handle = file;
    let mut read = 0usize;
    while read < head.len() {
        match handle.read(&mut head[read..]) {
            Ok(0) => break,
            Ok(n) => read += n,
            Err(e) => {
                return Err(ProjectIoError::Fs {
                    path: path.to_path_buf(),
                    source: e,
                })
            }
        }
    }
    if has_magic(&head[0..2]) {
        return Ok(PackedKind::Toe);
    }
    if read >= 6 && has_magic(&head[4..6]) {
        return Ok(PackedKind::Tox);
    }
    Err(ProjectIoError::NotPackedFormat(path.to_path_buf()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_with(bytes: &[u8]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("probe.bin");
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(bytes).unwrap();
        dir
    }

    #[test]
    fn toe_magic_at_zero_is_toe() {
        let dir = temp_with(&[b'1', b'0', 0, 0, 0, 9, 1, 2]);
        assert_eq!(
            sniff_packed(&dir.path().join("probe.bin")).unwrap(),
            PackedKind::Toe
        );
    }

    #[test]
    fn tox_magic_after_u32_prefix_is_tox() {
        let dir = temp_with(&[0, 0, 0, 1, b'1', b'0', 7, 7]);
        assert_eq!(
            sniff_packed(&dir.path().join("probe.bin")).unwrap(),
            PackedKind::Tox
        );
    }

    #[test]
    fn garbage_is_not_packed_format() {
        let dir = temp_with(b"not a td file at all");
        assert!(matches!(
            sniff_packed(&dir.path().join("probe.bin")),
            Err(ProjectIoError::NotPackedFormat(_))
        ));
    }

    #[test]
    fn missing_file_is_fs_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            sniff_packed(&dir.path().join("absent.bin")),
            Err(ProjectIoError::Fs { .. })
        ));
    }
}

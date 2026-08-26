//! TouchDesigner `.text` sidecar codec (V2-0 reverse-engineered).
//!
//! Layout (verified against 2025.32460 output, sizes 0/60/3590/101500):
//! `"2\n"` + u32LE(42) + u32LE(1)×4 + tag `0x02` + u32BE(len) + UTF-8 body.
//! Header is exactly [`HEADER_LEN`] bytes. Legacy/plain sidecars (older
//! exports) have no envelope — [`parse`] passes those through untouched.

/// Sidecar header size in bytes.
pub const HEADER_LEN: usize = 27;

const MAGIC: &[u8; 2] = b"2\n";
/// Constant word observed in every sample (meaning unknown; reproduced verbatim).
const WORD_42: [u8; 4] = 42u32.to_le_bytes();
const ONE: [u8; 4] = 1u32.to_le_bytes();
const TAG_LEN: u8 = 0x02;

/// Extract the text payload from a `.text` sidecar.
///
/// Enveloped files return the decoded body; legacy plain files (no magic) are
/// returned as-is so old exports keep working.
#[must_use]
pub fn parse(bytes: &[u8]) -> Vec<u8> {
    if !matches_envelope(bytes) {
        return bytes.to_vec();
    }
    let len = u32::from_be_bytes([bytes[23], bytes[24], bytes[25], bytes[26]]) as usize;
    let start = HEADER_LEN;
    bytes
        .get(start..start + len)
        .unwrap_or(&bytes[start..])
        .to_vec()
}

/// Encode a payload as an enveloped sidecar (LF-normalized).
#[must_use]
pub fn encode(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&WORD_42);
    for _ in 0..4 {
        out.extend_from_slice(&ONE);
    }
    out.push(TAG_LEN);
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

fn matches_envelope(bytes: &[u8]) -> bool {
    bytes.len() >= HEADER_LEN
        && &bytes[0..2] == MAGIC
        && bytes[2..6] == WORD_42
        && (0..4).all(|i| bytes[6 + i * 4..10 + i * 4] == ONE)
        && bytes[22] == TAG_LEN
}

/// Normalize CRLF → LF (TD stores LF-only).
#[must_use]
pub fn normalize_lf(bytes: Vec<u8>) -> Vec<u8> {
    bytes.into_iter().filter(|b| *b != b'\r').collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn sample_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/v2-probes/r0/sample_text_envelope_60b.bin")
    }

    #[test]
    fn committed_sample_parses_to_known_payload() {
        let bytes = std::fs::read(sample_path()).unwrap();
        assert_eq!(bytes.len(), 87);
        let payload = parse(&bytes);
        assert_eq!(payload.len(), 60);
        assert!(payload.starts_with(b"# TDMCP_ENV_PROBE"));
    }

    #[test]
    fn writer_round_trips_committed_sample() {
        let bytes = std::fs::read(sample_path()).unwrap();
        let payload = parse(&bytes);
        let reencoded = encode(&payload);
        assert_eq!(reencoded, bytes, "writer must reproduce the exact envelope");
    }

    #[test]
    fn legacy_plain_files_pass_through() {
        let plain = b"class FxExt:\r\n    pass\r\n".to_vec();
        assert_eq!(parse(&plain), plain);
    }

    #[test]
    fn normalize_lf_strips_carriage_returns() {
        assert_eq!(normalize_lf(b"a\r\nb\r\n".to_vec()), b"a\nb\n".to_vec());
    }
}

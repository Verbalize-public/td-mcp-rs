//! Best-effort same-process fingerprint for pid-reuse detection.

use serde::{Deserialize, Serialize};

/// Process attrs used for best-effort same-process checks.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessFingerprint {
    /// Project identity hint (`project.name`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Process image / exe path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// Process start time (opaque string; OS-specific).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_time: Option<String>,
}

impl ProcessFingerprint {
    /// Best-effort match: true if fingerprints are compatible (not a hard guarantee).
    ///
    /// Mismatch if both sides have a conflicting non-empty field.
    ///
    /// When **both** sides lack `start_time` (common on macOS), require a
    /// positive shared `title` or `image` — empty/empty no longer counts as
    /// the same process (avoids false resurrection on pid reuse).
    #[must_use]
    pub fn matches(&self, other: &Self) -> bool {
        if !field_ok(&self.title, &other.title)
            || !field_ok(&self.image, &other.image)
            || !field_ok(&self.start_time, &other.start_time)
        {
            return false;
        }
        if self.start_time.is_none() && other.start_time.is_none() {
            return positive_match(&self.title, &other.title)
                || positive_match(&self.image, &other.image);
        }
        true
    }
}

fn field_ok(a: &Option<String>, b: &Option<String>) -> bool {
    match (a, b) {
        (Some(x), Some(y)) => x == y,
        _ => true,
    }
}

fn positive_match(a: &Option<String>, b: &Option<String>) -> bool {
    matches!((a, b), (Some(x), Some(y)) if x == y)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;

    #[test]
    fn mismatch_on_conflicting_title() {
        let a = ProcessFingerprint {
            title: Some("A".into()),
            ..Default::default()
        };
        let b = ProcessFingerprint {
            title: Some("B".into()),
            ..Default::default()
        };
        assert!(!a.matches(&b));
    }

    #[test]
    fn match_when_start_time_agrees_and_title_partial() {
        let a = ProcessFingerprint {
            title: Some("A".into()),
            start_time: Some("t0".into()),
            ..Default::default()
        };
        let b = ProcessFingerprint {
            start_time: Some("t0".into()),
            ..Default::default()
        };
        assert!(a.matches(&b));
    }

    #[test]
    fn empty_fingerprints_do_not_match() {
        assert!(!ProcessFingerprint::default().matches(&ProcessFingerprint::default()));
    }

    #[test]
    fn both_missing_start_require_shared_title_or_image() {
        let a = ProcessFingerprint {
            title: Some("proj".into()),
            ..Default::default()
        };
        let b = ProcessFingerprint {
            title: Some("proj".into()),
            ..Default::default()
        };
        assert!(a.matches(&b));

        let partial = ProcessFingerprint {
            title: Some("proj".into()),
            ..Default::default()
        };
        assert!(
            !partial.matches(&ProcessFingerprint::default()),
            "title vs empty is not a positive shared field"
        );
    }
}

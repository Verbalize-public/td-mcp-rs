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
    #[must_use]
    pub fn matches(&self, other: &Self) -> bool {
        field_ok(&self.title, &other.title)
            && field_ok(&self.image, &other.image)
            && field_ok(&self.start_time, &other.start_time)
    }
}

fn field_ok(a: &Option<String>, b: &Option<String>) -> bool {
    match (a, b) {
        (Some(x), Some(y)) => x == y,
        _ => true,
    }
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
    fn match_when_one_side_missing() {
        let a = ProcessFingerprint {
            title: Some("A".into()),
            ..Default::default()
        };
        let b = ProcessFingerprint::default();
        assert!(a.matches(&b));
    }
}

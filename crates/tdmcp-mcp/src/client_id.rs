//! Stable per-client identity for operation receipts and evidence grants.
//!
//! A receipt is only useful if the caller can find it again after a reconnect
//! or a daemon restart, and only safe if one client cannot read or overwrite
//! another's. That needs a principal that outlives a TCP connection.
//!
//! # What this actually establishes
//!
//! The transport has exactly one credential, and it is a *shared* secret:
//! `AuthState { mode, psk }`. Every holder of that PSK is one principal, so a
//! per-client label can never be derived from it. This module therefore keeps
//! two things apart that are easy to conflate:
//!
//! * the **scope** — which credential realm the caller proved, and which is
//!   authenticated when a PSK was verified;
//! * the **label** — which client inside that scope is speaking, and which is
//!   always self-asserted.
//!
//! [`Assurance`] reports that distinction instead of collapsing it into a
//! boolean. With `auth.mode = "none"` nothing is authenticated and an identity
//! only *identifies* a caller; the module says so rather than letting a receipt
//! imply a guarantee the transport never made.
//!
//! The scope is carried as a digest, never as the secret, so a raw PSK cannot
//! reach an owner id, a log line, or a database row.

use std::fmt::Write as _;

use crate::evidence::{EvidenceError, OwnerId};
use sha2::{Digest as _, Sha256};

/// Transport header carrying a client's install-scoped identity claim.
///
/// The client generates this once at install and sends it on every request, so
/// it survives reconnects and daemon restarts. Absent is allowed; present but
/// malformed is refused rather than silently downgraded to anonymous.
pub const CLIENT_ID_HEADER: &str = "x-tdmcp-client";

/// Label used when a client sends no claim at all.
///
/// Deliberately not unique: several claim-less clients on one daemon share this
/// principal, which is the truth of the situation rather than a fiction of
/// separateness.
pub const ANONYMOUS_LABEL: &str = "anonymous";

/// Longest accepted client label. Bounded so a claim cannot be used to grow
/// owner ids, log lines, or database keys without limit.
pub const MAX_LABEL_BYTES: usize = 64;

/// Bytes of credential digest kept in the scope. Enough to separate scopes,
/// short enough that the scope is not a password-equivalent to publish.
const SCOPE_DIGEST_BYTES: usize = 16;

/// How much the transport actually proved about a caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Assurance {
    /// A PSK was verified for this request.
    ///
    /// The *scope* is authenticated. The label is still self-asserted: two
    /// clients sharing one PSK remain distinguishable only by a claim either
    /// could have made.
    VerifiedCredential,
    /// `auth.mode = "none"`. Nothing was authenticated.
    ///
    /// The identity separates concurrent callers so receipts stay attributable.
    /// It is not a security boundary, and nothing may treat it as one.
    Unauthenticated,
}

/// The credential realm a caller proved, or the local realm when it proved
/// nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// A verified PSK, kept only as a digest.
    Credential(String),
    /// No credential was presented; access is local and unauthenticated.
    Local,
}

/// A client's stable, install-scoped identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientIdentity {
    label: String,
    scope: Scope,
    assurance: Assurance,
}

impl ClientIdentity {
    /// Resolve the identity of one request.
    ///
    /// `claim` is the raw [`CLIENT_ID_HEADER`] value, if the client sent one.
    /// An absent claim is not an error — it becomes [`ANONYMOUS_LABEL`] — but a
    /// present claim that is malformed is refused, so a broken or hostile
    /// client is visible rather than absorbed into the anonymous principal.
    ///
    /// # Errors
    /// Returns [`EvidenceError::InvalidInput`] when the claim is not a
    /// plausible client label.
    pub fn resolve(claim: Option<&str>, auth_mode: &str, psk: &str) -> Result<Self, EvidenceError> {
        let label = match claim {
            Some(claim) => validate_label(claim)?,
            None => ANONYMOUS_LABEL.to_owned(),
        };
        let (scope, assurance) = if auth_mode == "psk" && !psk.is_empty() {
            (
                Scope::Credential(credential_digest(psk)),
                Assurance::VerifiedCredential,
            )
        } else {
            (Scope::Local, Assurance::Unauthenticated)
        };
        Ok(Self {
            label,
            scope,
            assurance,
        })
    }

    /// The self-asserted client label.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// The credential realm, or [`Scope::Local`] when none was presented.
    #[must_use]
    pub fn scope(&self) -> &Scope {
        &self.scope
    }

    /// How much the transport proved about this caller.
    #[must_use]
    pub fn assurance(&self) -> Assurance {
        self.assurance
    }

    /// Whether a credential was verified for this caller.
    ///
    /// This is about the *scope* only. The label remains self-asserted either
    /// way, so a caller must not read this as "this client's label is
    /// trustworthy" — see [`Assurance`].
    #[must_use]
    pub fn is_authenticated(&self) -> bool {
        self.assurance == Assurance::VerifiedCredential
    }

    /// The owner id this identity records receipts under.
    ///
    /// # Errors
    /// Returns [`EvidenceError::InvalidInput`] if the composed id is not a
    /// legal opaque id. With a validated label this cannot happen, so the error
    /// is a fail-closed guard rather than a reachable path.
    pub fn owner_id(&self) -> Result<OwnerId, EvidenceError> {
        let value = match &self.scope {
            Scope::Credential(digest) => format!("client:{digest}:{}", self.label),
            Scope::Local => format!("local:{}", self.label),
        };
        OwnerId::parse(value)
    }
}

/// Accept only a bounded, control-free, separator-free label.
///
/// `:` is excluded so a label cannot be crafted to look like a different scope
/// in an owner id or a log line. A client that wants a stable identity sends a
/// UUID; anything more structured is a client-local convention this daemon
/// does not need to interpret.
fn validate_label(claim: &str) -> Result<String, EvidenceError> {
    let label = claim.trim();
    let rejected = |why: &str| {
        Err(EvidenceError::InvalidInput(format!(
            "client identity label {why} \
             (send 1-{MAX_LABEL_BYTES} characters from A-Z a-z 0-9 . _ -)"
        )))
    };
    if label.is_empty() {
        return rejected("must not be empty");
    }
    if label.len() > MAX_LABEL_BYTES {
        return rejected("is too long");
    }
    if !label
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return rejected("contains an unsupported character");
    }
    Ok(label.to_owned())
}

/// Digest a credential so the secret itself never becomes an identifier.
fn credential_digest(psk: &str) -> String {
    let digest = Sha256::digest(psk.as_bytes());
    let mut out = String::with_capacity(SCOPE_DIGEST_BYTES * 2);
    for byte in digest.iter().take(SCOPE_DIGEST_BYTES) {
        // Writing into a String is infallible.
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;

    #[test]
    fn a_verified_credential_identifies_a_client_inside_that_credential() {
        let identity =
            ClientIdentity::resolve(Some("2f1c8b40-1111-4222-8333-44445555"), "psk", "s3cret")
                .unwrap();
        assert_eq!(identity.label(), "2f1c8b40-1111-4222-8333-44445555");
        assert_eq!(identity.assurance(), Assurance::VerifiedCredential);
        assert!(identity.is_authenticated());
        let owner = identity.owner_id().unwrap().as_str().to_owned();
        assert!(owner.starts_with("client:"), "owner was {owner}");
        assert!(owner.ends_with(":2f1c8b40-1111-4222-8333-44445555"));
    }

    #[test]
    fn the_credential_never_appears_in_an_owner_id() {
        // A receipt owner id reaches database rows and log lines, so the
        // secret must not survive into one even indirectly.
        let identity = ClientIdentity::resolve(Some("client-a"), "psk", "correct horse").unwrap();
        let owner = identity.owner_id().unwrap().as_str().to_owned();
        assert!(!owner.contains("correct"), "owner was {owner}");
        assert!(!owner.contains("horse"), "owner was {owner}");
    }

    #[test]
    fn the_same_client_keeps_its_identity_across_reconnects() {
        // The point of a stable principal: a receipt written before a restart
        // must be findable after it, which only holds if the owner id is a
        // pure function of (credential, label).
        let first = ClientIdentity::resolve(Some("client-a"), "psk", "s3cret").unwrap();
        let second = ClientIdentity::resolve(Some("client-a"), "psk", "s3cret").unwrap();
        assert_eq!(first.owner_id().unwrap(), second.owner_id().unwrap());
    }

    #[test]
    fn two_clients_sharing_one_credential_still_hold_separate_receipts() {
        // Same PSK, different labels. This is attribution, not authentication:
        // either client could claim either label, so the owner ids separate
        // their receipts without proving who sent what.
        let a = ClientIdentity::resolve(Some("client-a"), "psk", "s3cret").unwrap();
        let b = ClientIdentity::resolve(Some("client-b"), "psk", "s3cret").unwrap();
        assert_ne!(a.owner_id().unwrap(), b.owner_id().unwrap());
        assert!(a.is_authenticated() && b.is_authenticated());
    }

    #[test]
    fn the_same_label_under_different_credentials_does_not_collide() {
        // Otherwise a client rotating its PSK would silently read a stranger's
        // receipts under a familiar name.
        let a = ClientIdentity::resolve(Some("client-a"), "psk", "first").unwrap();
        let b = ClientIdentity::resolve(Some("client-a"), "psk", "second").unwrap();
        assert_ne!(a.owner_id().unwrap(), b.owner_id().unwrap());
    }

    #[test]
    fn unauthenticated_mode_identifies_without_authenticating() {
        let identity = ClientIdentity::resolve(Some("client-a"), "none", "").unwrap();
        assert_eq!(identity.assurance(), Assurance::Unauthenticated);
        assert!(!identity.is_authenticated());
        assert!(identity.owner_id().unwrap().as_str().starts_with("local:"));
    }

    #[test]
    fn a_psk_mode_with_an_empty_secret_does_not_claim_authentication() {
        // Config can carry mode=psk with no secret. Claiming a verified
        // credential there would be a guarantee nobody checked.
        let identity = ClientIdentity::resolve(Some("client-a"), "psk", "").unwrap();
        assert!(!identity.is_authenticated());
    }

    #[test]
    fn an_absent_claim_is_anonymous_rather_than_an_error() {
        // An older client sends no header at all. That must keep working.
        let identity = ClientIdentity::resolve(None, "psk", "s3cret").unwrap();
        assert_eq!(identity.label(), ANONYMOUS_LABEL);
        assert!(identity.is_authenticated());
    }

    #[test]
    fn claim_less_clients_share_one_principal() {
        // Two unlabelled clients are genuinely indistinguishable, and the
        // owner ids say so instead of pretending to separate them.
        let first = ClientIdentity::resolve(None, "psk", "s3cret").unwrap();
        let second = ClientIdentity::resolve(None, "psk", "s3cret").unwrap();
        assert_eq!(first.owner_id().unwrap(), second.owner_id().unwrap());
    }

    #[test]
    fn a_malformed_claim_is_refused_rather_than_downgraded_to_anonymous() {
        // Silently folding a bad claim into the anonymous principal would hide
        // a broken client and could merge two different clients' receipts.
        for bad in [
            "",
            "   ",
            "has space",
            "colon:split",
            "slash/es",
            "nul\0byte",
            "emoji🙂",
        ] {
            let resolved = ClientIdentity::resolve(Some(bad), "psk", "s3cret");
            assert!(
                matches!(resolved, Err(EvidenceError::InvalidInput(_))),
                "expected {bad:?} to be refused, got {resolved:?}"
            );
        }
    }

    #[test]
    fn a_label_cannot_masquerade_as_another_scope() {
        // If ':' were legal, "client-<digest>-victim" could collide with a
        // real scoped owner id and read its receipts.
        let attacked = ClientIdentity::resolve(Some("client:abc:client-a"), "psk", "s3cret");
        assert!(matches!(attacked, Err(EvidenceError::InvalidInput(_))));
    }

    #[test]
    fn an_over_long_label_is_refused() {
        let long = "a".repeat(MAX_LABEL_BYTES + 1);
        assert!(matches!(
            ClientIdentity::resolve(Some(&long), "psk", "s3cret"),
            Err(EvidenceError::InvalidInput(_))
        ));
    }

    #[test]
    fn a_label_at_the_bound_is_accepted() {
        // The bound is a bound, not a coincidence: the longest legal label
        // must work, or a legitimate client is refused for no reason.
        let longest = "a".repeat(MAX_LABEL_BYTES);
        let identity = ClientIdentity::resolve(Some(&longest), "psk", "s3cret").unwrap();
        assert_eq!(identity.label(), longest);
    }

    #[test]
    fn surrounding_whitespace_is_trimmed_rather_than_rejected() {
        // HTTP header values often carry incidental spacing. Trimming keeps a
        // well-behaved client working without widening what is accepted.
        let identity = ClientIdentity::resolve(Some("  client-a \t"), "psk", "s3cret").unwrap();
        assert_eq!(identity.label(), "client-a");
    }
}

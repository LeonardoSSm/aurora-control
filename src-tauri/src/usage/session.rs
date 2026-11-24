//! Session identity, kept apart from display labels (audit B01).
//!
//! Adapters derive a [`Session`] from the provider's own local sign-in files.
//! Identities and credential versions are reduced to keyed fingerprints: the
//! key is random per process, so a fingerprint is meaningless outside this
//! process, and nothing here is serialized, logged or sent over IPC.

use std::collections::hash_map::RandomState;
use std::fmt;
use std::hash::BuildHasher;
use std::sync::OnceLock;

/// Opaque, process-local digest of an identifier.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct Fingerprint(u64);

impl Fingerprint {
    /// Digest of `parts`, in order. Parts are length-prefixed by `Hash`, so
    /// `["ab", "c"]` and `["a", "bc"]` differ.
    pub fn of(parts: &[&str]) -> Self {
        static KEY: OnceLock<RandomState> = OnceLock::new();
        Self(KEY.get_or_init(RandomState::new).hash_one(parts))
    }
}

impl fmt::Debug for Fingerprint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Fingerprint(…)")
    }
}

/// What the local sign-in files say about the current session.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Session {
    /// No usable sign-in: the provider's client is logged out, or uses a mode
    /// without subscription quotas (API key). Cached quotas no longer apply.
    SignedOut,
    /// A sign-in may exist, but its account cannot be verified (unreadable or
    /// partially written files, missing profile). Proves no change either way.
    #[default]
    Unverified,
    /// A verified account. `credential` changes when the provider's client
    /// renews or replaces the token, while `identity` stays the same.
    Account {
        identity: Fingerprint,
        credential: Fingerprint,
    },
}

impl Session {
    pub fn account(identity: &[&str], credential: &str) -> Self {
        Self::Account {
            identity: Fingerprint::of(identity),
            credential: Fingerprint::of(&[credential]),
        }
    }

    pub fn identity(&self) -> Option<Fingerprint> {
        match self {
            Self::Account { identity, .. } => Some(*identity),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprints_are_stable_within_the_process_and_hide_their_input() {
        assert_eq!(Fingerprint::of(&["a", "b"]), Fingerprint::of(&["a", "b"]));
        assert_ne!(Fingerprint::of(&["ab", "c"]), Fingerprint::of(&["a", "bc"]));
        assert_eq!(
            format!("{:?}", Fingerprint::of(&["sensitive"])),
            "Fingerprint(…)"
        );
        let session = Session::account(&["acct-sentinel"], "token-sentinel");
        let debug = format!("{session:?}");
        assert!(!debug.contains("sentinel"), "{debug}");
    }

    #[test]
    fn renewing_the_credential_keeps_the_identity() {
        let first = Session::account(&["acct"], "token-1");
        let renewed = Session::account(&["acct"], "token-2");
        assert_ne!(first, renewed);
        assert_eq!(first.identity(), renewed.identity());
        assert_ne!(
            first.identity(),
            Session::account(&["other"], "token-1").identity()
        );
    }
}

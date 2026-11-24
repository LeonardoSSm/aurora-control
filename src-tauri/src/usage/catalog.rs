//! The provider catalog: one list, shared with the frontend.
//!
//! `src/usage/catalog.json` is the single source of the providers, their
//! order, display names, defaults, capabilities and refresh cadence. The Rust
//! side embeds it at compile time; the React side imports the same file. A
//! test checks that it matches [`Provider::ALL`] exactly, so a provider cannot
//! exist in one language and not in the other.
//!
//! Visual identity (colors) lives in `src/styles.css`, keyed by provider id;
//! a frontend test checks that every catalog entry has one.

use std::sync::LazyLock;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::model::Provider;

/// How far the adapter's data source goes.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Integration {
    /// Reads real quota data from the provider.
    Live,
    /// Detects the local sign-in only: no authorized quota source exists.
    AuthOnly,
    /// A quota source is implemented but switched off until its terms of use
    /// are verified (see `docs/provider-compatibility.md`).
    Gated,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Capability {
    SubscriptionWindows,
    ModelQuotas,
    Credits,
    LocalActivity,
    SessionLogFallback,
    Diagnostics,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Entry {
    pub id: Provider,
    pub name: String,
    pub vendor: String,
    /// New providers stay off until the user enables them.
    pub default_enabled: bool,
    pub refresh_seconds: u64,
    pub integration: Integration,
    pub capabilities: Vec<Capability>,
    /// What the user does to restore monitoring after a sign-in problem.
    pub reconnect: String,
}

static CATALOG: LazyLock<Vec<Entry>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../../../src/usage/catalog.json"))
        .expect("src/usage/catalog.json is validated by the catalog tests")
});

/// Every provider, in display order.
#[cfg(test)]
pub fn entries() -> &'static [Entry] {
    &CATALOG
}

pub fn entry(provider: Provider) -> &'static Entry {
    CATALOG
        .iter()
        .find(|entry| entry.id == provider)
        .expect("every provider has a catalog entry")
}

pub fn name(provider: Provider) -> &'static str {
    &entry(provider).name
}

/// Regular refresh interval, bounded to sane values.
pub fn refresh_interval(provider: Provider) -> Duration {
    Duration::from_secs(entry(provider).refresh_seconds.clamp(60, 3_600))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shared_catalog_matches_the_provider_enum_in_order() {
        let ids: Vec<Provider> = entries().iter().map(|entry| entry.id).collect();
        assert_eq!(ids, Provider::ALL);
        for entry in entries() {
            assert!(!entry.name.is_empty() && !entry.reconnect.is_empty());
            assert!((60..=3_600).contains(&entry.refresh_seconds));
        }
    }

    #[test]
    fn only_the_providers_of_the_previous_version_start_enabled() {
        let enabled: Vec<Provider> = entries()
            .iter()
            .filter(|entry| entry.default_enabled)
            .map(|entry| entry.id)
            .collect();
        assert_eq!(enabled, [Provider::Codex, Provider::ClaudeCode]);
        assert_eq!(entry(Provider::Gemini).integration, Integration::AuthOnly);
        assert_eq!(entry(Provider::Grok).integration, Integration::Gated);
        assert_eq!(name(Provider::ClaudeCode), "Claude Code");
    }
}

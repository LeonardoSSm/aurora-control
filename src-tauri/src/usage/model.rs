//! Wire contract shared with `src/usage/model.ts`. Serialized in camelCase.
//! Nothing in this module may carry a credential: it is sent to the WebView.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Provider {
    Codex,
    ClaudeCode,
}

/// Primary metric. `stale` keeps the last valid percentage.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Reading {
    Available { percent_used: u8 },
    Stale { percent_used: u8 },
    Unavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Source {
    Demo,
    Live,
}

/// Where the numbers of the last valid reading came from.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Origin {
    /// Provider usage endpoint, queried by this app.
    Api,
    /// Rate-limit record written by the provider's own client to its local session log.
    SessionLog,
    Fixture,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum QuotaKind {
    /// Rolling subscription window (for example 5 hours or 7 days).
    Window,
    /// Paid credits or extra usage on top of the subscription.
    Credits,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Quota {
    /// Stable identifier, such as `fiveHour` or `sevenDayOpus`.
    pub id: String,
    pub label: String,
    pub kind: QuotaKind,
    pub percent_used: u8,
    pub duration_minutes: Option<u32>,
    /// RFC 3339 with a UTC offset when the renewal time is known.
    pub resets_at: Option<String>,
}

/// The quota drawn by the ring. Explicitly identified by `id` and `label`.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsumptionWindow {
    pub id: String,
    pub label: String,
    pub duration_minutes: Option<u32>,
    /// RFC 3339 with a UTC offset when the renewal time is known.
    pub resets_at: Option<String>,
}

/// Token activity of local sessions. Not an account quota.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalActivity {
    pub window_minutes: u32,
    pub tokens: u64,
    pub sessions: u32,
}

/// Why the latest attempt did not produce a fresh reading.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(
    tag = "code",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Issue {
    /// No local sign-in for the provider.
    AuthMissing,
    /// The local sign-in expired. The provider's own client must renew it.
    AuthExpired,
    /// The provider rejected the credential (HTTP 401 or 403).
    AuthRejected,
    /// The account type has no quota data source (for example an API key).
    Unsupported,
    /// HTTP 429. `retry_at` is the earliest next request.
    RateLimited {
        retry_at: Option<String>,
    },
    Network,
    Timeout,
    /// The response did not match the expected schema.
    Malformed,
    Http {
        status: u16,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub provider: Provider,
    /// Display label (masked e-mail or account name), or absent when unknown.
    pub account: Option<String>,
    pub plan: Option<String>,
    pub source: Source,
    pub origin: Option<Origin>,
    #[serde(flatten)]
    pub reading: Reading,
    /// The metric drawn by the ring.
    pub window: Option<ConsumptionWindow>,
    /// The remaining quotas, excluding the primary one.
    pub quotas: Vec<Quota>,
    pub activity: Option<LocalActivity>,
    /// RFC 3339 for the observation time of the last valid reading.
    pub updated_at: Option<String>,
    /// RFC 3339 for the latest attempt, successful or not.
    pub checked_at: Option<String>,
    pub issue: Option<Issue>,
    pub refreshing: bool,
}

pub type Snapshot = Vec<Usage>;

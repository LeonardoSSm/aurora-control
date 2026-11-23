use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Provider {
    Codex,
    ClaudeCode,
}

// These variants define the wire contract for future provider adapters.
#[allow(dead_code)]
#[derive(Clone, Debug, Serialize)]
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

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Source {
    Demo,
    Live,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsumptionWindow {
    pub duration_minutes: u32,
    /// RFC 3339 with a UTC offset when the renewal time is known.
    pub resets_at: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub provider: Provider,
    /// Public-facing label, or absent when the account is unknown.
    pub account: Option<String>,
    pub source: Source,
    pub window: Option<ConsumptionWindow>,
    /// RFC 3339 with a UTC offset for the last successful reading.
    pub updated_at: Option<String>,
    #[serde(flatten)]
    pub reading: Reading,
}

pub type Snapshot = Vec<Usage>;

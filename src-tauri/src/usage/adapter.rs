//! Provider adapter contract. Adapters detect the local sign-in, query the
//! provider, and normalize the result. Credentials never leave the adapter.

use std::time::Duration;

use chrono::{DateTime, Utc};

use super::model::{ConsumptionWindow, LocalActivity, Origin, Provider, Quota, Source};

/// A normalized reading, before the scheduler decides its freshness.
#[derive(Clone, Debug, PartialEq)]
pub struct Fetched {
    pub account: Option<String>,
    pub plan: Option<String>,
    pub origin: Origin,
    /// All known quotas. The scheduler picks the primary one.
    pub quotas: Vec<Quota>,
    /// When the provider observed these numbers.
    pub observed_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FetchError {
    AuthMissing,
    AuthExpired,
    AuthRejected,
    Unsupported,
    RateLimited { retry_after: Option<Duration> },
    Network,
    Timeout,
    Malformed(String),
    Http(u16),
}

/// Everything an adapter learned during one attempt.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outcome {
    pub result: Option<Result<Fetched, FetchError>>,
    /// Cached data the provider's own client stored locally. Used when the
    /// live query fails.
    pub fallback: Option<Fetched>,
    /// Account and plan labels known even without a quota reading.
    pub account: Option<String>,
    pub plan: Option<String>,
    pub activity: Option<LocalActivity>,
}

impl Outcome {
    pub fn failed(error: FetchError) -> Self {
        Self {
            result: Some(Err(error)),
            ..Self::default()
        }
    }
}

pub trait UsageAdapter: Send + Sync + 'static {
    fn provider(&self) -> Provider;
    fn fetch(&self, now: DateTime<Utc>) -> Outcome;
    fn source(&self) -> Source {
        Source::Live
    }
}

/// The most restrictive window is the primary metric: it is the one closest
/// to blocking the user. Ties favor the shorter window.
pub fn primary_index(quotas: &[Quota]) -> Option<usize> {
    quotas
        .iter()
        .enumerate()
        .filter(|(_, quota)| quota.kind == super::model::QuotaKind::Window)
        .max_by(|(_, a), (_, b)| {
            a.percent_used.cmp(&b.percent_used).then_with(|| {
                b.duration_minutes
                    .unwrap_or(u32::MAX)
                    .cmp(&a.duration_minutes.unwrap_or(u32::MAX))
            })
        })
        .map(|(index, _)| index)
}

pub fn split_primary(mut quotas: Vec<Quota>) -> (Option<(u8, ConsumptionWindow)>, Vec<Quota>) {
    match primary_index(&quotas) {
        Some(index) => {
            let primary = quotas.remove(index);
            (
                Some((
                    primary.percent_used,
                    ConsumptionWindow {
                        id: primary.id,
                        label: primary.label,
                        duration_minutes: primary.duration_minutes,
                        resets_at: primary.resets_at,
                    },
                )),
                quotas,
            )
        }
        None => (None, quotas),
    }
}

pub fn percent(value: f64) -> u8 {
    if value.is_finite() {
        value.round().clamp(0.0, 100.0) as u8
    } else {
        0
    }
}

pub fn rfc3339(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Masks the local part of an e-mail address so the always-on-top window does
/// not expose it in screen shares. Other labels are returned unchanged.
pub fn mask_account(value: &str) -> String {
    let value = value.trim();
    match value.split_once('@') {
        Some((local, domain)) if !local.is_empty() && !domain.is_empty() => {
            let first: String = local.chars().take(1).collect();
            format!("{first}•••@{domain}")
        }
        _ => value.to_string(),
    }
}

pub fn title_case(plan: &str) -> String {
    let plan = plan.trim().replace(['_', '-'], " ");
    let mut chars = plan.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::model::QuotaKind;

    fn quota(id: &str, percent_used: u8, minutes: u32) -> Quota {
        Quota {
            id: id.into(),
            label: id.into(),
            kind: QuotaKind::Window,
            percent_used,
            duration_minutes: Some(minutes),
            resets_at: None,
        }
    }

    #[test]
    fn primary_is_the_most_restrictive_window_and_ties_favor_the_shorter() {
        let quotas = vec![quota("fiveHour", 0, 300), quota("sevenDay", 73, 10_080)];
        let (primary, rest) = split_primary(quotas);
        let (percent_used, window) = primary.unwrap();
        assert_eq!((percent_used, window.id.as_str()), (73, "sevenDay"));
        assert_eq!(rest.len(), 1);

        let quotas = vec![quota("sevenDay", 40, 10_080), quota("fiveHour", 40, 300)];
        assert_eq!(primary_index(&quotas), Some(1));
    }

    #[test]
    fn credits_never_become_the_primary_metric() {
        let mut credits = quota("extraUsage", 90, 0);
        credits.kind = QuotaKind::Credits;
        assert_eq!(primary_index(&[credits]), None);
    }

    #[test]
    fn masks_email_and_clamps_percentages() {
        assert_eq!(mask_account("dev@example.com"), "d•••@example.com");
        assert_eq!(mask_account("Equipe Aurora"), "Equipe Aurora");
        assert_eq!(percent(-3.0), 0);
        assert_eq!(percent(100.6), 100);
        assert_eq!(percent(f64::NAN), 0);
        assert_eq!(title_case("max_5x"), "Max 5x");
    }
}

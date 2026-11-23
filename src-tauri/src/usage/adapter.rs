//! Provider adapter contract. Adapters detect the local sign-in, query the
//! provider, and normalize the result. Credentials never leave the adapter.

use std::time::Duration;

use chrono::{DateTime, Utc};

use super::model::{ConsumptionWindow, LocalActivity, Origin, Provider, Quota, QuotaKind, Source};
use super::session::{Fingerprint, Session};

/// A normalized reading, before the scheduler decides its freshness.
#[derive(Clone, Debug, PartialEq)]
pub struct Fetched {
    /// The account these quotas belong to. `None` when no verifiable link to
    /// an account exists, for example a local session log.
    pub identity: Option<Fingerprint>,
    /// Display label. Never used to decide which account the data belongs to.
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
    /// The credential lives where this app does not read (system store,
    /// host application). Not proof of a logout.
    CredentialsUnavailable,
    /// The local credential file is unreadable, oversized or malformed.
    CredentialsInvalid,
    RateLimited {
        retry_after: Option<Duration>,
    },
    Network,
    Timeout,
    /// The provider's response has no trustworthy semantics.
    Malformed(String),
    Http {
        status: u16,
        retry_after: Option<Duration>,
    },
}

/// Everything an adapter learned during one attempt.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outcome {
    /// The session found in the local sign-in files during this attempt.
    pub session: Session,
    pub result: Option<Result<Fetched, FetchError>>,
    /// Cached data the provider's own client stored locally. Used when the
    /// live query fails. Never attributed to the current account.
    pub fallback: Option<Fetched>,
    /// Labels of the current session, known even without a quota reading.
    /// Display only.
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

    /// No usable sign-in: the cached quotas of the previous session are void.
    pub fn signed_out(error: FetchError) -> Self {
        Self {
            session: Session::SignedOut,
            ..Self::failed(error)
        }
    }
}

pub trait UsageAdapter: Send + Sync + 'static {
    fn provider(&self) -> Provider;
    fn fetch(&self, now: DateTime<Utc>) -> Outcome;
    fn source(&self) -> Source {
        Source::Live
    }
    /// The current session from local files only, without network access.
    /// Lets the scheduler notice a logout, an account switch or a renewed
    /// credential before deciding whether a manual refresh may run early.
    fn session(&self, _now: DateTime<Utc>) -> Option<Session> {
        None
    }
}

/// The most restrictive window is the primary metric: it is the one closest
/// to blocking the user. Ties favor the shorter window. Only eligible windows
/// qualify, and a window renewed without a new measurement yields to any
/// window that still has one (audit I02, I08).
pub fn primary_index(quotas: &[Quota]) -> Option<usize> {
    let candidates = || {
        quotas
            .iter()
            .enumerate()
            .filter(|(_, quota)| quota.kind == QuotaKind::Window && quota.eligible)
    };
    let measured = candidates().any(|(_, quota)| !quota.renewed);
    candidates()
        .filter(|(_, quota)| !measured || !quota.renewed)
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
                        renewed: primary.renewed,
                    },
                )),
                quotas,
            )
        }
        None => (None, quotas),
    }
}

/// A provider percentage, or `None` outside the documented 0–100 domain.
/// Half a point of tolerance absorbs rounding by the provider.
pub fn percent(value: f64) -> Option<u8> {
    (value.is_finite() && (-0.5..=100.5).contains(&value))
        .then(|| value.round().clamp(0.0, 100.0) as u8)
}

/// Reports a response that deviates from the known schema. Only the field
/// path is logged, never values.
pub fn schema_anomaly(provider: Provider, what: &str) {
    eprintln!("Aurora Control: {provider:?} response anomaly: {what}");
}
pub fn rfc3339(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Masks the local part of an e-mail address so the always-on-top window does
/// not expose it in screen shares. Other labels are returned unchanged.
pub fn mask_account(value: &str) -> String {
    let value = value.trim();
    let masked = match value.split_once('@') {
        Some((local, domain)) if !local.is_empty() && !domain.is_empty() => {
            let first: String = local.chars().take(1).collect();
            format!("{first}•••@{domain}")
        }
        _ => value.to_string(),
    };
    display_label(&masked).unwrap_or_default()
}

/// Longest label sent to the WebView, in characters.
pub const MAX_LABEL: usize = 48;

/// A display label without control characters, with collapsed whitespace and
/// at most [`MAX_LABEL`] characters (audit M02). `None` when nothing remains.
pub fn display_label(value: &str) -> Option<String> {
    let cleaned: String = value
        .split(|character: char| character.is_whitespace() || character.is_control())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let mut label: String = cleaned.chars().take(MAX_LABEL).collect();
    if cleaned.chars().count() > MAX_LABEL {
        label.pop();
        label.push('…');
    }
    (!label.is_empty()).then_some(label)
}

pub fn title_case(plan: &str) -> String {
    let plan = plan.trim().replace(['_', '-'], " ");
    let mut chars = plan.chars();
    let titled: String = match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    };
    display_label(&titled).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quota(id: &str, percent_used: u8, minutes: u32) -> Quota {
        Quota::window(id, id, percent_used, Some(minutes), None)
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
    fn i08_ineligible_metrics_are_listed_but_never_drawn() {
        let mut future = quota("sevenDayCowork", 99, 10_080);
        future.eligible = false;
        assert_eq!(primary_index(&[future.clone()]), None);
        assert_eq!(
            primary_index(&[future, quota("fiveHour", 10, 300)]),
            Some(1)
        );
    }

    #[test]
    fn i02_a_renewed_window_yields_to_any_measured_window() {
        let mut renewed = quota("fiveHour", 0, 300);
        renewed.renewed = true;
        let quotas = vec![renewed.clone(), quota("sevenDay", 0, 10_080)];
        // On a tie the shorter window would win, but it has no measurement.
        assert_eq!(primary_index(&quotas), Some(1));
        // When every window renewed, the primary is kept and flagged.
        let (primary, _) = split_primary(vec![renewed]);
        assert!(primary.unwrap().1.renewed);
    }

    #[test]
    fn masks_email_and_validates_percentages() {
        assert_eq!(mask_account("dev@example.com"), "d•••@example.com");
        assert_eq!(mask_account("Equipe Aurora"), "Equipe Aurora");
        assert_eq!(percent(12.6), Some(13));
        assert_eq!(percent(100.4), Some(100));
        assert_eq!(percent(-0.4), Some(0));
        assert_eq!(percent(-3.0), None);
        assert_eq!(percent(100.6), None);
        assert_eq!(percent(f64::NAN), None);
        assert_eq!(percent(f64::INFINITY), None);
        assert_eq!(title_case("max_5x"), "Max 5x");
    }

    #[test]
    fn m02_labels_are_bounded_and_stripped_of_control_characters() {
        assert_eq!(
            display_label("  Equipe\u{7}\nAurora "),
            Some("Equipe Aurora".into())
        );
        assert_eq!(display_label("\u{0}\t "), None);
        let long = "x".repeat(200);
        assert_eq!(display_label(&long).unwrap().chars().count(), MAX_LABEL);
        let masked = mask_account(&format!("dev@{}.com", "d".repeat(200)));
        assert!(masked.chars().count() <= MAX_LABEL, "{masked}");
    }
}

//! Refresh policy and freshness for one provider. Pure logic with an injected
//! clock, so every timing rule is unit-tested.

use std::time::Duration;

use chrono::{DateTime, Utc};

use super::adapter::{FetchError, Fetched, Outcome, rfc3339, split_primary};
use super::model::{Issue, LocalActivity, Provider, Quota, Reading, Source, Usage};

#[derive(Clone, Copy, Debug)]
pub struct Policy {
    /// Regular refresh interval.
    pub interval: Duration,
    /// Re-check cadence for local sign-in problems (file reads only).
    pub local_recheck: Duration,
    /// Minimum gap between attempts started by the user.
    pub manual_gap: Duration,
    /// Age after which a valid reading is shown as stale.
    pub stale_after: Duration,
    pub backoff_min: Duration,
    pub backoff_max: Duration,
    /// Minimum wait after HTTP 429, even without `Retry-After`.
    pub rate_limit_floor: Duration,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(5 * 60),
            local_recheck: Duration::from_secs(2 * 60),
            manual_gap: Duration::from_secs(20),
            stale_after: Duration::from_secs(15 * 60),
            backoff_min: Duration::from_secs(60),
            backoff_max: Duration::from_secs(30 * 60),
            rate_limit_floor: Duration::from_secs(60),
        }
    }
}

fn delta(duration: Duration) -> chrono::TimeDelta {
    chrono::TimeDelta::from_std(duration).unwrap_or(chrono::TimeDelta::MAX)
}

pub struct Tracker {
    provider: Provider,
    source: Source,
    policy: Policy,
    last_good: Option<Fetched>,
    account: Option<String>,
    plan: Option<String>,
    activity: Option<LocalActivity>,
    issue: Option<Issue>,
    failures: u32,
    next_due: DateTime<Utc>,
    not_before: Option<DateTime<Utc>>,
    last_attempt: Option<DateTime<Utc>>,
    in_flight: bool,
}

impl Tracker {
    pub fn new(provider: Provider, source: Source, policy: Policy, now: DateTime<Utc>) -> Self {
        Self {
            provider,
            source,
            policy,
            last_good: None,
            account: None,
            plan: None,
            activity: None,
            issue: None,
            failures: 0,
            next_due: now,
            not_before: None,
            last_attempt: None,
            in_flight: false,
        }
    }

    pub fn next_due(&self) -> DateTime<Utc> {
        self.next_due
    }

    /// Prevents concurrent queries, honors `Retry-After` for every trigger,
    /// and debounces repeated manual requests.
    pub fn should_fetch(&self, now: DateTime<Utc>, manual: bool) -> bool {
        if self.in_flight || self.not_before.is_some_and(|gate| now < gate) {
            return false;
        }
        if manual {
            self.last_attempt
                .is_none_or(|last| now - last >= delta(self.policy.manual_gap))
        } else {
            now >= self.next_due
        }
    }

    pub fn begin(&mut self, now: DateTime<Utc>) {
        self.in_flight = true;
        self.last_attempt = Some(now);
    }

    fn backoff(&self) -> Duration {
        let exponent = self.failures.saturating_sub(1).min(16);
        self.policy
            .backoff_min
            .saturating_mul(1 << exponent)
            .min(self.policy.backoff_max)
    }

    pub fn finish(&mut self, outcome: Outcome, now: DateTime<Utc>) {
        self.in_flight = false;
        if outcome.account.is_some() {
            self.account = outcome.account;
        }
        if outcome.plan.is_some() {
            self.plan = outcome.plan;
        }
        if outcome.activity.is_some() {
            self.activity = outcome.activity;
        }

        let Some(result) = outcome.result else {
            self.next_due = now + delta(self.policy.interval);
            return;
        };
        match result {
            Ok(fetched) => {
                self.last_good = Some(fetched);
                self.issue = None;
                self.failures = 0;
                self.not_before = None;
                self.next_due = now + delta(self.policy.interval);
            }
            Err(error) => {
                self.failures = self.failures.saturating_add(1);
                if let Some(fallback) = outcome.fallback {
                    let newer = self
                        .last_good
                        .as_ref()
                        .is_none_or(|good| fallback.observed_at > good.observed_at);
                    if newer {
                        self.last_good = Some(fallback);
                    }
                }
                let wait = match &error {
                    FetchError::AuthMissing | FetchError::AuthExpired | FetchError::Unsupported => {
                        self.policy.local_recheck
                    }
                    FetchError::AuthRejected | FetchError::Malformed(_) => {
                        self.backoff().max(self.policy.interval)
                    }
                    FetchError::RateLimited { retry_after } => {
                        let wait = retry_after
                            .unwrap_or_default()
                            .max(self.backoff())
                            .max(self.policy.rate_limit_floor);
                        self.not_before = Some(now + delta(wait));
                        wait
                    }
                    FetchError::Network | FetchError::Timeout | FetchError::Http(_) => {
                        self.backoff()
                    }
                };
                self.next_due = now + delta(wait);
                self.issue = Some(match error {
                    FetchError::AuthMissing => Issue::AuthMissing,
                    FetchError::AuthExpired => Issue::AuthExpired,
                    FetchError::AuthRejected => Issue::AuthRejected,
                    FetchError::Unsupported => Issue::Unsupported,
                    FetchError::RateLimited { .. } => Issue::RateLimited {
                        retry_at: self.not_before.map(rfc3339),
                    },
                    FetchError::Network => Issue::Network,
                    FetchError::Timeout => Issue::Timeout,
                    FetchError::Malformed(_) => Issue::Malformed,
                    FetchError::Http(status) => Issue::Http { status },
                });
            }
        }
    }

    pub fn usage(&self, now: DateTime<Utc>) -> Usage {
        let good = self.last_good.as_ref();
        let quotas: Vec<Quota> = good
            .map(|good| {
                good.quotas
                    .iter()
                    .map(|quota| roll_over(quota, now))
                    .collect()
            })
            .unwrap_or_default();
        let (primary, quotas) = split_primary(quotas);
        let fresh = self.issue.is_none()
            && good.is_some_and(|good| now - good.observed_at <= delta(self.policy.stale_after));
        let (reading, window) = match primary {
            Some((percent_used, window)) if fresh => {
                (Reading::Available { percent_used }, Some(window))
            }
            Some((percent_used, window)) => (Reading::Stale { percent_used }, Some(window)),
            None => (Reading::Unavailable, None),
        };

        Usage {
            provider: self.provider,
            account: good
                .and_then(|good| good.account.clone())
                .or_else(|| self.account.clone()),
            plan: good
                .and_then(|good| good.plan.clone())
                .or_else(|| self.plan.clone()),
            source: self.source,
            origin: good.map(|good| good.origin),
            reading,
            window,
            quotas,
            activity: self.activity.clone(),
            updated_at: good.map(|good| rfc3339(good.observed_at)),
            checked_at: self.last_attempt.map(rfc3339),
            issue: self.issue.clone(),
            refreshing: self.in_flight,
        }
    }
}

/// A stored window whose renewal time has passed no longer describes the
/// current consumption. Its percentage restarts and the next renewal is unknown.
fn roll_over(quota: &Quota, now: DateTime<Utc>) -> Quota {
    let expired = quota
        .resets_at
        .as_deref()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .is_some_and(|reset| reset <= now);
    if expired {
        Quota {
            percent_used: 0,
            resets_at: None,
            ..quota.clone()
        }
    } else {
        quota.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::model::{Origin, QuotaKind};
    use chrono::TimeZone;

    fn t0() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 23, 18, 0, 0).unwrap()
    }

    fn secs(seconds: i64) -> chrono::TimeDelta {
        chrono::TimeDelta::seconds(seconds)
    }

    fn fetched(percent: u8, at: DateTime<Utc>, origin: Origin) -> Fetched {
        Fetched {
            account: Some("d•••@example.com".into()),
            plan: Some("Plus".into()),
            origin,
            quotas: vec![
                Quota {
                    id: "fiveHour".into(),
                    label: "Sessão de 5 h".into(),
                    kind: QuotaKind::Window,
                    percent_used: 10,
                    duration_minutes: Some(300),
                    resets_at: Some(rfc3339(at + secs(3600))),
                },
                Quota {
                    id: "sevenDay".into(),
                    label: "Semanal".into(),
                    kind: QuotaKind::Window,
                    percent_used: percent,
                    duration_minutes: Some(10_080),
                    resets_at: Some(rfc3339(at + secs(86_400))),
                },
            ],
            observed_at: at,
        }
    }

    fn ok(fetched: Fetched) -> Outcome {
        Outcome {
            result: Some(Ok(fetched)),
            ..Outcome::default()
        }
    }

    fn tracker() -> Tracker {
        Tracker::new(Provider::Codex, Source::Live, Policy::default(), t0())
    }

    #[test]
    fn fetches_immediately_then_every_interval_and_never_concurrently() {
        let mut tracker = tracker();
        assert!(tracker.should_fetch(t0(), false));
        tracker.begin(t0());
        assert!(!tracker.should_fetch(t0(), true), "in flight");
        tracker.finish(ok(fetched(40, t0(), Origin::Api)), t0());
        assert!(!tracker.should_fetch(t0() + secs(299), false));
        assert!(tracker.should_fetch(t0() + secs(300), false));
    }

    #[test]
    fn manual_refresh_is_debounced() {
        let mut tracker = tracker();
        tracker.begin(t0());
        tracker.finish(ok(fetched(40, t0(), Origin::Api)), t0());
        assert!(!tracker.should_fetch(t0() + secs(5), true));
        assert!(tracker.should_fetch(t0() + secs(20), true));
    }

    #[test]
    fn available_reading_uses_the_most_restrictive_window_and_lists_the_rest() {
        let mut tracker = tracker();
        tracker.begin(t0());
        tracker.finish(ok(fetched(73, t0(), Origin::Api)), t0());
        let usage = tracker.usage(t0() + secs(60));
        assert_eq!(usage.reading, Reading::Available { percent_used: 73 });
        assert_eq!(usage.window.unwrap().id, "sevenDay");
        assert_eq!(usage.quotas.len(), 1);
        assert_eq!(usage.issue, None);
        assert_eq!(usage.updated_at.as_deref(), Some("2026-11-23T18:00:00Z"));
    }

    #[test]
    fn old_data_becomes_stale_without_a_new_failure() {
        let mut tracker = tracker();
        tracker.begin(t0());
        tracker.finish(ok(fetched(73, t0(), Origin::Api)), t0());
        let usage = tracker.usage(t0() + secs(16 * 60));
        assert_eq!(usage.reading, Reading::Stale { percent_used: 73 });
    }

    #[test]
    fn failures_keep_the_last_reading_as_stale_and_back_off_exponentially() {
        let mut tracker = tracker();
        tracker.begin(t0());
        tracker.finish(ok(fetched(50, t0(), Origin::Api)), t0());

        let mut now = t0() + secs(300);
        let mut waits = Vec::new();
        for _ in 0..7 {
            tracker.begin(now);
            tracker.finish(Outcome::failed(FetchError::Network), now);
            waits.push((tracker.next_due() - now).num_seconds());
            now = tracker.next_due();
        }
        assert_eq!(waits, [60, 120, 240, 480, 960, 1800, 1800]);
        let usage = tracker.usage(now);
        assert_eq!(usage.reading, Reading::Stale { percent_used: 50 });
        assert_eq!(usage.issue, Some(Issue::Network));

        tracker.begin(now);
        tracker.finish(ok(fetched(51, now, Origin::Api)), now);
        assert_eq!((tracker.next_due() - now).num_seconds(), 300);
        assert_eq!(tracker.usage(now).issue, None);
    }

    #[test]
    fn rate_limit_honors_retry_after_for_scheduled_and_manual_refresh() {
        let mut tracker = tracker();
        tracker.begin(t0());
        tracker.finish(
            Outcome::failed(FetchError::RateLimited {
                retry_after: Some(Duration::from_secs(600)),
            }),
            t0(),
        );
        assert!(!tracker.should_fetch(t0() + secs(599), true));
        assert!(!tracker.should_fetch(t0() + secs(599), false));
        assert!(tracker.should_fetch(t0() + secs(600), false));
        assert_eq!(
            tracker.usage(t0()).issue,
            Some(Issue::RateLimited {
                retry_at: Some("2026-11-23T18:10:00Z".into())
            })
        );

        // Without the header, the floor still applies.
        let mut tracker = super::tests::tracker();
        tracker.begin(t0());
        tracker.finish(
            Outcome::failed(FetchError::RateLimited { retry_after: None }),
            t0(),
        );
        assert!(!tracker.should_fetch(t0() + secs(59), true));
    }

    #[test]
    fn missing_sign_in_is_unavailable_and_rechecked_locally() {
        let mut tracker = tracker();
        tracker.begin(t0());
        tracker.finish(
            Outcome {
                result: Some(Err(FetchError::AuthMissing)),
                ..Outcome::default()
            },
            t0(),
        );
        let usage = tracker.usage(t0());
        assert_eq!(usage.reading, Reading::Unavailable);
        assert_eq!(usage.issue, Some(Issue::AuthMissing));
        assert_eq!((tracker.next_due() - t0()).num_seconds(), 120);
    }

    #[test]
    fn session_log_fallback_is_stale_and_expired_windows_roll_over() {
        let mut tracker = tracker();
        let observed = t0() - secs(2 * 3600);
        tracker.begin(t0());
        tracker.finish(
            Outcome {
                result: Some(Err(FetchError::AuthExpired)),
                fallback: Some(fetched(30, observed, Origin::SessionLog)),
                ..Outcome::default()
            },
            t0(),
        );
        // The 5-hour window renewed one hour after the observation.
        let usage = tracker.usage(t0());
        assert_eq!(usage.reading, Reading::Stale { percent_used: 30 });
        assert_eq!(usage.origin, Some(Origin::SessionLog));
        assert_eq!(usage.quotas[0].percent_used, 0);
        assert_eq!(usage.quotas[0].resets_at, None);
        assert_eq!(usage.issue, Some(Issue::AuthExpired));
    }

    #[test]
    fn an_older_fallback_never_replaces_a_newer_api_reading() {
        let mut tracker = tracker();
        tracker.begin(t0());
        tracker.finish(ok(fetched(60, t0(), Origin::Api)), t0());
        tracker.begin(t0() + secs(300));
        tracker.finish(
            Outcome {
                result: Some(Err(FetchError::Timeout)),
                fallback: Some(fetched(20, t0() - secs(60), Origin::SessionLog)),
                ..Outcome::default()
            },
            t0() + secs(300),
        );
        let usage = tracker.usage(t0() + secs(300));
        assert_eq!(usage.reading, Reading::Stale { percent_used: 60 });
        assert_eq!(usage.origin, Some(Origin::Api));
    }
}

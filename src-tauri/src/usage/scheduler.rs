//! Refresh policy and freshness for one provider. Pure logic with an injected
//! clock, so every timing rule is unit-tested.
//!
//! Cached quotas belong to the account that produced them (audit B01). They
//! are shown only while the current session is that same account, and are
//! discarded when a logout or an account switch is proven by the local sign-in
//! files. Every time computation is checked and bounded (audit B02).

use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};

use super::adapter::{FetchError, Fetched, Outcome, rfc3339, split_primary};
use super::model::{Issue, LocalActivity, Origin, Provider, Quota, Reading, Source, Usage};
use super::session::Session;
use super::time::{after, bounded};

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

/// Elapsed time from `earlier` to `later` is at least `span`.
fn elapsed(earlier: DateTime<Utc>, later: DateTime<Utc>, span: Duration) -> bool {
    TimeDelta::from_std(span).is_ok_and(|span| later.signed_duration_since(earlier) >= span)
}

pub struct Tracker {
    provider: Provider,
    source: Source,
    policy: Policy,
    /// The session reported by the latest attempt.
    session: Session,
    last_good: Option<Fetched>,
    /// `last_good` is the result of the most recent attempt.
    latest: bool,
    /// Labels of the current session, from the latest attempt only.
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
            session: Session::Unverified,
            last_good: None,
            latest: false,
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
                .is_none_or(|last| elapsed(last, now, self.policy.manual_gap))
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

    /// Drops everything tied to the previous account: quotas, the retry
    /// state, and the failure count that drives the backoff.
    fn forget(&mut self) {
        self.last_good = None;
        self.latest = false;
        self.failures = 0;
        self.not_before = None;
    }

    /// Applies what the local sign-in files say about the session.
    fn observe(&mut self, session: Session) {
        let previous = std::mem::replace(&mut self.session, session);
        match session {
            Session::SignedOut => self.forget(),
            Session::Account { identity, .. } => {
                let switched = previous.identity().is_some_and(|before| before != identity);
                let foreign = self
                    .last_good
                    .as_ref()
                    .and_then(|good| good.identity)
                    .is_some_and(|owner| owner != identity);
                if switched || foreign || previous == Session::SignedOut {
                    self.forget();
                }
            }
            // Proves nothing: keep the cache, but `usage` will not show
            // account data it cannot attribute.
            Session::Unverified => {}
        }
    }

    pub fn finish(&mut self, outcome: Outcome, now: DateTime<Utc>) {
        self.in_flight = false;
        self.observe(outcome.session);
        // Labels never carry over from an earlier attempt.
        self.account = outcome.account;
        self.plan = outcome.plan;
        self.activity = outcome.activity;

        let Some(result) = outcome.result else {
            self.next_due = after(now, self.policy.interval);
            return;
        };
        match result {
            Ok(fetched) => {
                if fetched.account.is_some() {
                    self.account.clone_from(&fetched.account);
                }
                if fetched.plan.is_some() {
                    self.plan.clone_from(&fetched.plan);
                }
                self.last_good = Some(fetched);
                self.latest = true;
                self.issue = None;
                self.failures = 0;
                self.not_before = None;
                self.next_due = after(now, self.policy.interval);
            }
            Err(error) => {
                self.latest = false;
                self.failures = self.failures.saturating_add(1);
                if let Some(mut fallback) = outcome
                    .fallback
                    .filter(|_| self.session != Session::SignedOut)
                {
                    // A local log has no verifiable link to an account.
                    fallback.identity = None;
                    fallback.account = None;
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
                        // A valid `Retry-After` wins even when it exceeds the
                        // backoff ceiling. Only absurd values are clamped.
                        let wait = bounded(
                            retry_after
                                .unwrap_or_default()
                                .max(self.backoff())
                                .max(self.policy.rate_limit_floor),
                        );
                        self.not_before = Some(after(now, wait));
                        wait
                    }
                    FetchError::Network | FetchError::Timeout | FetchError::Http(_) => {
                        self.backoff()
                    }
                };
                self.next_due = after(now, wait);
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

    /// Ends an attempt that could not complete normally (the adapter
    /// panicked). The worker stays alive and retries after the backoff.
    pub fn abort(&mut self, now: DateTime<Utc>) {
        let outcome = Outcome {
            session: self.session,
            account: self.account.take(),
            plan: self.plan.take(),
            activity: self.activity.take(),
            ..Outcome::failed(FetchError::Malformed("attempt aborted".into()))
        };
        self.finish(outcome, now);
    }

    /// Whether a cached reading may be shown for the current session.
    fn attributable(&self, good: &Fetched) -> bool {
        match (self.session, good.identity) {
            (Session::SignedOut, _) => false,
            (session, Some(owner)) => session.identity() == Some(owner),
            // Without an identity, a live reading is only trusted as the
            // result of the latest attempt. Local logs and fixtures are shown
            // as unattributed history.
            (_, None) => good.origin != Origin::Api || self.latest,
        }
    }

    pub fn usage(&self, now: DateTime<Utc>) -> Usage {
        let good = self
            .last_good
            .as_ref()
            .filter(|good| self.attributable(good));
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
            && good.is_some_and(|good| !elapsed(good.observed_at, now, self.policy.stale_after));
        let (reading, window) = match primary {
            Some((percent_used, window)) if fresh => {
                (Reading::Available { percent_used }, Some(window))
            }
            Some((percent_used, window)) => (Reading::Stale { percent_used }, Some(window)),
            None => (Reading::Unavailable, None),
        };

        Usage {
            provider: self.provider,
            account: self.account.clone(),
            plan: self.plan.clone(),
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

    fn account(name: &str) -> Session {
        Session::account(&[name], &format!("{name}-token"))
    }

    fn fetched(percent: u8, at: DateTime<Utc>, origin: Origin) -> Fetched {
        fetched_for(account("A"), "a•••@example.com", percent, at, origin)
    }

    fn fetched_for(
        session: Session,
        label: &str,
        percent: u8,
        at: DateTime<Utc>,
        origin: Origin,
    ) -> Fetched {
        Fetched {
            identity: session.identity(),
            account: Some(label.into()),
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
            session: account("A"),
            account: fetched.account.clone(),
            result: Some(Ok(fetched)),
            ..Outcome::default()
        }
    }

    fn failed_as(session: Session, label: Option<&str>, error: FetchError) -> Outcome {
        Outcome {
            session,
            account: label.map(str::to_owned),
            ..Outcome::failed(error)
        }
    }

    fn tracker() -> Tracker {
        Tracker::new(Provider::Codex, Source::Live, Policy::default(), t0())
    }

    /// Account A with a 91% weekly window, read at `t0`.
    fn tracker_with_a() -> Tracker {
        let mut tracker = tracker();
        tracker.begin(t0());
        tracker.finish(ok(fetched(91, t0(), Origin::Api)), t0());
        assert_eq!(
            tracker.usage(t0()).reading,
            Reading::Available { percent_used: 91 }
        );
        tracker
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
            tracker.finish(failed_as(account("A"), None, FetchError::Network), now);
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
        tracker.finish(Outcome::signed_out(FetchError::AuthMissing), t0());
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
                session: account("A"),
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
                session: account("A"),
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

    // --- B01: cache isolation by identity -------------------------------

    #[test]
    fn b01_logout_discards_quotas_labels_and_retry_state() {
        let mut tracker = tracker_with_a();
        tracker.begin(t0() + secs(30));
        tracker.finish(
            Outcome::signed_out(FetchError::AuthMissing),
            t0() + secs(30),
        );
        let usage = tracker.usage(t0() + secs(30));
        assert_eq!(usage.reading, Reading::Unavailable);
        assert_eq!(usage.account, None);
        assert_eq!(usage.plan, None);
        assert_eq!(usage.window, None);
        assert!(usage.quotas.is_empty());
        assert_eq!(usage.updated_at, None);
        assert_eq!(usage.issue, Some(Issue::AuthMissing));

        // Signing in again does not resurrect the old reading.
        tracker.begin(t0() + secs(200));
        tracker.finish(
            failed_as(account("A"), Some("a•••@example.com"), FetchError::Network),
            t0() + secs(200),
        );
        assert_eq!(
            tracker.usage(t0() + secs(200)).reading,
            Reading::Unavailable
        );
    }

    #[test]
    fn b01_account_switch_with_the_network_down_never_shows_the_previous_account() {
        let mut tracker = tracker_with_a();
        tracker.begin(t0() + secs(60));
        tracker.finish(
            failed_as(account("B"), Some("b•••@example.com"), FetchError::Network),
            t0() + secs(60),
        );
        let usage = tracker.usage(t0() + secs(60));
        assert_eq!(usage.account.as_deref(), Some("b•••@example.com"));
        assert_eq!(usage.reading, Reading::Unavailable);
        assert!(usage.quotas.is_empty());
        assert_eq!(usage.issue, Some(Issue::Network));
        // The backoff of account A does not carry over to account B.
        assert_eq!((tracker.next_due() - (t0() + secs(60))).num_seconds(), 60);
    }

    #[test]
    fn b01_a_different_label_alone_proves_nothing_and_shows_nothing_of_a() {
        // Unverified session with another label: A's quota is withheld.
        let mut tracker = tracker_with_a();
        tracker.begin(t0() + secs(60));
        tracker.finish(
            failed_as(
                Session::Unverified,
                Some("b•••@example.com"),
                FetchError::Network,
            ),
            t0() + secs(60),
        );
        let usage = tracker.usage(t0() + secs(60));
        assert_eq!(usage.account.as_deref(), Some("b•••@example.com"));
        assert_eq!(usage.reading, Reading::Unavailable);

        // Once the session is verified again as A, its cache is still valid.
        tracker.begin(t0() + secs(120));
        tracker.finish(
            failed_as(account("A"), Some("a•••@example.com"), FetchError::Network),
            t0() + secs(120),
        );
        assert_eq!(
            tracker.usage(t0() + secs(120)).reading,
            Reading::Stale { percent_used: 91 }
        );
    }

    #[test]
    fn b01_renewing_the_credential_of_the_same_account_keeps_the_cache() {
        let mut tracker = tracker_with_a();
        let renewed = Session::account(&["A"], "A-token-renewed");
        tracker.begin(t0() + secs(60));
        tracker.finish(
            failed_as(renewed, Some("a•••@example.com"), FetchError::Timeout),
            t0() + secs(60),
        );
        let usage = tracker.usage(t0() + secs(60));
        assert_eq!(usage.reading, Reading::Stale { percent_used: 91 });
        assert_eq!(usage.account.as_deref(), Some("a•••@example.com"));
    }

    #[test]
    fn b01_a_new_success_without_a_label_does_not_reuse_the_old_label() {
        let mut tracker = tracker_with_a();
        let mut unlabeled = fetched(20, t0() + secs(300), Origin::Api);
        unlabeled.account = None;
        unlabeled.plan = None;
        tracker.begin(t0() + secs(300));
        tracker.finish(
            Outcome {
                session: account("A"),
                result: Some(Ok(unlabeled)),
                ..Outcome::default()
            },
            t0() + secs(300),
        );
        let usage = tracker.usage(t0() + secs(300));
        assert_eq!(usage.reading, Reading::Available { percent_used: 20 });
        assert_eq!(usage.account, None);
        assert_eq!(usage.plan, None);
    }

    #[test]
    fn b01_session_logs_are_never_attributed_to_the_current_account() {
        let mut tracker = tracker();
        tracker.begin(t0());
        tracker.finish(
            Outcome {
                session: account("B"),
                account: Some("b•••@example.com".into()),
                result: Some(Err(FetchError::AuthExpired)),
                // The adapter must not label it, but the tracker enforces it.
                fallback: Some(fetched(30, t0() - secs(600), Origin::SessionLog)),
                ..Outcome::default()
            },
            t0(),
        );
        let usage = tracker.usage(t0());
        assert_eq!(usage.origin, Some(Origin::SessionLog));
        assert_eq!(usage.reading, Reading::Stale { percent_used: 30 });
        let stored = tracker.last_good.as_ref().unwrap();
        assert_eq!(stored.identity, None);
        assert_eq!(stored.account, None);

        // No fallback is kept after a logout.
        tracker.begin(t0() + secs(10));
        tracker.finish(
            Outcome {
                fallback: Some(fetched(30, t0(), Origin::SessionLog)),
                ..Outcome::signed_out(FetchError::AuthMissing)
            },
            t0() + secs(10),
        );
        assert_eq!(tracker.usage(t0() + secs(10)).reading, Reading::Unavailable);
    }

    #[test]
    fn b01_an_api_reading_without_identity_is_not_reused_after_a_failure() {
        let mut tracker = tracker();
        let mut reading = fetched(44, t0(), Origin::Api);
        reading.identity = None;
        tracker.begin(t0());
        tracker.finish(
            Outcome {
                session: Session::Unverified,
                result: Some(Ok(reading)),
                ..Outcome::default()
            },
            t0(),
        );
        assert_eq!(
            tracker.usage(t0()).reading,
            Reading::Available { percent_used: 44 }
        );
        tracker.begin(t0() + secs(300));
        tracker.finish(
            failed_as(Session::Unverified, None, FetchError::Network),
            t0() + secs(300),
        );
        assert_eq!(
            tracker.usage(t0() + secs(300)).reading,
            Reading::Unavailable
        );
    }

    // --- B02: checked time arithmetic ------------------------------------

    #[test]
    fn b02_an_absurd_retry_after_is_bounded_and_never_panics() {
        let mut tracker = tracker_with_a();
        let header = crate::usage::http::parse_retry_after("18446744073709551615", t0());
        tracker.begin(t0());
        tracker.finish(
            failed_as(
                account("A"),
                None,
                FetchError::RateLimited {
                    retry_after: header,
                },
            ),
            t0(),
        );
        assert_eq!((tracker.next_due() - t0()).num_hours(), 24);
        let usage = tracker.usage(t0());
        assert!(!usage.refreshing);
        assert_eq!(usage.reading, Reading::Stale { percent_used: 91 });
        assert_eq!(
            usage.issue,
            Some(Issue::RateLimited {
                retry_at: Some("2026-11-24T18:00:00Z".into())
            })
        );
    }

    #[test]
    fn b02_a_valid_retry_after_beyond_the_backoff_ceiling_is_preserved() {
        let mut tracker = tracker();
        tracker.begin(t0());
        tracker.finish(
            Outcome::failed(FetchError::RateLimited {
                retry_after: Some(Duration::from_secs(2 * 3600)),
            }),
            t0(),
        );
        assert_eq!((tracker.next_due() - t0()).num_seconds(), 7200);
        assert!(!tracker.should_fetch(t0() + secs(7199), true));
        assert!(tracker.should_fetch(t0() + secs(7200), false));
    }

    #[test]
    fn b02_the_end_of_the_time_range_saturates_instead_of_panicking() {
        let end = DateTime::<Utc>::MAX_UTC - secs(10);
        let mut tracker = Tracker::new(Provider::Codex, Source::Live, Policy::default(), end);
        tracker.begin(end);
        tracker.finish(
            Outcome::failed(FetchError::RateLimited {
                retry_after: Some(Duration::MAX),
            }),
            end,
        );
        assert_eq!(tracker.next_due(), DateTime::<Utc>::MAX_UTC);
        tracker.begin(end);
        tracker.finish(ok(fetched(10, t0(), Origin::Api)), end);
        let _ = tracker.usage(DateTime::<Utc>::MAX_UTC);
        let _ = tracker.usage(DateTime::<Utc>::MIN_UTC);
        assert!(!tracker.should_fetch(DateTime::<Utc>::MIN_UTC, true));
    }

    #[test]
    fn b02_an_aborted_attempt_leaves_a_valid_state() {
        let mut tracker = tracker_with_a();
        tracker.begin(t0() + secs(300));
        assert!(tracker.usage(t0() + secs(300)).refreshing);
        tracker.abort(t0() + secs(300));
        let usage = tracker.usage(t0() + secs(300));
        assert!(!usage.refreshing);
        assert_eq!(usage.issue, Some(Issue::Malformed));
        assert_eq!(usage.reading, Reading::Stale { percent_used: 91 });
        assert_eq!(usage.account.as_deref(), Some("a•••@example.com"));
        assert!(tracker.next_due() > t0() + secs(300));
    }
}

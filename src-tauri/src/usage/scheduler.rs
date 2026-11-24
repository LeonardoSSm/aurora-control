//! Refresh policy and freshness for one provider. Pure logic with an injected
//! clock, so every timing rule is unit-tested.
//!
//! - Cached quotas belong to the account that produced them (audit B01). They
//!   are shown only while the current session is that same account, and are
//!   discarded when a logout or an account switch is proven by the local
//!   sign-in files.
//! - Every time computation is checked and bounded (audit B02).
//! - Local waits use the monotonic clock; UTC is kept for provider times and
//!   presentation (audit I01). Three gates are separate: the next regular
//!   refresh, the earliest manual refresh, and the server-imposed wait.
//! - A window that renewed without a new measurement is not a reading (I02).

use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};

use super::adapter::{FetchError, Fetched, Outcome, rfc3339, split_metric};
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

impl Policy {
    /// The default policy with the provider's refresh cadence from the catalog.
    pub fn for_provider(provider: Provider) -> Self {
        Self {
            interval: super::catalog::refresh_interval(provider),
            ..Self::default()
        }
    }
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

/// One reading of both clocks: monotonic for local waits, UTC for provider
/// times and display.
#[derive(Clone, Copy, Debug)]
pub struct Now {
    pub instant: Instant,
    pub utc: DateTime<Utc>,
}

impl Now {
    pub fn current() -> Self {
        Self {
            instant: Instant::now(),
            utc: Utc::now(),
        }
    }

    /// Both clocks advanced by `wait` (bounded and saturated).
    #[cfg(test)]
    pub fn later(self, wait: Duration) -> Self {
        Self {
            instant: later(self.instant, wait),
            utc: after(self.utc, wait),
        }
    }
}

/// `instant + wait`, bounded. An unrepresentable sum keeps `instant`, which
/// only happens far outside the bounded range.
fn later(instant: Instant, wait: Duration) -> Instant {
    instant.checked_add(bounded(wait)).unwrap_or(instant)
}

/// Elapsed UTC time from `earlier` to `later` is at least `span`.
fn elapsed(earlier: DateTime<Utc>, later: DateTime<Utc>, span: Duration) -> bool {
    chrono::TimeDelta::from_std(span).is_ok_and(|span| later.signed_duration_since(earlier) >= span)
}

/// How long until the UTC time `at`, if it is in the future.
fn until(now: Now, at: DateTime<Utc>) -> Option<Instant> {
    let wait = at.signed_duration_since(now.utc).to_std().ok()?;
    Some(later(now.instant, wait))
}

pub struct Tracker {
    provider: Provider,
    source: Source,
    policy: Policy,
    /// The session reported by the latest attempt or probe.
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
    /// Next regular (scheduled) attempt.
    next_due: Instant,
    /// Earliest attempt started by the user.
    manual_after: Option<Instant>,
    /// Server-imposed wait (`Retry-After`): binds every trigger.
    not_before: Option<Instant>,
    last_attempt: Option<DateTime<Utc>>,
    in_flight: bool,
    /// The metric chosen for the ring, or `None` for the automatic choice.
    metric: Option<String>,
}

impl Tracker {
    pub fn new(provider: Provider, source: Source, policy: Policy, now: Now) -> Self {
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
            next_due: now.instant,
            manual_after: None,
            not_before: None,
            last_attempt: None,
            in_flight: false,
            metric: None,
        }
    }

    /// Chooses the ring's metric: `None` for the automatic choice, or a quota
    /// id. Only changes presentation: nothing is fetched again.
    pub fn select_metric(&mut self, metric: Option<String>) {
        self.metric = metric;
    }

    /// The verified account of the current session, if any. Process-local:
    /// used to keep alerts of one account from reaching another.
    pub fn identity(&self) -> Option<super::session::Fingerprint> {
        self.session.identity()
    }

    #[cfg(test)]
    pub fn next_due(&self) -> Instant {
        self.next_due
    }

    /// Prevents concurrent queries, honors `Retry-After` for every trigger,
    /// and keeps manual requests within the recovery rules of the last error.
    pub fn should_fetch(&self, now: Now, manual: bool) -> bool {
        if self.in_flight || self.not_before.is_some_and(|gate| now.instant < gate) {
            return false;
        }
        if manual {
            self.manual_after.is_none_or(|gate| now.instant >= gate)
        } else {
            now.instant >= self.next_due
        }
    }

    pub fn begin(&mut self, now: Now) {
        self.in_flight = true;
        self.last_attempt = Some(now.utc);
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
        self.manual_after = None;
    }

    /// Applies what the local sign-in files say about the session. Returns
    /// whether the state of a previous account was dropped.
    fn observe(&mut self, session: Session) -> bool {
        let previous = std::mem::replace(&mut self.session, session);
        let forget = match session {
            Session::SignedOut => true,
            Session::Account { identity, .. } => {
                let switched = previous.identity().is_some_and(|before| before != identity);
                let foreign = self
                    .last_good
                    .as_ref()
                    .and_then(|good| good.identity)
                    .is_some_and(|owner| owner != identity);
                switched || foreign || previous == Session::SignedOut
            }
            // Proves nothing: keep the cache, but `usage` will not show
            // account data it cannot attribute.
            Session::Unverified => false,
        };
        if forget {
            self.forget();
        }
        forget
    }

    /// A session read from local files between attempts. A new account or a
    /// renewed credential lifts the local recovery wait of the old one, so
    /// the user can refresh right after signing in again. Server-imposed
    /// waits stay, unless the account changed.
    pub fn probe(&mut self, session: Session, now: Now) {
        let before = self.session;
        let renewed = match (before, session) {
            (
                Session::Account {
                    identity: a,
                    credential: old,
                },
                Session::Account {
                    identity: b,
                    credential: new,
                },
            ) => a == b && old != new,
            _ => false,
        };
        let signed_in = session.identity().is_some() && before.identity() != session.identity();
        let fresh_account = self.observe(session) && session.identity().is_some();
        if fresh_account {
            // The schedule was set by the previous account (for example by
            // its `Retry-After`): this account refreshes now.
            self.next_due = now.instant;
        }
        if renewed || signed_in {
            self.manual_after = None;
            let local = matches!(
                self.issue,
                Some(
                    Issue::AuthMissing
                        | Issue::AuthExpired
                        | Issue::AuthRejected
                        | Issue::CredentialsUnavailable
                        | Issue::CredentialsInvalid
                        | Issue::Unsupported
                )
            );
            if local {
                self.next_due = now.instant;
            }
        }
    }

    pub fn finish(&mut self, outcome: Outcome, now: Now) {
        self.in_flight = false;
        // A switch seen here is followed by this outcome's own schedule.
        let _ = self.observe(outcome.session);
        // Labels never carry over from an earlier attempt.
        self.account = outcome.account;
        self.plan = outcome.plan;
        self.activity = outcome.activity;

        let Some(result) = outcome.result else {
            self.next_due = later(now.instant, self.policy.interval);
            self.manual_after = Some(later(now.instant, self.policy.manual_gap));
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
                self.next_due = later(now.instant, self.policy.interval);
                self.manual_after = Some(later(now.instant, self.policy.manual_gap));
            }
            Err(error) => self.fail(error, outcome.fallback, now),
        }
    }

    fn fail(&mut self, error: FetchError, fallback: Option<Fetched>, now: Now) {
        self.latest = false;
        self.failures = self.failures.saturating_add(1);
        if let Some(mut fallback) = fallback.filter(|_| self.session != Session::SignedOut) {
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

        let policy = self.policy;
        let backoff = self.backoff();
        // (regular wait, manual wait, server-imposed wait)
        let (wait, manual, imposed) = match &error {
            // Local file checks: nothing reaches the provider.
            FetchError::AuthMissing
            | FetchError::AuthExpired
            | FetchError::Unsupported
            | FetchError::CredentialsUnavailable
            | FetchError::CredentialsInvalid
            | FetchError::SourceBlocked(_)
            | FetchError::AwaitingClient => (policy.local_recheck, policy.manual_gap, None),
            // The request never got an answer: retrying soon is harmless.
            FetchError::Network | FetchError::Timeout => (backoff, policy.manual_gap, None),
            // The same credential fails again: the user waits like the
            // schedule, unless the credential changes (see `probe`).
            FetchError::AuthRejected => {
                let wait = backoff.max(policy.interval);
                (wait, wait, None)
            }
            FetchError::Malformed(_) => (backoff.max(policy.interval), backoff, None),
            // A valid `Retry-After` wins even above the backoff ceiling. Only
            // absurd values are clamped by `bounded`.
            FetchError::RateLimited { retry_after } => {
                let wait = bounded(
                    retry_after
                        .unwrap_or_default()
                        .max(backoff)
                        .max(policy.rate_limit_floor),
                );
                (wait, wait, Some(wait))
            }
            FetchError::Http {
                retry_after: Some(retry_after),
                ..
            } => {
                let wait = bounded((*retry_after).max(backoff));
                (wait, wait, Some(wait))
            }
            FetchError::Http {
                retry_after: None, ..
            } => (backoff, backoff, None),
        };
        self.next_due = later(now.instant, wait);
        self.manual_after = Some(later(now.instant, manual));
        self.not_before = imposed.map(|wait| later(now.instant, wait));
        let retry_at = imposed.map(|wait| rfc3339(after(now.utc, wait)));

        self.issue = Some(match error {
            FetchError::AuthMissing => Issue::AuthMissing,
            FetchError::AuthExpired => Issue::AuthExpired,
            FetchError::AuthRejected => Issue::AuthRejected,
            FetchError::Unsupported => Issue::Unsupported,
            FetchError::CredentialsUnavailable => Issue::CredentialsUnavailable,
            FetchError::CredentialsInvalid => Issue::CredentialsInvalid,
            FetchError::RateLimited { .. } => Issue::RateLimited { retry_at },
            FetchError::Network => Issue::Network,
            FetchError::Timeout => Issue::Timeout,
            FetchError::Malformed(_) => Issue::Malformed,
            FetchError::Http { status, .. } => Issue::Http { status, retry_at },
            FetchError::SourceBlocked(reason) => Issue::SourceBlocked { reason },
            FetchError::AwaitingClient => Issue::AwaitingClient,
        });
    }

    /// Ends an attempt that could not complete normally (the adapter
    /// panicked). The worker stays alive and retries after the backoff.
    pub fn abort(&mut self, now: Now) {
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
            // Without an identity, a live reading (provider API or official
            // CLI) is only trusted as the result of the latest attempt: after
            // a failure it may belong to an account the user has left. Local
            // logs and fixtures are shown as unattributed history.
            (_, None) => {
                !matches!(good.origin, Origin::Api | Origin::Cli | Origin::ClientLog) || self.latest
            }
        }
    }

    fn shown(&self) -> Option<&Fetched> {
        self.last_good
            .as_ref()
            .filter(|good| self.attributable(good))
    }

    /// The next moment the state changes without a network result: the
    /// regular refresh, the end of a server wait, a window renewal, or the
    /// reading turning stale. The worker publishes these transitions even
    /// while the provider is unreachable (audit I02).
    pub fn next_wake(&self, now: Now) -> Instant {
        let mut wake = self.next_due;
        if let Some(gate) = self.not_before {
            wake = wake.max(gate);
        }
        if let Some(good) = self.shown() {
            let stale = chrono::TimeDelta::from_std(self.policy.stale_after)
                .ok()
                .and_then(|span| good.observed_at.checked_add_signed(span));
            let resets = good.quotas.iter().filter_map(|quota| {
                quota
                    .resets_at
                    .as_deref()
                    .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                    .map(|reset| reset.with_timezone(&Utc))
            });
            for at in stale.into_iter().chain(resets) {
                if let Some(instant) = until(now, at) {
                    wake = wake.min(instant);
                }
            }
        }
        wake
    }

    /// The state shown by the rail, with the user's ring metric.
    pub fn usage(&self, now: Now) -> Usage {
        self.view(now, self.metric.as_deref())
    }

    /// The same state with the automatic metric. The ring metric is a
    /// presentation choice: whether the data is fresh and which windows were
    /// measured must not depend on it, so the alert engine reads this view.
    pub fn automatic_usage(&self, now: Now) -> Usage {
        self.view(now, None)
    }

    fn view(&self, now: Now, selected: Option<&str>) -> Usage {
        let good = self.shown();
        let quotas: Vec<Quota> = good
            .map(|good| {
                good.quotas
                    .iter()
                    .map(|quota| roll_over(quota, now.utc))
                    .collect()
            })
            .unwrap_or_default();
        // With data in hand, a chosen metric that is absent is reported as
        // missing. Without any data, nothing can be said about it yet.
        let has_data = !quotas.is_empty();
        let (primary, quotas) = split_metric(quotas, selected);
        let metric_missing = selected.is_some() && has_data && primary.is_none();
        let fresh = self.issue.is_none()
            && good
                .is_some_and(|good| !elapsed(good.observed_at, now.utc, self.policy.stale_after));
        let (reading, window) = match primary {
            Some((percent_used, window)) if fresh && !window.renewed => {
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
            selected_metric: self.metric.clone(),
            metric_missing,
        }
    }
}

/// A stored window whose renewal time has passed no longer describes the
/// current consumption. It awaits a new measurement: its percentage is not a
/// reading and its next renewal is unknown.
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
            renewed: true,
            ..quota.clone()
        }
    } else {
        quota.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::model::Origin;
    use chrono::TimeZone;
    use std::sync::OnceLock;

    fn t0() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 23, 18, 0, 0).unwrap()
    }

    fn secs(seconds: i64) -> chrono::TimeDelta {
        chrono::TimeDelta::seconds(seconds)
    }

    /// Both clocks at `t0 + seconds`.
    fn at(seconds: u64) -> Now {
        static BASE: OnceLock<Instant> = OnceLock::new();
        Now {
            instant: *BASE.get_or_init(Instant::now) + Duration::from_secs(seconds),
            utc: t0() + secs(seconds as i64),
        }
    }

    fn waited(tracker: &Tracker, from: Now) -> u64 {
        tracker.next_due().duration_since(from.instant).as_secs()
    }

    fn account(name: &str) -> Session {
        Session::account(&[name], &format!("{name}-token"))
    }

    fn fetched(percent: u8, observed: DateTime<Utc>, origin: Origin) -> Fetched {
        fetched_for(account("A"), "a•••@example.com", percent, observed, origin)
    }

    fn fetched_for(
        session: Session,
        label: &str,
        percent: u8,
        observed: DateTime<Utc>,
        origin: Origin,
    ) -> Fetched {
        Fetched {
            identity: session.identity(),
            account: Some(label.into()),
            plan: Some("Plus".into()),
            origin,
            quotas: vec![
                Quota::window(
                    "fiveHour",
                    "Sessão de 5 h",
                    10,
                    Some(300),
                    Some(rfc3339(observed + secs(3600))),
                ),
                Quota::window(
                    "sevenDay",
                    "Semanal",
                    percent,
                    Some(10_080),
                    Some(rfc3339(observed + secs(86_400))),
                ),
            ],
            observed_at: observed,
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
        Tracker::new(Provider::Codex, Source::Live, Policy::default(), at(0))
    }

    /// Account A with a 91% weekly window, read at `t0`.
    fn tracker_with_a() -> Tracker {
        let mut tracker = tracker();
        tracker.begin(at(0));
        tracker.finish(ok(fetched(91, t0(), Origin::Api)), at(0));
        assert_eq!(
            tracker.usage(at(0)).reading,
            Reading::Available { percent_used: 91 }
        );
        tracker
    }

    #[test]
    fn fetches_immediately_then_every_interval_and_never_concurrently() {
        let mut tracker = tracker();
        assert!(tracker.should_fetch(at(0), false));
        tracker.begin(at(0));
        assert!(!tracker.should_fetch(at(0), true), "in flight");
        tracker.finish(ok(fetched(40, t0(), Origin::Api)), at(0));
        assert!(!tracker.should_fetch(at(299), false));
        assert!(tracker.should_fetch(at(300), false));
    }

    #[test]
    fn manual_refresh_is_debounced() {
        let mut tracker = tracker();
        tracker.begin(at(0));
        tracker.finish(ok(fetched(40, t0(), Origin::Api)), at(0));
        assert!(!tracker.should_fetch(at(5), true));
        assert!(tracker.should_fetch(at(20), true));
    }

    #[test]
    fn available_reading_uses_the_most_restrictive_window_and_lists_the_rest() {
        let mut tracker = tracker();
        tracker.begin(at(0));
        tracker.finish(ok(fetched(73, t0(), Origin::Api)), at(0));
        let usage = tracker.usage(at(60));
        assert_eq!(usage.reading, Reading::Available { percent_used: 73 });
        assert_eq!(usage.window.unwrap().id, "sevenDay");
        assert_eq!(usage.quotas.len(), 1);
        assert_eq!(usage.issue, None);
        assert_eq!(usage.updated_at.as_deref(), Some("2026-11-23T18:00:00Z"));
    }

    #[test]
    fn old_data_becomes_stale_without_a_new_failure() {
        let mut tracker = tracker();
        tracker.begin(at(0));
        tracker.finish(ok(fetched(73, t0(), Origin::Api)), at(0));
        let usage = tracker.usage(at(16 * 60));
        assert_eq!(usage.reading, Reading::Stale { percent_used: 73 });
    }

    #[test]
    fn failures_keep_the_last_reading_as_stale_and_back_off_exponentially() {
        let mut tracker = tracker();
        tracker.begin(at(0));
        tracker.finish(ok(fetched(50, t0(), Origin::Api)), at(0));

        let mut now = at(300);
        let mut waits = Vec::new();
        for _ in 0..7 {
            tracker.begin(now);
            tracker.finish(failed_as(account("A"), None, FetchError::Network), now);
            let wait = waited(&tracker, now);
            waits.push(wait);
            now = now.later(Duration::from_secs(wait));
        }
        assert_eq!(waits, [60, 120, 240, 480, 960, 1800, 1800]);
        let usage = tracker.usage(now);
        assert_eq!(usage.reading, Reading::Stale { percent_used: 50 });
        assert_eq!(usage.issue, Some(Issue::Network));

        tracker.begin(now);
        tracker.finish(ok(fetched(51, now.utc, Origin::Api)), now);
        assert_eq!(waited(&tracker, now), 300);
        assert_eq!(tracker.usage(now).issue, None);
    }

    #[test]
    fn rate_limit_honors_retry_after_for_scheduled_and_manual_refresh() {
        let mut tracker = tracker();
        tracker.begin(at(0));
        tracker.finish(
            Outcome::failed(FetchError::RateLimited {
                retry_after: Some(Duration::from_secs(600)),
            }),
            at(0),
        );
        assert!(!tracker.should_fetch(at(599), true));
        assert!(!tracker.should_fetch(at(599), false));
        assert!(tracker.should_fetch(at(600), false));
        assert_eq!(
            tracker.usage(at(0)).issue,
            Some(Issue::RateLimited {
                retry_at: Some("2026-11-23T18:10:00Z".into())
            })
        );

        // Without the header, the floor still applies.
        let mut tracker = super::tests::tracker();
        tracker.begin(at(0));
        tracker.finish(
            Outcome::failed(FetchError::RateLimited { retry_after: None }),
            at(0),
        );
        assert!(!tracker.should_fetch(at(59), true));
    }

    #[test]
    fn missing_sign_in_is_unavailable_and_rechecked_locally() {
        let mut tracker = tracker();
        tracker.begin(at(0));
        tracker.finish(Outcome::signed_out(FetchError::AuthMissing), at(0));
        let usage = tracker.usage(at(0));
        assert_eq!(usage.reading, Reading::Unavailable);
        assert_eq!(usage.issue, Some(Issue::AuthMissing));
        assert_eq!(waited(&tracker, at(0)), 120);
        // A manual check of local files may run after the short gap.
        assert!(tracker.should_fetch(at(20), true));
    }

    #[test]
    fn session_log_fallback_is_stale_and_expired_windows_await_a_reading() {
        let mut tracker = tracker();
        let observed = t0() - secs(2 * 3600);
        tracker.begin(at(0));
        tracker.finish(
            Outcome {
                session: account("A"),
                result: Some(Err(FetchError::AuthExpired)),
                fallback: Some(fetched(30, observed, Origin::SessionLog)),
                ..Outcome::default()
            },
            at(0),
        );
        // The 5-hour window renewed one hour after the observation.
        let usage = tracker.usage(at(0));
        assert_eq!(usage.reading, Reading::Stale { percent_used: 30 });
        assert_eq!(usage.origin, Some(Origin::SessionLog));
        assert_eq!(usage.quotas[0].percent_used, 0);
        assert!(usage.quotas[0].renewed);
        assert_eq!(usage.quotas[0].resets_at, None);
        assert_eq!(usage.issue, Some(Issue::AuthExpired));
    }

    #[test]
    fn an_older_fallback_never_replaces_a_newer_api_reading() {
        let mut tracker = tracker();
        tracker.begin(at(0));
        tracker.finish(ok(fetched(60, t0(), Origin::Api)), at(0));
        tracker.begin(at(300));
        tracker.finish(
            Outcome {
                session: account("A"),
                result: Some(Err(FetchError::Timeout)),
                fallback: Some(fetched(20, t0() - secs(60), Origin::SessionLog)),
                ..Outcome::default()
            },
            at(300),
        );
        let usage = tracker.usage(at(300));
        assert_eq!(usage.reading, Reading::Stale { percent_used: 60 });
        assert_eq!(usage.origin, Some(Origin::Api));
    }

    // --- B01: cache isolation by identity -------------------------------

    #[test]
    fn b01_logout_discards_quotas_labels_and_retry_state() {
        let mut tracker = tracker_with_a();
        tracker.begin(at(30));
        tracker.finish(Outcome::signed_out(FetchError::AuthMissing), at(30));
        let usage = tracker.usage(at(30));
        assert_eq!(usage.reading, Reading::Unavailable);
        assert_eq!(usage.account, None);
        assert_eq!(usage.plan, None);
        assert_eq!(usage.window, None);
        assert!(usage.quotas.is_empty());
        assert_eq!(usage.updated_at, None);
        assert_eq!(usage.issue, Some(Issue::AuthMissing));

        // Signing in again does not resurrect the old reading.
        tracker.begin(at(200));
        tracker.finish(
            failed_as(account("A"), Some("a•••@example.com"), FetchError::Network),
            at(200),
        );
        assert_eq!(tracker.usage(at(200)).reading, Reading::Unavailable);
    }

    #[test]
    fn b01_account_switch_with_the_network_down_never_shows_the_previous_account() {
        let mut tracker = tracker_with_a();
        tracker.begin(at(60));
        tracker.finish(
            failed_as(account("B"), Some("b•••@example.com"), FetchError::Network),
            at(60),
        );
        let usage = tracker.usage(at(60));
        assert_eq!(usage.account.as_deref(), Some("b•••@example.com"));
        assert_eq!(usage.reading, Reading::Unavailable);
        assert!(usage.quotas.is_empty());
        assert_eq!(usage.issue, Some(Issue::Network));
        assert_eq!(waited(&tracker, at(60)), 60);
    }

    #[test]
    fn b01_a_different_label_alone_proves_nothing_and_shows_nothing_of_a() {
        let mut tracker = tracker_with_a();
        tracker.begin(at(60));
        tracker.finish(
            failed_as(
                Session::Unverified,
                Some("b•••@example.com"),
                FetchError::Network,
            ),
            at(60),
        );
        let usage = tracker.usage(at(60));
        assert_eq!(usage.account.as_deref(), Some("b•••@example.com"));
        assert_eq!(usage.reading, Reading::Unavailable);

        // Once the session is verified again as A, its cache is still valid.
        tracker.begin(at(120));
        tracker.finish(
            failed_as(account("A"), Some("a•••@example.com"), FetchError::Network),
            at(120),
        );
        assert_eq!(
            tracker.usage(at(120)).reading,
            Reading::Stale { percent_used: 91 }
        );
    }

    #[test]
    fn b01_renewing_the_credential_of_the_same_account_keeps_the_cache() {
        let mut tracker = tracker_with_a();
        let renewed = Session::account(&["A"], "A-token-renewed");
        tracker.probe(renewed, at(30));
        assert_eq!(
            tracker.usage(at(30)).reading,
            Reading::Available { percent_used: 91 }
        );
        tracker.begin(at(60));
        tracker.finish(
            failed_as(renewed, Some("a•••@example.com"), FetchError::Timeout),
            at(60),
        );
        let usage = tracker.usage(at(60));
        assert_eq!(usage.reading, Reading::Stale { percent_used: 91 });
        assert_eq!(usage.account.as_deref(), Some("a•••@example.com"));
    }

    #[test]
    fn b01_a_new_success_without_a_label_does_not_reuse_the_old_label() {
        let mut tracker = tracker_with_a();
        let mut unlabeled = fetched(20, t0() + secs(300), Origin::Api);
        unlabeled.account = None;
        unlabeled.plan = None;
        tracker.begin(at(300));
        tracker.finish(
            Outcome {
                session: account("A"),
                result: Some(Ok(unlabeled)),
                ..Outcome::default()
            },
            at(300),
        );
        let usage = tracker.usage(at(300));
        assert_eq!(usage.reading, Reading::Available { percent_used: 20 });
        assert_eq!(usage.account, None);
        assert_eq!(usage.plan, None);
    }

    #[test]
    fn b01_session_logs_are_never_attributed_to_the_current_account() {
        let mut tracker = tracker();
        tracker.begin(at(0));
        tracker.finish(
            Outcome {
                session: account("B"),
                account: Some("b•••@example.com".into()),
                result: Some(Err(FetchError::AuthExpired)),
                // The adapter must not label it, but the tracker enforces it.
                fallback: Some(fetched(30, t0() - secs(600), Origin::SessionLog)),
                ..Outcome::default()
            },
            at(0),
        );
        let usage = tracker.usage(at(0));
        assert_eq!(usage.origin, Some(Origin::SessionLog));
        assert_eq!(usage.reading, Reading::Stale { percent_used: 30 });
        let stored = tracker.last_good.as_ref().unwrap();
        assert_eq!(stored.identity, None);
        assert_eq!(stored.account, None);

        // No fallback is kept after a logout.
        tracker.begin(at(10));
        tracker.finish(
            Outcome {
                fallback: Some(fetched(30, t0(), Origin::SessionLog)),
                ..Outcome::signed_out(FetchError::AuthMissing)
            },
            at(10),
        );
        assert_eq!(tracker.usage(at(10)).reading, Reading::Unavailable);
    }

    #[test]
    fn b01_an_api_reading_without_identity_is_not_reused_after_a_failure() {
        let mut tracker = tracker();
        let mut reading = fetched(44, t0(), Origin::Api);
        reading.identity = None;
        tracker.begin(at(0));
        tracker.finish(
            Outcome {
                session: Session::Unverified,
                result: Some(Ok(reading)),
                ..Outcome::default()
            },
            at(0),
        );
        assert_eq!(
            tracker.usage(at(0)).reading,
            Reading::Available { percent_used: 44 }
        );
        tracker.begin(at(300));
        tracker.finish(
            failed_as(Session::Unverified, None, FetchError::Network),
            at(300),
        );
        assert_eq!(tracker.usage(at(300)).reading, Reading::Unavailable);
    }

    #[test]
    fn b01_a_probe_detects_logout_and_switch_before_the_next_attempt() {
        let mut tracker = tracker_with_a();
        tracker.probe(account("B"), at(10));
        assert_eq!(tracker.usage(at(10)).reading, Reading::Unavailable);

        let mut tracker = tracker_with_a();
        tracker.probe(Session::SignedOut, at(10));
        assert_eq!(tracker.usage(at(10)).reading, Reading::Unavailable);
    }

    #[test]
    fn b01_the_retry_state_of_one_account_does_not_bind_the_next() {
        let mut tracker = tracker_with_a();
        tracker.begin(at(0));
        tracker.finish(
            failed_as(
                account("A"),
                None,
                FetchError::RateLimited {
                    retry_after: Some(Duration::from_secs(3600)),
                },
            ),
            at(0),
        );
        assert!(!tracker.should_fetch(at(30), true));
        assert!(!tracker.should_fetch(at(30), false));
        // Another account signs in: A's Retry-After does not apply to it,
        // neither to a click nor to the automatic refresh (PR #1 review).
        tracker.probe(account("B"), at(30));
        assert!(tracker.should_fetch(at(30), true));
        assert!(
            tracker.should_fetch(at(30), false),
            "B's automatic refresh must not wait for A's deadline"
        );
        // The same after a logout and a sign-in to another account.
        let mut tracker = tracker_with_a();
        tracker.begin(at(0));
        tracker.finish(
            failed_as(
                account("A"),
                None,
                FetchError::RateLimited {
                    retry_after: Some(Duration::from_secs(3600)),
                },
            ),
            at(0),
        );
        tracker.probe(Session::SignedOut, at(20));
        tracker.probe(account("B"), at(30));
        assert!(tracker.should_fetch(at(30), false));
        // A renewed credential of the same account keeps the server wait.
        let mut tracker = tracker_with_a();
        tracker.begin(at(0));
        tracker.finish(
            failed_as(
                account("A"),
                None,
                FetchError::RateLimited {
                    retry_after: Some(Duration::from_secs(3600)),
                },
            ),
            at(0),
        );
        tracker.probe(Session::account(&["A"], "renewed"), at(30));
        assert!(!tracker.should_fetch(at(30), true));
        assert!(!tracker.should_fetch(at(30), false));
    }

    // --- B02: checked time arithmetic ------------------------------------

    #[test]
    fn b02_an_absurd_retry_after_is_bounded_and_never_panics() {
        let mut tracker = tracker_with_a();
        let header = crate::usage::http::parse_retry_after("18446744073709551615", t0());
        tracker.begin(at(0));
        tracker.finish(
            failed_as(
                account("A"),
                None,
                FetchError::RateLimited {
                    retry_after: header,
                },
            ),
            at(0),
        );
        assert_eq!(waited(&tracker, at(0)), 24 * 3600);
        let usage = tracker.usage(at(0));
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
        tracker.begin(at(0));
        tracker.finish(
            Outcome::failed(FetchError::RateLimited {
                retry_after: Some(Duration::from_secs(2 * 3600)),
            }),
            at(0),
        );
        assert_eq!(waited(&tracker, at(0)), 7200);
        assert!(!tracker.should_fetch(at(7199), true));
        assert!(tracker.should_fetch(at(7200), false));
    }

    #[test]
    fn b02_the_end_of_the_time_range_saturates_instead_of_panicking() {
        let end = Now {
            instant: at(0).instant,
            utc: DateTime::<Utc>::MAX_UTC - secs(10),
        };
        let mut tracker = Tracker::new(Provider::Codex, Source::Live, Policy::default(), end);
        tracker.begin(end);
        tracker.finish(
            Outcome::failed(FetchError::RateLimited {
                retry_after: Some(Duration::MAX),
            }),
            end,
        );
        assert_eq!(
            tracker.usage(end).issue,
            Some(Issue::RateLimited {
                retry_at: Some(rfc3339(DateTime::<Utc>::MAX_UTC))
            })
        );
        tracker.begin(end);
        tracker.finish(ok(fetched(10, t0(), Origin::Api)), end);
        let _ = tracker.next_wake(end);
        for utc in [DateTime::<Utc>::MAX_UTC, DateTime::<Utc>::MIN_UTC] {
            let now = Now { utc, ..end };
            let _ = tracker.usage(now);
            let _ = tracker.next_wake(now);
        }
    }

    #[test]
    fn b02_an_aborted_attempt_leaves_a_valid_state() {
        let mut tracker = tracker_with_a();
        tracker.begin(at(300));
        assert!(tracker.usage(at(300)).refreshing);
        tracker.abort(at(300));
        let usage = tracker.usage(at(300));
        assert!(!usage.refreshing);
        assert_eq!(usage.issue, Some(Issue::Malformed));
        assert_eq!(usage.reading, Reading::Stale { percent_used: 91 });
        assert_eq!(usage.account.as_deref(), Some("a•••@example.com"));
        assert!(tracker.next_due() > at(300).instant);
    }

    // --- I01: recovery gates ---------------------------------------------

    #[test]
    fn i01_service_unavailable_retry_after_binds_both_triggers() {
        let mut tracker = tracker_with_a();
        tracker.begin(at(0));
        tracker.finish(
            failed_as(
                account("A"),
                None,
                FetchError::Http {
                    status: 503,
                    retry_after: Some(Duration::from_secs(300)),
                },
            ),
            at(0),
        );
        assert_eq!(waited(&tracker, at(0)), 300);
        assert!(!tracker.should_fetch(at(20), true));
        assert!(!tracker.should_fetch(at(299), false));
        assert!(tracker.should_fetch(at(300), true));
        assert_eq!(
            tracker.usage(at(0)).issue,
            Some(Issue::Http {
                status: 503,
                retry_at: Some("2026-11-23T18:05:00Z".into())
            })
        );
    }

    #[test]
    fn i01_a_click_cannot_retry_a_rejected_credential_early() {
        let mut tracker = tracker();
        tracker.begin(at(0));
        tracker.finish(
            failed_as(account("A"), None, FetchError::AuthRejected),
            at(0),
        );
        assert_eq!(waited(&tracker, at(0)), 300);
        assert!(!tracker.should_fetch(at(20), true));
        assert!(!tracker.should_fetch(at(299), true));
        assert!(tracker.should_fetch(at(300), true));
    }

    #[test]
    fn i01_signing_in_again_lifts_the_local_recovery_wait() {
        let mut tracker = tracker();
        tracker.begin(at(0));
        tracker.finish(
            failed_as(account("A"), None, FetchError::AuthRejected),
            at(0),
        );
        // The provider's own client renewed the token.
        tracker.probe(Session::account(&["A"], "fresh"), at(25));
        assert!(tracker.should_fetch(at(25), true));
        assert!(tracker.should_fetch(at(25), false));
    }

    #[test]
    fn i01_network_failures_allow_a_manual_retry_after_the_short_gap() {
        let mut tracker = tracker();
        tracker.begin(at(0));
        tracker.finish(failed_as(account("A"), None, FetchError::Network), at(0));
        assert!(!tracker.should_fetch(at(19), true));
        assert!(tracker.should_fetch(at(20), true));
        assert!(!tracker.should_fetch(at(20), false), "automatic waits 60 s");
    }

    #[test]
    fn i01_malformed_responses_hold_manual_retries_for_the_backoff() {
        let mut tracker = tracker();
        tracker.begin(at(0));
        tracker.finish(
            failed_as(account("A"), None, FetchError::Malformed("x".into())),
            at(0),
        );
        assert!(!tracker.should_fetch(at(59), true));
        assert!(tracker.should_fetch(at(60), true));
        assert!(!tracker.should_fetch(at(60), false));
    }

    // --- I02: renewal without a new measurement ----------------------------

    fn renewing(percent: u8, in_seconds: i64, other: Option<u8>) -> Fetched {
        let mut quotas = vec![Quota::window(
            "fiveHour",
            "Sessão de 5 h",
            percent,
            Some(300),
            Some(rfc3339(t0() + secs(in_seconds))),
        )];
        if let Some(other) = other {
            quotas.push(Quota::window(
                "sevenDay",
                "Semanal",
                other,
                Some(10_080),
                Some(rfc3339(t0() + secs(86_400))),
            ));
        }
        Fetched {
            quotas,
            ..fetched(0, t0(), Origin::Api)
        }
    }

    #[test]
    fn i02_a_single_renewed_window_is_never_a_fresh_zero() {
        let mut tracker = tracker();
        tracker.begin(at(0));
        tracker.finish(ok(renewing(91, 60, None)), at(0));
        assert_eq!(
            tracker.usage(at(59)).reading,
            Reading::Available { percent_used: 91 }
        );
        let usage = tracker.usage(at(61));
        assert_eq!(usage.reading, Reading::Stale { percent_used: 0 });
        let window = usage.window.unwrap();
        assert!(window.renewed);
        assert_eq!(window.resets_at, None);
        assert_eq!(usage.updated_at.as_deref(), Some("2026-11-23T18:00:00Z"));
    }

    #[test]
    fn i02_a_renewed_window_leaves_the_primary_to_a_measured_one() {
        let mut tracker = tracker();
        tracker.begin(at(0));
        tracker.finish(ok(renewing(91, 60, Some(40))), at(0));
        assert_eq!(tracker.usage(at(0)).window.unwrap().id, "fiveHour");
        let usage = tracker.usage(at(61));
        assert_eq!(usage.reading, Reading::Available { percent_used: 40 });
        assert_eq!(usage.window.unwrap().id, "sevenDay");
        assert!(usage.quotas[0].renewed);
    }

    #[test]
    fn i02_renewals_and_ageing_are_wake_ups_even_while_offline() {
        let mut tracker = tracker();
        tracker.begin(at(0));
        tracker.finish(ok(renewing(91, 60, Some(40))), at(0));
        // The renewal comes before the regular refresh.
        assert_eq!(tracker.next_wake(at(0)), at(60).instant);
        // After it, the next transition is the regular refresh (300 s),
        // then the reading turning stale (900 s) once the provider fails.
        assert_eq!(tracker.next_wake(at(61)), at(300).instant);
        tracker.begin(at(300));
        tracker.finish(failed_as(account("A"), None, FetchError::Network), at(300));
        assert_eq!(tracker.next_wake(at(300)), at(360).instant);
        tracker.begin(at(360));
        tracker.finish(failed_as(account("A"), None, FetchError::Network), at(360));
        assert_eq!(tracker.next_wake(at(360)), at(480).instant);
        tracker.begin(at(480));
        tracker.finish(failed_as(account("A"), None, FetchError::Network), at(480));
        assert_eq!(tracker.next_wake(at(480)), at(720).instant);
        tracker.begin(at(720));
        tracker.finish(failed_as(account("A"), None, FetchError::Network), at(720));
        // The backoff (until 1200 s) passes the age limit: the stale
        // transition at 900 s still wakes the worker.
        assert_eq!(tracker.next_wake(at(720)), at(900).instant);
    }

    #[test]
    fn a_chosen_metric_replaces_the_automatic_choice() {
        let mut tracker = tracker();
        tracker.begin(at(0));
        tracker.finish(ok(fetched(73, t0(), Origin::Api)), at(1));
        // Automatic: the weekly window (73%) is the most consumed.
        let usage = tracker.usage(at(2));
        assert_eq!(usage.window.unwrap().id, "sevenDay");
        assert_eq!(usage.selected_metric, None);

        tracker.select_metric(Some("fiveHour".into()));
        let usage = tracker.usage(at(2));
        assert_eq!(usage.reading, Reading::Available { percent_used: 10 });
        assert_eq!(usage.window.unwrap().id, "fiveHour");
        assert_eq!(usage.selected_metric.as_deref(), Some("fiveHour"));
        assert!(!usage.metric_missing);
        // The other window is still listed.
        assert_eq!(usage.quotas.len(), 1);
        assert_eq!(usage.quotas[0].id, "sevenDay");
    }

    #[test]
    fn a_missing_chosen_metric_is_reported_instead_of_switched() {
        let mut tracker = tracker();
        tracker.select_metric(Some("sevenDayOpus".into()));
        // No data yet: nothing can be said about the metric.
        assert!(!tracker.usage(at(0)).metric_missing);

        tracker.begin(at(0));
        tracker.finish(ok(fetched(73, t0(), Origin::Api)), at(1));
        let usage = tracker.usage(at(2));
        assert!(usage.metric_missing);
        assert_eq!(usage.reading, Reading::Unavailable);
        assert!(usage.window.is_none());
        assert_eq!(usage.quotas.len(), 2, "every quota stays listed");

        // Back to the automatic choice.
        tracker.select_metric(None);
        let usage = tracker.usage(at(2));
        assert!(!usage.metric_missing);
        assert_eq!(usage.reading, Reading::Available { percent_used: 73 });
    }

    #[test]
    fn a_chosen_metric_that_renewed_awaits_a_new_reading() {
        let mut tracker = tracker();
        tracker.select_metric(Some("fiveHour".into()));
        tracker.begin(at(0));
        tracker.finish(ok(fetched(73, t0(), Origin::Api)), at(1));
        // The five-hour window renews one hour after the observation.
        let usage = tracker.usage(at(3_601));
        let window = usage.window.unwrap();
        assert_eq!(window.id, "fiveHour");
        assert!(window.renewed);
        assert_eq!(usage.reading, Reading::Stale { percent_used: 0 });
    }

    #[test]
    fn a_blocked_source_is_a_local_state_that_never_reaches_the_network() {
        let mut tracker = tracker();
        tracker.begin(at(0));
        tracker.finish(
            Outcome {
                session: account("A"),
                account: Some("a•••@example.com".into()),
                ..Outcome::failed(FetchError::SourceBlocked(
                    crate::usage::model::BlockReason::CliSafety,
                ))
            },
            at(0),
        );
        let usage = tracker.usage(at(1));
        assert_eq!(usage.reading, Reading::Unavailable);
        assert_eq!(
            usage.issue,
            Some(Issue::SourceBlocked {
                reason: crate::usage::model::BlockReason::CliSafety
            })
        );
        assert_eq!(usage.account.as_deref(), Some("a•••@example.com"));
        // Re-checked like other local states, with no server wait.
        assert_eq!(waited(&tracker, at(0)), 120);
        assert!(tracker.should_fetch(at(20), true));
    }

    #[test]
    fn the_refresh_cadence_comes_from_the_catalog() {
        for provider in Provider::ALL {
            assert_eq!(
                Policy::for_provider(provider).interval,
                crate::usage::catalog::refresh_interval(provider)
            );
        }
    }

    #[test]
    fn the_alert_view_ignores_the_ring_metric() {
        let mut tracker = tracker();
        tracker.begin(at(0));
        tracker.finish(ok(fetched(73, t0(), Origin::Api)), at(1));

        // A chosen metric that vanished makes the ring show nothing...
        tracker.select_metric(Some("sevenDayOpus".into()));
        assert_eq!(tracker.usage(at(2)).reading, Reading::Unavailable);
        // ...but the reading is still fresh and fully measured for alerts.
        let view = tracker.automatic_usage(at(2));
        assert_eq!(view.reading, Reading::Available { percent_used: 73 });
        assert!(!view.metric_missing);
        assert_eq!(view.quotas.len(), 1);

        // A chosen window that renewed (60 s after the reading, well within
        // its freshness): the ring awaits a reading, while the other window
        // is still a fresh measurement for alerts.
        let mut early = fetched(73, t0(), Origin::Api);
        early.quotas[0].resets_at = Some(rfc3339(t0() + secs(60)));
        let mut renewing = Tracker::new(Provider::Codex, Source::Live, Policy::default(), at(0));
        renewing.begin(at(0));
        renewing.finish(ok(early), at(1));
        renewing.select_metric(Some("fiveHour".into()));
        assert!(matches!(
            renewing.usage(at(120)).reading,
            Reading::Stale { .. }
        ));
        let view = renewing.automatic_usage(at(120));
        assert_eq!(view.reading, Reading::Available { percent_used: 73 });
        assert_eq!(view.window.unwrap().id, "sevenDay");
    }

    #[test]
    fn an_unverified_cli_reading_is_hidden_after_a_failed_attempt() {
        let mut tracker = Tracker::new(
            Provider::Antigravity,
            Source::Live,
            Policy::default(),
            at(0),
        );
        let reading = Fetched {
            identity: None,
            account: None,
            plan: None,
            origin: Origin::Cli,
            quotas: vec![Quota::window(
                "agy-gemini-5h",
                "Gemini · 5 h",
                69,
                Some(300),
                None,
            )],
            observed_at: t0(),
        };
        tracker.begin(at(0));
        tracker.finish(
            Outcome {
                session: Session::Unverified,
                result: Some(Ok(reading)),
                ..Outcome::default()
            },
            at(1),
        );
        assert_eq!(
            tracker.usage(at(2)).reading,
            Reading::Available { percent_used: 69 }
        );
        // The next attempt fails: the reading may belong to an account the
        // user has left in the CLI, so it is no longer shown.
        tracker.begin(at(400));
        tracker.finish(
            Outcome {
                session: Session::Unverified,
                ..Outcome::failed(FetchError::Timeout)
            },
            at(401),
        );
        let usage = tracker.usage(at(402));
        assert_eq!(usage.reading, Reading::Unavailable);
        assert_eq!(usage.origin, None);
    }
}

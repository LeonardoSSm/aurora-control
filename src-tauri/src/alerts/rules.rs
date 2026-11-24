//! Alert rules: pure logic over the published usage state.
//!
//! The engine sees each provider's [`Observation`] (the same tracker state the
//! rail shows) and decides which alerts a change deserves. It never queries a
//! provider. State is kept per provider and per verified account:
//!
//! - **Baseline.** The first valid reading of an account (after start-up,
//!   after enabling a provider, after an account switch) only records levels.
//!   Nothing accumulated before it is announced, so a restart never repeats an
//!   alert and enabling a provider at 96% does not fire 70, 85 and 95.
//! - **Consumption.** A quota alerts when a fresh reading crosses a configured
//!   threshold above the level already announced. Several thresholds crossed
//!   at once, on one or more quotas, become one alert at the highest level.
//!   Staying above a threshold, or dipping and coming back without a renewal,
//!   never alerts again.
//! - **Renewal.** Announced only when a *new reading* shows the quota below
//!   the level it had reached, after its renewal time (or, without a known
//!   renewal time, when the reading simply came back down). A renewal time
//!   that passes on the clock is not a confirmation: the scheduler marks such
//!   a window "renewed, awaiting a reading", which is not a fresh reading and
//!   is ignored here. A confirmed renewal re-arms the quota's thresholds.
//! - **Sign-in.** One alert per transition into an expired, unavailable or
//!   rejected sign-in; nothing while the problem persists. Optionally one when
//!   a fresh reading arrives after a problem (reconnection).
//! - **Isolation and order.** A different account identity drops the state of
//!   the previous one. Readings without a verified account, stale readings,
//!   readings not newer than the last one processed, demo data and disabled
//!   providers raise nothing.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::preferences::Notifications;
use crate::usage::{
    Fingerprint, Issue, Observation, Provider, QuotaKind, Reading, Source, Usage, catalog,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthProblem {
    Expired,
    Unavailable,
    Rejected,
}

impl AuthProblem {
    /// Sign-in states worth an alert. An unreadable credential file is left
    /// out: it is usually the provider rewriting it, and clears on its own.
    fn of(issue: Option<&Issue>) -> Option<Self> {
        match issue? {
            Issue::AuthExpired => Some(Self::Expired),
            Issue::AuthMissing | Issue::CredentialsUnavailable => Some(Self::Unavailable),
            Issue::AuthRejected => Some(Self::Rejected),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum AlertKind {
    /// The highest threshold crossed by this reading.
    Consumption {
        threshold: u8,
        percent: u8,
    },
    Renewal,
    Auth(AuthProblem),
    Reconnected,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Alert {
    pub provider: Provider,
    pub kind: AlertKind,
    pub title: String,
    pub body: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum AuthMemo {
    /// Nothing observed yet for this account: the first state is a baseline.
    #[default]
    Unknown,
    Fine,
    Problem(AuthProblem),
}

#[derive(Clone, Debug)]
struct QuotaMemo {
    /// Highest threshold already announced (or present at the baseline).
    level: u8,
    resets_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Default)]
struct Memo {
    account: Option<Fingerprint>,
    /// Observation time of the last reading processed.
    last_reading: Option<DateTime<Utc>>,
    quotas: HashMap<String, QuotaMemo>,
    auth: AuthMemo,
}

/// The settings that apply to one provider.
pub struct Settings<'a> {
    pub notifications: &'a Notifications,
    /// The provider's own alert switch.
    pub provider_alerts: bool,
}

impl Settings<'_> {
    fn active(&self) -> bool {
        self.notifications.enabled && self.provider_alerts
    }
}

#[derive(Default)]
pub struct Engine {
    memos: HashMap<Provider, Memo>,
}

fn parse_time(value: Option<&str>) -> Option<DateTime<Utc>> {
    value
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|time| time.with_timezone(&Utc))
}

/// A measured window of a fresh reading.
struct Measured<'a> {
    id: &'a str,
    label: &'a str,
    percent: u8,
    resets_at: Option<DateTime<Utc>>,
}

/// The windows a fresh reading measured: the ring's metric and every listed
/// window that may stand for the account's consumption. Renewed windows have
/// no measurement; credits are not consumption windows.
fn measured(usage: &Usage) -> Vec<Measured<'_>> {
    let mut windows = Vec::new();
    if let (Reading::Available { percent_used }, Some(window)) = (&usage.reading, &usage.window)
        && !window.renewed
    {
        windows.push(Measured {
            id: &window.id,
            label: &window.label,
            percent: *percent_used,
            resets_at: parse_time(window.resets_at.as_deref()),
        });
    }
    for quota in &usage.quotas {
        if quota.kind == QuotaKind::Window && quota.eligible && !quota.renewed {
            windows.push(Measured {
                id: &quota.id,
                label: &quota.label,
                percent: quota.percent_used,
                resets_at: parse_time(quota.resets_at.as_deref()),
            });
        }
    }
    windows
}

/// The highest configured threshold at or below `percent`, or 0.
fn level(thresholds: &[u8], percent: u8) -> u8 {
    thresholds
        .iter()
        .copied()
        .filter(|threshold| *threshold <= percent)
        .max()
        .unwrap_or(0)
}

impl Engine {
    /// Forgets a provider: disabling it, or leaving demo data, starts over
    /// from a baseline when it comes back.
    pub fn forget(&mut self, provider: Provider) {
        self.memos.remove(&provider);
    }

    /// The alerts one observation deserves. Updates the state even when
    /// alerts are off or muted, so nothing piles up to be replayed later.
    pub fn observe(&mut self, observation: &Observation, settings: &Settings) -> Vec<Alert> {
        let usage = &observation.usage;
        let provider = usage.provider;
        if !observation.enabled || usage.source != Source::Live {
            self.forget(provider);
            return Vec::new();
        }
        let memo = self.memos.entry(provider).or_default();
        if let Some(identity) = observation.identity
            && memo.account != Some(identity)
        {
            // A different (or first) verified account: nothing carries over.
            *memo = Memo {
                account: Some(identity),
                ..Memo::default()
            };
        }

        let mut alerts = Vec::new();
        let name = catalog::name(provider);
        let fresh = matches!(usage.reading, Reading::Available { .. }) && usage.issue.is_none();

        // Sign-in transitions.
        match (AuthProblem::of(usage.issue.as_ref()), memo.auth) {
            (Some(problem), AuthMemo::Unknown) => memo.auth = AuthMemo::Problem(problem),
            (Some(problem), AuthMemo::Problem(previous)) if problem == previous => {}
            (Some(problem), _) => {
                memo.auth = AuthMemo::Problem(problem);
                if settings.notifications.auth {
                    alerts.push(auth_alert(provider, name, problem));
                }
            }
            (None, AuthMemo::Problem(_)) if fresh => {
                memo.auth = AuthMemo::Fine;
                if settings.notifications.reconnect {
                    alerts.push(Alert {
                        provider,
                        kind: AlertKind::Reconnected,
                        title: format!("{name} — Monitoramento restaurado"),
                        body: "Uma nova consulta foi concluída com sucesso.".into(),
                    });
                }
            }
            (None, AuthMemo::Unknown) if fresh => memo.auth = AuthMemo::Fine,
            _ => {}
        }

        // Quotas: only fresh readings of a verified account, newer than the
        // last one processed.
        let observed = parse_time(usage.updated_at.as_deref());
        let newer = observed.is_some_and(|at| memo.last_reading.is_none_or(|last| at > last));
        if fresh
            && newer
            && observation.identity.is_some()
            && memo.account == observation.identity
            && let Some(at) = observed
        {
            memo.last_reading = Some(at);
            let thresholds = &settings.notifications.thresholds;
            let mut crossed: Option<(u8, u8, String)> = None;
            let mut renewed: Vec<String> = Vec::new();
            for window in measured(usage) {
                let now_level = level(thresholds, window.percent);
                let Some(previous) = memo.quotas.get(window.id).cloned() else {
                    // Baseline for this quota.
                    memo.quotas.insert(
                        window.id.to_owned(),
                        QuotaMemo {
                            level: now_level,
                            resets_at: window.resets_at,
                        },
                    );
                    continue;
                };
                let past_reset = previous.resets_at.is_none_or(|reset| at >= reset);
                let next_level =
                    if previous.level > 0 && window.percent < previous.level && past_reset {
                        renewed.push(window.label.to_owned());
                        now_level
                    } else if now_level > previous.level {
                        if crossed
                            .as_ref()
                            .is_none_or(|(threshold, _, _)| now_level > *threshold)
                        {
                            crossed = Some((now_level, window.percent, window.label.to_owned()));
                        }
                        now_level
                    } else {
                        previous.level
                    };
                memo.quotas.insert(
                    window.id.to_owned(),
                    QuotaMemo {
                        level: next_level,
                        resets_at: window.resets_at,
                    },
                );
            }
            if let Some((threshold, percent, label)) = crossed {
                alerts.push(consumption_alert(
                    provider, name, threshold, percent, &label,
                ));
            }
            if !renewed.is_empty() && settings.notifications.renewal {
                alerts.push(Alert {
                    provider,
                    kind: AlertKind::Renewal,
                    title: format!("{name} — Limite renovado"),
                    body: format!(
                        "A nova consulta confirmou a recuperação da sua cota ({}).",
                        renewed.join(", ")
                    ),
                });
            }
        }

        if settings.active() {
            alerts
        } else {
            Vec::new()
        }
    }
}

fn consumption_alert(
    provider: Provider,
    name: &str,
    threshold: u8,
    percent: u8,
    label: &str,
) -> Alert {
    let title = if threshold >= 100 {
        format!("{name} — Cota esgotada")
    } else {
        format!("{name} — {threshold}% da cota")
    };
    Alert {
        provider,
        kind: AlertKind::Consumption { threshold, percent },
        title,
        body: format!("{label}: {percent}% utilizado."),
    }
}

fn auth_alert(provider: Provider, name: &str, problem: AuthProblem) -> Alert {
    let reconnect = &catalog::entry(provider).reconnect;
    let (title, body) = match problem {
        AuthProblem::Expired => (format!("{name} — Sessão expirada"), reconnect.clone()),
        AuthProblem::Unavailable => (
            format!("{name} — Credencial indisponível"),
            format!("A sessão local não foi encontrada. {reconnect}"),
        ),
        AuthProblem::Rejected => (
            format!("{name} — Autenticação rejeitada"),
            format!("O provedor recusou a sessão local. {reconnect}"),
        ),
    };
    Alert {
        provider,
        kind: AlertKind::Auth(problem),
        title,
        body,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::{ConsumptionWindow, Quota};

    fn account(name: &str) -> Option<Fingerprint> {
        Some(Fingerprint::of(&[name]))
    }

    fn at(minute: i64) -> String {
        (DateTime::parse_from_rfc3339("2026-11-24T12:00:00Z").unwrap()
            + chrono::TimeDelta::minutes(minute))
        .with_timezone(&Utc)
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    }

    /// A fresh reading of the five-hour window at `percent`, observed at
    /// `minute`, renewing at minute 300.
    fn reading(percent: u8, minute: i64) -> Usage {
        Usage {
            provider: Provider::Codex,
            account: None,
            plan: None,
            source: Source::Live,
            origin: None,
            reading: Reading::Available {
                percent_used: percent,
            },
            window: Some(ConsumptionWindow {
                id: "fiveHour".into(),
                label: "Janela de 5 h".into(),
                duration_minutes: Some(300),
                resets_at: Some(at(300)),
                renewed: false,
            }),
            quotas: vec![],
            activity: None,
            updated_at: Some(at(minute)),
            checked_at: Some(at(minute)),
            issue: None,
            refreshing: false,
            selected_metric: None,
            metric_missing: false,
        }
    }

    fn seen(usage: Usage, identity: Option<Fingerprint>) -> Observation {
        Observation {
            enabled: true,
            identity,
            usage,
        }
    }

    fn settings(notifications: &Notifications) -> Settings<'_> {
        Settings {
            notifications,
            provider_alerts: true,
        }
    }

    fn kinds(alerts: &[Alert]) -> Vec<AlertKind> {
        alerts.iter().map(|alert| alert.kind.clone()).collect()
    }

    fn all_thresholds() -> Notifications {
        Notifications {
            thresholds: vec![70, 85, 95, 100],
            reconnect: true,
            ..Notifications::default()
        }
    }

    #[test]
    fn crossing_a_threshold_alerts_once() {
        let prefs = all_thresholds();
        let mut engine = Engine::default();
        // First reading: baseline, even at 82%.
        assert!(
            engine
                .observe(&seen(reading(82, 0), account("A")), &settings(&prefs))
                .is_empty()
        );
        let alerts = engine.observe(&seen(reading(87, 5), account("A")), &settings(&prefs));
        assert_eq!(
            kinds(&alerts),
            [AlertKind::Consumption {
                threshold: 85,
                percent: 87
            }]
        );
        assert_eq!(alerts[0].title, "Codex — 85% da cota");
        assert_eq!(alerts[0].body, "Janela de 5 h: 87% utilizado.");
        // Staying above it, polling after polling: nothing.
        for minute in 10..20 {
            assert!(
                engine
                    .observe(&seen(reading(88, minute), account("A")), &settings(&prefs))
                    .is_empty()
            );
        }
    }

    #[test]
    fn several_thresholds_in_one_update_become_one_alert_at_the_highest() {
        let prefs = all_thresholds();
        let mut engine = Engine::default();
        engine.observe(&seen(reading(50, 0), account("A")), &settings(&prefs));
        let alerts = engine.observe(&seen(reading(100, 5), account("A")), &settings(&prefs));
        assert_eq!(
            kinds(&alerts),
            [AlertKind::Consumption {
                threshold: 100,
                percent: 100
            }]
        );
        assert_eq!(alerts[0].title, "Codex — Cota esgotada");
    }

    #[test]
    fn the_first_reading_of_a_provider_never_fires_accumulated_thresholds() {
        let prefs = all_thresholds();
        let mut engine = Engine::default();
        assert!(
            engine
                .observe(&seen(reading(96, 0), account("A")), &settings(&prefs))
                .is_empty()
        );
        // Only the next crossing counts.
        let alerts = engine.observe(&seen(reading(100, 5), account("A")), &settings(&prefs));
        assert_eq!(alerts.len(), 1);
    }

    #[test]
    fn a_dip_without_renewal_does_not_rearm_the_threshold() {
        let prefs = all_thresholds();
        let mut engine = Engine::default();
        engine.observe(&seen(reading(80, 0), account("A")), &settings(&prefs));
        assert_eq!(
            engine
                .observe(&seen(reading(86, 5), account("A")), &settings(&prefs))
                .len(),
            1
        );
        // Before the renewal time, a lower value is not a renewal.
        assert!(
            engine
                .observe(&seen(reading(83, 10), account("A")), &settings(&prefs))
                .is_empty()
        );
        assert!(
            engine
                .observe(&seen(reading(86, 15), account("A")), &settings(&prefs))
                .is_empty()
        );
    }

    #[test]
    fn a_renewal_is_announced_only_when_a_new_reading_confirms_it() {
        let prefs = all_thresholds();
        let mut engine = Engine::default();
        engine.observe(&seen(reading(80, 0), account("A")), &settings(&prefs));
        engine.observe(&seen(reading(96, 5), account("A")), &settings(&prefs));

        // The renewal time passed, but no new reading: the window is shown as
        // renewed and awaiting a reading (stale), which confirms nothing.
        let mut waiting = reading(0, 301);
        waiting.reading = Reading::Stale { percent_used: 0 };
        waiting.window.as_mut().unwrap().renewed = true;
        assert!(
            engine
                .observe(&seen(waiting, account("A")), &settings(&prefs))
                .is_empty()
        );

        // A fresh reading after the renewal confirms it.
        let mut fresh = reading(3, 302);
        fresh.window.as_mut().unwrap().resets_at = Some(at(600));
        let alerts = engine.observe(&seen(fresh, account("A")), &settings(&prefs));
        assert_eq!(kinds(&alerts), [AlertKind::Renewal]);
        assert_eq!(alerts[0].title, "Codex — Limite renovado");
        assert!(
            alerts[0]
                .body
                .starts_with("A nova consulta confirmou a recuperação da sua cota")
        );

        // Thresholds are re-armed by the confirmed renewal.
        let mut again = reading(72, 400);
        again.window.as_mut().unwrap().resets_at = Some(at(600));
        assert_eq!(
            kinds(&engine.observe(&seen(again, account("A")), &settings(&prefs))),
            [AlertKind::Consumption {
                threshold: 70,
                percent: 72
            }]
        );
    }

    #[test]
    fn stale_and_out_of_order_readings_raise_nothing() {
        let prefs = all_thresholds();
        let mut engine = Engine::default();
        engine.observe(&seen(reading(50, 10), account("A")), &settings(&prefs));
        // Older than the last one processed.
        assert!(
            engine
                .observe(&seen(reading(99, 5), account("A")), &settings(&prefs))
                .is_empty()
        );
        // The same reading again.
        assert!(
            engine
                .observe(&seen(reading(99, 10), account("A")), &settings(&prefs))
                .is_empty()
        );
        // Stale (for example a session log, or an old reading after an error).
        let mut stale = reading(99, 20);
        stale.reading = Reading::Stale { percent_used: 99 };
        assert!(
            engine
                .observe(&seen(stale, account("A")), &settings(&prefs))
                .is_empty()
        );
        // A reading with an issue is not fresh either.
        let mut failing = reading(99, 25);
        failing.issue = Some(Issue::Network);
        assert!(
            engine
                .observe(&seen(failing, account("A")), &settings(&prefs))
                .is_empty()
        );
        // The next fresh reading still alerts from the last processed one.
        assert_eq!(
            engine
                .observe(&seen(reading(99, 30), account("A")), &settings(&prefs))
                .len(),
            1
        );
    }

    #[test]
    fn an_account_switch_never_carries_alerts_over() {
        let prefs = all_thresholds();
        let mut engine = Engine::default();
        engine.observe(&seen(reading(80, 0), account("A")), &settings(&prefs));
        // Account B at 96%: a new baseline, not a crossing from A's 80%.
        assert!(
            engine
                .observe(&seen(reading(96, 5), account("B")), &settings(&prefs))
                .is_empty()
        );
        // Back to A: also a new baseline, A's levels were dropped.
        assert!(
            engine
                .observe(&seen(reading(97, 10), account("A")), &settings(&prefs))
                .is_empty()
        );
        // Readings without a verified account never alert on quotas.
        assert!(
            engine
                .observe(&seen(reading(100, 15), None), &settings(&prefs))
                .is_empty()
        );
    }

    #[test]
    fn disabled_providers_and_demo_data_raise_nothing_and_start_over() {
        let prefs = all_thresholds();
        let mut engine = Engine::default();
        engine.observe(&seen(reading(50, 0), account("A")), &settings(&prefs));
        let mut hidden = seen(reading(99, 5), account("A"));
        hidden.enabled = false;
        assert!(engine.observe(&hidden, &settings(&prefs)).is_empty());
        // Enabled again at 99%: baseline, not a jump from 50%.
        assert!(
            engine
                .observe(&seen(reading(99, 10), account("A")), &settings(&prefs))
                .is_empty()
        );

        let mut demo = reading(10, 0);
        demo.source = Source::Demo;
        let mut engine = Engine::default();
        engine.observe(&seen(demo.clone(), account("A")), &settings(&prefs));
        demo.reading = Reading::Available { percent_used: 99 };
        demo.updated_at = Some(at(5));
        assert!(
            engine
                .observe(&seen(demo, account("A")), &settings(&prefs))
                .is_empty()
        );
    }

    #[test]
    fn switched_off_alerts_still_advance_the_state() {
        let mut prefs = all_thresholds();
        prefs.enabled = false;
        let mut engine = Engine::default();
        engine.observe(&seen(reading(50, 0), account("A")), &settings(&prefs));
        assert!(
            engine
                .observe(&seen(reading(90, 5), account("A")), &settings(&prefs))
                .is_empty()
        );
        // Turned back on: the 85% crossing that happened while off is not replayed.
        prefs.enabled = true;
        assert!(
            engine
                .observe(&seen(reading(91, 10), account("A")), &settings(&prefs))
                .is_empty()
        );
        // The provider switch works the same way.
        let mut engine = Engine::default();
        let muted = Settings {
            notifications: &prefs,
            provider_alerts: false,
        };
        engine.observe(&seen(reading(50, 0), account("A")), &muted);
        assert!(
            engine
                .observe(&seen(reading(90, 5), account("A")), &muted)
                .is_empty()
        );
    }

    #[test]
    fn sign_in_problems_alert_once_per_transition_and_reconnection_is_optional() {
        let prefs = all_thresholds();
        let mut engine = Engine::default();
        engine.observe(&seen(reading(10, 0), account("A")), &settings(&prefs));

        let mut expired = reading(10, 0);
        expired.reading = Reading::Stale { percent_used: 10 };
        expired.issue = Some(Issue::AuthExpired);
        let alerts = engine.observe(&seen(expired.clone(), account("A")), &settings(&prefs));
        assert_eq!(kinds(&alerts), [AlertKind::Auth(AuthProblem::Expired)]);
        assert_eq!(alerts[0].title, "Codex — Sessão expirada");
        // Every retry while still expired: nothing.
        for _ in 0..5 {
            assert!(
                engine
                    .observe(&seen(expired.clone(), account("A")), &settings(&prefs))
                    .is_empty()
            );
        }
        // A transient unreadable file is not a sign-in alert.
        let mut rewriting = expired.clone();
        rewriting.issue = Some(Issue::CredentialsInvalid);
        assert!(
            engine
                .observe(&seen(rewriting, account("A")), &settings(&prefs))
                .is_empty()
        );
        // Rejected is a different problem.
        let mut rejected = expired.clone();
        rejected.issue = Some(Issue::AuthRejected);
        assert_eq!(
            kinds(&engine.observe(&seen(rejected, account("A")), &settings(&prefs))),
            [AlertKind::Auth(AuthProblem::Rejected)]
        );
        // Back with a fresh reading.
        let alerts = engine.observe(&seen(reading(12, 30), account("A")), &settings(&prefs));
        assert_eq!(kinds(&alerts), [AlertKind::Reconnected]);
        // Reconnection alerts are opt-in.
        let quiet_reconnect = Notifications {
            reconnect: false,
            ..all_thresholds()
        };
        let mut engine = Engine::default();
        engine.observe(
            &seen(reading(10, 0), account("A")),
            &settings(&quiet_reconnect),
        );
        engine.observe(&seen(expired, account("A")), &settings(&quiet_reconnect));
        assert!(
            engine
                .observe(
                    &seen(reading(12, 30), account("A")),
                    &settings(&quiet_reconnect)
                )
                .is_empty()
        );
    }

    #[test]
    fn a_problem_present_at_start_up_is_a_baseline() {
        let prefs = all_thresholds();
        let mut engine = Engine::default();
        let mut missing = reading(0, 0);
        missing.reading = Reading::Unavailable;
        missing.window = None;
        missing.issue = Some(Issue::AuthMissing);
        assert!(
            engine
                .observe(&seen(missing.clone(), None), &settings(&prefs))
                .is_empty()
        );
        assert!(
            engine
                .observe(&seen(missing, None), &settings(&prefs))
                .is_empty()
        );
        let alerts = engine.observe(&seen(reading(5, 10), account("A")), &settings(&prefs));
        // A new account appeared: its state starts from a baseline too.
        assert!(alerts.is_empty());
    }

    #[test]
    fn secondary_windows_alert_and_credits_never_do() {
        let prefs = all_thresholds();
        let mut engine = Engine::default();
        let with_weekly = |weekly: u8, credits: u8, minute: i64| {
            let mut usage = reading(10, minute);
            usage.quotas = vec![
                Quota::window("sevenDay", "Semanal", weekly, Some(10_080), Some(at(5_000))),
                Quota {
                    kind: QuotaKind::Credits,
                    eligible: false,
                    ..Quota::window("spend", "Gastos", credits, None, None)
                },
            ];
            usage
        };
        engine.observe(
            &seen(with_weekly(60, 10, 0), account("A")),
            &settings(&prefs),
        );
        let alerts = engine.observe(
            &seen(with_weekly(71, 99, 5), account("A")),
            &settings(&prefs),
        );
        assert_eq!(
            kinds(&alerts),
            [AlertKind::Consumption {
                threshold: 70,
                percent: 71
            }]
        );
        assert_eq!(alerts[0].body, "Semanal: 71% utilizado.");
    }
}

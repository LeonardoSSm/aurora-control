//! Smart alerts: consumption thresholds, confirmed renewals and sign-in
//! problems, delivered as native Windows notifications.
//!
//! - [`rules`]: the pure engine that turns observations into alerts.
//! - [`quiet`]: the daily quiet period.
//! - This module: wiring. It runs after every usage publication, reads the
//!   same tracker state the rail shows (`Service::observations`) and never
//!   queries a provider. The engine lock is held while observing, so
//!   publications from different worker threads are evaluated one at a time,
//!   each with the newest state.
//!
//! Nothing is persisted: after a restart the first reading of each account is
//! a baseline, which is the conservative policy that never repeats an alert
//! (see `docs/notification-engine.md`).

pub mod quiet;
pub mod rules;

use std::sync::Mutex;

use chrono::Timelike;
use tauri::{AppHandle, Manager};

use crate::preferences::{Preferences, Store};
use crate::sync::lock;
use crate::usage::{Observation, Service};
use rules::{Alert, Engine, Settings};

/// Delivers one alert. A failure is reported and dropped: alerts are never
/// queued or retried, so a failing channel cannot cause a burst later.
pub trait Notifier {
    fn deliver(&self, title: &str, body: &str) -> Result<(), String>;
}

/// The engine state, managed by Tauri.
#[derive(Default)]
pub struct Alerts {
    engine: Mutex<Engine>,
}

/// Runs the engine over the current observations with the saved settings.
pub fn collect(
    engine: &mut Engine,
    observations: &[Observation],
    preferences: &Preferences,
) -> Vec<Alert> {
    observations
        .iter()
        .flat_map(|observation| {
            let provider_alerts = preferences.providers.get(observation.usage.provider).alerts;
            engine.observe(
                observation,
                &Settings {
                    notifications: &preferences.notifications,
                    provider_alerts,
                },
            )
        })
        .collect()
}

/// Delivers `alerts` unless the quiet period is on at `minute` (local minutes
/// since midnight). Muted alerts are dropped, never replayed. Returns how many
/// were delivered.
pub fn deliver(
    alerts: &[Alert],
    preferences: &Preferences,
    minute: u16,
    notifier: &dyn Notifier,
) -> usize {
    if alerts.is_empty() || quiet::is_quiet(&preferences.notifications.quiet, minute) {
        return 0;
    }
    alerts
        .iter()
        .filter(|alert| match notifier.deliver(&alert.title, &alert.body) {
            Ok(()) => true,
            Err(error) => {
                eprintln!(
                    "Aurora Control: a {:?} notification could not be shown: {error}",
                    alert.provider
                );
                false
            }
        })
        .count()
}

/// The local time of day, read on every evaluation so clock and time-zone
/// changes apply at once.
fn local_minute() -> u16 {
    let now = chrono::Local::now();
    (now.hour() * 60 + now.minute()) as u16
}

struct Native<'a>(&'a AppHandle);

impl Notifier for Native<'_> {
    fn deliver(&self, title: &str, body: &str) -> Result<(), String> {
        use tauri_plugin_notification::NotificationExt;
        self.0
            .notification()
            .builder()
            .title(title)
            .body(body)
            .show()
            .map_err(|error| error.to_string())
    }
}

/// Called after every usage publication.
pub fn evaluate(app: &AppHandle) {
    let preferences = app.state::<Store>().get();
    let alerts = {
        let state = app.state::<Alerts>();
        let mut engine = lock(&state.engine);
        let observations = app.state::<Service>().observations();
        collect(&mut engine, &observations, &preferences)
    };
    deliver(&alerts, &preferences, local_minute(), &Native(app));
}

/// Shows a test notification now, regardless of the quiet period, so the
/// user can check how Windows presents Aurora Control's notifications.
#[tauri::command]
pub fn send_test_notification(app: AppHandle) -> Result<(), String> {
    Native(&app).deliver(
        "Aurora Control — Notificação de teste",
        "As notificações do Aurora Control estão funcionando.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::{ConsumptionWindow, Fingerprint, Provider, Reading, Source, Usage};
    use std::cell::RefCell;

    #[derive(Default)]
    struct Recorder {
        shown: RefCell<Vec<String>>,
        fail: bool,
    }

    impl Notifier for Recorder {
        fn deliver(&self, title: &str, _body: &str) -> Result<(), String> {
            if self.fail {
                return Err("toast channel unavailable".into());
            }
            self.shown.borrow_mut().push(title.into());
            Ok(())
        }
    }

    fn observation(provider: Provider, percent: u8, minute: i64) -> Observation {
        let at = |minute: i64| {
            (chrono::DateTime::parse_from_rfc3339("2026-11-24T12:00:00Z").unwrap()
                + chrono::TimeDelta::minutes(minute))
            .to_rfc3339()
        };
        Observation {
            enabled: true,
            identity: Some(Fingerprint::of(&[&format!("{provider:?}")])),
            usage: Usage {
                provider,
                account: None,
                plan: None,
                source: Source::Live,
                origin: None,
                reading: Reading::Available {
                    percent_used: percent,
                },
                window: Some(ConsumptionWindow {
                    id: "w".into(),
                    label: "Semanal".into(),
                    duration_minutes: None,
                    resets_at: Some(at(10_000)),
                    renewed: false,
                }),
                quotas: vec![],
                activity: None,
                updated_at: Some(at(minute)),
                checked_at: None,
                issue: None,
                refreshing: false,
                selected_metric: None,
                metric_missing: false,
            },
        }
    }

    #[test]
    fn each_provider_is_evaluated_with_its_own_switch() {
        let mut preferences = Preferences::default();
        preferences.providers.claude_code.alerts = false;
        let mut engine = Engine::default();
        let first = [
            observation(Provider::Codex, 50, 0),
            observation(Provider::ClaudeCode, 50, 0),
        ];
        assert!(collect(&mut engine, &first, &preferences).is_empty());
        let second = [
            observation(Provider::Codex, 96, 5),
            observation(Provider::ClaudeCode, 96, 5),
        ];
        let alerts = collect(&mut engine, &second, &preferences);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].provider, Provider::Codex);
        assert_eq!(alerts[0].title, "Codex — 95% da cota");
    }

    #[test]
    fn quiet_hours_drop_alerts_without_replaying_them_later() {
        let mut preferences = Preferences::default();
        preferences.notifications.quiet = crate::preferences::QuietHours {
            enabled: true,
            start: 22 * 60,
            end: 7 * 60,
        };
        let mut engine = Engine::default();
        collect(
            &mut engine,
            &[observation(Provider::Codex, 50, 0)],
            &preferences,
        );
        let alerts = collect(
            &mut engine,
            &[observation(Provider::Codex, 90, 5)],
            &preferences,
        );
        let recorder = Recorder::default();
        // 23:30: muted.
        assert_eq!(deliver(&alerts, &preferences, 23 * 60 + 30, &recorder), 0);
        // 08:00: the engine already moved past the crossing; nothing is replayed.
        let later = collect(
            &mut engine,
            &[observation(Provider::Codex, 91, 600)],
            &preferences,
        );
        assert!(later.is_empty());
        assert_eq!(deliver(&later, &preferences, 8 * 60, &recorder), 0);
        assert!(recorder.shown.borrow().is_empty());
        // Outside the quiet period, alerts are shown.
        assert_eq!(deliver(&alerts, &preferences, 12 * 60, &recorder), 1);
    }

    #[test]
    fn a_failed_delivery_is_reported_and_never_retried() {
        let preferences = Preferences::default();
        let mut engine = Engine::default();
        collect(
            &mut engine,
            &[observation(Provider::Codex, 50, 0)],
            &preferences,
        );
        let alerts = collect(
            &mut engine,
            &[observation(Provider::Codex, 90, 5)],
            &preferences,
        );
        let failing = Recorder {
            fail: true,
            ..Recorder::default()
        };
        assert_eq!(deliver(&alerts, &preferences, 12 * 60, &failing), 0);
        // The crossing is not raised again on the next poll.
        assert!(
            collect(
                &mut engine,
                &[observation(Provider::Codex, 90, 10)],
                &preferences
            )
            .is_empty()
        );
    }

    #[test]
    fn a_restart_starts_from_a_baseline() {
        let preferences = Preferences::default();
        let mut engine = Engine::default();
        collect(
            &mut engine,
            &[observation(Provider::Codex, 50, 0)],
            &preferences,
        );
        assert_eq!(
            collect(
                &mut engine,
                &[observation(Provider::Codex, 90, 5)],
                &preferences
            )
            .len(),
            1
        );
        // A new process: the same 90% reading is not announced again.
        let mut restarted = Engine::default();
        assert!(
            collect(
                &mut restarted,
                &[observation(Provider::Codex, 90, 6)],
                &preferences
            )
            .is_empty()
        );
    }
}

//! Fictitious readings for tests and visual validation. Enabled with
//! `AURORA_DEMO=1`, or `AURORA_DEMO=<codex>,<claude>,<antigravity>,<grok>` to
//! choose the primary percentages. Never touches the network or provider
//! files. Every reading is marked `source: demo` and shown as a demonstration:
//! the Antigravity and Grok fixtures exist to validate the four-indicator layout,
//! not to stand in for a quota source those providers do not have.

use chrono::{DateTime, TimeDelta, Utc};

use super::adapter::{Fetched, Outcome, UsageAdapter, rfc3339};
use super::model::{LocalActivity, Origin, Provider, Quota, Source};
use super::session::Session;

pub struct DemoAdapter {
    provider: Provider,
    percent: u8,
}

impl DemoAdapter {
    pub fn new(provider: Provider, percent: u8) -> Self {
        Self {
            provider,
            percent: percent.min(100),
        }
    }

    /// Parses `AURORA_DEMO`. Returns `None` when demo mode is off.
    pub fn from_env(value: Option<&str>) -> Option<Vec<Self>> {
        let value = value?.trim();
        if value.is_empty() || value == "0" {
            return None;
        }
        let mut numbers = value
            .split(',')
            .map(|part| part.trim().parse::<u8>().ok().filter(|_| value != "1"));
        let defaults = [64, 37, 82, 21];
        Some(
            Provider::ALL
                .into_iter()
                .zip(defaults)
                .map(|(provider, default)| {
                    Self::new(provider, numbers.next().flatten().unwrap_or(default))
                })
                .collect(),
        )
    }
}

fn window(
    id: &str,
    label: &str,
    percent_used: u8,
    minutes: u32,
    resets_at: DateTime<Utc>,
) -> Quota {
    Quota::window(
        id,
        label,
        percent_used,
        Some(minutes),
        Some(rfc3339(resets_at)),
    )
}

impl UsageAdapter for DemoAdapter {
    fn provider(&self) -> Provider {
        self.provider
    }

    fn source(&self) -> Source {
        Source::Demo
    }

    fn fetch(&self, now: DateTime<Utc>) -> Outcome {
        let hour = TimeDelta::hours(1);
        let secondary = self.percent / 2;
        let quotas = match self.provider {
            Provider::Codex => vec![
                window(
                    "codex-primary",
                    "Janela de 5 h",
                    self.percent,
                    300,
                    now + hour * 3,
                ),
                window(
                    "codex-secondary",
                    "Semanal",
                    secondary,
                    10_080,
                    now + hour * 70,
                ),
            ],
            Provider::ClaudeCode => vec![
                window(
                    "fiveHour",
                    "Sessão de 5 h",
                    self.percent,
                    300,
                    now + hour * 2,
                ),
                window(
                    "sevenDay",
                    "Semanal · todos os modelos",
                    secondary,
                    10_080,
                    now + hour * 90,
                ),
                window(
                    "sevenDayOpus",
                    "Semanal · Opus",
                    secondary / 2,
                    10_080,
                    now + hour * 90,
                ),
            ],
            Provider::Antigravity => vec![
                window(
                    "agy-gemini-5h",
                    "Gemini · 5 h",
                    self.percent,
                    300,
                    now + hour * 4,
                ),
                window(
                    "agy-gemini-weekly",
                    "Gemini · Semanal",
                    secondary,
                    10_080,
                    now + hour * 160,
                ),
                window(
                    "agy-3p-5h",
                    "Claude e GPT · 5 h",
                    secondary / 2,
                    300,
                    now + hour * 3,
                ),
            ],
            Provider::Grok => vec![window(
                "grok-credits",
                "Créditos semanais",
                self.percent,
                10_080,
                now + hour * 100,
            )],
        };
        let session = Session::account(&["demo", &format!("{:?}", self.provider)], "demo");
        Outcome {
            session,
            account: Some("Conta de exemplo".into()),
            result: Some(Ok(Fetched {
                identity: session.identity(),
                account: Some("Conta de exemplo".into()),
                plan: None,
                origin: Origin::Fixture,
                quotas,
                observed_at: now,
            })),
            activity: (self.provider == Provider::ClaudeCode).then_some(LocalActivity {
                window_minutes: 300,
                tokens: 184_250,
                sessions: 2,
                partial: false,
            }),
            ..Outcome::default()
        }
    }
}

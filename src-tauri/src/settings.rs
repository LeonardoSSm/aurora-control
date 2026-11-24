//! Typed commands of the settings window. Granted to that window only (see
//! `capabilities/settings.json`); the overlays cannot call them, and the
//! settings window cannot call the overlay commands.

use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::autostart;
use crate::preferences::{LoadStatus, Preferences, Store, THRESHOLDS};
use crate::usage::catalog::{self, Capability, Integration};
use crate::usage::{Diagnostics, Issue, Origin, Provider, QuotaKind, Reading, Service, Usage};
use crate::windows::{self, monitors};

/// A metric the ring of a provider may show.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetricOption {
    pub id: String,
    pub label: String,
    pub duration_minutes: Option<u32>,
}

/// One provider as the settings window shows it: catalog data, connection
/// state and the metrics of its latest reading. No credential, no internal
/// identifier.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderView {
    id: Provider,
    name: &'static str,
    vendor: &'static str,
    integration: Integration,
    capabilities: &'static [Capability],
    reconnect: &'static str,
    /// `available`, `stale` or `unavailable`.
    status: &'static str,
    /// Where the last valid reading came from (API, official CLI, local log).
    origin: Option<Origin>,
    issue: Option<Issue>,
    account: Option<String>,
    plan: Option<String>,
    updated_at: Option<String>,
    checked_at: Option<String>,
    /// Windows of the latest reading that can be drawn by the ring.
    metrics: Vec<MetricOption>,
    metric_missing: bool,
}

fn provider_view(usage: Usage) -> ProviderView {
    let entry = catalog::entry(usage.provider);
    let mut metrics: Vec<MetricOption> = usage
        .window
        .iter()
        .map(|window| MetricOption {
            id: window.id.clone(),
            label: window.label.clone(),
            duration_minutes: window.duration_minutes,
        })
        .chain(
            usage
                .quotas
                .iter()
                .filter(|quota| quota.kind == QuotaKind::Window)
                .map(|quota| MetricOption {
                    id: quota.id.clone(),
                    label: quota.label.clone(),
                    duration_minutes: quota.duration_minutes,
                }),
        )
        .collect();
    // Shortest windows first: 5 h, daily, weekly, then the rest by label.
    metrics.sort_by(|a, b| {
        a.duration_minutes
            .unwrap_or(u32::MAX)
            .cmp(&b.duration_minutes.unwrap_or(u32::MAX))
            .then_with(|| a.label.cmp(&b.label))
    });
    ProviderView {
        id: usage.provider,
        name: &entry.name,
        vendor: &entry.vendor,
        integration: entry.integration,
        capabilities: &entry.capabilities,
        reconnect: &entry.reconnect,
        status: match usage.reading {
            Reading::Available { .. } => "available",
            Reading::Stale { .. } => "stale",
            Reading::Unavailable => "unavailable",
        },
        origin: usage.origin,
        issue: usage.issue,
        account: usage.account,
        plan: usage.plan,
        updated_at: usage.updated_at,
        checked_at: usage.checked_at,
        metrics,
        metric_missing: usage.metric_missing,
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsView {
    preferences: Preferences,
    status: LoadStatus,
    monitors: Vec<monitors::Monitor>,
    placement: windows::Placement,
    /// `AURORA_SIDE` decides the side for this session.
    side_override: bool,
    startup: autostart::Status,
    /// The registry disagrees with the saved start-up preference.
    startup_mismatch: bool,
    version: &'static str,
    demo: bool,
    /// Every catalog provider, in display order.
    providers: Vec<ProviderView>,
    /// Consumption levels the user may choose to be alerted at.
    thresholds: &'static [u8],
}

fn executable() -> Option<std::path::PathBuf> {
    std::env::current_exe().ok()
}

fn startup_status() -> autostart::Status {
    match executable() {
        Some(exe) => autostart::status(&autostart::Registry::run(), &exe),
        None => autostart::Status::Unknown,
    }
}

fn view(app: &AppHandle) -> SettingsView {
    let store = app.state::<Store>();
    let preferences = store.get();
    let (_, side_override) = crate::preferences::effective_side(&preferences);
    let startup = startup_status();
    let service = app.state::<Service>();
    SettingsView {
        status: store.status(),
        side_override,
        startup_mismatch: autostart::out_of_sync(startup, preferences.launch_at_startup),
        preferences,
        monitors: monitors::enumerate(),
        placement: windows::placement(app),
        startup,
        version: env!("CARGO_PKG_VERSION"),
        demo: service.is_demo(),
        providers: service.snapshot().into_iter().map(provider_view).collect(),
        thresholds: &THRESHOLDS,
    }
}

#[tauri::command]
pub fn get_settings(app: AppHandle) -> SettingsView {
    view(&app)
}

/// Validates and saves `preferences`, applies the start-up choice, then asks
/// the window controller to apply side, monitor, providers and metrics.
#[tauri::command]
pub fn set_preferences(app: AppHandle, preferences: Preferences) -> Result<SettingsView, String> {
    let preferences = preferences.validate()?;
    let store = app.state::<Store>();
    let previous = store.get();
    // A change of the choice, or a registry that disagrees with it (for
    // example after a restore that failed): saving applies it again.
    let startup_changes = preferences.launch_at_startup != previous.launch_at_startup
        || autostart::out_of_sync(startup_status(), preferences.launch_at_startup);
    if startup_changes {
        // The registry entry and the saved preference change together: a
        // failed save restores the entry as it was.
        let exe = executable().ok_or("Executável não localizado")?;
        let enabled = preferences.launch_at_startup;
        autostart::apply_then(&autostart::Registry::run(), &exe, enabled, || {
            store.set(preferences)
        })?;
    } else {
        store.set(preferences)?;
    }
    windows::preferences_changed(&app);
    Ok(view(&app))
}

/// The sign-in diagnosis of one provider. A disabled provider has no file
/// read, so it has no diagnosis.
#[tauri::command]
pub fn get_diagnostics(app: AppHandle, provider: Provider) -> Option<Diagnostics> {
    app.state::<Service>().diagnostics(provider)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::{ConsumptionWindow, Quota, Source};

    #[test]
    fn the_metric_options_list_every_window_and_never_credits() {
        let usage = Usage {
            provider: Provider::ClaudeCode,
            account: None,
            plan: None,
            source: Source::Live,
            origin: None,
            reading: Reading::Available { percent_used: 40 },
            window: Some(ConsumptionWindow {
                id: "sevenDay".into(),
                label: "Semanal".into(),
                duration_minutes: Some(10_080),
                resets_at: None,
                renewed: false,
            }),
            quotas: vec![
                Quota::window("fiveHour", "Sessão de 5 h", 10, Some(300), None),
                Quota {
                    kind: QuotaKind::Credits,
                    ..Quota::window("extra", "Uso extra", 5, None, None)
                },
            ],
            activity: None,
            updated_at: None,
            checked_at: None,
            issue: None,
            refreshing: false,
            selected_metric: None,
            metric_missing: false,
        };
        let view = provider_view(usage);
        let ids: Vec<&str> = view
            .metrics
            .iter()
            .map(|metric| metric.id.as_str())
            .collect();
        assert_eq!(ids, ["fiveHour", "sevenDay"]);
        assert_eq!(view.name, "Claude Code");
        assert_eq!(view.status, "available");
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["integration"], "live");
        assert_eq!(json["capabilities"][0], "subscriptionWindows");
    }
}

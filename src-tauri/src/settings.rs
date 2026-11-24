//! Typed commands of the settings window. Granted to that window only (see
//! `capabilities/settings.json`); the overlays cannot call them, and the
//! settings window cannot call the overlay commands.

use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::autostart;
use crate::preferences::{LoadStatus, Preferences, Store};
use crate::usage::{Diagnostics, Provider, Service};
use crate::windows::{self, monitors};

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
    SettingsView {
        status: store.status(),
        side_override,
        startup_mismatch: autostart::out_of_sync(startup, preferences.launch_at_startup),
        preferences,
        monitors: monitors::enumerate(),
        placement: windows::placement(app),
        startup,
        version: env!("CARGO_PKG_VERSION"),
        demo: app.state::<Service>().is_demo(),
    }
}

#[tauri::command]
pub fn get_settings(app: AppHandle) -> SettingsView {
    view(&app)
}

/// Validates and saves `preferences`, applies the start-up choice, then asks
/// the window controller to apply side, monitor and providers.
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

#[tauri::command]
pub fn get_claude_diagnostics(app: AppHandle) -> Option<Diagnostics> {
    app.state::<Service>().diagnostics(Provider::ClaudeCode)
}

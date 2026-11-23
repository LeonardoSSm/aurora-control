//! User preferences: local, versioned, validated, written atomically.
//!
//! Only stable product choices are stored: the rail side, the selected
//! monitor (a stable digest plus a display name), the visible providers and
//! the start-up choice. Usage data, retry deadlines and account identities are
//! never written here (audit I07, M04).
//!
//! Precedence: environment (`AURORA_SIDE`) > preferences > defaults. An
//! environment override is applied for the session and never persisted.
//!
//! Recovery: an unreadable, oversized or invalid file is kept aside as
//! `preferences.corrupt-<time>.json` and defaults are used. A file written by
//! a newer version is left untouched (read-only defaults) until the user
//! changes a setting.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::sync::lock;
use crate::usage::Provider;
use crate::windows::Side;

pub const VERSION: u32 = 1;
const FILE: &str = "preferences.json";
const MAX_FILE: u64 = 64 * 1024;
const MAX_NAME: usize = 96;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorChoice {
    /// Stable digest of the monitor device (`m-` and 16 hex digits).
    pub id: String,
    /// Shown when the monitor is disconnected.
    pub name: String,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Providers {
    pub codex: bool,
    pub claude_code: bool,
}

impl Providers {
    pub fn visible(&self) -> Vec<Provider> {
        let mut visible = Vec::new();
        if self.codex {
            visible.push(Provider::Codex);
        }
        if self.claude_code {
            visible.push(Provider::ClaudeCode);
        }
        visible
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preferences {
    pub version: u32,
    pub side: Side,
    /// `None`: the primary monitor.
    pub monitor: Option<MonitorChoice>,
    pub providers: Providers,
    pub launch_at_startup: bool,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            version: VERSION,
            side: Side::Right,
            monitor: None,
            providers: Providers {
                codex: true,
                claude_code: true,
            },
            launch_at_startup: false,
        }
    }
}

fn valid_monitor_id(id: &str) -> bool {
    id.len() == 18
        && id.starts_with("m-")
        && id[2..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

impl Preferences {
    /// Checks every field. Names are cleaned rather than rejected.
    pub fn validate(mut self) -> Result<Self, String> {
        if self.version != VERSION {
            return Err(format!("unsupported version {}", self.version));
        }
        if !self.providers.codex && !self.providers.claude_code {
            return Err("at least one provider must stay visible".into());
        }
        if let Some(monitor) = &mut self.monitor {
            if !valid_monitor_id(&monitor.id) {
                return Err("invalid monitor identifier".into());
            }
            let name: String = monitor
                .name
                .chars()
                .filter(|character| !character.is_control())
                .take(MAX_NAME)
                .collect();
            monitor.name = name.trim().to_string();
        }
        Ok(self)
    }
}

/// How the stored file was found at start-up.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LoadStatus {
    /// No file yet: defaults.
    Default,
    Loaded,
    /// The file was invalid; it was kept aside and defaults are used.
    Recovered,
    /// Written by a newer version: left untouched until a change is saved.
    NewerVersion,
    /// No configuration folder: preferences last for this session only.
    Unavailable,
}

/// Reads and validates a preferences document. Pure: no file access.
pub fn parse(text: &str) -> Result<Preferences, LoadStatus> {
    let value: Value = serde_json::from_str(text).map_err(|_| LoadStatus::Recovered)?;
    let version = value
        .get("version")
        .and_then(Value::as_u64)
        .ok_or(LoadStatus::Recovered)?;
    if version > u64::from(VERSION) {
        return Err(LoadStatus::NewerVersion);
    }
    let value = migrate(value, version).ok_or(LoadStatus::Recovered)?;
    serde_json::from_value::<Preferences>(value)
        .map_err(|_| LoadStatus::Recovered)?
        .validate()
        .map_err(|_| LoadStatus::Recovered)
}

/// Upgrades an older document to the current version. Version 1 is the first
/// persisted format, so there is nothing older to convert yet; the hook keeps
/// the upgrade path explicit for the next format change.
fn migrate(value: Value, version: u64) -> Option<Value> {
    match version {
        1 => Some(value),
        _ => None,
    }
}

pub struct Store {
    path: Option<PathBuf>,
    current: Mutex<Preferences>,
    status: Mutex<LoadStatus>,
}

impl Store {
    /// Loads `preferences.json` from `dir`, recovering from invalid files.
    pub fn load(dir: Option<PathBuf>) -> Self {
        let Some(dir) = dir else {
            return Self::memory(LoadStatus::Unavailable);
        };
        let path = dir.join(FILE);
        let (preferences, status) = match crate::usage::read_limited(&path, MAX_FILE) {
            Err(crate::usage::ReadError::Missing) => (Preferences::default(), LoadStatus::Default),
            Err(_) => (Preferences::default(), set_aside(&path)),
            Ok(text) => match parse(&text) {
                Ok(preferences) => (preferences, LoadStatus::Loaded),
                Err(LoadStatus::NewerVersion) => (Preferences::default(), LoadStatus::NewerVersion),
                Err(_) => (Preferences::default(), set_aside(&path)),
            },
        };
        Self {
            path: Some(path),
            current: Mutex::new(preferences),
            status: Mutex::new(status),
        }
    }

    fn memory(status: LoadStatus) -> Self {
        Self {
            path: None,
            current: Mutex::new(Preferences::default()),
            status: Mutex::new(status),
        }
    }

    pub fn get(&self) -> Preferences {
        lock(&self.current).clone()
    }

    pub fn status(&self) -> LoadStatus {
        *lock(&self.status)
    }

    /// Validates, writes atomically, then replaces the current value. On a
    /// write failure the previous value stays current.
    pub fn set(&self, preferences: Preferences) -> Result<Preferences, String> {
        let preferences = preferences.validate()?;
        if let Some(path) = &self.path {
            write_atomic(path, &preferences).map_err(|error| error.to_string())?;
            *lock(&self.status) = LoadStatus::Loaded;
        }
        *lock(&self.current) = preferences.clone();
        Ok(preferences)
    }
}

/// Keeps an invalid file for inspection and reports the recovery.
fn set_aside(path: &Path) -> LoadStatus {
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    let aside = path.with_file_name(format!("preferences.corrupt-{stamp}.json"));
    if fs::rename(path, &aside).is_err() {
        eprintln!("Aurora Control: could not set aside invalid preferences");
    }
    LoadStatus::Recovered
}

/// Writes to a temporary file in the same folder, flushes it to disk, then
/// renames it over the target. Readers see the old or the new file, never a
/// partial one.
fn write_atomic(path: &Path, preferences: &Preferences) -> std::io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| std::io::Error::other("preferences path has no folder"))?;
    fs::create_dir_all(dir)?;
    let temporary = dir.join(format!("{FILE}.{}.tmp", std::process::id()));
    let result = (|| {
        let mut file = fs::File::create(&temporary)?;
        let text = serde_json::to_vec_pretty(preferences).map_err(std::io::Error::other)?;
        file.write_all(&text)?;
        file.sync_all()?;
        drop(file);
        // On Windows `rename` replaces the target (MOVEFILE_REPLACE_EXISTING).
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// The side to use for this session.
pub fn effective_side(preferences: &Preferences) -> (Side, bool) {
    side_override(std::env::var("AURORA_SIDE").ok().as_deref())
        .map_or((preferences.side, false), |side| (side, true))
}

fn side_override(value: Option<&str>) -> Option<Side> {
    match value?.trim() {
        "left" => Some(Side::Left),
        "right" => Some(Side::Right),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::testing::TempDir;

    fn valid() -> Preferences {
        Preferences {
            side: Side::Left,
            monitor: Some(MonitorChoice {
                id: "m-0123456789abcdef".into(),
                name: "Dell U2720Q".into(),
            }),
            providers: Providers {
                codex: false,
                claude_code: true,
            },
            launch_at_startup: true,
            ..Preferences::default()
        }
    }

    #[test]
    fn saved_preferences_survive_a_restart() {
        let dir = TempDir::new("prefs-roundtrip");
        let store = Store::load(Some(dir.path().into()));
        assert_eq!(store.status(), LoadStatus::Default);
        assert_eq!(store.get(), Preferences::default());
        store.set(valid()).unwrap();
        let restarted = Store::load(Some(dir.path().into()));
        assert_eq!(restarted.status(), LoadStatus::Loaded);
        assert_eq!(restarted.get(), valid());
        // No temporary file is left behind.
        let names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names, [FILE]);
    }

    #[test]
    fn corrupted_files_are_set_aside_and_defaults_used() {
        for corrupted in [
            "{\"version\": 1, \"side\": \"lef",
            "[]",
            "{\"side\": \"left\"}",
            "{\"version\": 1, \"side\": \"up\", \"monitor\": null, \"providers\": {\"codex\": true, \"claudeCode\": true}, \"launchAtStartup\": false}",
            "{\"version\": 1, \"side\": \"left\", \"monitor\": null, \"providers\": {\"codex\": false, \"claudeCode\": false}, \"launchAtStartup\": false}",
            "{\"version\": 1, \"side\": \"left\", \"monitor\": {\"id\": \"..\\\\x\", \"name\": \"x\"}, \"providers\": {\"codex\": true, \"claudeCode\": true}, \"launchAtStartup\": false}",
        ] {
            let dir = TempDir::new("prefs-corrupt");
            dir.write(FILE, corrupted);
            let store = Store::load(Some(dir.path().into()));
            assert_eq!(store.status(), LoadStatus::Recovered, "{corrupted}");
            assert_eq!(store.get(), Preferences::default());
            let kept = fs::read_dir(dir.path())
                .unwrap()
                .filter_map(Result::ok)
                .any(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with("preferences.corrupt-")
                });
            assert!(kept, "the invalid file is kept for inspection");
            assert!(!dir.path().join(FILE).exists());
        }
        // Oversized and binary files too.
        let dir = TempDir::new("prefs-large");
        dir.write(FILE, &" ".repeat(70 * 1024));
        assert_eq!(
            Store::load(Some(dir.path().into())).status(),
            LoadStatus::Recovered
        );
    }

    #[test]
    fn a_newer_version_is_left_untouched_until_the_user_saves() {
        let dir = TempDir::new("prefs-newer");
        let future = r#"{"version": 2, "side": "left", "theme": "neon"}"#;
        dir.write(FILE, future);
        let store = Store::load(Some(dir.path().into()));
        assert_eq!(store.status(), LoadStatus::NewerVersion);
        assert_eq!(store.get(), Preferences::default());
        assert_eq!(fs::read_to_string(dir.path().join(FILE)).unwrap(), future);
    }

    #[test]
    fn unknown_fields_of_the_current_version_are_tolerated() {
        let text = r#"{"version": 1, "side": "left", "monitor": null,
            "providers": {"codex": true, "claudeCode": true}, "launchAtStartup": false,
            "addedLater": 5}"#;
        assert_eq!(parse(text).unwrap().side, Side::Left);
    }

    #[test]
    fn invalid_changes_are_refused_and_the_previous_value_kept() {
        let dir = TempDir::new("prefs-invalid");
        let store = Store::load(Some(dir.path().into()));
        store.set(valid()).unwrap();
        let none = Preferences {
            providers: Providers {
                codex: false,
                claude_code: false,
            },
            ..valid()
        };
        assert!(store.set(none).is_err());
        assert_eq!(store.get(), valid());
        let mut renamed = valid();
        renamed.monitor.as_mut().unwrap().name = format!("Tela\u{0}\n{}", "x".repeat(300));
        let saved = store.set(renamed).unwrap();
        assert_eq!(saved.monitor.unwrap().name.chars().count(), MAX_NAME);
    }

    #[test]
    fn the_environment_wins_for_the_session_only() {
        assert_eq!(side_override(Some("left")), Some(Side::Left));
        assert_eq!(side_override(Some(" right ")), Some(Side::Right));
        assert_eq!(side_override(Some("top")), None);
        assert_eq!(side_override(None), None);
    }

    #[test]
    fn without_a_configuration_folder_preferences_last_for_the_session() {
        let store = Store::load(None);
        assert_eq!(store.status(), LoadStatus::Unavailable);
        store.set(valid()).unwrap();
        assert_eq!(store.get(), valid());
    }
}

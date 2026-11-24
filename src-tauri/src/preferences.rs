//! User preferences: local, versioned, validated, written atomically.
//!
//! Only stable product choices are stored: the rail side and visibility, the
//! selected monitor (a stable digest plus a display name), each provider's
//! activation, ring metric and alerts, the notification choices and the
//! start-up choice. Usage data, retry deadlines, alert history and account
//! identities are never written here (audit I07, M04).
//!
//! Versions: 1 (0.2.0) had two provider switches; 2 (0.3.0) has one entry
//! per catalog provider and the notification settings; 3 (0.3.1) replaces
//! Gemini with Antigravity. [`migrate`] upgrades older documents one version
//! at a time, keeping every choice; providers added in a version start
//! disabled.
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
use serde_json::{Value, json};

use crate::sync::lock;
use crate::usage::{Provider, catalog};
use crate::windows::Side;

pub const VERSION: u32 = 3;
const FILE: &str = "preferences.json";
const MAX_FILE: u64 = 64 * 1024;
const MAX_NAME: usize = 96;
const MAX_METRIC: usize = 64;
/// Consumption levels a user may be alerted at, in percent.
pub const THRESHOLDS: [u8; 4] = [70, 85, 95, 100];
const MINUTES_PER_DAY: u16 = 24 * 60;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorChoice {
    /// Stable digest of the monitor device (`m-` and 16 hex digits).
    pub id: String,
    /// Shown when the monitor is disconnected.
    pub name: String,
}

/// The choices for one provider.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderChoice {
    /// Shown on the rail. A disabled provider reads no file, sends no
    /// request and raises no alert.
    pub enabled: bool,
    /// The quota drawn by the ring; `None` picks the most consumed window.
    #[serde(default)]
    pub metric: Option<String>,
    /// Alerts for this provider (when notifications are on).
    #[serde(default = "enabled")]
    pub alerts: bool,
}

fn enabled() -> bool {
    true
}

impl ProviderChoice {
    fn new(enabled: bool) -> Self {
        Self {
            enabled,
            metric: None,
            alerts: true,
        }
    }
}

/// One entry per catalog provider. A struct, not a map, so both languages
/// keep a typed contract.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Providers {
    pub codex: ProviderChoice,
    pub claude_code: ProviderChoice,
    pub antigravity: ProviderChoice,
    pub grok: ProviderChoice,
}

impl Default for Providers {
    fn default() -> Self {
        let choice = |provider| ProviderChoice::new(catalog::entry(provider).default_enabled);
        Self {
            codex: choice(Provider::Codex),
            claude_code: choice(Provider::ClaudeCode),
            antigravity: choice(Provider::Antigravity),
            grok: choice(Provider::Grok),
        }
    }
}

impl Providers {
    pub fn get(&self, provider: Provider) -> &ProviderChoice {
        match provider {
            Provider::Codex => &self.codex,
            Provider::ClaudeCode => &self.claude_code,
            Provider::Antigravity => &self.antigravity,
            Provider::Grok => &self.grok,
        }
    }

    /// Enabled providers, in catalog order: the rail's indicators.
    pub fn visible(&self) -> Vec<Provider> {
        Provider::ALL
            .into_iter()
            .filter(|provider| self.get(*provider).enabled)
            .collect()
    }

    /// The ring metric of every provider.
    pub fn metrics(&self) -> Vec<(Provider, Option<String>)> {
        Provider::ALL
            .into_iter()
            .map(|provider| (provider, self.get(provider).metric.clone()))
            .collect()
    }
}

/// A daily period without notifications, in local minutes since midnight.
/// `start > end` crosses midnight (for example 22:00 to 07:00).
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QuietHours {
    pub enabled: bool,
    pub start: u16,
    pub end: u16,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Notifications {
    /// Master switch.
    pub enabled: bool,
    /// Consumption levels to alert at, a sorted subset of [`THRESHOLDS`].
    pub thresholds: Vec<u8>,
    /// A renewal confirmed by a new reading.
    pub renewal: bool,
    /// Expired, missing or rejected sign-ins.
    pub auth: bool,
    /// A provider readable again after a sign-in problem.
    pub reconnect: bool,
    pub quiet: QuietHours,
}

impl Default for Notifications {
    fn default() -> Self {
        Self {
            enabled: true,
            thresholds: vec![85, 95, 100],
            renewal: true,
            auth: true,
            reconnect: false,
            quiet: QuietHours {
                enabled: false,
                start: 22 * 60,
                end: 7 * 60,
            },
        }
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
    /// The rail is shown. The tray can still hide or show it.
    pub rail_visible: bool,
    pub notifications: Notifications,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            version: VERSION,
            side: Side::Right,
            monitor: None,
            providers: Providers::default(),
            launch_at_startup: false,
            rail_visible: true,
            notifications: Notifications::default(),
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

/// Quota ids are short identifiers built by the adapters (`fiveHour`,
/// `codex-primary`, `grok-credits`).
fn valid_metric(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_METRIC
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:".contains(&byte))
}

impl Preferences {
    /// Checks every field. Names are cleaned rather than rejected. Zero
    /// visible providers is valid: the rail hides, and the tray and the
    /// settings stay available.
    pub fn validate(mut self) -> Result<Self, String> {
        if self.version != VERSION {
            return Err(format!("unsupported version {}", self.version));
        }
        let invalid_metric = Provider::ALL.into_iter().any(|provider| {
            self.providers
                .get(provider)
                .metric
                .as_deref()
                .is_some_and(|metric| !valid_metric(metric))
        });
        if invalid_metric {
            return Err("invalid metric identifier".into());
        }
        let notifications = &mut self.notifications;
        if notifications
            .thresholds
            .iter()
            .any(|threshold| !THRESHOLDS.contains(threshold))
        {
            return Err("invalid alert threshold".into());
        }
        notifications.thresholds.sort_unstable();
        notifications.thresholds.dedup();
        let quiet = notifications.quiet;
        if quiet.start >= MINUTES_PER_DAY || quiet.end >= MINUTES_PER_DAY {
            return Err("invalid quiet hours".into());
        }
        if quiet.enabled && quiet.start == quiet.end {
            return Err("quiet hours must not be empty".into());
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
    /// A file of an older version was read and upgraded in memory. The file
    /// itself is rewritten in the current format at the next change.
    Migrated,
    /// The file was invalid; it was kept aside and defaults are used.
    Recovered,
    /// Written by a newer version: left untouched until a change is saved.
    NewerVersion,
    /// No configuration folder: preferences last for this session only.
    Unavailable,
}

/// Reads and validates a preferences document. Pure: no file access.
/// Also returns whether the document was upgraded from an older version.
pub fn parse(text: &str) -> Result<(Preferences, bool), LoadStatus> {
    let value: Value = serde_json::from_str(text).map_err(|_| LoadStatus::Recovered)?;
    let version = value
        .get("version")
        .and_then(Value::as_u64)
        .ok_or(LoadStatus::Recovered)?;
    if version > u64::from(VERSION) {
        return Err(LoadStatus::NewerVersion);
    }
    let value = migrate(value, version).ok_or(LoadStatus::Recovered)?;
    let preferences = serde_json::from_value::<Preferences>(value)
        .map_err(|_| LoadStatus::Recovered)?
        .validate()
        .map_err(|_| LoadStatus::Recovered)?;
    Ok((preferences, version < u64::from(VERSION)))
}

/// Upgrades an older document, one version at a time, to the current one.
fn migrate(mut value: Value, version: u64) -> Option<Value> {
    match version {
        1 => {
            // 0.2.0: `providers: { codex: bool, claudeCode: bool }`. Each
            // provider keeps its state; everything else is kept as written.
            let object = value.as_object_mut()?;
            let old = object.get("providers")?;
            let codex = old.get("codex")?.as_bool()?;
            let claude = old.get("claudeCode")?.as_bool()?;
            let choice =
                |enabled: bool| json!({ "enabled": enabled, "metric": null, "alerts": true });
            object.insert(
                "providers".into(),
                json!({
                    "codex": choice(codex),
                    "claudeCode": choice(claude),
                    // New in 0.3.0: off until the user turns them on (the
                    // Gemini slot becomes Antigravity in version 3).
                    "gemini": choice(false),
                    "grok": choice(false),
                }),
            );
            object.insert("railVisible".into(), json!(true));
            object.insert(
                "notifications".into(),
                serde_json::to_value(Notifications::default()).ok()?,
            );
            object.insert("version".into(), json!(2));
            migrate(value, 2)
        }
        2 => {
            // 0.3.0 → 0.3.1: Google ended Gemini CLI sign-in for Gemini Code
            // Assist for individuals and moved it to Antigravity. The Gemini
            // slot becomes Antigravity, which runs the official CLI: it starts
            // disabled whatever Gemini was, and its metric choice is reset
            // (the quota ids differ). Its alert switch is kept.
            let object = value.as_object_mut()?;
            let providers = object.get_mut("providers")?.as_object_mut()?;
            let gemini = providers.remove("gemini")?;
            let alerts = gemini
                .get("alerts")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            providers.insert(
                "antigravity".into(),
                json!({ "enabled": false, "metric": null, "alerts": alerts }),
            );
            object.insert("version".into(), json!(3));
            migrate(value, 3)
        }
        3 => Some(value),
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
                Ok((preferences, false)) => (preferences, LoadStatus::Loaded),
                // The older file stays as it is until the user saves a
                // change; a downgrade to 0.2.0 can still read it meanwhile.
                Ok((preferences, true)) => (preferences, LoadStatus::Migrated),
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

/// Debug builds only: `AURORA_CONFIG_DIR` replaces the preferences folder, so
/// validation scripts never read or write the user's real preferences. Tauri
/// resolves the folder through the Windows known-folder API, which ignores
/// the `APPDATA` variable. Release builds ignore this variable.
pub fn debug_config_dir() -> Option<PathBuf> {
    if !cfg!(debug_assertions) {
        return None;
    }
    std::env::var_os("AURORA_CONFIG_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
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
        let mut preferences = Preferences {
            side: Side::Left,
            monitor: Some(MonitorChoice {
                id: "m-0123456789abcdef".into(),
                name: "Dell U2720Q".into(),
            }),
            launch_at_startup: true,
            ..Preferences::default()
        };
        preferences.providers.codex.enabled = false;
        preferences.providers.grok.enabled = true;
        preferences.providers.claude_code.metric = Some("fiveHour".into());
        preferences.notifications.thresholds = vec![70, 100];
        preferences
    }

    /// A document exactly as 0.2.0 wrote it.
    const V1: &str = r#"{
  "version": 1,
  "side": "left",
  "monitor": { "id": "m-0123456789abcdef", "name": "Dell U2720Q" },
  "providers": { "codex": false, "claudeCode": true },
  "launchAtStartup": true
}"#;

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
    fn defaults_keep_the_new_providers_off_and_alerts_on() {
        let defaults = Preferences::default();
        assert_eq!(
            defaults.providers.visible(),
            [Provider::Codex, Provider::ClaudeCode]
        );
        assert!(defaults.notifications.enabled && defaults.rail_visible);
        assert_eq!(defaults.notifications.thresholds, [85, 95, 100]);
        assert!(!defaults.notifications.quiet.enabled);
    }

    #[test]
    fn a_version_1_file_is_migrated_keeping_every_choice() {
        let (preferences, migrated) = parse(V1).unwrap();
        assert!(migrated);
        assert_eq!(preferences.version, VERSION);
        assert_eq!(preferences.side, Side::Left);
        assert_eq!(
            preferences.monitor,
            Some(MonitorChoice {
                id: "m-0123456789abcdef".into(),
                name: "Dell U2720Q".into(),
            })
        );
        assert!(preferences.launch_at_startup);
        assert!(!preferences.providers.codex.enabled);
        assert!(preferences.providers.claude_code.enabled);
        // New providers stay off; nothing is read from them.
        assert!(!preferences.providers.antigravity.enabled);
        assert!(!preferences.providers.grok.enabled);
        assert_eq!(preferences.providers.visible(), [Provider::ClaudeCode]);
        assert!(
            Provider::ALL.into_iter().all(|provider| preferences
                .providers
                .get(provider)
                .metric
                .is_none())
        );
        assert!(preferences.rail_visible);
        assert_eq!(preferences.notifications, Notifications::default());

        // The other v1 combination.
        let both = V1.replace("\"codex\": false", "\"codex\": true");
        assert_eq!(
            parse(&both).unwrap().0.providers.visible(),
            [Provider::Codex, Provider::ClaudeCode]
        );
    }

    /// A document exactly as 0.3.0 wrote it, with Gemini enabled and its
    /// alerts off.
    const V2: &str = r#"{
  "version": 2, "side": "right", "monitor": null,
  "providers": {
    "codex": { "enabled": true, "metric": "codex-primary", "alerts": true },
    "claudeCode": { "enabled": false, "metric": null, "alerts": false },
    "gemini": { "enabled": true, "metric": "gemini-pro", "alerts": false },
    "grok": { "enabled": true, "metric": null, "alerts": true }
  },
  "launchAtStartup": true, "railVisible": false,
  "notifications": { "enabled": true, "thresholds": [70, 100], "renewal": false, "auth": true,
    "reconnect": true, "quiet": { "enabled": true, "start": 1380, "end": 360 } }
}"#;

    #[test]
    fn a_version_2_file_becomes_antigravity_disabled_and_keeps_the_rest() {
        let (preferences, migrated) = parse(V2).unwrap();
        assert!(migrated);
        assert_eq!(preferences.version, 3);
        // Antigravity runs the official CLI: never enabled by a migration.
        assert!(!preferences.providers.antigravity.enabled);
        assert_eq!(preferences.providers.antigravity.metric, None);
        assert!(
            !preferences.providers.antigravity.alerts,
            "alert switch kept"
        );
        // Everything else as written.
        assert_eq!(
            preferences.providers.codex.metric.as_deref(),
            Some("codex-primary")
        );
        assert!(!preferences.providers.claude_code.enabled);
        assert!(!preferences.providers.claude_code.alerts);
        assert!(preferences.providers.grok.enabled);
        assert!(preferences.launch_at_startup && !preferences.rail_visible);
        assert_eq!(preferences.notifications.thresholds, [70, 100]);
        assert!(!preferences.notifications.renewal && preferences.notifications.reconnect);
        assert_eq!(preferences.notifications.quiet.start, 1380);
        // A v2 document without the gemini entry is invalid, not guessed.
        let broken = V2.replace("\"gemini\"", "\"other\"");
        assert_eq!(parse(&broken).unwrap_err(), LoadStatus::Recovered);
    }

    #[test]
    fn a_migrated_file_is_rewritten_only_when_the_user_saves() {
        let dir = TempDir::new("prefs-migrate");
        dir.write(FILE, V1);
        let store = Store::load(Some(dir.path().into()));
        assert_eq!(store.status(), LoadStatus::Migrated);
        assert_eq!(store.get().side, Side::Left);
        // Still the 0.2.0 file on disk: a downgrade keeps working.
        assert_eq!(fs::read_to_string(dir.path().join(FILE)).unwrap(), V1);

        let mut changed = store.get();
        changed.providers.antigravity.enabled = true;
        store.set(changed.clone()).unwrap();
        let restarted = Store::load(Some(dir.path().into()));
        assert_eq!(restarted.status(), LoadStatus::Loaded);
        assert_eq!(restarted.get(), changed);
        let written: Value =
            serde_json::from_str(&fs::read_to_string(dir.path().join(FILE)).unwrap()).unwrap();
        assert_eq!(written["version"], 3);
        assert_eq!(written["providers"]["antigravity"]["enabled"], true);
        assert_eq!(written["monitor"]["id"], "m-0123456789abcdef");
    }

    #[test]
    fn corrupted_files_are_set_aside_and_defaults_used() {
        let v2 = serde_json::to_string(&valid()).unwrap();
        for corrupted in [
            "{\"version\": 1, \"side\": \"lef".to_string(),
            "[]".into(),
            "{\"side\": \"left\"}".into(),
            V1.replace("\"left\"", "\"up\""),
            V1.replace("m-0123456789abcdef", "..\\\\x"),
            V1.replace("\"codex\": false, ", ""),
            v2.replace("\"fiveHour\"", "\"five hour!\""),
            v2.replace("[70,100]", "[50]"),
            v2.replace("\"start\":1320", "\"start\":1440"),
            v2.replace("\"grok\":", "\"copilot\":"),
            r#"{"version": 0}"#.into(),
        ] {
            let dir = TempDir::new("prefs-corrupt");
            dir.write(FILE, &corrupted);
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
        let future = r#"{"version": 9, "side": "left", "theme": "neon"}"#;
        dir.write(FILE, future);
        let store = Store::load(Some(dir.path().into()));
        assert_eq!(store.status(), LoadStatus::NewerVersion);
        assert_eq!(store.get(), Preferences::default());
        assert_eq!(fs::read_to_string(dir.path().join(FILE)).unwrap(), future);
    }

    #[test]
    fn unknown_top_level_fields_of_the_current_version_are_tolerated() {
        let mut value = serde_json::to_value(valid()).unwrap();
        value["addedLater"] = json!(5);
        assert_eq!(parse(&value.to_string()).unwrap().0, valid());
        // And in a v1 document.
        let v1 = V1.replace("\"version\": 1,", "\"version\": 1, \"addedLater\": 5,");
        assert_eq!(parse(&v1).unwrap().0.side, Side::Left);
    }

    #[test]
    fn zero_visible_providers_is_a_valid_choice() {
        let mut none = valid();
        for provider in Provider::ALL {
            match provider {
                Provider::Codex => none.providers.codex.enabled = false,
                Provider::ClaudeCode => none.providers.claude_code.enabled = false,
                Provider::Antigravity => none.providers.antigravity.enabled = false,
                Provider::Grok => none.providers.grok.enabled = false,
            }
        }
        let saved = none.validate().unwrap();
        assert!(saved.providers.visible().is_empty());
    }

    #[test]
    fn invalid_changes_are_refused_and_the_previous_value_kept() {
        let dir = TempDir::new("prefs-invalid");
        let store = Store::load(Some(dir.path().into()));
        store.set(valid()).unwrap();
        let mut threshold = valid();
        threshold.notifications.thresholds = vec![42];
        assert!(store.set(threshold).is_err());
        let mut metric = valid();
        metric.providers.grok.metric = Some("x".repeat(65));
        assert!(store.set(metric).is_err());
        let mut quiet = valid();
        quiet.notifications.quiet = QuietHours {
            enabled: true,
            start: 600,
            end: 600,
        };
        assert!(store.set(quiet).is_err());
        assert_eq!(store.get(), valid());

        let mut renamed = valid();
        renamed.monitor.as_mut().unwrap().name = format!("Tela\u{0}\n{}", "x".repeat(300));
        let saved = store.set(renamed).unwrap();
        assert_eq!(saved.monitor.unwrap().name.chars().count(), MAX_NAME);
        // Thresholds are normalized, not refused, when only out of order.
        let mut unordered = valid();
        unordered.notifications.thresholds = vec![100, 70, 100];
        assert_eq!(
            store.set(unordered).unwrap().notifications.thresholds,
            [70, 100]
        );
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

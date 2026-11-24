//! The safety breaker of a CLI-based source, persisted across restarts.
//!
//! When the official CLI ran a model turn, or answered a read-only command in
//! a way that cannot prove it did not, the source is suspended. The
//! suspension is written to `<config>/safety-<provider>.json` so a restart
//! (or a crash loop, or an automatic start with Windows) cannot spend the
//! user's quota again. Only an explicit, informed reactivation by the user in
//! the settings window removes it.
//!
//! Fail-safe: a file that exists but cannot be read or understood counts as
//! tripped. Without a configuration folder the breaker lasts for the session.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::sync::lock;

const VERSION: u32 = 1;
const MAX_FILE: u64 = 4 * 1024;

/// Why the source was suspended.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Cause {
    /// The CLI reported a model turn or token usage.
    ModelTurn,
    /// The CLI exited successfully with an answer that does not prove the
    /// absence of a model turn (format change, a prompt answered as text).
    Unconfirmed,
    /// The breaker file exists but could not be read: kept suspended.
    Unknown,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Trip {
    pub cause: Cause,
    /// When it tripped, if known.
    pub at: Option<DateTime<Utc>>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    version: u32,
    cause: Cause,
    tripped_at: String,
}

pub struct Breaker {
    path: Option<PathBuf>,
    state: Mutex<Option<Trip>>,
}

impl Breaker {
    /// The breaker of `provider` (a catalog id), restored from `dir`.
    pub fn load(dir: Option<&Path>, provider: &str) -> Self {
        let path = dir.map(|dir| dir.join(format!("safety-{provider}.json")));
        let state = path.as_deref().and_then(read);
        if state.is_some() {
            eprintln!("Aurora Control: {provider} stays suspended by its safety breaker");
        }
        Self {
            path,
            state: Mutex::new(state),
        }
    }

    /// A breaker that lives for the session only.
    #[cfg(test)]
    pub fn memory() -> Self {
        Self {
            path: None,
            state: Mutex::new(None),
        }
    }

    pub fn tripped(&self) -> Option<Trip> {
        lock(&self.state).clone()
    }

    /// Suspends the source. The first cause is kept: a later one adds
    /// nothing the user needs to know.
    pub fn trip(&self, cause: Cause, now: DateTime<Utc>) {
        let mut state = lock(&self.state);
        if state.is_some() {
            return;
        }
        *state = Some(Trip {
            cause,
            at: Some(now),
        });
        if let Some(path) = &self.path
            && let Err(error) = write(path, cause, now)
        {
            // Still suspended for this session.
            eprintln!("Aurora Control: the safety breaker could not be saved: {error}");
        }
    }

    /// The user re-enabled the source after reading why it was suspended.
    /// Fails, keeping the suspension, if the file cannot be removed.
    pub fn reset(&self) -> Result<(), String> {
        let mut state = lock(&self.state);
        if let Some(path) = &self.path {
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err("O bloqueio de segurança não pôde ser removido.".into()),
            }
        }
        *state = None;
        Ok(())
    }
}

fn read(path: &Path) -> Option<Trip> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        // It may exist: stay suspended.
        Err(_) => {
            return Some(Trip {
                cause: Cause::Unknown,
                at: None,
            });
        }
    };
    let unknown = Trip {
        cause: Cause::Unknown,
        at: None,
    };
    if metadata.len() > MAX_FILE {
        return Some(unknown);
    }
    let Ok(text) = fs::read_to_string(path) else {
        return Some(unknown);
    };
    match serde_json::from_str::<Record>(&text) {
        Ok(record) => Some(Trip {
            cause: record.cause,
            at: DateTime::parse_from_rfc3339(&record.tripped_at)
                .ok()
                .map(|time| time.with_timezone(&Utc)),
        }),
        Err(_) => Some(unknown),
    }
}

fn write(path: &Path, cause: Cause, now: DateTime<Utc>) -> std::io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| std::io::Error::other("breaker path has no folder"))?;
    fs::create_dir_all(dir)?;
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    let result = (|| {
        let record = Record {
            version: VERSION,
            cause,
            tripped_at: super::adapter::rfc3339(now),
        };
        let mut file = fs::File::create(&temporary)?;
        file.write_all(&serde_json::to_vec_pretty(&record).map_err(std::io::Error::other)?)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::local::testing::TempDir;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-11-24T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn a_trip_survives_a_restart_until_the_user_resets_it() {
        let dir = TempDir::new("breaker");
        let breaker = Breaker::load(Some(dir.path()), "antigravity");
        assert_eq!(breaker.tripped(), None);
        breaker.trip(Cause::ModelTurn, now());
        // A later cause does not overwrite the first.
        breaker.trip(Cause::Unconfirmed, now());

        let restarted = Breaker::load(Some(dir.path()), "antigravity");
        assert_eq!(
            restarted.tripped(),
            Some(Trip {
                cause: Cause::ModelTurn,
                at: Some(now())
            })
        );
        restarted.reset().unwrap();
        assert_eq!(restarted.tripped(), None);
        assert_eq!(
            Breaker::load(Some(dir.path()), "antigravity").tripped(),
            None
        );
        // Resetting an already removed file is not an error.
        breaker.reset().unwrap();
        assert!(!dir.path().join("safety-antigravity.json").exists());
    }

    #[test]
    fn an_unreadable_or_foreign_file_keeps_the_source_suspended() {
        let dir = TempDir::new("breaker-bad");
        for content in ["", "{", "[]", r#"{"version":1,"cause":"other"}"#] {
            dir.write("safety-antigravity.json", content);
            assert_eq!(
                Breaker::load(Some(dir.path()), "antigravity")
                    .tripped()
                    .map(|trip| trip.cause),
                Some(Cause::Unknown),
                "{content:?}"
            );
        }
        dir.write(
            "safety-antigravity.json",
            &"x".repeat(MAX_FILE as usize + 1),
        );
        assert!(
            Breaker::load(Some(dir.path()), "antigravity")
                .tripped()
                .is_some()
        );
    }

    #[test]
    fn without_a_folder_the_breaker_lasts_for_the_session() {
        let breaker = Breaker::load(None, "antigravity");
        breaker.trip(Cause::Unconfirmed, now());
        assert_eq!(
            breaker.tripped().map(|trip| trip.cause),
            Some(Cause::Unconfirmed)
        );
        breaker.reset().unwrap();
        assert_eq!(breaker.tripped(), None);
        assert_eq!(Breaker::memory().tripped(), None);
    }
}

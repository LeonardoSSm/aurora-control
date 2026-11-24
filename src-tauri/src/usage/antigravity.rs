//! Google Antigravity adapter: quota from the official Antigravity CLI.
//!
//! Source: the CLI's headless (print) mode, which answers read-only slash
//! commands itself. `agy -p /usage --output-format json` returns the account's
//! quota groups without starting an agent turn, spending quota or creating a
//! conversation (Antigravity CLI changelog 1.1.11; headless mode is documented
//! for scripts at https://antigravity.google/docs/cli/headless). The CLI uses
//! its own cached sign-in: this app never reads, copies or sends the
//! credential, and never contacts Google itself.
//!
//! The Antigravity Additional Terms (section 6) forbid third-party software
//! that accesses the service, for example with the Antigravity OAuth. Here the
//! official client does the access, through the mode the docs offer for
//! scripts; see `docs/cycle-03-1/provider-terms-review.md` and
//! `docs/cycle-04/security-audit.md` for the reasoning and the residual risk.
//! The provider stays disabled until the user enables it.
//!
//! The subprocess is controlled by this module and [`super::process`]:
//! - fixed executable (`%LOCALAPPDATA%\agy\bin\agy.exe`, where the official
//!   installer puts it) and fixed arguments; nothing comes from the WebView;
//! - no console window, no stdin, a neutral working directory (home);
//! - timeout, output limit, a Job Object that takes every descendant down
//!   with the CLI, cancellation when the provider is disabled (an attempt
//!   cancelled once is void, even if the provider is enabled again at once),
//!   and a single run at a time;
//! - `--settings '{"disableSlashCommands":false}'` (session-only, never saved)
//!   so a user setting cannot turn `/usage` into a prompt;
//! - a breaker ([`super::safety`]): if the CLI reports a model turn or token
//!   usage, or answers successfully without proving it did not, the source is
//!   suspended, across restarts, until the user re-enables it knowingly.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::adapter::{
    FetchError, Fetched, Outcome, UsageAdapter, percent, rfc3339, schema_anomaly,
};
use super::local::{env_dir, home_dir};
use super::model::{BlockReason, DiagnosticRow, DiagnosticState, Origin, Provider, Quota};
use super::process::{self, CliOutput, Control, Limits, RunError};
use super::safety::{Breaker, Cause};
use super::session::Session;
use super::time;

const LIMITS: Limits = Limits {
    timeout: Duration::from_secs(45),
    max_output: 256 * 1024,
};
const MAX_BUCKETS: usize = 32;
const ARGS: [&str; 6] = [
    "-p",
    "/usage",
    "--output-format",
    "json",
    "--settings",
    r#"{"disableSlashCommands":false}"#,
];

/// Runs the CLI. A trait so tests replay outputs without starting processes.
pub trait Runner: Send + Sync + 'static {
    fn run(&self, control: &Control) -> Result<CliOutput, RunError>;
    fn installed(&self) -> bool;
}

/// The production runner: the official `agy.exe`.
pub struct Agy {
    exe: Option<PathBuf>,
    cwd: Option<PathBuf>,
}

impl Agy {
    pub fn detect() -> Self {
        Self {
            exe: env_dir("LOCALAPPDATA").map(|dir| dir.join("agy").join("bin").join("agy.exe")),
            cwd: home_dir(),
        }
    }
}

impl Runner for Agy {
    fn installed(&self) -> bool {
        self.exe.as_ref().is_some_and(|exe| exe.is_file())
    }

    fn run(&self, control: &Control) -> Result<CliOutput, RunError> {
        let exe = self
            .exe
            .as_ref()
            .filter(|exe| exe.is_file())
            .ok_or(RunError::Missing)?;
        let mut command = Command::new(exe);
        command
            .args(ARGS)
            // The CLI would otherwise add a log file to its own folder on every
            // run; one file of ours, overwritten each time, keeps it clean.
            .arg("--log-file")
            .arg(std::env::temp_dir().join("aurora-control-agy.log"));
        if let Some(cwd) = &self.cwd {
            command.current_dir(cwd);
        }
        process::run(command, control, LIMITS)
    }
}

pub struct Antigravity<R: Runner> {
    runner: R,
    /// Disables cancel the run in progress and void pending attempts.
    control: Control,
    /// One CLI run at a time, whatever calls the adapter.
    running: Mutex<()>,
    /// Persistent suspension after a model turn or an unconfirmed answer.
    breaker: Breaker,
}

impl Antigravity<Agy> {
    /// `config` is the app's configuration folder, where the breaker lives.
    pub fn detect(config: Option<&Path>) -> Self {
        Self::new(Agy::detect(), Breaker::load(config, "antigravity"))
    }
}

impl<R: Runner> Antigravity<R> {
    pub fn new(runner: R, breaker: Breaker) -> Self {
        Self {
            runner,
            control: Control::new(),
            running: Mutex::new(()),
            breaker,
        }
    }
}

fn window_minutes(window: &str) -> Option<u32> {
    match window {
        "5h" => Some(300),
        "weekly" => Some(7 * 24 * 60),
        "daily" => Some(24 * 60),
        _ => None,
    }
}

fn window_label(window: &str) -> &'static str {
    match window {
        "5h" => "5 h",
        "weekly" => "Semanal",
        "daily" => "Diária",
        _ => "Janela",
    }
}

/// A short group label: "Gemini Models" → "Gemini", "Claude and GPT models"
/// → "Claude e GPT".
fn group_label(name: &str) -> String {
    let trimmed = name
        .trim()
        .trim_end_matches(" Models")
        .trim_end_matches(" models")
        .replace(" and ", " e ");
    super::adapter::display_label(&trimmed).unwrap_or_else(|| "Modelos".into())
}

/// A quota id safe for preferences (`[A-Za-z0-9._:-]`, at most 64 bytes).
fn quota_id(raw: &str) -> Option<String> {
    let id: String = raw
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || "-_.:".contains(*character))
        .take(48)
        .collect();
    (!id.is_empty()).then(|| format!("agy-{id}"))
}

/// Why a run must not be trusted as a quota report.
#[derive(Debug, PartialEq)]
pub enum Rejected {
    /// The CLI ran an agent turn or spent tokens.
    Turn,
    /// The answer does not prove that no turn ran: no JSON envelope, no turn
    /// or token counters, or a command other than `/usage`.
    Unconfirmed,
    /// A `/usage` report with zero turns and tokens, but no usable data.
    Error(FetchError),
}

/// A counter of the envelope, if it is present and numeric.
fn counter(value: &Value) -> Option<u64> {
    value.as_u64()
}

/// Normalizes the CLI's `/usage` JSON. Only `command.name == "usage"` with
/// explicit zero turns and zero tokens is a report. A turn, or an answer
/// that cannot prove there was none, is rejected so the breaker trips.
pub fn parse_usage(stdout: &str, now: DateTime<Utc>) -> Result<Vec<Quota>, Rejected> {
    let malformed = |what: &str| Rejected::Error(FetchError::Malformed(what.into()));
    let Ok(value) = serde_json::from_str::<Value>(stdout.trim()) else {
        return Err(Rejected::Unconfirmed);
    };
    let turns = counter(&value["num_turns"]);
    let tokens = counter(&value["usage"]["total_tokens"]);
    if turns.is_some_and(|turns| turns > 0) || tokens.is_some_and(|tokens| tokens > 0) {
        return Err(Rejected::Turn);
    }
    if turns.is_none() || tokens.is_none() || value["command"]["name"].as_str() != Some("usage") {
        return Err(Rejected::Unconfirmed);
    }
    let groups = value["command"]["data"]["groups"]
        .as_array()
        .ok_or_else(|| malformed("agy groups"))?;
    let mut quotas = Vec::new();
    for group in groups {
        let name = group_label(group["name"].as_str().unwrap_or_default());
        for bucket in group["buckets"].as_array().into_iter().flatten() {
            if quotas.len() >= MAX_BUCKETS {
                break;
            }
            if bucket["disabled"].as_bool() == Some(true) {
                continue;
            }
            let Some(id) = bucket["id"].as_str().and_then(quota_id) else {
                continue;
            };
            let window = bucket["window"].as_str().unwrap_or_default();
            // `remaining_fraction` is 0..1. A fraction outside it is not a
            // reading; the field path is reported, never the value.
            let Some(remaining) = bucket["remaining_fraction"].as_f64() else {
                schema_anomaly(Provider::Antigravity, "bucket.remaining_fraction missing");
                continue;
            };
            let Some(used) = (remaining.is_finite() && (0.0..=1.0).contains(&remaining))
                .then(|| percent((1.0 - remaining) * 100.0))
                .flatten()
            else {
                schema_anomaly(
                    Provider::Antigravity,
                    "bucket.remaining_fraction outside 0-1",
                );
                continue;
            };
            let reset = bucket["reset_time"]
                .as_str()
                .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
                .map(|time| time.with_timezone(&Utc));
            let minutes = window_minutes(window);
            quotas.push(Quota {
                // Only windows whose meaning is known may drive the ring
                // automatically; others are listed.
                eligible: minutes.is_some(),
                ..Quota::window(
                    id,
                    format!("{name} · {}", window_label(window)),
                    used,
                    minutes,
                    time::plausible_reset(reset, now).map(rfc3339),
                )
            });
        }
    }
    if !quotas.iter().any(|quota| quota.eligible) {
        return Err(malformed("agy: no known window"));
    }
    Ok(quotas)
}

/// Maps a failed run (non-zero exit) to what it means for the user.
fn failed_run(output: &CliOutput) -> FetchError {
    let text = output.stderr.to_ascii_lowercase();
    match output.code {
        // `/usage` with a GEMINI_API_KEY: an API key has no account quota.
        Some(2) => FetchError::Unsupported,
        _ if text.contains("sign in")
            || text.contains("log in")
            || text.contains("login")
            || text.contains("not signed")
            || text.contains("authenticat") =>
        {
            FetchError::AuthMissing
        }
        _ if text.contains("eligib") => FetchError::Unsupported,
        _ => FetchError::Malformed(format!("agy exit {:?}", output.code)),
    }
}

fn cause_text(cause: Cause) -> &'static str {
    match cause {
        Cause::ModelTurn => "o CLI executou uma rodada do modelo em vez de só informar a cota",
        Cause::Unconfirmed => {
            "o CLI respondeu sem provar que não executou uma rodada do modelo (formato inesperado)"
        }
        Cause::Unknown => "o registro do bloqueio de segurança não pôde ser lido",
    }
}

impl<R: Runner> Antigravity<R> {
    fn query(&self, now: DateTime<Utc>) -> Result<Fetched, FetchError> {
        if self.breaker.tripped().is_some() {
            return Err(FetchError::SourceBlocked(BlockReason::CliSafety));
        }
        let Ok(_running) = self.running.try_lock() else {
            // Another run is in progress: never two CLIs at once.
            return Err(FetchError::Timeout);
        };
        let output = match self.runner.run(&self.control) {
            Ok(output) => output,
            Err(RunError::Missing) => return Err(FetchError::AuthMissing),
            Err(RunError::Timeout | RunError::Cancelled) => return Err(FetchError::Timeout),
            Err(RunError::Spawn) => return Err(FetchError::Network),
            Err(RunError::TooLarge) => return Err(FetchError::Malformed("agy output size".into())),
        };
        if output.code != Some(0) {
            // A failed run may still carry the envelope: a turn is a turn.
            if let Err(Rejected::Turn) = parse_usage(&output.stdout, now) {
                return Err(self.trip(Cause::ModelTurn, now));
            }
            return Err(failed_run(&output));
        }
        match parse_usage(&output.stdout, now) {
            Ok(quotas) => Ok(Fetched {
                // The CLI does not say which account it read: the reading is
                // shown while it is the latest, and never feeds account-bound
                // state such as alerts.
                identity: None,
                account: None,
                plan: None,
                origin: Origin::Cli,
                quotas,
                observed_at: now,
            }),
            Err(Rejected::Turn) => Err(self.trip(Cause::ModelTurn, now)),
            Err(Rejected::Unconfirmed) => Err(self.trip(Cause::Unconfirmed, now)),
            Err(Rejected::Error(error)) => Err(error),
        }
    }

    fn trip(&self, cause: Cause, now: DateTime<Utc>) -> FetchError {
        self.breaker.trip(cause, now);
        eprintln!("Aurora Control: Antigravity queries suspended ({cause:?})");
        FetchError::SourceBlocked(BlockReason::CliSafety)
    }
}

impl<R: Runner> UsageAdapter for Antigravity<R> {
    fn provider(&self) -> Provider {
        Provider::Antigravity
    }

    /// No account can be verified locally: the CLI keeps its sign-in in the
    /// system credential store and reports no account in its quota output.
    fn session(&self, _now: DateTime<Utc>) -> Option<Session> {
        Some(if self.runner.installed() {
            Session::Unverified
        } else {
            Session::SignedOut
        })
    }

    /// Never clears a cancellation: a provider disabled between the worker's
    /// check and this call must not start the CLI. Only `resume`, called
    /// when the provider is enabled again, clears it.
    fn fetch(&self, now: DateTime<Utc>) -> Outcome {
        if !self.runner.installed() {
            return Outcome::signed_out(FetchError::AuthMissing);
        }
        Outcome {
            session: Session::Unverified,
            result: Some(self.query(now)),
            ..Outcome::default()
        }
    }

    fn cancel(&self) {
        self.control.cancel();
    }

    fn resume(&self) {
        self.control.resume();
    }

    fn reset_safety(&self) -> Result<bool, String> {
        if self.breaker.tripped().is_none() {
            return Ok(false);
        }
        self.breaker.reset()?;
        eprintln!("Aurora Control: Antigravity queries re-enabled by the user");
        Ok(true)
    }

    fn diagnostics(&self, _now: DateTime<Utc>) -> Option<Vec<DiagnosticRow>> {
        let mut rows = vec![if self.runner.installed() {
            DiagnosticRow::new(
                "Antigravity CLI",
                DiagnosticState::Ok,
                "Instalado (agy.exe do instalador oficial)",
            )
        } else {
            DiagnosticRow::new(
                "Antigravity CLI",
                DiagnosticState::Info,
                "Não encontrado em %LOCALAPPDATA%\\agy\\bin. Instale o Antigravity CLI.",
            )
        }];
        rows.push(match self.breaker.tripped() {
            Some(trip) => DiagnosticRow {
                at: trip.at.map(rfc3339),
                ..DiagnosticRow::new(
                    "Fonte de cotas",
                    DiagnosticState::Error,
                    format!("Suspensa por segurança: {}", cause_text(trip.cause)),
                )
            },
            None => DiagnosticRow::new(
                "Fonte de cotas",
                DiagnosticState::Ok,
                "agy -p /usage (CLI oficial, sem rodada do modelo)",
            ),
        });
        rows.push(DiagnosticRow::new(
            "Conta",
            DiagnosticState::Info,
            "Não verificável localmente; a cota é a da sessão do CLI e não gera alertas de consumo",
        ));
        Some(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Instant;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-11-24T05:20:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    /// The envelope the CLI 1.5.16 printed for `-p /usage --output-format json`
    /// on 2026-11-24, with descriptions shortened.
    const REAL: &str = r#"{
      "conversation_id": "", "status": "SUCCESS", "response": "…", "duration_seconds": 0, "num_turns": 0,
      "usage": {"input_tokens": 0, "output_tokens": 0, "thinking_tokens": 0, "cache_read_tokens": 0, "total_tokens": 0},
      "command": {"name": "usage", "data": {"description": "…", "groups": [
        {"name": "Gemini Models", "description": "…", "buckets": [
          {"id": "gemini-weekly", "name": "Weekly Limit Remaining", "description": "…", "window": "weekly", "remaining_fraction": 0.8414430618286133, "reset_time": "2026-12-01T04:20:44Z"},
          {"id": "gemini-5h", "name": "Five Hour Limit Remaining", "description": "…", "window": "5h", "remaining_fraction": 0.4157721996307373, "reset_time": "2026-11-24T09:20:44Z"}]},
        {"name": "Claude and GPT models", "description": "…", "buckets": [
          {"id": "3p-weekly", "name": "Weekly Limit Remaining", "window": "weekly", "remaining_fraction": 1, "reset_time": "2026-12-01T05:18:16Z"},
          {"id": "3p-5h", "name": "Five Hour Limit Remaining", "window": "5h", "remaining_fraction": 1, "reset_time": "2026-11-24T10:18:16Z"}]}]}}
    }"#;

    /// Replays outputs and counts runs.
    struct Script {
        replies: Mutex<Vec<Result<CliOutput, RunError>>>,
        runs: Mutex<u32>,
        installed: bool,
    }

    impl Script {
        fn new(replies: Vec<Result<CliOutput, RunError>>) -> Self {
            Self {
                replies: Mutex::new(replies),
                runs: Mutex::new(0),
                installed: true,
            }
        }

        fn ok(stdout: &str) -> Result<CliOutput, RunError> {
            Ok(CliOutput {
                code: Some(0),
                stdout: stdout.into(),
                stderr: String::new(),
            })
        }
    }

    impl Runner for Script {
        fn run(&self, control: &Control) -> Result<CliOutput, RunError> {
            // Like the production runner: nothing starts once cancelled.
            if control.is_cancelled() {
                return Err(RunError::Cancelled);
            }
            *self.runs.lock().unwrap() += 1;
            self.replies.lock().unwrap().remove(0)
        }

        fn installed(&self) -> bool {
            self.installed
        }
    }

    fn adapter(replies: Vec<Result<CliOutput, RunError>>) -> Antigravity<Script> {
        Antigravity::new(Script::new(replies), Breaker::memory())
    }

    fn result(outcome: &Outcome) -> &Result<Fetched, FetchError> {
        outcome.result.as_ref().unwrap()
    }

    const BLOCKED: FetchError = FetchError::SourceBlocked(BlockReason::CliSafety);

    #[test]
    fn the_real_report_becomes_four_windows_in_two_groups() {
        let quotas = parse_usage(REAL, now()).unwrap();
        let view: Vec<(&str, &str, u8, Option<u32>)> = quotas
            .iter()
            .map(|quota| {
                (
                    quota.id.as_str(),
                    quota.label.as_str(),
                    quota.percent_used,
                    quota.duration_minutes,
                )
            })
            .collect();
        assert_eq!(
            view,
            [
                ("agy-gemini-weekly", "Gemini · Semanal", 16, Some(10_080)),
                ("agy-gemini-5h", "Gemini · 5 h", 58, Some(300)),
                ("agy-3p-weekly", "Claude e GPT · Semanal", 0, Some(10_080)),
                ("agy-3p-5h", "Claude e GPT · 5 h", 0, Some(300)),
            ]
        );
        assert_eq!(quotas[1].resets_at.as_deref(), Some("2026-11-24T09:20:44Z"));
        assert!(quotas.iter().all(|quota| quota.eligible));
        // The most consumed window drives the ring automatically.
        assert_eq!(
            crate::usage::adapter::primary_index(&quotas).map(|index| quotas[index].id.as_str()),
            Some("agy-gemini-5h")
        );
    }

    #[test]
    fn a_successful_run_is_a_cli_reading_without_an_account() {
        let adapter = adapter(vec![Script::ok(REAL)]);
        let outcome = adapter.fetch(now());
        let fetched = result(&outcome).as_ref().unwrap();
        assert_eq!(fetched.origin, Origin::Cli);
        assert_eq!(fetched.identity, None);
        assert_eq!(fetched.quotas.len(), 4);
        assert_eq!(outcome.session, Session::Unverified);
    }

    #[test]
    fn a_model_turn_trips_the_breaker_and_stops_all_later_runs() {
        let turn = REAL
            .replace("\"num_turns\": 0", "\"num_turns\": 1")
            .replace("\"total_tokens\": 0", "\"total_tokens\": 22428");
        assert_eq!(parse_usage(&turn, now()), Err(Rejected::Turn));
        // Tokens alone are enough.
        let tokens = REAL.replace("\"total_tokens\": 0", "\"total_tokens\": 5");
        assert_eq!(parse_usage(&tokens, now()), Err(Rejected::Turn));

        let adapter = adapter(vec![Script::ok(&turn), Script::ok(REAL)]);
        assert_eq!(result(&adapter.fetch(now())), &Err(BLOCKED));
        // Suspended: the CLI is not started again.
        assert_eq!(result(&adapter.fetch(now())), &Err(BLOCKED));
        assert_eq!(*adapter.runner.runs.lock().unwrap(), 1);
        assert_eq!(
            adapter.breaker.tripped().map(|trip| trip.cause),
            Some(Cause::ModelTurn)
        );
    }

    #[test]
    fn the_breaker_survives_a_restart_and_only_an_explicit_reset_lifts_it() {
        let dir = crate::usage::local::testing::TempDir::new("agy-breaker");
        let turn = REAL.replace("\"num_turns\": 0", "\"num_turns\": 1");
        let first = Antigravity::new(
            Script::new(vec![Script::ok(&turn)]),
            Breaker::load(Some(dir.path()), "antigravity"),
        );
        assert_eq!(result(&first.fetch(now())), &Err(BLOCKED));

        // A restart (or an automatic start with Windows): still suspended,
        // and the CLI never runs.
        let restarted = Antigravity::new(
            Script::new(vec![Script::ok(REAL)]),
            Breaker::load(Some(dir.path()), "antigravity"),
        );
        assert_eq!(result(&restarted.fetch(now())), &Err(BLOCKED));
        assert_eq!(*restarted.runner.runs.lock().unwrap(), 0);
        let rows = format!("{:?}", restarted.diagnostics(now()).unwrap());
        assert!(rows.contains("rodada do modelo"), "{rows}");

        // Disabling and enabling the provider is not a reset.
        restarted.cancel();
        restarted.resume();
        assert_eq!(result(&restarted.fetch(now())), &Err(BLOCKED));

        // The user re-enables it knowingly.
        assert_eq!(restarted.reset_safety(), Ok(true));
        assert!(result(&restarted.fetch(now())).is_ok());
        assert_eq!(restarted.reset_safety(), Ok(false));
    }

    #[test]
    fn an_answer_that_cannot_prove_the_absence_of_a_turn_trips_the_breaker() {
        // What a mangled argument produced on 2026-11-24: a normal prompt.
        let answered = r#"{"conversation_id": "x", "status": "SUCCESS", "response": "It looks like…", "num_turns": 1,
            "usage": {"total_tokens": 22428}}"#;
        assert_eq!(parse_usage(answered, now()), Err(Rejected::Turn));
        for unconfirmed in [
            // Plain text: a prompt answered without the JSON envelope.
            "Here is a summary of your usage…",
            "",
            "{}",
            // Counters missing or not numbers.
            r#"{"command": {"name": "usage", "data": {"groups": []}}}"#,
            r#"{"num_turns": 0, "command": {"name": "usage", "data": {}}}"#,
            r#"{"num_turns": "0", "usage": {"total_tokens": 0}, "command": {"name": "usage"}}"#,
            // Another command than the one asked for.
            r#"{"num_turns": 0, "usage": {"total_tokens": 0}, "command": {"name": "model", "data": {}}}"#,
        ] {
            assert_eq!(
                parse_usage(unconfirmed, now()),
                Err(Rejected::Unconfirmed),
                "{unconfirmed}"
            );
        }
        let adapter = adapter(vec![Script::ok("Sure! Your quota is…"), Script::ok(REAL)]);
        assert_eq!(result(&adapter.fetch(now())), &Err(BLOCKED));
        assert_eq!(result(&adapter.fetch(now())), &Err(BLOCKED));
        assert_eq!(*adapter.runner.runs.lock().unwrap(), 1);
        assert_eq!(
            adapter.breaker.tripped().map(|trip| trip.cause),
            Some(Cause::Unconfirmed)
        );
    }

    #[test]
    fn a_confirmed_report_without_usable_data_is_malformed_not_a_trip() {
        let envelope = |data: &str| {
            format!(
                r#"{{"num_turns": 0, "usage": {{"total_tokens": 0}}, "command": {{"name": "usage", "data": {data}}}}}"#
            )
        };
        for data in [
            "{}",
            r#"{"groups": []}"#,
            r#"{"groups": [{"name": "G", "buckets": [{"id": "g-w", "window": "weekly", "remaining_fraction": 1.7}]}]}"#,
            r#"{"groups": [{"name": "G", "buckets": [{"id": "g-x", "window": "monthly-ish", "remaining_fraction": 0.5}]}]}"#,
        ] {
            assert!(
                matches!(
                    parse_usage(&envelope(data), now()),
                    Err(Rejected::Error(FetchError::Malformed(_)))
                ),
                "{data}"
            );
        }
        // Invalid buckets are skipped; valid ones remain.
        let mixed = envelope(
            r#"{"groups": [{"name": "G", "buckets": [
            {"id": "bad", "window": "5h", "remaining_fraction": -0.2},
            {"id": "off", "window": "5h", "remaining_fraction": 0.1, "disabled": true},
            {"id": "ok", "window": "5h", "remaining_fraction": 0.25, "reset_time": "2099-01-01T00:00:00Z"}]}]}"#,
        );
        let quotas = parse_usage(&mixed, now()).unwrap();
        assert_eq!(quotas.len(), 1);
        assert_eq!(
            (quotas[0].id.as_str(), quotas[0].percent_used),
            ("agy-ok", 75)
        );
        // An implausible renewal is dropped, the reading kept.
        assert_eq!(quotas[0].resets_at, None);
        let adapter = adapter(vec![Script::ok(&envelope("{}"))]);
        assert!(matches!(
            result(&adapter.fetch(now())),
            Err(FetchError::Malformed(_))
        ));
        assert_eq!(adapter.breaker.tripped(), None);
    }

    #[test]
    fn failures_map_to_user_states() {
        let failed = |code: i32, stderr: &str| {
            Err::<CliOutput, RunError>(RunError::Spawn).or(Ok(CliOutput {
                code: Some(code),
                stdout: String::new(),
                stderr: stderr.into(),
            }))
        };
        let cases = [
            (
                failed(2, "API keys have no account quota"),
                FetchError::Unsupported,
            ),
            (
                failed(1, "Please sign in by running agy"),
                FetchError::AuthMissing,
            ),
            (
                failed(1, "boom"),
                FetchError::Malformed("agy exit Some(1)".into()),
            ),
            (Err(RunError::Timeout), FetchError::Timeout),
            (Err(RunError::Cancelled), FetchError::Timeout),
            (Err(RunError::Spawn), FetchError::Network),
            (
                Err(RunError::TooLarge),
                FetchError::Malformed("agy output size".into()),
            ),
            (Err(RunError::Missing), FetchError::AuthMissing),
        ];
        for (reply, expected) in cases {
            let adapter = adapter(vec![reply]);
            assert_eq!(result(&adapter.fetch(now())), &Err(expected));
            // A failed exit is not evidence of a turn.
            assert_eq!(adapter.breaker.tripped(), None);
        }
    }

    #[test]
    fn without_the_cli_the_session_is_signed_out_and_nothing_runs() {
        let mut script = Script::new(vec![]);
        script.installed = false;
        let adapter = Antigravity::new(script, Breaker::memory());
        assert_eq!(adapter.session(now()), Some(Session::SignedOut));
        let outcome = adapter.fetch(now());
        assert_eq!(result(&outcome), &Err(FetchError::AuthMissing));
        assert_eq!(outcome.session, Session::SignedOut);
        assert_eq!(*adapter.runner.runs.lock().unwrap(), 0);
    }

    #[test]
    fn the_arguments_are_fixed_and_keep_slash_commands_on() {
        assert_eq!(ARGS[..4], ["-p", "/usage", "--output-format", "json"]);
        let settings: Value = serde_json::from_str(ARGS[5]).unwrap();
        assert_eq!(
            settings,
            serde_json::json!({ "disableSlashCommands": false })
        );
    }

    #[test]
    fn cancel_and_single_run_are_honored() {
        // A runner that waits for the cancel flag, like a real slow CLI.
        struct Slow;
        impl Runner for Slow {
            fn run(&self, control: &Control) -> Result<CliOutput, RunError> {
                let started = Instant::now();
                while !control.is_cancelled() {
                    if started.elapsed() > Duration::from_secs(5) {
                        return Err(RunError::Timeout);
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(RunError::Cancelled)
            }
            fn installed(&self) -> bool {
                true
            }
        }
        let adapter = std::sync::Arc::new(Antigravity::new(Slow, Breaker::memory()));
        let worker = {
            let adapter = std::sync::Arc::clone(&adapter);
            std::thread::spawn(move || adapter.fetch(now()))
        };
        std::thread::sleep(Duration::from_millis(50));
        // A second caller while the first runs: refused, nothing started.
        assert_eq!(adapter.query(now()), Err(FetchError::Timeout));
        adapter.cancel();
        let outcome = worker.join().unwrap();
        assert_eq!(result(&outcome), &Err(FetchError::Timeout));
    }

    #[test]
    fn a_cancel_before_fetch_is_kept_until_the_provider_is_enabled_again() {
        let adapter = adapter(vec![Script::ok(REAL)]);
        // Disabled after the worker's check, before it called fetch.
        adapter.cancel();
        assert_eq!(result(&adapter.fetch(now())), &Err(FetchError::Timeout));
        assert_eq!(
            *adapter.runner.runs.lock().unwrap(),
            0,
            "the CLI never started"
        );
        // A second fetch while still disabled: still nothing.
        adapter.fetch(now());
        assert_eq!(*adapter.runner.runs.lock().unwrap(), 0);
        // Enabled again: the next fetch runs.
        adapter.resume();
        assert!(result(&adapter.fetch(now())).is_ok());
        assert_eq!(*adapter.runner.runs.lock().unwrap(), 1);
    }

    #[test]
    fn rapid_disable_and_enable_cycles_never_run_two_clis() {
        // Counts concurrent runs of a runner that honors the control.
        struct Counting {
            active: AtomicBool,
            overlap: AtomicBool,
            runs: Mutex<u32>,
        }
        impl Runner for Counting {
            fn run(&self, control: &Control) -> Result<CliOutput, RunError> {
                if control.is_cancelled() {
                    return Err(RunError::Cancelled);
                }
                if self.active.swap(true, Ordering::SeqCst) {
                    self.overlap.store(true, Ordering::SeqCst);
                }
                *self.runs.lock().unwrap() += 1;
                std::thread::sleep(Duration::from_millis(2));
                self.active.store(false, Ordering::SeqCst);
                Ok(CliOutput {
                    code: Some(0),
                    stdout: REAL.into(),
                    stderr: String::new(),
                })
            }
            fn installed(&self) -> bool {
                true
            }
        }
        let adapter = std::sync::Arc::new(Antigravity::new(
            Counting {
                active: AtomicBool::new(false),
                overlap: AtomicBool::new(false),
                runs: Mutex::new(0),
            },
            Breaker::memory(),
        ));
        let workers: Vec<_> = (0..4)
            .map(|_| {
                let adapter = std::sync::Arc::clone(&adapter);
                std::thread::spawn(move || {
                    for _ in 0..50 {
                        adapter.fetch(now());
                    }
                })
            })
            .collect();
        for _ in 0..200 {
            adapter.cancel();
            adapter.resume();
        }
        for worker in workers {
            worker.join().unwrap();
        }
        assert!(!adapter.runner.overlap.load(Ordering::SeqCst));
        // Ends enabled: a fetch runs.
        assert!(result(&adapter.fetch(now())).is_ok());
    }

    /// Runs the real `agy.exe` through the production runner. Opt-in only:
    /// `cargo test --locked live_antigravity -- --ignored --nocapture`.
    /// Prints only the normalized quotas (no account data exists in them).
    #[test]
    #[ignore = "runs the installed Antigravity CLI"]
    fn live_antigravity_cli() {
        let adapter = Antigravity::new(Agy::detect(), Breaker::memory());
        assert!(adapter.runner.installed(), "agy.exe not installed");
        let started = Instant::now();
        let outcome = adapter.fetch(Utc::now());
        let elapsed = started.elapsed();
        let fetched = result(&outcome).as_ref().expect("a quota report");
        for quota in &fetched.quotas {
            println!(
                "{} | {} | {}% used | {:?} min | resets {:?}",
                quota.id, quota.label, quota.percent_used, quota.duration_minutes, quota.resets_at
            );
        }
        println!("origin {:?}, {} ms", fetched.origin, elapsed.as_millis());
        assert_eq!(adapter.breaker.tripped(), None, "no model turn");
    }
}

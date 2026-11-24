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
//! scripts; see `docs/cycle-03-1/provider-terms-review.md` for the reasoning
//! and the residual risk. The provider stays disabled until the user enables
//! it.
//!
//! The subprocess is controlled by this module only:
//! - fixed executable (`%LOCALAPPDATA%\agy\bin\agy.exe`, where the official
//!   installer puts it) and fixed arguments; nothing comes from the WebView;
//! - no console window, no stdin, a neutral working directory (home);
//! - timeout, output limit, cancellation when the provider is disabled, and a
//!   single run at a time;
//! - `--settings '{"disableSlashCommands":false}'` (session-only, never saved)
//!   so a user setting cannot turn `/usage` into a prompt;
//! - a breaker: if the CLI ever reports a model turn or token usage, the
//!   source is suspended until the app restarts, so a CLI change can never
//!   keep spending the user's quota.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::adapter::{
    FetchError, Fetched, Outcome, UsageAdapter, percent, rfc3339, schema_anomaly,
};
use super::local::{env_dir, home_dir};
use super::model::{BlockReason, DiagnosticRow, DiagnosticState, Origin, Provider, Quota};
use super::session::Session;
use super::time;

const TIMEOUT: Duration = Duration::from_secs(45);
const MAX_OUTPUT: usize = 256 * 1024;
const MAX_BUCKETS: usize = 32;
const ARGS: [&str; 6] = [
    "-p",
    "/usage",
    "--output-format",
    "json",
    "--settings",
    r#"{"disableSlashCommands":false}"#,
];

/// What a finished CLI run produced.
#[derive(Clone, Debug, PartialEq)]
pub struct CliOutput {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunError {
    /// The executable is not installed.
    Missing,
    /// It could not be started.
    Spawn,
    Timeout,
    /// More output than any quota report needs.
    TooLarge,
    /// The provider was disabled while the run was in progress.
    Cancelled,
}

/// Runs the CLI. A trait so tests replay outputs without starting processes.
pub trait Runner: Send + Sync + 'static {
    fn run(&self, cancel: &AtomicBool) -> Result<CliOutput, RunError>;
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

/// Reads at most `limit` bytes from `reader`, flagging anything beyond.
fn drain(mut reader: impl Read, limit: usize) -> (Vec<u8>, bool) {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 8192];
    let mut over = false;
    loop {
        match reader.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                if buffer.len() + read > limit {
                    over = true;
                    // Keep reading (and discarding) so the child never blocks
                    // on a full pipe before it is killed.
                    continue;
                }
                buffer.extend_from_slice(&chunk[..read]);
            }
        }
    }
    (buffer, over)
}

fn stop(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

impl Runner for Agy {
    fn installed(&self) -> bool {
        self.exe.as_ref().is_some_and(|exe| exe.is_file())
    }

    fn run(&self, cancel: &AtomicBool) -> Result<CliOutput, RunError> {
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
            .arg(std::env::temp_dir().join("aurora-control-agy.log"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(cwd) = &self.cwd {
            command.current_dir(cwd);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW: no console flashes on the user's screen.
            command.creation_flags(0x0800_0000);
        }
        let mut child = command.spawn().map_err(|_| RunError::Spawn)?;
        let stdout = child.stdout.take().ok_or(RunError::Spawn)?;
        let stderr = child.stderr.take().ok_or(RunError::Spawn)?;
        let out = std::thread::spawn(move || drain(stdout, MAX_OUTPUT));
        let err = std::thread::spawn(move || drain(stderr, MAX_OUTPUT));

        let started = Instant::now();
        let status = loop {
            if cancel.load(Ordering::SeqCst) {
                stop(&mut child);
                return Err(RunError::Cancelled);
            }
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if started.elapsed() >= TIMEOUT => {
                    stop(&mut child);
                    return Err(RunError::Timeout);
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(_) => {
                    stop(&mut child);
                    return Err(RunError::Spawn);
                }
            }
        };
        let (stdout, over_out) = out.join().unwrap_or_default();
        let (stderr, _) = err.join().unwrap_or_default();
        if over_out {
            return Err(RunError::TooLarge);
        }
        Ok(CliOutput {
            code: status.code(),
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
        })
    }
}

pub struct Antigravity<R: Runner> {
    runner: R,
    /// Set while the provider is disabled: a run in progress is killed.
    cancel: AtomicBool,
    /// One CLI run at a time, whatever calls the adapter.
    running: Mutex<()>,
    /// The CLI once reported a model turn or token usage: suspended until the
    /// app restarts.
    tripped: AtomicBool,
}

impl Antigravity<Agy> {
    pub fn detect() -> Self {
        Self::new(Agy::detect())
    }
}

impl<R: Runner> Antigravity<R> {
    pub fn new(runner: R) -> Self {
        Self {
            runner,
            cancel: AtomicBool::new(false),
            running: Mutex::new(()),
            tripped: AtomicBool::new(false),
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
    Error(FetchError),
}

/// Normalizes the CLI's `/usage` JSON. Only `command.name == "usage"` with no
/// turn and no token usage is a report; anything else is rejected, and a turn
/// trips the breaker.
pub fn parse_usage(stdout: &str, now: DateTime<Utc>) -> Result<Vec<Quota>, Rejected> {
    let malformed = |what: &str| Rejected::Error(FetchError::Malformed(what.into()));
    let value: Value = serde_json::from_str(stdout.trim()).map_err(|_| malformed("agy json"))?;
    let turns = value["num_turns"].as_u64().unwrap_or(0);
    let tokens = value["usage"]["total_tokens"].as_u64().unwrap_or(0);
    if turns > 0 || tokens > 0 {
        return Err(Rejected::Turn);
    }
    if value["command"]["name"].as_str() != Some("usage") {
        return Err(malformed("agy command"));
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

impl<R: Runner> Antigravity<R> {
    fn query(&self, now: DateTime<Utc>) -> Result<Fetched, FetchError> {
        if self.tripped.load(Ordering::SeqCst) {
            return Err(FetchError::SourceBlocked(BlockReason::CliSafety));
        }
        let Ok(_running) = self.running.try_lock() else {
            // Another run is in progress: never two CLIs at once.
            return Err(FetchError::Timeout);
        };
        let output = match self.runner.run(&self.cancel) {
            Ok(output) => output,
            Err(RunError::Missing) => return Err(FetchError::AuthMissing),
            Err(RunError::Timeout | RunError::Cancelled) => return Err(FetchError::Timeout),
            Err(RunError::Spawn) => return Err(FetchError::Network),
            Err(RunError::TooLarge) => return Err(FetchError::Malformed("agy output size".into())),
        };
        if output.code != Some(0) {
            // A failed run may still carry the envelope: a turn is a turn.
            if let Err(Rejected::Turn) = parse_usage(&output.stdout, now) {
                self.trip();
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
            Err(Rejected::Turn) => {
                self.trip();
                Err(FetchError::SourceBlocked(BlockReason::CliSafety))
            }
            Err(Rejected::Error(error)) => Err(error),
        }
    }

    fn trip(&self) {
        self.tripped.store(true, Ordering::SeqCst);
        eprintln!("Aurora Control: the Antigravity CLI ran a model turn; queries suspended");
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

    fn fetch(&self, now: DateTime<Utc>) -> Outcome {
        self.cancel.store(false, Ordering::SeqCst);
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
        self.cancel.store(true, Ordering::SeqCst);
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
        rows.push(DiagnosticRow::new(
            "Fonte de cotas",
            if self.tripped.load(Ordering::SeqCst) {
                DiagnosticState::Error
            } else {
                DiagnosticState::Ok
            },
            if self.tripped.load(Ordering::SeqCst) {
                "Suspensa: o CLI executou uma rodada do modelo"
            } else {
                "agy -p /usage (CLI oficial, sem rodada do modelo)"
            },
        ));
        rows.push(DiagnosticRow::new(
            "Conta",
            DiagnosticState::Info,
            "Não verificável localmente; a cota é a da sessão do CLI",
        ));
        Some(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        fn run(&self, _cancel: &AtomicBool) -> Result<CliOutput, RunError> {
            *self.runs.lock().unwrap() += 1;
            self.replies.lock().unwrap().remove(0)
        }

        fn installed(&self) -> bool {
            self.installed
        }
    }

    fn result(outcome: &Outcome) -> &Result<Fetched, FetchError> {
        outcome.result.as_ref().unwrap()
    }

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
        let adapter = Antigravity::new(Script::new(vec![Script::ok(REAL)]));
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

        let adapter = Antigravity::new(Script::new(vec![Script::ok(&turn), Script::ok(REAL)]));
        assert_eq!(
            result(&adapter.fetch(now())),
            &Err(FetchError::SourceBlocked(BlockReason::CliSafety))
        );
        // Suspended: the CLI is not started again.
        assert_eq!(
            result(&adapter.fetch(now())),
            &Err(FetchError::SourceBlocked(BlockReason::CliSafety))
        );
        assert_eq!(*adapter.runner.runs.lock().unwrap(), 1);
    }

    #[test]
    fn a_prompt_answered_by_the_model_is_never_a_report() {
        // What a mangled argument produced on 2026-11-24: a normal prompt.
        let answered = r#"{"conversation_id": "x", "status": "SUCCESS", "response": "It looks like…", "num_turns": 1,
            "usage": {"total_tokens": 22428}}"#;
        assert_eq!(parse_usage(answered, now()), Err(Rejected::Turn));
    }

    #[test]
    fn incomplete_malformed_and_out_of_range_reports_are_rejected() {
        for body in [
            "not json",
            "{}",
            r#"{"num_turns": 0, "command": {"name": "model", "data": {}}}"#,
            r#"{"num_turns": 0, "command": {"name": "usage", "data": {}}}"#,
            r#"{"num_turns": 0, "command": {"name": "usage", "data": {"groups": []}}}"#,
            r#"{"num_turns": 0, "command": {"name": "usage", "data": {"groups": [{"name": "G", "buckets": [
                {"id": "g-w", "window": "weekly", "remaining_fraction": 1.7}]}]}}}"#,
            r#"{"num_turns": 0, "command": {"name": "usage", "data": {"groups": [{"name": "G", "buckets": [
                {"id": "g-x", "window": "monthly-ish", "remaining_fraction": 0.5}]}]}}}"#,
        ] {
            assert!(
                matches!(
                    parse_usage(body, now()),
                    Err(Rejected::Error(FetchError::Malformed(_)))
                ),
                "{body}"
            );
        }
        // Invalid buckets are skipped; valid ones remain.
        let mixed = r#"{"num_turns": 0, "command": {"name": "usage", "data": {"groups": [{"name": "G", "buckets": [
            {"id": "bad", "window": "5h", "remaining_fraction": -0.2},
            {"id": "off", "window": "5h", "remaining_fraction": 0.1, "disabled": true},
            {"id": "ok", "window": "5h", "remaining_fraction": 0.25, "reset_time": "2099-01-01T00:00:00Z"}]}]}}}"#;
        let quotas = parse_usage(mixed, now()).unwrap();
        assert_eq!(quotas.len(), 1);
        assert_eq!(
            (quotas[0].id.as_str(), quotas[0].percent_used),
            ("agy-ok", 75)
        );
        // An implausible renewal is dropped, the reading kept.
        assert_eq!(quotas[0].resets_at, None);
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
            let adapter = Antigravity::new(Script::new(vec![reply]));
            assert_eq!(result(&adapter.fetch(now())), &Err(expected));
        }
    }

    #[test]
    fn without_the_cli_the_session_is_signed_out_and_nothing_runs() {
        let mut script = Script::new(vec![]);
        script.installed = false;
        let adapter = Antigravity::new(script);
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
    fn drain_stops_collecting_past_the_limit() {
        let (bytes, over) = drain(&b"abcdef"[..], 4);
        assert!(over);
        assert!(bytes.len() <= 4);
        let (bytes, over) = drain(&b"abc"[..], 4);
        assert_eq!((bytes.as_slice(), over), (&b"abc"[..], false));
    }

    #[test]
    fn cancel_and_single_run_are_honored() {
        // A runner that waits for the cancel flag, like a real slow CLI.
        struct Slow;
        impl Runner for Slow {
            fn run(&self, cancel: &AtomicBool) -> Result<CliOutput, RunError> {
                let started = Instant::now();
                while !cancel.load(Ordering::SeqCst) {
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
        let adapter = std::sync::Arc::new(Antigravity::new(Slow));
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

    /// Runs the real `agy.exe` through the production runner. Opt-in only:
    /// `cargo test --locked live_antigravity -- --ignored --nocapture`.
    /// Prints only the normalized quotas (no account data exists in them).
    #[test]
    #[ignore = "runs the installed Antigravity CLI"]
    fn live_antigravity_cli() {
        let adapter = Antigravity::detect();
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
        assert!(!adapter.tripped.load(Ordering::SeqCst), "no model turn");
    }
}

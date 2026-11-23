//! OpenAI Codex adapter.
//!
//! Sources, in order:
//! 1. `GET https://chatgpt.com/backend-api/wham/usage`, the endpoint the Codex
//!    client itself uses for its rate-limit display. It is **internal and
//!    undocumented**: its schema may change without notice. All knowledge of it
//!    is confined to `parse_usage`.
//! 2. Fallback: the newest `token_count` record with `rate_limits` that the
//!    Codex client wrote to `sessions/YYYY/MM/DD/rollout-*.jsonl`.
//!
//! Sign-in comes from `auth.json` in `CODEX_HOME` (default `~/.codex`). The
//! file is only read. This adapter never refreshes or writes tokens: when the
//! access token expires, Codex renews it the next time it runs.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, TimeDelta, Utc};
use serde::Deserialize;
use serde::de::IgnoredAny;
use serde_json::Value;

use super::adapter::{
    FetchError, Fetched, Outcome, UsageAdapter, mask_account, percent, rfc3339, schema_anomaly,
    title_case,
};
use super::http::{Transport, Ureq, check_status};
use super::local::{
    Budget, ReadError, env_dir, home_dir, jwt_claims, read_limited, read_tail_within,
};
use super::model::{Origin, Provider, Quota, QuotaKind};
use super::session::Session;
use super::time;

const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const AUTH_CLAIM: &str = "https://api.openai.com/auth";
/// Treat a token this close to expiry as expired, so it is never sent stale.
const EXPIRY_MARGIN: TimeDelta = TimeDelta::seconds(30);
const ROLLOUT_TAIL: u64 = 512 * 1024;
const ROLLOUT_FILES: usize = 6;
/// Directory entries examined per folder of `sessions/`, and day folders visited.
const SCAN_ENTRIES: usize = 2048;
const SCAN_DAYS: usize = 10;
const MAX_CREDENTIALS: u64 = 256 * 1024;
const MAX_CONFIG: u64 = 256 * 1024;
const MAX_ADDITIONAL: usize = 16;
/// Older observations describe windows that have all renewed.
const ROLLOUT_MAX_AGE: TimeDelta = TimeDelta::days(8);
/// Tolerated clock skew for records stamped slightly in the future.
const ROLLOUT_FUTURE_SKEW: TimeDelta = TimeDelta::minutes(5);

pub struct Codex<T: Transport> {
    home: Option<PathBuf>,
    transport: T,
}

impl Codex<Ureq> {
    pub fn detect() -> Self {
        let home = env_dir("CODEX_HOME").or_else(|| home_dir().map(|home| home.join(".codex")));
        Self::new(home, Ureq::new())
    }
}

impl<T: Transport> Codex<T> {
    pub fn new(home: Option<PathBuf>, transport: T) -> Self {
        Self { home, transport }
    }
}

#[derive(Deserialize)]
struct AuthFile {
    /// Declared by recent Codex versions: `chatgpt` or `apikey`.
    auth_mode: Option<String>,
    /// Presence only. The value is never retained.
    #[serde(rename = "OPENAI_API_KEY")]
    api_key: Option<IgnoredAny>,
    tokens: Option<Tokens>,
}

#[derive(Deserialize)]
struct Tokens {
    access_token: Option<String>,
    account_id: Option<String>,
    id_token: Option<String>,
}

struct SignIn {
    access_token: String,
    account_id: String,
    expires_at: Option<DateTime<Utc>>,
    session: Session,
    account: Option<String>,
    plan: Option<String>,
}

/// A failed sign-in read and what it proves about the session.
fn signed_out(error: FetchError) -> (FetchError, Session) {
    (error, Session::SignedOut)
}

fn unverified(error: FetchError) -> (FetchError, Session) {
    (error, Session::Unverified)
}

/// Codex can keep its credentials in the system credential store instead of
/// `auth.json` (`cli_auth_credentials_store = "keyring"` or `"auto"` in
/// `config.toml`). This app never reads that store, so a missing file is then
/// no proof of a logout.
fn uses_system_store(home: &Path) -> bool {
    let Ok(config) = read_limited(&home.join("config.toml"), MAX_CONFIG) else {
        return false;
    };
    config.lines().any(|line| {
        let line = line.split('#').next().unwrap_or_default();
        let Some((key, value)) = line.split_once('=') else {
            return false;
        };
        key.trim() == "cli_auth_credentials_store"
            && matches!(value.trim().trim_matches(['"', '\'']), "keyring" | "auto")
    })
}

fn read_sign_in(home: &Path) -> Result<SignIn, (FetchError, Session)> {
    let text = match read_limited(&home.join("auth.json"), MAX_CREDENTIALS) {
        Ok(text) => text,
        Err(ReadError::Missing) if uses_system_store(home) => {
            return Err(unverified(FetchError::CredentialsUnavailable));
        }
        Err(ReadError::Missing) => return Err(signed_out(FetchError::AuthMissing)),
        Err(ReadError::TooLarge | ReadError::Unreadable) => {
            return Err(unverified(FetchError::CredentialsInvalid));
        }
    };
    let file: AuthFile =
        serde_json::from_str(&text).map_err(|_| unverified(FetchError::CredentialsInvalid))?;
    let mode = file.auth_mode.as_deref().map(str::to_ascii_lowercase);
    // A declared API-key mode wins over leftover OAuth fields: an API key has
    // no ChatGPT plan windows to report.
    if matches!(mode.as_deref(), Some("apikey" | "api_key")) {
        return Err(signed_out(FetchError::Unsupported));
    }
    let tokens = file.tokens;
    let access_token = tokens
        .as_ref()
        .and_then(|tokens| tokens.access_token.clone());
    let account_id = tokens.as_ref().and_then(|tokens| tokens.account_id.clone());
    let (Some(access_token), Some(account_id)) = (
        access_token.filter(|value| !value.is_empty()),
        account_id.filter(|value| !value.is_empty()),
    ) else {
        return Err(signed_out(if file.api_key.is_some() {
            FetchError::Unsupported
        } else {
            FetchError::AuthMissing
        }));
    };

    let identity = tokens
        .and_then(|tokens| tokens.id_token)
        .and_then(|token| jwt_claims(&token));
    // The workspace account plus the user inside it. Display labels play no
    // part, so a renamed or re-masked e-mail never looks like a new account.
    let user = identity
        .as_ref()
        .and_then(|claims| claims["sub"].as_str().or_else(|| claims["email"].as_str()))
        .unwrap_or_default();
    let session = Session::account(&["codex", &account_id, user], &access_token);
    let account = identity
        .as_ref()
        .and_then(|claims| claims["email"].as_str())
        .map(mask_account);
    let plan = identity
        .as_ref()
        .and_then(|claims| claims[AUTH_CLAIM]["chatgpt_plan_type"].as_str())
        .map(title_case);
    let expires_at = jwt_claims(&access_token)
        .and_then(|claims| claims["exp"].as_f64())
        .and_then(time::from_epoch);
    Ok(SignIn {
        access_token,
        account_id,
        expires_at,
        session,
        account,
        plan,
    })
}

pub fn window_label(minutes: u32) -> String {
    match minutes {
        300 => "Janela de 5 h".into(),
        10_080 => "Semanal".into(),
        43_000..=44_700 => "Mensal".into(),
        0..60 => format!("Janela de {minutes} min"),
        60..1_440 if minutes.is_multiple_of(60) => format!("Janela de {} h", minutes / 60),
        _ if minutes.is_multiple_of(1_440) => format!("Janela de {} dias", minutes / 1_440),
        _ => format!("Janela de {} h", (minutes as f64 / 60.0).round()),
    }
}

/// A validated percentage of `value`, or `None` with an anomaly report.
fn used_percent(value: &Value, key: &str, what: &str) -> Option<u8> {
    let raw = value.get(key).filter(|raw| !raw.is_null())?;
    let checked = raw.as_f64().and_then(percent);
    if checked.is_none() {
        schema_anomaly(Provider::Codex, &format!("{what}.{key} outside 0-100"));
    }
    checked
}

/// One `*_window` object of the usage endpoint.
fn api_window(
    value: &Value,
    id: &str,
    prefix: Option<&str>,
    eligible: bool,
    now: DateTime<Utc>,
) -> Option<Quota> {
    let used = used_percent(value, "used_percent", id)?;
    let minutes = value["limit_window_seconds"]
        .as_f64()
        .and_then(time::window_minutes);
    let resets_at = time::plausible_reset(
        value["reset_at"]
            .as_f64()
            .and_then(time::from_epoch)
            .or_else(|| {
                value["reset_after_seconds"]
                    .as_f64()
                    .and_then(|seconds| time::offset(now, seconds))
            }),
        now,
    );
    let base = minutes.map(window_label).unwrap_or_else(|| "Janela".into());
    Some(Quota {
        eligible,
        ..Quota::window(
            id,
            prefix.map_or(base.clone(), |prefix| format!("{prefix} · {base}")),
            used,
            minutes,
            resets_at.map(rfc3339),
        )
    })
}

fn rate_limit_windows(
    limit: &Value,
    id: &str,
    prefix: Option<&str>,
    eligible: bool,
    now: DateTime<Utc>,
    quotas: &mut Vec<Quota>,
) {
    for (role, key) in [
        ("primary", "primary_window"),
        ("secondary", "secondary_window"),
    ] {
        if let Some(quota) = limit
            .get(key)
            .filter(|window| window.is_object())
            .and_then(|window| api_window(window, &format!("{id}-{role}"), prefix, eligible, now))
        {
            quotas.push(quota);
        }
    }
}

/// Quotas, masked account label and plan.
type ParsedUsage = (Vec<Quota>, Option<String>, Option<String>);

/// Normalizes the internal `wham/usage` response. Availability requires at
/// least one valid window of the main `rate_limit`: code review, additional
/// limits and spend controls are listed, but never drawn as the ring, and a
/// response with only those is not a reading (audit I08).
pub fn parse_usage(body: &str, now: DateTime<Utc>) -> Result<ParsedUsage, FetchError> {
    let value: Value =
        serde_json::from_str(body).map_err(|_| FetchError::Malformed("usage json".into()))?;
    if !value.is_object() {
        return Err(FetchError::Malformed("usage root".into()));
    }
    let mut quotas = Vec::new();
    if let Some(limit) = value.get("rate_limit").filter(|limit| limit.is_object()) {
        rate_limit_windows(limit, "codex", None, true, now, &mut quotas);
    }
    if !quotas.iter().any(|quota| quota.eligible) {
        return Err(FetchError::Malformed("no main rate-limit window".into()));
    }
    if let Some(limit) = value
        .get("code_review_rate_limit")
        .filter(|limit| limit.is_object())
    {
        rate_limit_windows(
            limit,
            "codeReview",
            Some("Revisão de código"),
            false,
            now,
            &mut quotas,
        );
    }
    if let Some(extra) = value["additional_rate_limits"].as_array() {
        for (index, entry) in extra.iter().enumerate().take(MAX_ADDITIONAL) {
            let name = entry["limit_name"]
                .as_str()
                .or_else(|| entry["metered_feature"].as_str())
                .map(title_case)
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| format!("Limite adicional {}", index + 1));
            if let Some(limit) = entry.get("rate_limit").filter(|limit| limit.is_object()) {
                rate_limit_windows(
                    limit,
                    &format!("additional{index}"),
                    Some(&name),
                    false,
                    now,
                    &mut quotas,
                );
            }
        }
    }
    let spend = &value["spend_control"]["individual_limit"];
    if let Some(used) = used_percent(spend, "used_percent", "spend_control") {
        quotas.push(Quota {
            kind: QuotaKind::Credits,
            eligible: false,
            ..Quota::window(
                "spendLimit",
                "Limite de gastos",
                used,
                None,
                time::plausible_reset(spend["reset_at"].as_f64().and_then(time::from_epoch), now)
                    .map(rfc3339),
            )
        });
    }
    let plan = value["plan_type"].as_str().map(title_case);
    let account = value["email"].as_str().map(mask_account);
    Ok((quotas, account, plan))
}

/// One `rate_limits` window as written by the Codex client to its rollout log.
fn rollout_window(value: &Value, id: &str, observed: DateTime<Utc>) -> Option<Quota> {
    let used = used_percent(value, "used_percent", "rollout")?;
    let minutes = value["window_minutes"]
        .as_u64()
        .and_then(|minutes| u32::try_from(minutes).ok())
        .filter(|minutes| (1..=time::MAX_WINDOW_MINUTES).contains(minutes));
    let resets_at = time::plausible_reset(
        value["resets_at"]
            .as_f64()
            .and_then(time::from_epoch)
            .or_else(|| {
                value["resets_in_seconds"]
                    .as_f64()
                    .and_then(|seconds| time::offset(observed, seconds))
            }),
        observed,
    );
    Some(Quota::window(
        id,
        minutes.map(window_label).unwrap_or_else(|| "Janela".into()),
        used,
        minutes,
        resets_at.map(rfc3339),
    ))
}

/// Parses the newest usable `rate_limits` record of a rollout log.
pub fn parse_rollout(text: &str, now: DateTime<Utc>) -> Option<Fetched> {
    text.lines()
        .rev()
        .filter(|line| line.contains("\"rate_limits\""))
        .find_map(|line| {
            let record: Value = serde_json::from_str(line).ok()?;
            let limits = record["payload"]
                .get("rate_limits")
                .or_else(|| record.get("rate_limits"))
                .filter(|limits| limits.is_object())?;
            if limits["limit_id"].as_str().is_some_and(|id| id != "codex") {
                return None;
            }
            let observed = DateTime::parse_from_rfc3339(record["timestamp"].as_str()?)
                .ok()?
                .with_timezone(&Utc);
            // Ancient records describe renewed windows; records from the
            // future would always look like the newest observation.
            let age = now.signed_duration_since(observed);
            if age > ROLLOUT_MAX_AGE || age < -ROLLOUT_FUTURE_SKEW {
                return None;
            }
            let quotas: Vec<Quota> = [
                ("codex-primary", "primary"),
                ("codex-secondary", "secondary"),
            ]
            .into_iter()
            .filter_map(|(id, key)| {
                limits
                    .get(key)
                    .filter(|window| window.is_object())
                    .and_then(|window| rollout_window(window, id, observed))
            })
            .collect();
            (!quotas.is_empty()).then(|| Fetched {
                // Rollout records carry no account: never attributed.
                identity: None,
                account: None,
                plan: limits["plan_type"].as_str().map(title_case),
                origin: Origin::SessionLog,
                quotas,
                observed_at: observed,
            })
        })
}

/// Lists the newest rollout logs by walking `sessions/YYYY/MM/DD` backwards.
/// Only numeric folders are followed, each folder is scanned up to
/// `SCAN_ENTRIES` entries, at most `SCAN_DAYS` day folders are opened, and the
/// shared budget bounds the total work (audit I06).
fn newest_rollouts(home: &Path, budget: &mut Budget) -> Vec<PathBuf> {
    fn sorted_dirs(path: &Path, budget: &mut Budget) -> Vec<PathBuf> {
        let Ok(entries) = fs::read_dir(path) else {
            return Vec::new();
        };
        let mut dirs: Vec<PathBuf> = entries
            .take(SCAN_ENTRIES)
            .take_while(|_| budget.entry())
            .flatten()
            .filter(|entry| {
                entry.file_name().to_str().is_some_and(|name| {
                    !name.is_empty() && name.bytes().all(|byte| byte.is_ascii_digit())
                })
            })
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .map(|entry| entry.path())
            .collect();
        dirs.sort();
        dirs.reverse();
        dirs
    }

    let mut files = Vec::new();
    let mut days = 0;
    'days: for year in sorted_dirs(&home.join("sessions"), budget) {
        for month in sorted_dirs(&year, budget) {
            for day in sorted_dirs(&month, budget) {
                days += 1;
                let Ok(entries) = fs::read_dir(&day) else {
                    continue;
                };
                let mut found: Vec<(std::time::SystemTime, PathBuf)> = entries
                    .take(SCAN_ENTRIES)
                    .take_while(|_| budget.entry())
                    .flatten()
                    .filter(|entry| {
                        let name = entry.file_name();
                        let name = name.to_string_lossy();
                        name.starts_with("rollout-") && name.ends_with(".jsonl")
                    })
                    .filter_map(|entry| {
                        Some((entry.metadata().ok()?.modified().ok()?, entry.path()))
                    })
                    .collect();
                found.sort();
                files.extend(found.into_iter().rev().map(|(_, path)| path));
                if files.len() >= ROLLOUT_FILES || days >= SCAN_DAYS || budget.exhausted() {
                    break 'days;
                }
            }
        }
    }
    files.truncate(ROLLOUT_FILES);
    files
}

fn rollout_fallback(home: &Path, now: DateTime<Utc>) -> Option<Fetched> {
    let mut budget = Budget::rollouts();
    newest_rollouts(home, &mut budget)
        .iter()
        .filter_map(|path| read_tail_within(path, ROLLOUT_TAIL, &mut budget).ok())
        .map(|tail| tail.text)
        .filter_map(|text| parse_rollout(&text, now))
        .max_by_key(|fetched| fetched.observed_at)
}

impl<T: Transport> UsageAdapter for Codex<T> {
    fn provider(&self) -> Provider {
        Provider::Codex
    }

    fn session(&self, _now: DateTime<Utc>) -> Option<Session> {
        Some(match &self.home {
            None => Session::SignedOut,
            Some(home) => match read_sign_in(home) {
                Ok(sign_in) => sign_in.session,
                Err((_, session)) => session,
            },
        })
    }

    fn fetch(&self, now: DateTime<Utc>) -> Outcome {
        let Some(home) = &self.home else {
            return Outcome::signed_out(FetchError::AuthMissing);
        };
        let sign_in = match read_sign_in(home) {
            Ok(sign_in) => sign_in,
            Err((error, session)) => {
                return Outcome {
                    session,
                    ..Outcome::failed(error)
                };
            }
        };
        let mut outcome = Outcome {
            session: sign_in.session,
            account: sign_in.account.clone(),
            plan: sign_in.plan.clone(),
            ..Outcome::default()
        };

        let result = if sign_in
            .expires_at
            .is_some_and(|exp| exp <= time::plus(now, EXPIRY_MARGIN))
        {
            Err(FetchError::AuthExpired)
        } else {
            let response = self.transport.get(
                USAGE_URL,
                &[
                    ("Authorization", &format!("Bearer {}", sign_in.access_token)),
                    ("ChatGPT-Account-Id", &sign_in.account_id),
                    ("Accept", "application/json"),
                    ("Cache-Control", "no-cache"),
                ],
            );
            check_status(response, now).and_then(|response| parse_usage(&response.body, now))
        };
        drop(sign_in);

        outcome.result = Some(result.map(|(quotas, account, plan)| Fetched {
            identity: outcome.session.identity(),
            account: account.or_else(|| outcome.account.clone()),
            plan: plan.or_else(|| outcome.plan.clone()),
            origin: Origin::Api,
            quotas,
            observed_at: now,
        }));
        if matches!(outcome.result, Some(Err(_))) {
            // Unattributed history: no account label is attached to it.
            outcome.fallback = rollout_fallback(home, now);
        }
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::http::TransportError;
    use crate::usage::http::fake::Script;
    use crate::usage::local::testing::{TempDir, jwt};
    use chrono::TimeZone;
    use serde_json::json;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 23, 18, 5, 25).unwrap()
    }

    const USAGE: &str = r#"{
        "email": "dev@example.com",
        "plan_type": "plus",
        "rate_limit": {
            "allowed": true,
            "limit_reached": false,
            "primary_window": { "used_percent": 0, "limit_window_seconds": 18000, "reset_after_seconds": 18000, "reset_at": 1795475125 },
            "secondary_window": { "used_percent": 73, "limit_window_seconds": 604800, "reset_after_seconds": 243832, "reset_at": 1795700957 }
        },
        "code_review_rate_limit": null,
        "additional_rate_limits": null,
        "spend_control": { "reached": false, "individual_limit": null }
    }"#;

    fn signed_in(dir: &TempDir, exp: i64) {
        let access = jwt(&json!({ "exp": exp }));
        let id = jwt(&json!({
            "email": "dev@example.com",
            AUTH_CLAIM: { "chatgpt_plan_type": "plus" }
        }));
        dir.write(
            "auth.json",
            &json!({
                "auth_mode": "chatgpt",
                "OPENAI_API_KEY": null,
                "tokens": { "access_token": access, "account_id": "acct", "id_token": id, "refresh_token": "r" }
            })
            .to_string(),
        );
    }

    fn rollout(dir: &TempDir, timestamp: &str, primary: u8, resets_at: i64) {
        let record = json!({
            "timestamp": timestamp,
            "type": "event_msg",
            "payload": { "type": "token_count", "rate_limits": {
                "limit_id": "codex",
                "primary": { "used_percent": primary, "window_minutes": 300, "resets_at": resets_at },
                "secondary": { "used_percent": 72, "window_minutes": 10080, "resets_at": 1795700957 },
                "plan_type": "plus"
            }}
        });
        dir.write(
            "sessions/2026/11/23/rollout-2026-11-23T17-00-00-x.jsonl",
            &format!("{{\"type\":\"session_meta\"}}\n{record}\n"),
        );
    }

    #[test]
    fn parses_both_windows_of_the_live_response() {
        let (quotas, account, plan) = parse_usage(USAGE, now()).unwrap();
        assert_eq!(account.as_deref(), Some("d•••@example.com"));
        assert_eq!(plan.as_deref(), Some("Plus"));
        assert_eq!(quotas.len(), 2);
        assert_eq!(quotas[0].label, "Janela de 5 h");
        assert_eq!(quotas[0].percent_used, 0);
        assert_eq!(quotas[0].resets_at.as_deref(), Some("2026-11-23T23:05:25Z"));
        assert_eq!(quotas[1].label, "Semanal");
        assert_eq!(quotas[1].percent_used, 73);
        assert_eq!(quotas[1].duration_minutes, Some(10_080));
    }

    #[test]
    fn parses_additional_limits_relative_resets_and_spend_controls() {
        let body = json!({
            "rate_limit": { "primary_window": { "used_percent": 12.6, "limit_window_seconds": 18000, "reset_after_seconds": 60 } },
            "additional_rate_limits": [{ "limit_name": "spark", "rate_limit": {
                "primary_window": { "used_percent": 5, "limit_window_seconds": 604800, "reset_at": 1795700957 } } }],
            "code_review_rate_limit": { "primary_window": { "used_percent": 1, "limit_window_seconds": 604800 } },
            "spend_control": { "individual_limit": { "used_percent": 40 } }
        });
        let (quotas, _, _) = parse_usage(&body.to_string(), now()).unwrap();
        let labels: Vec<_> = quotas.iter().map(|quota| quota.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Janela de 5 h",
                "Revisão de código · Semanal",
                "Spark · Semanal",
                "Limite de gastos"
            ]
        );
        assert_eq!(quotas[0].percent_used, 13);
        assert_eq!(quotas[0].resets_at.as_deref(), Some("2026-11-23T18:06:25Z"));
        assert_eq!(quotas[3].kind, QuotaKind::Credits);
    }

    #[test]
    fn malformed_responses_are_rejected() {
        for body in [
            "",
            "[]",
            "{\"rate_limit\":{}}",
            "{\"rate_limit\":{\"primary_window\":{\"used_percent\":\"x\"}}}",
        ] {
            assert!(
                matches!(parse_usage(body, now()), Err(FetchError::Malformed(_))),
                "{body}"
            );
        }
    }

    #[test]
    fn live_query_sends_account_header_and_reports_success() {
        let dir = TempDir::new("codex-live");
        signed_in(&dir, now().timestamp() + 3600);
        let script = Script::new(vec![Script::ok(200, USAGE)]);
        let codex = Codex::new(Some(dir.path().into()), script);
        let outcome = codex.fetch(now());
        let fetched = outcome.result.unwrap().unwrap();
        assert_eq!(fetched.origin, Origin::Api);
        assert_eq!(fetched.quotas.len(), 2);
        assert_eq!(outcome.fallback, None);
        let requests = codex.transport.requests.lock().unwrap();
        assert_eq!(requests[0].0, USAGE_URL);
        assert!(requests[0].1.contains(&"ChatGPT-Account-Id".to_string()));
    }

    #[test]
    fn missing_or_api_key_sign_in_never_reaches_the_network() {
        let dir = TempDir::new("codex-missing");
        let codex = Codex::new(Some(dir.path().into()), Script::new(vec![]));
        assert_eq!(
            codex.fetch(now()).result,
            Some(Err(FetchError::AuthMissing))
        );

        dir.write("auth.json", r#"{"OPENAI_API_KEY":"sk-test","tokens":null}"#);
        assert_eq!(
            codex.fetch(now()).result,
            Some(Err(FetchError::Unsupported))
        );
        assert_eq!(codex.transport.request_count(), 0);
    }

    #[test]
    fn expired_token_is_not_sent_and_falls_back_to_the_session_log() {
        let dir = TempDir::new("codex-expired");
        signed_in(&dir, now().timestamp() - 10);
        rollout(&dir, "2026-11-23T17:16:10.604Z", 53, 1795455833);
        let codex = Codex::new(Some(dir.path().into()), Script::new(vec![]));
        let outcome = codex.fetch(now());
        assert_eq!(outcome.result, Some(Err(FetchError::AuthExpired)));
        let fallback = outcome.fallback.unwrap();
        assert_eq!(fallback.origin, Origin::SessionLog);
        assert_eq!(fallback.quotas[0].percent_used, 53);
        // B01: the rollout has no verifiable account link.
        assert_eq!(fallback.account, None);
        assert_eq!(fallback.identity, None);
        assert_eq!(codex.transport.request_count(), 0);
    }

    fn auth(dir: &TempDir, account_id: &str, sub: &str, access: &str) {
        let id = jwt(&json!({ "sub": sub, "email": "dev@example.com" }));
        let access = jwt(&json!({ "exp": now().timestamp() + 3600, "nonce": access }));
        dir.write(
            "auth.json",
            &json!({ "auth_mode": "chatgpt", "tokens": {
                "access_token": access, "account_id": account_id, "id_token": id } })
            .to_string(),
        );
    }

    #[test]
    fn b01_session_identity_follows_the_account_not_the_label_or_token() {
        let dir = TempDir::new("codex-identity");
        let codex = Codex::new(
            Some(dir.path().into()),
            Script::new(vec![
                Err(TransportError::Network),
                Err(TransportError::Network),
                Err(TransportError::Network),
                Err(TransportError::Network),
            ]),
        );
        auth(&dir, "acct-1", "user-1", "t1");
        let first = codex.fetch(now()).session;
        auth(&dir, "acct-1", "user-1", "t2");
        let renewed = codex.fetch(now()).session;
        assert_ne!(first, renewed, "credential version changes");
        assert_eq!(first.identity(), renewed.identity(), "same account");
        assert!(first.identity().is_some());
        // Same e-mail label, different user or workspace: another account.
        auth(&dir, "acct-1", "user-2", "t1");
        assert_ne!(codex.fetch(now()).session.identity(), first.identity());
        auth(&dir, "acct-2", "user-1", "t1");
        assert_ne!(codex.fetch(now()).session.identity(), first.identity());
    }

    #[test]
    fn b01_logout_and_api_key_prove_a_signed_out_session_but_a_broken_file_does_not() {
        let dir = TempDir::new("codex-session");
        let codex = Codex::new(Some(dir.path().into()), Script::new(vec![]));
        assert_eq!(codex.fetch(now()).session, Session::SignedOut);
        dir.write("auth.json", r#"{"OPENAI_API_KEY":"sk-test","tokens":null}"#);
        assert_eq!(codex.fetch(now()).session, Session::SignedOut);
        dir.write("auth.json", r#"{"tokens": {"access_token": "#);
        let partial = codex.fetch(now());
        assert_eq!(partial.session, Session::Unverified);
        assert_eq!(partial.result, Some(Err(FetchError::CredentialsInvalid)));
        assert_eq!(codex.transport.request_count(), 0);
    }

    #[test]
    fn b02_out_of_range_times_in_the_response_never_panic() {
        let body = json!({ "rate_limit": {
            "primary_window": { "used_percent": 5, "limit_window_seconds": 1e300, "reset_after_seconds": 1e300 },
            "secondary_window": { "used_percent": 6, "limit_window_seconds": -5, "reset_at": 9.3e18 }
        }, "spend_control": { "individual_limit": { "used_percent": 1, "reset_at": -1e300 } } });
        let (quotas, _, _) = parse_usage(&body.to_string(), now()).unwrap();
        assert!(quotas.iter().all(|quota| quota.resets_at.is_none()));
        assert!(quotas.iter().all(|quota| quota.duration_minutes.is_none()));

        let rollout = r#"{"timestamp":"2026-11-23T17:00:00Z","rate_limits":{"primary":{"used_percent":9,"window_minutes":18446744073709551615,"resets_in_seconds":9223372036854775807}}}"#;
        let fetched = parse_rollout(rollout, now()).unwrap();
        assert_eq!(fetched.quotas[0].resets_at, None);
        assert_eq!(fetched.quotas[0].duration_minutes, None);
        let rollout = r#"{"timestamp":"2026-11-23T17:00:00Z","rate_limits":{"primary":{"used_percent":9,"resets_at":-9223372036854775808}}}"#;
        assert_eq!(
            parse_rollout(rollout, now()).unwrap().quotas[0].resets_at,
            None
        );
        // A record stamped in the future is ignored.
        let future = r#"{"timestamp":"2027-01-01T00:00:00Z","rate_limits":{"primary":{"used_percent":9,"window_minutes":300}}}"#;
        assert_eq!(parse_rollout(future, now()), None);
    }

    #[test]
    fn b02_an_access_token_expiring_at_the_end_of_time_does_not_panic() {
        let dir = TempDir::new("codex-exp-max");
        signed_in(&dir, i64::MAX);
        let codex = Codex::new(
            Some(dir.path().into()),
            Script::new(vec![Err(TransportError::Network)]),
        );
        assert_eq!(codex.fetch(now()).result, Some(Err(FetchError::Network)));
    }

    #[test]
    fn network_errors_and_rate_limits_keep_the_fallback() {
        let dir = TempDir::new("codex-errors");
        signed_in(&dir, now().timestamp() + 3600);
        rollout(&dir, "2026-11-23T17:16:10.604Z", 53, 1795455833);
        let limited = Ok(crate::usage::http::Response {
            status: 429,
            retry_after: Some("90".into()),
            body: String::new(),
        });
        let codex = Codex::new(
            Some(dir.path().into()),
            Script::new(vec![
                Err(TransportError::Network),
                limited,
                Script::ok(401, "{}"),
            ]),
        );
        let first = codex.fetch(now());
        assert_eq!(first.result, Some(Err(FetchError::Network)));
        assert!(first.fallback.is_some());
        assert_eq!(
            codex.fetch(now()).result,
            Some(Err(FetchError::RateLimited {
                retry_after: Some(std::time::Duration::from_secs(90))
            }))
        );
        assert_eq!(
            codex.fetch(now()).result,
            Some(Err(FetchError::AuthRejected))
        );
    }

    #[test]
    fn rollout_parser_ignores_other_buckets_and_ancient_records() {
        let other = r#"{"timestamp":"2026-11-23T17:00:00Z","payload":{"rate_limits":{"limit_id":"other","primary":{"used_percent":9,"window_minutes":300}}}}"#;
        assert_eq!(parse_rollout(other, now()), None);
        let old = r#"{"timestamp":"2026-10-01T00:00:00Z","payload":{"rate_limits":{"primary":{"used_percent":9,"window_minutes":300}}}}"#;
        assert_eq!(parse_rollout(old, now()), None);
        let relative = r#"{"timestamp":"2026-11-23T17:00:00Z","rate_limits":{"primary":{"used_percent":9,"window_minutes":300,"resets_in_seconds":60}}}"#;
        let fetched = parse_rollout(relative, now()).unwrap();
        assert_eq!(
            fetched.quotas[0].resets_at.as_deref(),
            Some("2026-11-23T17:01:00Z")
        );
    }

    #[test]
    fn i08_credit_only_or_out_of_domain_responses_are_not_readings() {
        for body in [
            r#"{"spend_control":{"individual_limit":{"used_percent":40}}}"#,
            r#"{"code_review_rate_limit":{"primary_window":{"used_percent":3}}}"#,
            r#"{"rate_limit":{"primary_window":{"used_percent":180}}}"#,
            r#"{"rate_limit":{"primary_window":{"used_percent":null}}}"#,
        ] {
            assert!(
                matches!(parse_usage(body, now()), Err(FetchError::Malformed(_))),
                "{body}"
            );
        }
        // Secondary metrics are listed but never eligible for the ring.
        let body = json!({
            "rate_limit": { "primary_window": { "used_percent": 10, "limit_window_seconds": 18000 } },
            "code_review_rate_limit": { "primary_window": { "used_percent": 99 } },
            "additional_rate_limits": [{ "limit_name": "spark", "rate_limit": {
                "primary_window": { "used_percent": 98 } } }]
        });
        let (quotas, _, _) = parse_usage(&body.to_string(), now()).unwrap();
        let eligible: Vec<_> = quotas
            .iter()
            .filter(|quota| quota.eligible)
            .map(|quota| quota.id.as_str())
            .collect();
        assert_eq!(eligible, ["codex-primary"]);
        assert_eq!(quotas.len(), 3);
    }

    #[test]
    fn i06_declared_mode_and_system_store_are_honored() {
        let dir = TempDir::new("codex-mode");
        let codex = Codex::new(Some(dir.path().into()), Script::new(vec![]));
        // Leftover OAuth fields do not override a declared API-key mode.
        signed_in(&dir, now().timestamp() + 3600);
        let text = std::fs::read_to_string(dir.path().join("auth.json")).unwrap();
        dir.write("auth.json", &text.replace("\"chatgpt\"", "\"apikey\""));
        let outcome = codex.fetch(now());
        assert_eq!(outcome.result, Some(Err(FetchError::Unsupported)));
        assert_eq!(outcome.session, Session::SignedOut);
        // Credentials kept in the system store: not a logout, not readable.
        std::fs::remove_file(dir.path().join("auth.json")).unwrap();
        dir.write(
            "config.toml",
            "model = \"x\"\ncli_auth_credentials_store = \"keyring\" # os\n",
        );
        let outcome = codex.fetch(now());
        assert_eq!(
            outcome.result,
            Some(Err(FetchError::CredentialsUnavailable))
        );
        assert_eq!(outcome.session, Session::Unverified);
        dir.write("config.toml", "cli_auth_credentials_store = \"file\"\n");
        assert_eq!(codex.fetch(now()).session, Session::SignedOut);
        // An oversized auth.json is never loaded.
        dir.write("auth.json", &" ".repeat(300 * 1024));
        assert_eq!(
            codex.fetch(now()).result,
            Some(Err(FetchError::CredentialsInvalid))
        );
        assert_eq!(codex.transport.request_count(), 0);
    }

    #[test]
    fn window_labels() {
        assert_eq!(window_label(300), "Janela de 5 h");
        assert_eq!(window_label(10_080), "Semanal");
        assert_eq!(window_label(43_200), "Mensal");
        assert_eq!(window_label(30), "Janela de 30 min");
        assert_eq!(window_label(120), "Janela de 2 h");
        assert_eq!(window_label(2_880), "Janela de 2 dias");
    }
}

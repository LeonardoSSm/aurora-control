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
    FetchError, Fetched, Outcome, UsageAdapter, mask_account, percent, rfc3339, title_case,
};
use super::http::{Transport, Ureq, check_status};
use super::local::{env_dir, home_dir, jwt_claims, read_tail};
use super::model::{Origin, Provider, Quota, QuotaKind};

const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const AUTH_CLAIM: &str = "https://api.openai.com/auth";
/// Treat a token this close to expiry as expired, so it is never sent stale.
const EXPIRY_MARGIN: TimeDelta = TimeDelta::seconds(30);
const ROLLOUT_TAIL: u64 = 512 * 1024;
const ROLLOUT_FILES: usize = 6;
/// Older observations describe windows that have all renewed.
const ROLLOUT_MAX_AGE: TimeDelta = TimeDelta::days(8);

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
    account: Option<String>,
    plan: Option<String>,
}

fn read_sign_in(home: &Path) -> Result<SignIn, FetchError> {
    let text = match fs::read_to_string(home.join("auth.json")) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(FetchError::AuthMissing);
        }
        Err(_) => return Err(FetchError::Malformed("auth.json unreadable".into())),
    };
    let file: AuthFile = serde_json::from_str(&text)
        .map_err(|_| FetchError::Malformed("auth.json schema".into()))?;
    let tokens = file.tokens;
    let access_token = tokens
        .as_ref()
        .and_then(|tokens| tokens.access_token.clone());
    let account_id = tokens.as_ref().and_then(|tokens| tokens.account_id.clone());
    let (Some(access_token), Some(account_id)) = (
        access_token.filter(|value| !value.is_empty()),
        account_id.filter(|value| !value.is_empty()),
    ) else {
        // An API key has no ChatGPT plan windows to report.
        return Err(if file.api_key.is_some() {
            FetchError::Unsupported
        } else {
            FetchError::AuthMissing
        });
    };

    let identity = tokens
        .and_then(|tokens| tokens.id_token)
        .and_then(|token| jwt_claims(&token));
    let account = identity
        .as_ref()
        .and_then(|claims| claims["email"].as_str())
        .map(mask_account);
    let plan = identity
        .as_ref()
        .and_then(|claims| claims[AUTH_CLAIM]["chatgpt_plan_type"].as_str())
        .map(title_case);
    let expires_at = jwt_claims(&access_token)
        .and_then(|claims| claims["exp"].as_i64())
        .and_then(|exp| DateTime::from_timestamp(exp, 0));
    Ok(SignIn {
        access_token,
        account_id,
        expires_at,
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

/// One `*_window` object of the usage endpoint.
fn api_window(value: &Value, id: &str, prefix: Option<&str>, now: DateTime<Utc>) -> Option<Quota> {
    let used = value.get("used_percent")?.as_f64()?;
    let minutes = value["limit_window_seconds"]
        .as_f64()
        .map(|seconds| (seconds / 60.0).round() as u32);
    let resets_at = value["reset_at"]
        .as_f64()
        .and_then(|epoch| DateTime::from_timestamp(epoch as i64, 0))
        .or_else(|| {
            value["reset_after_seconds"]
                .as_f64()
                .map(|seconds| now + TimeDelta::seconds(seconds as i64))
        });
    let base = minutes.map(window_label).unwrap_or_else(|| "Janela".into());
    Some(Quota {
        id: id.into(),
        label: prefix.map_or(base.clone(), |prefix| format!("{prefix} · {base}")),
        kind: QuotaKind::Window,
        percent_used: percent(used),
        duration_minutes: minutes,
        resets_at: resets_at.map(rfc3339),
    })
}

fn rate_limit_windows(
    limit: &Value,
    id: &str,
    prefix: Option<&str>,
    now: DateTime<Utc>,
    quotas: &mut Vec<Quota>,
) {
    for (role, key) in [
        ("primary", "primary_window"),
        ("secondary", "secondary_window"),
    ] {
        if let Some(quota) = limit
            .get(key)
            .and_then(|window| api_window(window, &format!("{id}-{role}"), prefix, now))
        {
            quotas.push(quota);
        }
    }
}

/// Quotas, masked account label and plan.
type ParsedUsage = (Vec<Quota>, Option<String>, Option<String>);

/// Normalizes the internal `wham/usage` response.
pub fn parse_usage(body: &str, now: DateTime<Utc>) -> Result<ParsedUsage, FetchError> {
    let value: Value =
        serde_json::from_str(body).map_err(|_| FetchError::Malformed("usage json".into()))?;
    if !value.is_object() {
        return Err(FetchError::Malformed("usage root".into()));
    }
    let mut quotas = Vec::new();
    if let Some(limit) = value.get("rate_limit").filter(|limit| limit.is_object()) {
        rate_limit_windows(limit, "codex", None, now, &mut quotas);
    }
    if let Some(limit) = value
        .get("code_review_rate_limit")
        .filter(|limit| limit.is_object())
    {
        rate_limit_windows(
            limit,
            "codeReview",
            Some("Revisão de código"),
            now,
            &mut quotas,
        );
    }
    if let Some(extra) = value["additional_rate_limits"].as_array() {
        for (index, entry) in extra.iter().enumerate() {
            let name = entry["limit_name"]
                .as_str()
                .or_else(|| entry["metered_feature"].as_str())
                .map(title_case)
                .unwrap_or_else(|| format!("Limite adicional {}", index + 1));
            if let Some(limit) = entry.get("rate_limit") {
                rate_limit_windows(
                    limit,
                    &format!("additional{index}"),
                    Some(&name),
                    now,
                    &mut quotas,
                );
            }
        }
    }
    let spend = &value["spend_control"]["individual_limit"];
    if let Some(used) = spend["used_percent"].as_f64() {
        quotas.push(Quota {
            id: "spendLimit".into(),
            label: "Limite de gastos".into(),
            kind: QuotaKind::Credits,
            percent_used: percent(used),
            duration_minutes: None,
            resets_at: spend["reset_at"]
                .as_f64()
                .and_then(|epoch| DateTime::from_timestamp(epoch as i64, 0))
                .map(rfc3339),
        });
    }
    if quotas.is_empty() {
        return Err(FetchError::Malformed("no rate-limit windows".into()));
    }
    let plan = value["plan_type"].as_str().map(title_case);
    let account = value["email"].as_str().map(mask_account);
    Ok((quotas, account, plan))
}

/// One `rate_limits` window as written by the Codex client to its rollout log.
fn rollout_window(value: &Value, id: &str, observed: DateTime<Utc>) -> Option<Quota> {
    let used = value.get("used_percent")?.as_f64()?;
    let minutes = value["window_minutes"]
        .as_u64()
        .map(|minutes| minutes as u32);
    let resets_at = value["resets_at"]
        .as_i64()
        .and_then(|epoch| DateTime::from_timestamp(epoch, 0))
        .or_else(|| {
            value["resets_in_seconds"]
                .as_i64()
                .map(|seconds| observed + TimeDelta::seconds(seconds))
        });
    Some(Quota {
        id: id.into(),
        label: minutes.map(window_label).unwrap_or_else(|| "Janela".into()),
        kind: QuotaKind::Window,
        percent_used: percent(used),
        duration_minutes: minutes,
        resets_at: resets_at.map(rfc3339),
    })
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
            if now - observed > ROLLOUT_MAX_AGE {
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
                    .and_then(|window| rollout_window(window, id, observed))
            })
            .collect();
            (!quotas.is_empty()).then(|| Fetched {
                account: None,
                plan: limits["plan_type"].as_str().map(title_case),
                origin: Origin::SessionLog,
                quotas,
                observed_at: observed,
            })
        })
}

/// Lists the newest rollout logs by walking `sessions/YYYY/MM/DD` backwards.
fn newest_rollouts(home: &Path) -> Vec<PathBuf> {
    fn sorted_dirs(path: &Path) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = fs::read_dir(path)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect();
        dirs.sort();
        dirs.reverse();
        dirs
    }

    let mut files = Vec::new();
    'days: for year in sorted_dirs(&home.join("sessions")) {
        for month in sorted_dirs(&year) {
            for day in sorted_dirs(&month) {
                let mut found: Vec<(std::time::SystemTime, PathBuf)> = fs::read_dir(&day)
                    .into_iter()
                    .flatten()
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
                if files.len() >= ROLLOUT_FILES {
                    break 'days;
                }
            }
        }
    }
    files.truncate(ROLLOUT_FILES);
    files
}

fn rollout_fallback(home: &Path, now: DateTime<Utc>) -> Option<Fetched> {
    newest_rollouts(home)
        .iter()
        .filter_map(|path| read_tail(path, ROLLOUT_TAIL).ok())
        .filter_map(|text| parse_rollout(&text, now))
        .max_by_key(|fetched| fetched.observed_at)
}

impl<T: Transport> UsageAdapter for Codex<T> {
    fn provider(&self) -> Provider {
        Provider::Codex
    }

    fn fetch(&self, now: DateTime<Utc>) -> Outcome {
        let Some(home) = &self.home else {
            return Outcome::failed(FetchError::AuthMissing);
        };
        let sign_in = match read_sign_in(home) {
            Ok(sign_in) => sign_in,
            Err(error) => return Outcome::failed(error),
        };
        let mut outcome = Outcome {
            account: sign_in.account.clone(),
            plan: sign_in.plan.clone(),
            ..Outcome::default()
        };

        let result = if sign_in
            .expires_at
            .is_some_and(|exp| exp <= now + EXPIRY_MARGIN)
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
            account: account.or_else(|| outcome.account.clone()),
            plan: plan.or_else(|| outcome.plan.clone()),
            origin: Origin::Api,
            quotas,
            observed_at: now,
        }));
        if matches!(outcome.result, Some(Err(_))) {
            outcome.fallback = rollout_fallback(home, now).map(|mut fallback| {
                fallback.account = outcome.account.clone();
                fallback.plan = fallback.plan.or_else(|| outcome.plan.clone());
                fallback
            });
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
        assert_eq!(fallback.account.as_deref(), Some("d•••@example.com"));
        assert_eq!(codex.transport.request_count(), 0);
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
    fn window_labels() {
        assert_eq!(window_label(300), "Janela de 5 h");
        assert_eq!(window_label(10_080), "Semanal");
        assert_eq!(window_label(43_200), "Mensal");
        assert_eq!(window_label(30), "Janela de 30 min");
        assert_eq!(window_label(120), "Janela de 2 h");
        assert_eq!(window_label(2_880), "Janela de 2 dias");
    }
}

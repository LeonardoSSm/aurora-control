//! Claude Code adapter.
//!
//! Account quotas come from `GET https://api.anthropic.com/api/oauth/usage`
//! with the `oauth-2025-04-20` beta header, the endpoint Claude Code's own
//! `/usage` view relies on. It is **internal and undocumented**: all knowledge
//! of its schema is confined to `parse_usage`.
//!
//! Sign-in comes from `.credentials.json` in `CLAUDE_CONFIG_DIR` (default
//! `~/.claude`), as written by the Claude Code CLI. The file is only read. This
//! adapter never refreshes tokens, runs the CLI, installs hooks or changes
//! settings, so Remote Control and running sessions are unaffected.
//!
//! Three things are kept apart:
//! - subscription quotas (rolling windows and extra usage): from the endpoint;
//! - conversation tokens of local sessions: summed from the session transcripts
//!   in `projects/`, reported as `LocalActivity`, never as a quota;
//! - API costs: an API-key login has no quota endpoint, so it is `Unsupported`.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, TimeDelta, Utc};
use serde::Deserialize;
use serde::de::IgnoredAny;
use serde_json::Value;

use super::adapter::{
    FetchError, Fetched, Outcome, UsageAdapter, mask_account, percent, title_case,
};
use super::http::{Transport, Ureq, check_status};
use super::local::{env_dir, home_dir, read_tail};
use super::model::{LocalActivity, Origin, Provider, Quota, QuotaKind};
use super::session::Session;
use super::time;

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const OAUTH_BETA: &str = "oauth-2025-04-20";
const EXPIRY_MARGIN: TimeDelta = TimeDelta::seconds(30);
const ACTIVITY_MINUTES: u32 = 300;
const TRANSCRIPT_TAIL: u64 = 8 * 1024 * 1024;

pub struct ClaudeCode<T: Transport> {
    config: Option<PathBuf>,
    /// `.claude.json`, which holds the account profile.
    profile: Option<PathBuf>,
    transport: T,
}

impl ClaudeCode<Ureq> {
    pub fn detect() -> Self {
        let custom = env_dir("CLAUDE_CONFIG_DIR");
        let home = home_dir();
        let config = custom
            .clone()
            .or_else(|| home.as_ref().map(|home| home.join(".claude")));
        // Claude Code keeps the profile beside the default folder, or inside a
        // custom configuration folder.
        let profile = match custom {
            Some(dir) => Some(dir.join(".claude.json")),
            None => home.map(|home| home.join(".claude.json")),
        };
        Self::new(config, profile, Ureq::new())
    }
}

impl<T: Transport> ClaudeCode<T> {
    pub fn new(config: Option<PathBuf>, profile: Option<PathBuf>, transport: T) -> Self {
        Self {
            config,
            profile,
            transport,
        }
    }
}

#[derive(Deserialize)]
struct CredentialsFile {
    #[serde(rename = "claudeAiOauth")]
    oauth: Option<OAuth>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OAuth {
    access_token: Option<String>,
    /// Milliseconds since the Unix epoch.
    expires_at: Option<f64>,
    subscription_type: Option<String>,
    scopes: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProfileFile {
    oauth_account: Option<OAuthAccount>,
    /// Presence only: an API-key login. The value is never retained.
    primary_api_key: Option<IgnoredAny>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OAuthAccount {
    account_uuid: Option<String>,
    organization_uuid: Option<String>,
    email_address: Option<String>,
    organization_name: Option<String>,
}

struct Profile {
    /// Stable account key parts (never displayed), when the profile has them.
    identity: Option<Vec<String>>,
    account: Option<String>,
    api_key: bool,
}

fn read_profile(path: Option<&Path>) -> Profile {
    let file = path
        .and_then(|path| fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str::<ProfileFile>(&text).ok());
    let Some(file) = file else {
        return Profile {
            identity: None,
            account: None,
            api_key: false,
        };
    };
    let api_key = file.primary_api_key.is_some();
    let Some(account) = file.oauth_account else {
        return Profile {
            identity: None,
            account: None,
            api_key,
        };
    };
    let email = account.email_address.filter(|email| !email.is_empty());
    // The account and organization UUIDs identify the session. The e-mail is
    // a weaker fallback for older profiles. Labels never identify.
    let identity = match (&account.account_uuid, &email) {
        (Some(uuid), _) if !uuid.is_empty() => Some(vec![
            uuid.clone(),
            account.organization_uuid.clone().unwrap_or_default(),
        ]),
        (_, Some(email)) => Some(vec![email.to_lowercase()]),
        _ => None,
    };
    Profile {
        identity,
        account: email
            .map(|email| mask_account(&email))
            .or(account.organization_name),
        api_key,
    }
}

struct SignIn {
    access_token: String,
    plan: Option<String>,
}

/// Why no request can be made, what it proves about the session, and the
/// plan label when the credentials file still names one.
struct NoSignIn {
    error: FetchError,
    signed_out: bool,
    plan: Option<String>,
}

impl NoSignIn {
    fn signed_out(error: FetchError) -> Self {
        Self {
            error,
            signed_out: true,
            plan: None,
        }
    }

    fn unverified(error: FetchError, plan: Option<String>) -> Self {
        Self {
            error,
            signed_out: false,
            plan,
        }
    }
}

fn read_sign_in(config: &Path, api_key: bool, now: DateTime<Utc>) -> Result<SignIn, NoSignIn> {
    let absent = || {
        NoSignIn::signed_out(if api_key {
            FetchError::Unsupported
        } else {
            FetchError::AuthMissing
        })
    };
    let text = match fs::read_to_string(config.join(".credentials.json")) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Err(absent()),
        Err(_) => {
            return Err(NoSignIn::unverified(
                FetchError::Malformed("credentials unreadable".into()),
                None,
            ));
        }
    };
    let file: CredentialsFile = serde_json::from_str(&text).map_err(|_| {
        NoSignIn::unverified(FetchError::Malformed("credentials schema".into()), None)
    })?;
    let Some(oauth) = file.oauth else {
        return Err(absent());
    };
    let plan = oauth.subscription_type.as_deref().map(title_case);
    let Some(access_token) = oauth.access_token.filter(|token| !token.is_empty()) else {
        // The CLI logged out and blanked its token.
        return Err(NoSignIn::signed_out(FetchError::AuthMissing));
    };
    // `user:profile` grants access to the usage endpoint. Tokens minted for
    // inference only (for example by `claude setup-token`) cannot read it.
    if oauth
        .scopes
        .as_ref()
        .is_some_and(|scopes| !scopes.iter().any(|scope| scope == "user:profile"))
    {
        return Err(NoSignIn::unverified(FetchError::Unsupported, plan));
    }
    let expires_at = oauth.expires_at.and_then(time::from_epoch_millis);
    if expires_at.is_none_or(|expiry| expiry <= time::plus(now, EXPIRY_MARGIN)) {
        return Err(NoSignIn::unverified(FetchError::AuthExpired, plan));
    }
    Ok(SignIn { access_token, plan })
}

/// The session for a sign-in that exists, identified by the profile.
fn session(profile: &Profile, credential: &str) -> Session {
    match &profile.identity {
        Some(parts) => {
            let mut key: Vec<&str> = vec!["claude"];
            key.extend(parts.iter().map(String::as_str));
            Session::account(&key, credential)
        }
        None => Session::Unverified,
    }
}

fn known_window(key: &str) -> Option<(&'static str, &'static str, u32)> {
    Some(match key {
        "five_hour" => ("fiveHour", "Sessão de 5 h", 300),
        "seven_day" => ("sevenDay", "Semanal · todos os modelos", 10_080),
        "seven_day_opus" => ("sevenDayOpus", "Semanal · Opus", 10_080),
        "seven_day_sonnet" => ("sevenDaySonnet", "Semanal · Sonnet", 10_080),
        "seven_day_oauth_apps" => ("sevenDayOauthApps", "Semanal · apps conectados", 10_080),
        _ => return None,
    })
}

fn camel(key: &str) -> String {
    let mut out = String::new();
    let mut upper = false;
    for character in key.chars() {
        if character == '_' {
            upper = true;
        } else if upper {
            out.extend(character.to_uppercase());
            upper = false;
        } else {
            out.push(character);
        }
    }
    out
}

fn reset(value: &Value) -> Option<String> {
    let text = value.as_str()?;
    let time = DateTime::parse_from_rfc3339(text).ok()?;
    Some(super::adapter::rfc3339(time.with_timezone(&Utc)))
}

/// Normalizes the internal `oauth/usage` response. `utilization` is a
/// percentage between 0 and 100. Windows with `null` utilization are not in use
/// for the account and are skipped.
pub fn parse_usage(body: &str) -> Result<Vec<Quota>, FetchError> {
    let value: Value =
        serde_json::from_str(body).map_err(|_| FetchError::Malformed("usage json".into()))?;
    let Some(object) = value.as_object() else {
        return Err(FetchError::Malformed("usage root".into()));
    };
    let mut quotas = Vec::new();
    for (key, entry) in object {
        if key == "extra_usage" {
            continue;
        }
        let Some(utilization) = entry.get("utilization").and_then(Value::as_f64) else {
            continue;
        };
        let (id, label, minutes) = match known_window(key) {
            Some((id, label, minutes)) => (id.to_string(), label.to_string(), Some(minutes)),
            None => (
                camel(key),
                title_case(key),
                key.starts_with("seven_day").then_some(10_080),
            ),
        };
        quotas.push(Quota {
            id,
            label,
            kind: QuotaKind::Window,
            percent_used: percent(utilization),
            duration_minutes: minutes,
            resets_at: reset(&entry["resets_at"]),
        });
    }
    // Keep a stable order: shorter windows first, then by identifier.
    quotas.sort_by(|a, b| {
        a.duration_minutes
            .unwrap_or(u32::MAX)
            .cmp(&b.duration_minutes.unwrap_or(u32::MAX))
            .then_with(|| a.id.cmp(&b.id))
    });

    let extra = &value["extra_usage"];
    if let (Some(true), Some(utilization)) =
        (extra["is_enabled"].as_bool(), extra["utilization"].as_f64())
    {
        quotas.push(Quota {
            id: "extraUsage".into(),
            label: "Uso extra · mensal".into(),
            kind: QuotaKind::Credits,
            percent_used: percent(utilization),
            duration_minutes: None,
            resets_at: None,
        });
    }
    if !quotas.iter().any(|quota| quota.kind == QuotaKind::Window) {
        return Err(FetchError::Malformed("no usage windows".into()));
    }
    Ok(quotas)
}

/// Sums the tokens of assistant turns written to local session transcripts in
/// the last five hours. Streamed turns repeat the same message id, so each id
/// counts once. Cache reads are excluded: they re-read context already counted.
pub fn local_activity(projects: &Path, now: DateTime<Utc>) -> Option<LocalActivity> {
    let since = time::minus(now, TimeDelta::minutes(i64::from(ACTIVITY_MINUTES)));
    let since_file = time::system_time(since)?;
    let mut seen = HashSet::new();
    let mut sessions = HashSet::new();
    let mut tokens = 0u64;
    let directories = fs::read_dir(projects).ok()?;
    for directory in directories.flatten() {
        let Ok(files) = fs::read_dir(directory.path()) else {
            continue;
        };
        for file in files.flatten() {
            let path = file.path();
            let recent = file
                .metadata()
                .and_then(|metadata| metadata.modified())
                .is_ok_and(|modified| modified >= since_file);
            if !recent || path.extension().is_none_or(|ext| ext != "jsonl") {
                continue;
            }
            let Ok(text) = read_tail(&path, TRANSCRIPT_TAIL) else {
                continue;
            };
            for line in text.lines().filter(|line| line.contains("\"usage\"")) {
                let Ok(record) = serde_json::from_str::<Value>(line) else {
                    continue;
                };
                if record["type"] != "assistant" {
                    continue;
                }
                let Some(at) = record["timestamp"]
                    .as_str()
                    .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                else {
                    continue;
                };
                if at.with_timezone(&Utc) < since {
                    continue;
                }
                let message = &record["message"];
                let id = message["id"].as_str().unwrap_or_default().to_owned();
                if id.is_empty() || !seen.insert(id) {
                    continue;
                }
                let usage = &message["usage"];
                tokens = [
                    "input_tokens",
                    "cache_creation_input_tokens",
                    "output_tokens",
                ]
                .iter()
                .filter_map(|key| usage[*key].as_u64())
                .fold(tokens, u64::saturating_add);
                if let Some(session) = record["sessionId"].as_str() {
                    sessions.insert(session.to_owned());
                }
            }
        }
    }
    Some(LocalActivity {
        window_minutes: ACTIVITY_MINUTES,
        tokens,
        sessions: sessions.len() as u32,
    })
}

impl<T: Transport> UsageAdapter for ClaudeCode<T> {
    fn provider(&self) -> Provider {
        Provider::ClaudeCode
    }

    fn fetch(&self, now: DateTime<Utc>) -> Outcome {
        let Some(config) = &self.config else {
            return Outcome::signed_out(FetchError::AuthMissing);
        };
        let profile = read_profile(self.profile.as_deref());
        let mut outcome = Outcome {
            account: profile.account.clone(),
            activity: local_activity(&config.join("projects"), now),
            ..Outcome::default()
        };

        let sign_in = match read_sign_in(config, profile.api_key, now) {
            Ok(sign_in) => sign_in,
            Err(failure) => {
                if failure.signed_out {
                    // No session: neither its labels nor its quotas apply.
                    outcome.session = Session::SignedOut;
                    outcome.account = None;
                } else {
                    // A sign-in exists (expired, restricted or unreadable).
                    // Its account is the profile's, when the profile has one.
                    outcome.session = session(&profile, "");
                    outcome.plan = failure.plan;
                }
                outcome.result = Some(Err(failure.error));
                return outcome;
            }
        };
        outcome.session = session(&profile, &sign_in.access_token);
        outcome.plan = sign_in.plan.clone();
        let response = self.transport.get(
            USAGE_URL,
            &[
                ("Authorization", &format!("Bearer {}", sign_in.access_token)),
                ("anthropic-beta", OAUTH_BETA),
                ("Accept", "application/json"),
            ],
        );
        drop(sign_in);
        outcome.result = Some(
            check_status(response, now)
                .and_then(|response| parse_usage(&response.body))
                .map(|quotas| Fetched {
                    identity: outcome.session.identity(),
                    account: outcome.account.clone(),
                    plan: outcome.plan.clone(),
                    origin: Origin::Api,
                    quotas,
                    observed_at: now,
                }),
        );
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::http::TransportError;
    use crate::usage::http::fake::Script;
    use crate::usage::local::testing::TempDir;
    use chrono::TimeZone;
    use serde_json::json;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 23, 18, 0, 0).unwrap()
    }

    const USAGE: &str = r#"{
        "five_hour": { "utilization": 37.0, "resets_at": "2026-11-23T21:00:00.123456+00:00" },
        "seven_day": { "utilization": 12.4, "resets_at": "2026-11-27T10:00:00+00:00" },
        "seven_day_oauth_apps": null,
        "seven_day_opus": { "utilization": 0.0, "resets_at": null },
        "seven_day_sonnet": { "utilization": null, "resets_at": null },
        "extra_usage": { "is_enabled": true, "monthly_limit": 5000, "used_credits": 1000, "utilization": 20.0 }
    }"#;

    fn setup(dir: &TempDir, expires_at: i64, scopes: &[&str]) -> ClaudeCode<Script> {
        dir.write(
            "config/.credentials.json",
            &json!({
                "mcpOAuth": { "server": { "accessToken": "unrelated" } },
                "claudeAiOauth": {
                    "accessToken": "token",
                    "refreshToken": "refresh",
                    "expiresAt": expires_at,
                    "scopes": scopes,
                    "subscriptionType": "max"
                }
            })
            .to_string(),
        );
        dir.write(
            "profile.json",
            &json!({ "oauthAccount": { "emailAddress": "dev@example.com" } }).to_string(),
        );
        ClaudeCode::new(
            Some(dir.path().join("config")),
            Some(dir.path().join("profile.json")),
            Script::new(vec![]),
        )
    }

    fn with_script(adapter: ClaudeCode<Script>, script: Script) -> ClaudeCode<Script> {
        ClaudeCode::new(adapter.config, adapter.profile, script)
    }

    const SCOPES: &[&str] = &["user:inference", "user:profile"];

    #[test]
    fn parses_windows_in_order_and_keeps_extra_usage_as_credits() {
        let quotas = parse_usage(USAGE).unwrap();
        let ids: Vec<_> = quotas.iter().map(|quota| quota.id.as_str()).collect();
        assert_eq!(ids, ["fiveHour", "sevenDay", "sevenDayOpus", "extraUsage"]);
        assert_eq!(quotas[0].percent_used, 37);
        assert_eq!(quotas[0].resets_at.as_deref(), Some("2026-11-23T21:00:00Z"));
        assert_eq!(quotas[1].label, "Semanal · todos os modelos");
        assert_eq!(quotas[2].resets_at, None);
        assert_eq!(quotas[3].kind, QuotaKind::Credits);
    }

    #[test]
    fn unknown_windows_are_kept_with_a_readable_label() {
        let quotas =
            parse_usage(r#"{"seven_day_cowork":{"utilization":5,"resets_at":null}}"#).unwrap();
        assert_eq!(quotas[0].id, "sevenDayCowork");
        assert_eq!(quotas[0].label, "Seven day cowork");
        assert_eq!(quotas[0].duration_minutes, Some(10_080));
    }

    #[test]
    fn malformed_responses_are_rejected() {
        for body in ["", "null", "{}", r#"{"five_hour":{"utilization":"high"}}"#] {
            assert!(
                matches!(parse_usage(body), Err(FetchError::Malformed(_))),
                "{body}"
            );
        }
    }

    #[test]
    fn detected_sign_in_queries_with_the_beta_header() {
        let dir = TempDir::new("claude-live");
        let adapter = setup(&dir, now().timestamp_millis() + 3_600_000, SCOPES);
        let adapter = with_script(adapter, Script::new(vec![Script::ok(200, USAGE)]));
        let outcome = adapter.fetch(now());
        let fetched = outcome.result.unwrap().unwrap();
        assert_eq!(fetched.account.as_deref(), Some("d•••@example.com"));
        assert_eq!(fetched.plan.as_deref(), Some("Max"));
        assert_eq!(fetched.quotas.len(), 4);
        let requests = adapter.transport.requests.lock().unwrap();
        assert_eq!(requests[0].0, USAGE_URL);
        assert!(requests[0].1.contains(&"anthropic-beta".to_string()));
    }

    #[test]
    fn expired_or_inference_only_tokens_are_never_sent() {
        let dir = TempDir::new("claude-expired");
        let adapter = setup(&dir, now().timestamp_millis() - 1, SCOPES);
        let outcome = adapter.fetch(now());
        assert_eq!(outcome.result, Some(Err(FetchError::AuthExpired)));
        assert_eq!(outcome.plan.as_deref(), Some("Max"));
        assert_eq!(outcome.account.as_deref(), Some("d•••@example.com"));

        let adapter = setup(&dir, 0, SCOPES);
        assert_eq!(
            adapter.fetch(now()).result,
            Some(Err(FetchError::AuthExpired))
        );

        let adapter = setup(
            &dir,
            now().timestamp_millis() + 3_600_000,
            &["user:inference"],
        );
        assert_eq!(
            adapter.fetch(now()).result,
            Some(Err(FetchError::Unsupported))
        );
        assert_eq!(adapter.transport.request_count(), 0);
    }

    #[test]
    fn missing_sign_in_and_api_key_accounts() {
        let dir = TempDir::new("claude-missing");
        let adapter = ClaudeCode::new(
            Some(dir.path().join("config")),
            Some(dir.path().join("profile.json")),
            Script::new(vec![]),
        );
        assert_eq!(
            adapter.fetch(now()).result,
            Some(Err(FetchError::AuthMissing))
        );
        dir.write("profile.json", r#"{"primaryApiKey":"sk-ant-test"}"#);
        assert_eq!(
            adapter.fetch(now()).result,
            Some(Err(FetchError::Unsupported))
        );
    }

    #[test]
    fn http_failures_are_classified() {
        let dir = TempDir::new("claude-http");
        let adapter = setup(&dir, now().timestamp_millis() + 3_600_000, SCOPES);
        let adapter = with_script(
            adapter,
            Script::new(vec![
                Script::ok(401, r#"{"type":"error"}"#),
                Ok(crate::usage::http::Response {
                    status: 429,
                    retry_after: None,
                    body: String::new(),
                }),
                Err(TransportError::Timeout),
                Script::ok(200, "<html>"),
            ]),
        );
        assert_eq!(
            adapter.fetch(now()).result,
            Some(Err(FetchError::AuthRejected))
        );
        assert_eq!(
            adapter.fetch(now()).result,
            Some(Err(FetchError::RateLimited { retry_after: None }))
        );
        assert_eq!(adapter.fetch(now()).result, Some(Err(FetchError::Timeout)));
        assert!(matches!(
            adapter.fetch(now()).result,
            Some(Err(FetchError::Malformed(_)))
        ));
    }

    fn profile(dir: &TempDir, uuid: &str, email: &str) {
        dir.write(
            "profile.json",
            &json!({ "oauthAccount": {
                "accountUuid": uuid, "organizationUuid": "org-1", "emailAddress": email } })
            .to_string(),
        );
    }

    fn credentials(dir: &TempDir, token: &str) {
        dir.write(
            "config/.credentials.json",
            &json!({ "claudeAiOauth": {
                "accessToken": token,
                "expiresAt": now().timestamp_millis() + 3_600_000,
                "scopes": SCOPES,
                "subscriptionType": "pro" } })
            .to_string(),
        );
    }

    fn adapter_in(dir: &TempDir, script: Script) -> ClaudeCode<Script> {
        ClaudeCode::new(
            Some(dir.path().join("config")),
            Some(dir.path().join("profile.json")),
            script,
        )
    }

    #[test]
    fn b01_identity_comes_from_the_account_uuid_not_the_label_or_token() {
        let dir = TempDir::new("claude-identity");
        let adapter = adapter_in(
            &dir,
            Script::new((0..4).map(|_| Err(TransportError::Network)).collect()),
        );
        profile(&dir, "uuid-a", "dev@example.com");
        credentials(&dir, "token-1");
        let first = adapter.fetch(now());
        credentials(&dir, "token-2");
        let renewed = adapter.fetch(now());
        assert!(first.session.identity().is_some());
        assert_eq!(first.session.identity(), renewed.session.identity());
        assert_ne!(first.session, renewed.session);
        // Same masked label, different account.
        profile(&dir, "uuid-b", "dan@example.com");
        let other = adapter.fetch(now());
        assert_eq!(other.account, first.account, "labels collide");
        assert_ne!(other.session.identity(), first.session.identity());
        // Without a profile the account cannot be verified.
        std::fs::remove_file(dir.path().join("profile.json")).unwrap();
        assert_eq!(adapter.fetch(now()).session, Session::Unverified);
    }

    #[test]
    fn b01_logout_is_a_signed_out_session_without_labels() {
        let dir = TempDir::new("claude-logout");
        profile(&dir, "uuid-a", "dev@example.com");
        let adapter = adapter_in(&dir, Script::new(vec![]));
        let missing = adapter.fetch(now());
        assert_eq!(missing.session, Session::SignedOut);
        assert_eq!(missing.account, None);
        credentials(&dir, "");
        let blank = adapter.fetch(now());
        assert_eq!(blank.session, Session::SignedOut);
        assert_eq!(blank.result, Some(Err(FetchError::AuthMissing)));
        // A partially written file proves nothing.
        dir.write("config/.credentials.json", r#"{"claudeAiOauth": {"acc"#);
        assert!(adapter.fetch(now()).session.identity().is_some());
        assert_eq!(adapter.transport.request_count(), 0);
    }

    #[test]
    fn b02_an_expiry_outside_the_range_is_treated_as_expired() {
        let dir = TempDir::new("claude-expiry-range");
        profile(&dir, "uuid-a", "dev@example.com");
        for expires_at in [json!(1e300), json!(-1e300), json!(9.3e18)] {
            dir.write(
                "config/.credentials.json",
                &json!({ "claudeAiOauth": { "accessToken": "t", "expiresAt": expires_at,
                    "scopes": SCOPES } })
                .to_string(),
            );
            let adapter = adapter_in(&dir, Script::new(vec![]));
            assert_eq!(
                adapter.fetch(now()).result,
                Some(Err(FetchError::AuthExpired))
            );
        }
    }

    #[test]
    fn local_activity_counts_recent_turns_once_and_excludes_cache_reads() {
        let dir = TempDir::new("claude-activity");
        let turn = |id: &str, session: &str, at: &str, output: u64| {
            json!({
                "type": "assistant",
                "sessionId": session,
                "timestamp": at,
                "message": { "id": id, "usage": {
                    "input_tokens": 10, "cache_creation_input_tokens": 100,
                    "cache_read_input_tokens": 99999, "output_tokens": output } }
            })
            .to_string()
        };
        let lines = [
            turn("a", "s1", "2026-11-23T17:00:00Z", 5),
            turn("a", "s1", "2026-11-23T17:00:01Z", 5),
            turn("b", "s2", "2026-11-23T17:30:00Z", 40),
            turn("old", "s3", "2026-11-23T12:00:00Z", 1000),
            r#"{"type":"user","message":{"usage":{}}}"#.to_string(),
        ];
        dir.write("projects/C--repo/session.jsonl", &lines.join("\n"));
        let activity = local_activity(&dir.path().join("projects"), now()).unwrap();
        assert_eq!(activity.tokens, 115 + 150);
        assert_eq!(activity.sessions, 2);
        assert_eq!(activity.window_minutes, 300);
    }
}

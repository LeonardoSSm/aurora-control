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

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, TimeDelta, Utc};
use serde::Deserialize;
use serde::de::IgnoredAny;
use serde_json::Value;

use super::adapter::{
    FetchError, Fetched, Outcome, UsageAdapter, display_label, mask_account, percent,
    schema_anomaly, title_case,
};
use super::http::{Transport, Ureq, check_status};
use super::local::{Budget, ReadError, env_dir, home_dir, read_limited, read_tail_within};
use super::model::{
    DiagnosticRow, DiagnosticState, LocalActivity, Origin, Provider, Quota, QuotaKind,
};
use super::session::Session;
use super::time;

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const OAUTH_BETA: &str = "oauth-2025-04-20";
const EXPIRY_MARGIN: TimeDelta = TimeDelta::seconds(30);
const ACTIVITY_MINUTES: u32 = 300;
const TRANSCRIPT_TAIL: u64 = 8 * 1024 * 1024;
const MAX_CREDENTIALS: u64 = 256 * 1024;
/// `.claude.json` also keeps per-project history; a few hundred KiB is typical.
const MAX_PROFILE: u64 = 8 * 1024 * 1024;
const MAX_USAGE_KEYS: usize = 64;

pub struct ClaudeCode<T: Transport> {
    config: Option<PathBuf>,
    /// `.claude.json`, which holds the account profile.
    profile: Option<PathBuf>,
    transport: T,
    /// `CLAUDE_CONFIG_DIR` chose the folder (for the diagnosis).
    custom_dir: bool,
}

impl ClaudeCode<Ureq> {
    pub fn detect() -> Self {
        let custom = env_dir("CLAUDE_CONFIG_DIR");
        let custom_dir = custom.is_some();
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
        Self {
            custom_dir,
            ..Self::new(config, profile, Ureq::new())
        }
    }
}

impl<T: Transport> ClaudeCode<T> {
    pub fn new(config: Option<PathBuf>, profile: Option<PathBuf>, transport: T) -> Self {
        Self {
            config,
            profile,
            transport,
            custom_dir: false,
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
        .and_then(|path| read_limited(path, MAX_PROFILE).ok())
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
            .or_else(|| account.organization_name.as_deref().and_then(display_label)),
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

/// Reads `.credentials.json`.
///
/// - No file, or no `claudeAiOauth` token: the CLI has no session in this
///   file. That proves a logout only when the profile has no account either;
///   otherwise the session may live in Claude Desktop or another host that
///   supplies credentials directly, so the credential is merely unavailable
///   to this app (audit I06).
/// - Unreadable, oversized or malformed file: invalid, proves nothing.
/// - `scopes` present without `user:profile`: an inference-only token.
///   `scopes` absent (older CLI versions): the request is attempted, and a
///   401/403 answer is reported as a rejected session.
fn read_sign_in(config: &Path, profile: &Profile, now: DateTime<Utc>) -> Result<SignIn, NoSignIn> {
    let absent = || {
        if profile.identity.is_some() {
            NoSignIn::unverified(FetchError::CredentialsUnavailable, None)
        } else {
            NoSignIn::signed_out(if profile.api_key {
                FetchError::Unsupported
            } else {
                FetchError::AuthMissing
            })
        }
    };
    let text = match read_limited(&config.join(".credentials.json"), MAX_CREDENTIALS) {
        Ok(text) => text,
        Err(ReadError::Missing) => return Err(absent()),
        Err(ReadError::TooLarge | ReadError::Unreadable) => {
            return Err(NoSignIn::unverified(FetchError::CredentialsInvalid, None));
        }
    };
    let file: CredentialsFile = serde_json::from_str(&text)
        .map_err(|_| NoSignIn::unverified(FetchError::CredentialsInvalid, None))?;
    let Some(oauth) = file.oauth else {
        return Err(absent());
    };
    let plan = oauth.subscription_type.as_deref().map(title_case);
    let Some(access_token) = oauth.access_token.filter(|token| !token.is_empty()) else {
        // The CLI blanks its token on logout, and a host app may supply the
        // credential instead of this file.
        return Err(absent());
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

/// Duration of a key that is not known yet but follows a known window family
/// (`five_hour_*`, `seven_day_*`). Anything else has no window semantics.
fn window_family(key: &str) -> Option<u32> {
    let suffix = |prefix: &str| {
        key.strip_prefix(prefix).is_some_and(|rest| {
            !rest.is_empty()
                && rest
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        })
    };
    if suffix("five_hour_") {
        Some(300)
    } else if suffix("seven_day_") {
        Some(10_080)
    } else {
        None
    }
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

fn reset(value: &Value, now: DateTime<Utc>) -> Option<String> {
    let text = value.as_str()?;
    let time = DateTime::parse_from_rfc3339(text).ok()?.with_timezone(&Utc);
    time::plausible_reset(Some(time), now).map(super::adapter::rfc3339)
}

fn utilization(entry: &Value, key: &str) -> Option<u8> {
    let checked = entry["utilization"].as_f64().and_then(percent);
    if checked.is_none() {
        schema_anomaly(
            Provider::ClaudeCode,
            &format!("{key}.utilization outside 0-100"),
        );
    }
    checked
}

/// Normalizes the internal `oauth/usage` response. `utilization` is a
/// percentage between 0 and 100. Windows with `null` utilization are not in
/// use for the account and are skipped.
///
/// Semantics are explicit (audit I08): only known keys may become the primary
/// metric, and at least one of them must be valid for a reading. New keys of
/// a known family (`five_hour_*`, `seven_day_*`) are listed but never drawn.
/// Other keys have no window semantics and are ignored.
pub fn parse_usage(body: &str, now: DateTime<Utc>) -> Result<Vec<Quota>, FetchError> {
    let value: Value =
        serde_json::from_str(body).map_err(|_| FetchError::Malformed("usage json".into()))?;
    let Some(object) = value.as_object() else {
        return Err(FetchError::Malformed("usage root".into()));
    };
    let mut quotas = Vec::new();
    for (key, entry) in object.iter().take(MAX_USAGE_KEYS) {
        if key == "extra_usage" || entry.get("utilization").is_none_or(Value::is_null) {
            continue;
        }
        let (id, label, minutes, eligible) = match (known_window(key), window_family(key)) {
            (Some((id, label, minutes)), _) => (id.to_string(), label.to_string(), minutes, true),
            (None, Some(minutes)) => (camel(key), title_case(key), minutes, false),
            (None, None) => {
                schema_anomaly(
                    Provider::ClaudeCode,
                    &format!("{key} has no window semantics"),
                );
                continue;
            }
        };
        let Some(used) = utilization(entry, key) else {
            continue;
        };
        quotas.push(Quota {
            eligible,
            ..Quota::window(
                id,
                label,
                used,
                Some(minutes),
                reset(&entry["resets_at"], now),
            )
        });
    }
    if !quotas.iter().any(|quota| quota.eligible) {
        return Err(FetchError::Malformed("no known usage window".into()));
    }
    // Keep a stable order: shorter windows first, then by identifier.
    quotas.sort_by(|a, b| {
        a.duration_minutes
            .unwrap_or(u32::MAX)
            .cmp(&b.duration_minutes.unwrap_or(u32::MAX))
            .then_with(|| a.id.cmp(&b.id))
    });

    let extra = &value["extra_usage"];
    if extra["is_enabled"].as_bool() == Some(true)
        && let Some(used) = utilization(extra, "extra_usage")
    {
        quotas.push(Quota {
            kind: QuotaKind::Credits,
            eligible: false,
            ..Quota::window("extraUsage", "Uso extra · mensal", used, None, None)
        });
    }
    Ok(quotas)
}

/// Whether a tail that starts mid-file still covers the whole window: its
/// first dated record is at or before `since`. Otherwise earlier records of
/// the window may lie in the part that was not read.
fn covers_since(text: &str, since: DateTime<Utc>) -> bool {
    text.lines()
        .take(64)
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find_map(|record| {
            record["timestamp"]
                .as_str()
                .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        })
        .is_some_and(|first| first.with_timezone(&Utc) <= since)
}

/// Sums the tokens of assistant turns written to local session transcripts in
/// the last five hours. Cache reads are excluded: they re-read context
/// already counted.
///
/// Streamed turns repeat the same message id with growing totals, so each id
/// keeps its largest value, wherever it appears. The scan runs within a
/// cumulative budget of entries, files, bytes, records and time; when a limit
/// is reached the result is marked partial (audit I06).
pub fn local_activity(projects: &Path, now: DateTime<Utc>) -> Option<LocalActivity> {
    scan_activity(projects, now, TRANSCRIPT_TAIL)
}

fn scan_activity(projects: &Path, now: DateTime<Utc>, tail_limit: u64) -> Option<LocalActivity> {
    let since = time::minus(now, TimeDelta::minutes(i64::from(ACTIVITY_MINUTES)));
    let since_file = time::system_time(since)?;
    let mut budget = Budget::transcripts();
    let mut turns: HashMap<String, u64> = HashMap::new();
    let mut sessions = HashSet::new();
    // A tail cut before the start of the activity window may miss records.
    let mut cut_window = false;
    let directories = fs::read_dir(projects).ok()?;
    'scan: for directory in directories.flatten() {
        if !budget.entry() {
            break;
        }
        let Ok(files) = fs::read_dir(directory.path()) else {
            continue;
        };
        for file in files.flatten() {
            if !budget.entry() {
                break 'scan;
            }
            let path = file.path();
            let recent = file
                .metadata()
                .and_then(|metadata| metadata.modified())
                .is_ok_and(|modified| modified >= since_file);
            if !recent || path.extension().is_none_or(|ext| ext != "jsonl") {
                continue;
            }
            let Ok(tail) = read_tail_within(&path, tail_limit, &mut budget) else {
                // A recent transcript left unread (locked, denied, or beyond
                // the budget) leaves the total incomplete.
                cut_window = true;
                continue;
            };
            if tail.cut && !covers_since(&tail.text, since) {
                cut_window = true;
            }
            let text = tail.text;
            for line in text.lines().filter(|line| line.contains("\"usage\"")) {
                if !budget.record() {
                    break 'scan;
                }
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
                let Some(id) = message["id"].as_str().filter(|id| !id.is_empty()) else {
                    continue;
                };
                let usage = &message["usage"];
                let total = [
                    "input_tokens",
                    "cache_creation_input_tokens",
                    "output_tokens",
                ]
                .iter()
                .filter_map(|key| usage[*key].as_u64())
                .fold(0, u64::saturating_add);
                let kept = turns.entry(id.to_owned()).or_default();
                *kept = (*kept).max(total);
                if let Some(session) = record["sessionId"].as_str() {
                    sessions.insert(session.to_owned());
                }
            }
        }
    }
    Some(LocalActivity {
        window_minutes: ACTIVITY_MINUTES,
        tokens: turns.values().copied().fold(0, u64::saturating_add),
        sessions: u32::try_from(sessions.len()).unwrap_or(u32::MAX),
        partial: budget.exhausted() || cut_window,
    })
}

/// Diagnosis of the Claude Code sign-in, from local files only. Reports
/// states and masked labels; the token itself, file contents and account
/// identifiers never leave this function.
pub fn diagnose(
    config: Option<&Path>,
    profile_path: Option<&Path>,
    custom_dir: bool,
    now: DateTime<Utc>,
) -> Vec<DiagnosticRow> {
    use DiagnosticState::{Error, Info, Ok as Good, Warning};
    let mut rows = vec![DiagnosticRow::new(
        "Pasta de configuração",
        Info,
        if custom_dir {
            "Definida por CLAUDE_CONFIG_DIR"
        } else {
            "Padrão do Claude Code (~\\.claude)"
        },
    )];
    let Some(config) = config else {
        rows.push(DiagnosticRow::new(
            "Pasta de configuração",
            Error,
            "Pasta do usuário não encontrada",
        ));
        return rows;
    };

    let profile = read_profile(profile_path);
    rows.push(
        match (&profile.identity, &profile.account, profile.api_key) {
            (Some(_), account, _) => DiagnosticRow::new(
                "Perfil (.claude.json)",
                Good,
                format!(
                    "Conta vinculada: {}",
                    account.as_deref().unwrap_or("sem rótulo")
                ),
            ),
            (None, _, true) => DiagnosticRow::new(
                "Perfil (.claude.json)",
                Info,
                "Login por chave de API: não há cotas de assinatura",
            ),
            (None, _, false) => DiagnosticRow::new(
                "Perfil (.claude.json)",
                Warning,
                "Sem conta OAuth no perfil",
            ),
        },
    );

    let label = "Credencial do CLI (.credentials.json)";
    let text = match read_limited(&config.join(".credentials.json"), MAX_CREDENTIALS) {
        Ok(text) => text,
        Err(ReadError::Missing) => {
            rows.push(DiagnosticRow::new(
                label,
                Warning,
                "Arquivo ausente. Se o Claude Code roda dentro do app Claude Desktop, \
                 a credencial é fornecida pelo app e não fica neste arquivo.",
            ));
            return rows;
        }
        Err(ReadError::TooLarge) => {
            rows.push(DiagnosticRow::new(
                label,
                Error,
                "Maior que o limite de 256 KiB",
            ));
            return rows;
        }
        Err(ReadError::Unreadable) => {
            rows.push(DiagnosticRow::new(label, Error, "Arquivo ilegível"));
            return rows;
        }
    };
    let Ok(file) = serde_json::from_str::<CredentialsFile>(&text) else {
        rows.push(DiagnosticRow::new(
            label,
            Error,
            "Formato inesperado (o arquivo pode estar sendo regravado)",
        ));
        return rows;
    };
    let Some(oauth) = file.oauth else {
        rows.push(DiagnosticRow::new(
            label,
            Warning,
            "Sem sessão do Claude.ai",
        ));
        return rows;
    };
    let token = oauth
        .access_token
        .as_deref()
        .is_some_and(|token| !token.is_empty());
    rows.push(if token {
        DiagnosticRow::new(label, Good, "Token presente (o valor nunca é exibido)")
    } else {
        DiagnosticRow::new(
            label,
            Warning,
            "Token vazio: o CLI saiu da conta, ou o app Claude Desktop fornece a credencial",
        )
    });

    let expiry = oauth.expires_at.and_then(time::from_epoch_millis);
    rows.push(match expiry {
        None => DiagnosticRow::new("Validade", Warning, "Sem data de expiração válida"),
        Some(expiry) if expiry <= time::plus(now, EXPIRY_MARGIN) => DiagnosticRow {
            at: Some(super::adapter::rfc3339(expiry)),
            ..DiagnosticRow::new(
                "Validade",
                Warning,
                "Expirado. Execute “claude” no terminal para renovar",
            )
        },
        Some(expiry) => DiagnosticRow {
            at: Some(super::adapter::rfc3339(expiry)),
            ..DiagnosticRow::new("Validade", Good, "Válido")
        },
    });

    rows.push(match &oauth.scopes {
        Some(scopes) if scopes.iter().any(|scope| scope == "user:profile") => {
            DiagnosticRow::new("Escopo user:profile", Good, "Concedido")
        }
        Some(_) => DiagnosticRow::new(
            "Escopo user:profile",
            Error,
            "Ausente: token só de inferência (por exemplo, claude setup-token)",
        ),
        None => DiagnosticRow::new(
            "Escopo user:profile",
            Info,
            "Não declarado: a consulta é tentada e uma recusa é informada",
        ),
    });
    if let Some(plan) = oauth.subscription_type.as_deref().map(title_case) {
        rows.push(DiagnosticRow::new("Plano", Info, plan));
    }
    rows
}

impl<T: Transport> ClaudeCode<T> {
    fn current_session(&self, now: DateTime<Utc>) -> Session {
        let Some(config) = &self.config else {
            return Session::SignedOut;
        };
        let profile = read_profile(self.profile.as_deref());
        match read_sign_in(config, &profile, now) {
            Ok(sign_in) => session(&profile, &sign_in.access_token),
            Err(failure) if failure.signed_out => Session::SignedOut,
            Err(_) => session(&profile, ""),
        }
    }
}

impl<T: Transport> UsageAdapter for ClaudeCode<T> {
    fn provider(&self) -> Provider {
        Provider::ClaudeCode
    }

    fn session(&self, now: DateTime<Utc>) -> Option<Session> {
        Some(self.current_session(now))
    }
    fn diagnostics(&self, now: DateTime<Utc>) -> Option<Vec<DiagnosticRow>> {
        Some(diagnose(
            self.config.as_deref(),
            self.profile.as_deref(),
            self.custom_dir,
            now,
        ))
    }

    fn fetch(&self, now: DateTime<Utc>) -> Outcome {
        let Some(config) = &self.config else {
            return Outcome::signed_out(FetchError::AuthMissing);
        };
        let profile = read_profile(self.profile.as_deref());
        let mut outcome = Outcome {
            account: profile.account.clone(),
            ..Outcome::default()
        };

        match read_sign_in(config, &profile, now) {
            Err(failure) if failure.signed_out => {
                // No session: neither its labels, its quotas nor its local
                // activity are read.
                outcome.session = Session::SignedOut;
                outcome.account = None;
                outcome.result = Some(Err(failure.error));
                return outcome;
            }
            Err(failure) => {
                // A sign-in exists (expired, restricted, unavailable to this
                // app or unreadable). Its account is the profile's, if known.
                outcome.session = session(&profile, "");
                outcome.plan = failure.plan;
                outcome.result = Some(Err(failure.error));
            }
            Ok(sign_in) => {
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
                        .and_then(|response| parse_usage(&response.body, now))
                        .map(|quotas| Fetched {
                            identity: outcome.session.identity(),
                            account: outcome.account.clone(),
                            plan: outcome.plan.clone(),
                            origin: Origin::Api,
                            quotas,
                            observed_at: now,
                        }),
                );
            }
        }
        // Local activity is read after the quota attempt, so its scan never
        // delays the sign-in check or the request.
        outcome.activity = local_activity(&config.join("projects"), now);
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
        let quotas = parse_usage(USAGE, now()).unwrap();
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
        let quotas = parse_usage(
            r#"{"five_hour":{"utilization":1},"seven_day_cowork":{"utilization":99,"resets_at":null}}"#,
            now(),
        )
        .unwrap();
        assert_eq!(quotas[1].id, "sevenDayCowork");
        assert_eq!(quotas[1].label, "Seven day cowork");
        assert_eq!(quotas[1].duration_minutes, Some(10_080));
        // I08: listed, but never the primary metric.
        assert!(!quotas[1].eligible);
        assert!(quotas[0].eligible);
    }

    #[test]
    fn i08_responses_without_trustworthy_semantics_are_rejected() {
        for body in [
            // Only a future metric of a known family.
            r#"{"seven_day_cowork":{"utilization":5}}"#,
            // A top-level object with utilization but no window semantics.
            r#"{"monthly_tokens":{"utilization":50}}"#,
            // Only credits.
            r#"{"extra_usage":{"is_enabled":true,"utilization":20}}"#,
            // Known window outside the percentage domain.
            r#"{"five_hour":{"utilization":250}}"#,
            r#"{"five_hour":{"utilization":-4}}"#,
        ] {
            assert!(
                matches!(parse_usage(body, now()), Err(FetchError::Malformed(_))),
                "{body}"
            );
        }
        // An invalid window is dropped when a valid one remains; an
        // implausible renewal becomes unknown.
        let quotas = parse_usage(
            r#"{"five_hour":{"utilization":20,"resets_at":"2099-01-01T00:00:00Z"},
                "seven_day":{"utilization":1e9},"odd":{"utilization":3}}"#,
            now(),
        )
        .unwrap();
        assert_eq!(quotas.len(), 1);
        assert_eq!(quotas[0].resets_at, None);
    }

    #[test]
    fn malformed_responses_are_rejected() {
        for body in ["", "null", "{}", r#"{"five_hour":{"utilization":"high"}}"#] {
            assert!(
                matches!(parse_usage(body, now()), Err(FetchError::Malformed(_))),
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
        dir.write("profile.json", r#"{"oauthAccount":null}"#);
        let adapter = adapter_in(&dir, Script::new(vec![]));
        let missing = adapter.fetch(now());
        assert_eq!(missing.session, Session::SignedOut);
        assert_eq!(missing.result, Some(Err(FetchError::AuthMissing)));
        assert_eq!(missing.account, None);
        assert_eq!(
            missing.activity, None,
            "no transcript scan without a session"
        );
        credentials(&dir, "");
        let blank = adapter.fetch(now());
        assert_eq!(blank.session, Session::SignedOut);
        assert_eq!(blank.result, Some(Err(FetchError::AuthMissing)));
        assert_eq!(adapter.transport.request_count(), 0);
    }

    #[test]
    fn i06_a_missing_file_with_an_account_profile_is_not_a_logout() {
        // Claude Desktop or another host may hold the credential.
        let dir = TempDir::new("claude-unavailable");
        profile(&dir, "uuid-a", "dev@example.com");
        let adapter = adapter_in(&dir, Script::new(vec![]));
        let missing = adapter.fetch(now());
        assert_eq!(
            missing.result,
            Some(Err(FetchError::CredentialsUnavailable))
        );
        assert!(missing.session.identity().is_some());
        assert_eq!(missing.account.as_deref(), Some("d•••@example.com"));
        credentials(&dir, "");
        assert_eq!(
            adapter.fetch(now()).result,
            Some(Err(FetchError::CredentialsUnavailable))
        );
        // A partially written or oversized file is invalid, and proves nothing.
        dir.write("config/.credentials.json", r#"{"claudeAiOauth": {"acc"#);
        let partial = adapter.fetch(now());
        assert_eq!(partial.result, Some(Err(FetchError::CredentialsInvalid)));
        assert!(partial.session.identity().is_some());
        dir.write("config/.credentials.json", &" ".repeat(300 * 1024));
        assert_eq!(
            adapter.fetch(now()).result,
            Some(Err(FetchError::CredentialsInvalid))
        );
        dir.write("config/.credentials.json", "[1, 2]");
        assert_eq!(
            adapter.fetch(now()).result,
            Some(Err(FetchError::CredentialsInvalid))
        );
        assert_eq!(adapter.transport.request_count(), 0);
    }

    #[test]
    fn i06_absent_scopes_are_attempted_and_a_refusal_is_reported() {
        let dir = TempDir::new("claude-no-scopes");
        profile(&dir, "uuid-a", "dev@example.com");
        dir.write(
            "config/.credentials.json",
            &json!({ "claudeAiOauth": { "accessToken": "t",
                "expiresAt": now().timestamp_millis() + 3_600_000 } })
            .to_string(),
        );
        let adapter = adapter_in(&dir, Script::new(vec![Script::ok(403, "{}")]));
        assert_eq!(
            adapter.fetch(now()).result,
            Some(Err(FetchError::AuthRejected))
        );
        assert_eq!(adapter.transport.request_count(), 1);
    }

    #[test]
    fn i06_the_session_probe_reads_local_files_only() {
        let dir = TempDir::new("claude-probe");
        profile(&dir, "uuid-a", "dev@example.com");
        credentials(&dir, "token-1");
        let adapter = adapter_in(&dir, Script::new(vec![]));
        let first = adapter.session(now()).unwrap();
        credentials(&dir, "token-2");
        let renewed = adapter.session(now()).unwrap();
        assert_eq!(first.identity(), renewed.identity());
        assert_ne!(first, renewed);
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

    #[test]
    fn i06_a_streamed_turn_keeps_its_final_total_and_the_scan_is_bounded() {
        let dir = TempDir::new("claude-progressive");
        let turn = |output: u64| {
            json!({ "type": "assistant", "sessionId": "s1", "timestamp": "2026-11-23T17:00:00Z",
                "message": { "id": "m", "usage": { "input_tokens": 10, "output_tokens": output } } })
            .to_string()
        };
        // The first record of the id is partial; a later one has the total.
        dir.write(
            "projects/p/a.jsonl",
            &[turn(5), turn(50), turn(20)].join(
                "
",
            ),
        );
        let activity = local_activity(&dir.path().join("projects"), now()).unwrap();
        assert_eq!(activity.tokens, 60);
        assert!(!activity.partial);

        // Many files: the cumulative budget stops the scan and says so.
        for index in 0..300 {
            dir.write(&format!("projects/p/{index}.jsonl"), &turn(1));
        }
        let activity = local_activity(&dir.path().join("projects"), now()).unwrap();
        assert!(activity.partial);
    }
}

#[cfg(test)]
mod truncation_tests {
    use super::*;
    use crate::usage::local::testing::TempDir;
    use chrono::TimeZone;
    use serde_json::json;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 23, 18, 0, 0).unwrap()
    }

    fn turn(id: &str, at: &str) -> String {
        json!({ "type": "assistant", "sessionId": "s", "timestamp": at,
            "message": { "id": id, "usage": { "input_tokens": 10, "output_tokens": 0 } } })
        .to_string()
    }

    #[test]
    fn a_tail_cut_inside_the_window_is_partial() {
        // Every record is inside the 5-hour window: cutting the file loses
        // some of them.
        let dir = TempDir::new("claude-cut-inside");
        let lines: Vec<String> = (0..40)
            .map(|index| turn(&format!("m{index}"), "2026-11-23T17:00:00Z"))
            .collect();
        dir.write("projects/p/a.jsonl", &lines.join("\n"));
        let activity = scan_activity(&dir.path().join("projects"), now(), 1024).unwrap();
        assert!(activity.tokens < 400, "only the tail was read");
        assert!(activity.partial);
    }

    #[test]
    fn a_tail_that_still_covers_the_window_is_complete() {
        // Old records first, then the window: the cut only drops old ones.
        let dir = TempDir::new("claude-cut-before");
        let mut lines: Vec<String> = (0..40)
            .map(|index| turn(&format!("old{index}"), "2026-11-23T08:00:00Z"))
            .collect();
        lines.push(turn("new", "2026-11-23T17:30:00Z"));
        dir.write("projects/p/a.jsonl", &lines.join("\n"));
        let activity = scan_activity(&dir.path().join("projects"), now(), 1024).unwrap();
        assert_eq!(activity.tokens, 10);
        assert!(!activity.partial);
    }

    #[test]
    fn an_unreadable_recent_transcript_is_partial() {
        // A recent entry that cannot be opened as a file (here a directory
        // with the transcript extension, as a locked file would fail).
        let dir = TempDir::new("claude-unreadable");
        dir.write(
            "projects/p/a.jsonl",
            &turn("readable", "2026-11-23T17:30:00Z"),
        );
        std::fs::create_dir_all(dir.path().join("projects/p/b.jsonl")).unwrap();
        let activity = scan_activity(&dir.path().join("projects"), now(), TRANSCRIPT_TAIL).unwrap();
        assert_eq!(activity.tokens, 10);
        assert!(
            activity.partial,
            "the unread transcript may hold more turns"
        );
    }
}

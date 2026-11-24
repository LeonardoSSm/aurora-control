//! xAI Grok adapter.
//!
//! Source investigated: Grok Build, xAI's official coding CLI (open source,
//! github.com/xai-org/grok-build). Its `/usage` view reads the account's
//! Grok Build credit allowance from
//! `GET https://cli-chat-proxy.grok.com/v1/billing?format=credits`
//! (`crates/codegen/xai-grok-shell/src/extensions/billing.rs`), authenticated
//! with the session token that `grok login` stores in `~/.grok/auth.json`.
//! The response carries `config.creditUsagePercent` (0–100) and
//! `config.currentPeriod` (`USAGE_PERIOD_TYPE_WEEKLY` or monthly, with start
//! and end). That allowance belongs to the user's grok.com account (SuperGrok
//! and similar plans); it is not an xAI API key limit.
//!
//! The endpoint is internal to Grok Build and undocumented. xAI's terms of
//! service could not be read during the investigation (HTTP 403), so their
//! position on third-party software using this token is unverified. Until
//! the owner verifies them, [`LIVE_QUERY_AUTHORIZED`] keeps the query off: the
//! adapter reads the local sign-in and reports `sourceBlocked: termsUnverified`
//! without contacting xAI. The parser below is complete and tested, so enabling
//! the source is a one-line, reviewable change.
//!
//! API-key logins (`auth_mode: api_key`, or `XAI_API_KEY` only) are
//! incompatible accounts: xAI API limits and credits belong to a team and are
//! only exposed by the Management API with a management key, which this app
//! does not ask for.
//!
//! The adapter never refreshes, writes or copies the token: Grok Build renews
//! it in the background and rewrites `auth.json` itself.

use std::path::{Path, PathBuf};

use chrono::{DateTime, TimeDelta, Utc};
use serde_json::Value;

use super::adapter::{
    FetchError, Fetched, Outcome, UsageAdapter, mask_account, percent, rfc3339, schema_anomaly,
};
use super::http::{Transport, Ureq, check_status};
use super::local::{ReadError, env_dir, home_dir, read_limited};
use super::model::{
    BlockReason, DiagnosticRow, DiagnosticState, Origin, Provider, Quota, QuotaKind,
};
use super::session::Session;
use super::time;

/// Off until xAI's terms are verified for third-party use of the Grok Build
/// session (see `docs/provider-compatibility.md`).
pub const LIVE_QUERY_AUTHORIZED: bool = false;

const BILLING_URL: &str = "https://cli-chat-proxy.grok.com/v1/billing?format=credits";
/// Token type header Grok Build sends with its session token.
const TOKEN_HEADER: (&str, &str) = ("X-XAI-Token-Auth", "xai-grok-cli");
const MAX_CREDENTIALS: u64 = 256 * 1024;
/// Grok Build's lifetime for credentials without a server expiry.
const TOKEN_TTL: TimeDelta = TimeDelta::days(30);
const EXPIRY_MARGIN: TimeDelta = TimeDelta::seconds(30);
const MONTH_MINUTES: u32 = 30 * 24 * 60;
const WEEK_MINUTES: u32 = 7 * 24 * 60;

pub struct Grok<T: Transport> {
    home: Option<PathBuf>,
    transport: T,
    live: bool,
}

impl Grok<Ureq> {
    pub fn detect() -> Self {
        let home = env_dir("GROK_HOME").or_else(|| home_dir().map(|home| home.join(".grok")));
        Self::new(home, Ureq::new(), LIVE_QUERY_AUTHORIZED)
    }
}

impl<T: Transport> Grok<T> {
    pub fn new(home: Option<PathBuf>, transport: T, live: bool) -> Self {
        Self {
            home,
            transport,
            live,
        }
    }
}

/// A first-party grok.com session from `auth.json`.
struct SignIn {
    key: String,
    user_id: String,
    session: Session,
    account: Option<String>,
    team: Option<String>,
}

/// Why no usable session was found, and what it proves.
struct Failure {
    error: FetchError,
    session: Session,
}

impl Failure {
    fn signed_out(error: FetchError) -> Self {
        Self {
            error,
            session: Session::SignedOut,
        }
    }

    fn unverified(error: FetchError) -> Self {
        Self {
            error,
            session: Session::Unverified,
        }
    }
}

fn field(value: &Value, key: &str) -> Option<String> {
    value[key]
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

/// The credential was issued by xAI's own identity provider.
fn xai_issuer(entry: &Value) -> bool {
    entry["oidc_issuer"]
        .as_str()
        .is_some_and(|issuer| issuer == "https://auth.x.ai" || issuer.ends_with(".x.ai"))
}

fn expired(entry: &Value, now: DateTime<Utc>) -> bool {
    let parse = |key: &str| {
        entry[key]
            .as_str()
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .map(|time| time.with_timezone(&Utc))
    };
    let expires = parse("expires_at")
        .or_else(|| parse("create_time").and_then(|created| created.checked_add_signed(TOKEN_TTL)));
    // Without either time the age is unknown: let the server decide.
    expires.is_some_and(|expires| time::plus(now, EXPIRY_MARGIN) >= expires)
}

/// Reads `auth.json`: an object keyed by `<issuer>::<client>`, each value a
/// stored credential. Only first-party OIDC or external sessions issued by
/// x.ai carry the grok.com allowance.
fn read_sign_in(home: &Path, now: DateTime<Utc>) -> Result<SignIn, Failure> {
    let text = match read_limited(&home.join("auth.json"), MAX_CREDENTIALS) {
        Ok(text) => text,
        Err(ReadError::Missing) => return Err(Failure::signed_out(FetchError::AuthMissing)),
        Err(ReadError::TooLarge | ReadError::Unreadable) => {
            return Err(Failure::unverified(FetchError::CredentialsInvalid));
        }
    };
    let value: Value = serde_json::from_str(&text)
        .map_err(|_| Failure::unverified(FetchError::CredentialsInvalid))?;
    let Some(entries) = value.as_object() else {
        return Err(Failure::unverified(FetchError::CredentialsInvalid));
    };
    let mut api_key = false;
    let mut chosen = None;
    for entry in entries.values().filter(|entry| entry.is_object()) {
        match entry["auth_mode"].as_str() {
            Some("oidc" | "external") if xai_issuer(entry) => {
                chosen = Some(entry);
                break;
            }
            Some("api_key") => api_key = true,
            _ => {}
        }
    }
    let Some(entry) = chosen else {
        return Err(Failure::signed_out(if api_key {
            FetchError::Unsupported
        } else {
            FetchError::AuthMissing
        }));
    };
    let (Some(key), Some(user_id)) = (field(entry, "key"), field(entry, "user_id")) else {
        return Err(Failure::unverified(FetchError::CredentialsInvalid));
    };
    let team = field(entry, "team_id");
    let session = Session::account(&["grok", &user_id, team.as_deref().unwrap_or("")], &key);
    if expired(entry, now) {
        return Err(Failure {
            error: FetchError::AuthExpired,
            session,
        });
    }
    Ok(SignIn {
        key,
        user_id,
        session,
        account: field(entry, "email").map(|email| mask_account(&email)),
        team: field(entry, "team_name").and_then(|name| super::adapter::display_label(&name)),
    })
}

/// A cent amount (`{"val": "123"}` or `{"val": 123}`); proto3 omits zero, so
/// `{}` is 0.
fn cents(value: &Value) -> Option<f64> {
    if !value.is_object() {
        return None;
    }
    match &value["val"] {
        Value::Null => Some(0.0),
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse::<f64>().ok(),
        _ => None,
    }
    .filter(|amount| amount.is_finite())
}

fn moment(value: &Value) -> Option<DateTime<Utc>> {
    value
        .as_str()
        .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
        .map(|time| time.with_timezone(&Utc))
}

/// Label and length of the allowance period.
fn period(
    kind: Option<&str>,
    start: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
) -> (String, Option<u32>) {
    let measured = start.zip(end).and_then(|(start, end)| {
        let seconds = end.signed_duration_since(start).num_seconds();
        time::window_minutes(seconds as f64)
    });
    match kind {
        Some("USAGE_PERIOD_TYPE_WEEKLY") => {
            ("Créditos semanais".into(), measured.or(Some(WEEK_MINUTES)))
        }
        Some("USAGE_PERIOD_TYPE_MONTHLY") => {
            ("Créditos mensais".into(), measured.or(Some(MONTH_MINUTES)))
        }
        _ => ("Créditos do período".into(), measured),
    }
}

/// Normalizes the Grok Build credits response. The included allowance is the
/// ring's metric; on-demand spending against its cap is listed as credits.
/// `creditUsagePercent` wins over the deprecated `used`/`monthlyLimit`.
pub fn parse_billing(body: &str, now: DateTime<Utc>) -> Result<Vec<Quota>, FetchError> {
    let value: Value =
        serde_json::from_str(body).map_err(|_| FetchError::Malformed("billing json".into()))?;
    let config = value
        .get("config")
        .filter(|config| config.is_object())
        .ok_or_else(|| FetchError::Malformed("billing config".into()))?;

    let used = match config
        .get("creditUsagePercent")
        .filter(|raw| !raw.is_null())
    {
        Some(raw) => {
            let checked = raw.as_f64().and_then(percent);
            if checked.is_none() {
                schema_anomaly(Provider::Grok, "config.creditUsagePercent outside 0-100");
            }
            checked
        }
        None => match (cents(&config["used"]), cents(&config["monthlyLimit"])) {
            (Some(used), Some(limit)) if limit > 0.0 && used >= 0.0 => {
                percent((used / limit * 100.0).min(100.0))
            }
            _ => None,
        },
    };
    let Some(used) = used else {
        return Err(FetchError::Malformed("no credit usage".into()));
    };

    let current = &config["currentPeriod"];
    let (start, end, kind) = if current.is_object() {
        (
            moment(&current["start"]),
            moment(&current["end"]),
            current["type"].as_str(),
        )
    } else {
        (
            moment(&config["billingPeriodStart"]),
            moment(&config["billingPeriodEnd"]),
            None,
        )
    };
    let (label, minutes) = period(kind, start, end);
    let resets_at = time::plausible_reset(end, now);
    let mut quotas = vec![Quota::window(
        "grok-credits",
        label,
        used,
        minutes,
        resets_at.map(rfc3339),
    )];

    if let (Some(spent), Some(cap)) = (
        cents(&config["onDemandUsed"]),
        cents(&config["onDemandCap"]),
    ) && cap > 0.0
        && spent >= 0.0
        && let Some(share) = percent((spent / cap * 100.0).min(100.0))
    {
        quotas.push(Quota {
            kind: QuotaKind::Credits,
            eligible: false,
            ..Quota::window("grok-onDemand", "Uso sob demanda", share, None, None)
        });
    }
    Ok(quotas)
}

impl<T: Transport> Grok<T> {
    fn query(&self, sign_in: &SignIn, now: DateTime<Utc>) -> Result<Fetched, FetchError> {
        let authorization = format!("Bearer {}", sign_in.key);
        let response = check_status(
            self.transport.get(
                BILLING_URL,
                &[
                    ("Authorization", &authorization),
                    TOKEN_HEADER,
                    ("x-userid", &sign_in.user_id),
                    ("Accept", "application/json"),
                ],
            ),
            now,
        )?;
        let quotas = parse_billing(&response.body, now)?;
        Ok(Fetched {
            identity: sign_in.session.identity(),
            account: sign_in.account.clone(),
            plan: None,
            origin: Origin::Api,
            quotas,
            observed_at: now,
        })
    }
}

impl<T: Transport> UsageAdapter for Grok<T> {
    fn provider(&self) -> Provider {
        Provider::Grok
    }

    fn session(&self, now: DateTime<Utc>) -> Option<Session> {
        Some(match &self.home {
            Some(home) => match read_sign_in(home, now) {
                Ok(sign_in) => sign_in.session,
                Err(failure) => failure.session,
            },
            None => Session::SignedOut,
        })
    }

    fn fetch(&self, now: DateTime<Utc>) -> Outcome {
        let Some(home) = &self.home else {
            return Outcome::signed_out(FetchError::AuthMissing);
        };
        let sign_in = match read_sign_in(home, now) {
            Ok(sign_in) => sign_in,
            Err(failure) => {
                return Outcome {
                    session: failure.session,
                    ..Outcome::failed(failure.error)
                };
            }
        };
        let result = if self.live {
            self.query(&sign_in, now)
        } else {
            Err(FetchError::SourceBlocked(BlockReason::TermsUnverified))
        };
        Outcome {
            session: sign_in.session,
            account: sign_in.account.clone().or_else(|| sign_in.team.clone()),
            plan: None,
            result: Some(result),
            ..Outcome::default()
        }
    }

    fn diagnostics(&self, now: DateTime<Utc>) -> Option<Vec<DiagnosticRow>> {
        let Some(home) = &self.home else {
            return Some(vec![DiagnosticRow::new(
                "Pasta do Grok Build",
                DiagnosticState::Error,
                "Pasta pessoal do usuário não encontrada",
            )]);
        };
        let mut rows = Vec::new();
        match read_sign_in(home, now) {
            Ok(sign_in) => {
                rows.push(DiagnosticRow::new(
                    "Sessão",
                    DiagnosticState::Ok,
                    "Login do Grok Build com conta grok.com",
                ));
                if let Some(account) = sign_in.account.or(sign_in.team) {
                    rows.push(DiagnosticRow::new("Conta", DiagnosticState::Ok, account));
                }
            }
            Err(failure) => rows.push(match failure.error {
                FetchError::AuthMissing => DiagnosticRow::new(
                    "Sessão",
                    DiagnosticState::Info,
                    "Nenhum login encontrado. Execute “grok login”.",
                ),
                FetchError::Unsupported => DiagnosticRow::new(
                    "Sessão",
                    DiagnosticState::Warning,
                    "Login por chave de API: limites de API não são cotas de assinatura",
                ),
                FetchError::AuthExpired => DiagnosticRow::new(
                    "Sessão",
                    DiagnosticState::Warning,
                    "Expirada. Abra o Grok Build para renová-la.",
                ),
                _ => DiagnosticRow::new(
                    "Sessão",
                    DiagnosticState::Warning,
                    "Arquivo de login ilegível ou incompleto",
                ),
            }),
        }
        rows.push(if self.live {
            DiagnosticRow::new(
                "Fonte de cotas",
                DiagnosticState::Info,
                "Créditos do Grok Build (endpoint interno)",
            )
        } else {
            DiagnosticRow::new(
                "Fonte de cotas",
                DiagnosticState::Warning,
                "Desativada até a verificação dos termos de uso da xAI",
            )
        });
        Some(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::http::{Response, TransportError};
    use crate::usage::local::testing::TempDir;
    use std::sync::Mutex;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-11-24T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    /// Replays one response and records the request, never the token.
    struct Scripted {
        reply: Result<Response, TransportError>,
        calls: Mutex<Vec<(String, Vec<String>)>>,
    }

    impl Scripted {
        fn ok(body: &str) -> Self {
            Self::status(200, None, body)
        }

        fn status(status: u16, retry_after: Option<&str>, body: &str) -> Self {
            Self {
                reply: Ok(Response {
                    status,
                    retry_after: retry_after.map(str::to_owned),
                    body: body.into(),
                }),
                calls: Mutex::new(Vec::new()),
            }
        }

        fn failing(error: TransportError) -> Self {
            Self {
                reply: Err(error),
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    impl Transport for Scripted {
        fn get(&self, url: &str, headers: &[(&str, &str)]) -> Result<Response, TransportError> {
            self.calls.lock().unwrap().push((
                url.into(),
                headers.iter().map(|(name, _)| (*name).to_owned()).collect(),
            ));
            self.reply.clone()
        }
    }

    fn session_entry(user: &str, expires_at: &str) -> String {
        format!(
            r#"{{
  "https://auth.x.ai::b1a00492": {{
    "key": "sentinel-session-{user}",
    "auth_mode": "oidc",
    "create_time": "2026-11-01T00:00:00Z",
    "user_id": "{user}",
    "email": "{user}@example.com",
    "expires_at": "{expires_at}",
    "refresh_token": "sentinel-refresh",
    "oidc_issuer": "https://auth.x.ai",
    "oidc_client_id": "b1a00492",
    "team_id": "team-1",
    "team_name": "Equipe"
  }}
}}"#
        )
    }

    fn signed_in(user: &str) -> TempDir {
        let dir = TempDir::new("grok");
        dir.write("auth.json", &session_entry(user, "2026-11-25T00:00:00Z"));
        dir
    }

    const WEEKLY: &str = r#"{
      "config": {
        "creditUsagePercent": 41.6,
        "currentPeriod": {"type": "USAGE_PERIOD_TYPE_WEEKLY", "start": "2026-11-20T00:00:00Z", "end": "2026-11-27T00:00:00Z"},
        "monthlyLimit": {"val": "5000"},
        "used": {"val": "100"},
        "onDemandCap": {"val": "2000"},
        "onDemandUsed": {"val": "500"},
        "prepaidBalance": {},
        "isUnifiedBillingUser": true,
        "history": [{"billingCycle": {"year": 2026, "month": 10}, "totalUsed": {"val": "1"}}]
      }
    }"#;

    fn result(outcome: &Outcome) -> &Result<Fetched, FetchError> {
        outcome.result.as_ref().unwrap()
    }

    #[test]
    fn without_verified_terms_the_session_is_read_but_xai_is_never_contacted() {
        let dir = signed_in("u1");
        let grok = Grok::new(Some(dir.path().into()), Scripted::ok(WEEKLY), false);
        let outcome = grok.fetch(now());
        assert_eq!(
            result(&outcome),
            &Err(FetchError::SourceBlocked(BlockReason::TermsUnverified))
        );
        assert_eq!(outcome.account.as_deref(), Some("u•••@example.com"));
        assert!(outcome.session.identity().is_some());
        assert!(
            grok.transport.calls.lock().unwrap().is_empty(),
            "no request"
        );
        assert!(!format!("{outcome:?}").contains("sentinel"));
        // The production switch stays off.
        const { assert!(!LIVE_QUERY_AUTHORIZED) };
    }

    #[test]
    fn a_valid_response_yields_the_weekly_allowance_and_on_demand_credits() {
        let dir = signed_in("u1");
        let grok = Grok::new(Some(dir.path().into()), Scripted::ok(WEEKLY), true);
        let outcome = grok.fetch(now());
        let fetched = result(&outcome).as_ref().unwrap();
        assert_eq!(fetched.origin, Origin::Api);
        assert_eq!(fetched.identity, outcome.session.identity());
        let credits = &fetched.quotas[0];
        assert_eq!(
            (
                credits.id.as_str(),
                credits.label.as_str(),
                credits.percent_used
            ),
            ("grok-credits", "Créditos semanais", 42)
        );
        assert_eq!(credits.duration_minutes, Some(WEEK_MINUTES));
        assert_eq!(credits.resets_at.as_deref(), Some("2026-11-27T00:00:00Z"));
        assert!(credits.eligible);
        let demand = &fetched.quotas[1];
        assert_eq!((demand.kind, demand.percent_used), (QuotaKind::Credits, 25));
        // The request went to the documented host with the session headers;
        // header values (the token) are never recorded.
        let calls = grok.transport.calls.lock().unwrap();
        assert_eq!(calls[0].0, BILLING_URL);
        assert!(calls[0].1.iter().any(|name| name == "Authorization"));
        assert!(calls[0].1.iter().any(|name| name == "X-XAI-Token-Auth"));
    }

    #[test]
    fn the_deprecated_shape_and_monthly_periods_are_understood() {
        let legacy = r#"{"config": {"monthlyLimit": {"val": "4000"}, "used": {"val": "1000"},
            "billingPeriodStart": "2026-11-01T00:00:00Z", "billingPeriodEnd": "2026-12-01T00:00:00Z"}}"#;
        let quotas = parse_billing(legacy, now()).unwrap();
        assert_eq!(quotas[0].percent_used, 25);
        assert_eq!(quotas[0].label, "Créditos do período");
        assert_eq!(quotas[0].duration_minutes, Some(30 * 24 * 60));
        let monthly = r#"{"config": {"creditUsagePercent": 100,
            "currentPeriod": {"type": "USAGE_PERIOD_TYPE_MONTHLY", "end": "2026-12-01T00:00:00Z"}}}"#;
        let quotas = parse_billing(monthly, now()).unwrap();
        assert_eq!(
            (quotas[0].label.as_str(), quotas[0].percent_used),
            ("Créditos mensais", 100)
        );
        // Zero-valued cents arrive as `{}`.
        let zero = r#"{"config": {"used": {}, "monthlyLimit": {"val": 100}}}"#;
        assert_eq!(parse_billing(zero, now()).unwrap()[0].percent_used, 0);
    }

    #[test]
    fn incomplete_malformed_or_out_of_range_responses_are_not_readings() {
        for body in [
            "not json",
            "[]",
            "{}",
            r#"{"config": null}"#,
            r#"{"config": {}}"#,
            r#"{"config": {"creditUsagePercent": 140}}"#,
            r#"{"config": {"creditUsagePercent": -3}}"#,
            r#"{"config": {"creditUsagePercent": "x"}}"#,
            r#"{"config": {"used": {"val": "10"}, "monthlyLimit": {"val": "0"}}}"#,
            r#"{"config": {"used": {"val": "NaN"}, "monthlyLimit": {"val": "10"}}}"#,
        ] {
            assert!(
                matches!(parse_billing(body, now()), Err(FetchError::Malformed(_))),
                "{body}"
            );
        }
        // An implausible renewal is dropped, the reading kept.
        let far = r#"{"config": {"creditUsagePercent": 5,
            "currentPeriod": {"type": "USAGE_PERIOD_TYPE_WEEKLY", "end": "2099-01-01T00:00:00Z"}}}"#;
        let quotas = parse_billing(far, now()).unwrap();
        assert_eq!(quotas[0].resets_at, None);
        assert_eq!(quotas[0].duration_minutes, Some(WEEK_MINUTES));
    }

    #[test]
    fn network_rate_limits_and_server_errors_are_classified() {
        let cases = [
            (
                Scripted::failing(TransportError::Network),
                FetchError::Network,
            ),
            (
                Scripted::failing(TransportError::Timeout),
                FetchError::Timeout,
            ),
            (Scripted::status(401, None, ""), FetchError::AuthRejected),
            (
                Scripted::status(429, Some("120"), ""),
                FetchError::RateLimited {
                    retry_after: Some(std::time::Duration::from_secs(120)),
                },
            ),
            (
                Scripted::status(503, Some("300"), ""),
                FetchError::Http {
                    status: 503,
                    retry_after: Some(std::time::Duration::from_secs(300)),
                },
            ),
        ];
        for (transport, expected) in cases {
            let dir = signed_in("u1");
            let grok = Grok::new(Some(dir.path().into()), transport, true);
            assert_eq!(result(&grok.fetch(now())), &Err(expected));
        }
    }

    #[test]
    fn missing_expired_api_key_and_invalid_sign_ins() {
        let dir = TempDir::new("grok-none");
        let grok = Grok::new(Some(dir.path().into()), Scripted::ok(WEEKLY), true);
        let outcome = grok.fetch(now());
        assert_eq!(result(&outcome), &Err(FetchError::AuthMissing));
        assert_eq!(outcome.session, Session::SignedOut);

        dir.write("auth.json", &session_entry("u1", "2026-11-24T11:00:00Z"));
        let outcome = grok.fetch(now());
        assert_eq!(result(&outcome), &Err(FetchError::AuthExpired));
        assert!(
            outcome.session.identity().is_some(),
            "an expired session keeps its account"
        );

        // No `expires_at`: 30 days after creation.
        dir.write(
            "auth.json",
            r#"{"s": {"key": "k", "auth_mode": "oidc", "user_id": "u", "oidc_issuer": "https://auth.x.ai",
                "create_time": "2026-10-01T00:00:00Z"}}"#,
        );
        assert_eq!(result(&grok.fetch(now())), &Err(FetchError::AuthExpired));

        dir.write(
            "auth.json",
            r#"{"api-key": {"key": "xai-sentinel", "auth_mode": "api_key", "user_id": "u", "create_time": "2026-11-01T00:00:00Z"}}"#,
        );
        let outcome = grok.fetch(now());
        assert_eq!(result(&outcome), &Err(FetchError::Unsupported));
        assert_eq!(outcome.session, Session::SignedOut);

        // A third-party issuer is not a grok.com account.
        dir.write(
            "auth.json",
            r#"{"corp": {"key": "k", "auth_mode": "oidc", "user_id": "u", "oidc_issuer": "https://login.example.com"}}"#,
        );
        assert_eq!(result(&grok.fetch(now())), &Err(FetchError::AuthMissing));

        for broken in [
            "{\"a\":",
            "[]",
            r#"{"s": {"auth_mode": "oidc", "oidc_issuer": "https://auth.x.ai"}}"#,
        ] {
            dir.write("auth.json", broken);
            let outcome = grok.fetch(now());
            assert_eq!(
                result(&outcome),
                &Err(FetchError::CredentialsInvalid),
                "{broken}"
            );
            assert_eq!(outcome.session, Session::Unverified);
        }
        assert!(grok.transport.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn switching_accounts_changes_the_identity_and_renewal_keeps_it() {
        let dir = signed_in("u1");
        let grok = Grok::new(Some(dir.path().into()), Scripted::ok(WEEKLY), false);
        let first = grok.session(now()).unwrap();
        dir.write(
            "auth.json",
            &session_entry("u1", "2026-11-26T00:00:00Z").replace("sentinel-session-u1", "renewed"),
        );
        let renewed = grok.session(now()).unwrap();
        assert_eq!(first.identity(), renewed.identity());
        assert_ne!(first, renewed);
        dir.write("auth.json", &session_entry("u2", "2026-11-25T00:00:00Z"));
        assert_ne!(grok.session(now()).unwrap().identity(), first.identity());
    }

    #[test]
    fn diagnostics_say_why_the_source_is_off() {
        let dir = signed_in("u1");
        let rows = Grok::new(Some(dir.path().into()), Scripted::ok(WEEKLY), false)
            .diagnostics(now())
            .unwrap();
        let text = format!("{rows:?}");
        assert!(text.contains("termos de uso da xAI"));
        assert!(
            !text.contains("sentinel") && !text.contains("u1@"),
            "{text}"
        );
    }
}

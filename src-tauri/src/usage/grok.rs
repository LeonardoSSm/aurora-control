//! Grok / Cursor adapter: the weekly allowance of the user's xAI
//! subscription, which Grok and Cursor share.
//!
//! Source: Grok Build, xAI's official coding CLI (open source,
//! github.com/xai-org/grok-build). Its `/usage` view draws the account's
//! credits config, fetched on start-up, after every prompt, on `/usage` and
//! on its own poll. Each fetch is also written by the CLI itself to its local
//! log (`$GROK_HOME/logs/unified.jsonl`, event `billing: fetched credits
//! config`). This adapter reads the newest such record ([`super::grok_log`]):
//! a passive, read-only use of a file the official client produces. It is a
//! local record, not a live query: its time is the time Grok Build fetched it,
//! and it ages like any other reading when Grok Build is not in use.
//!
//! Shared allowance: Cursor's documentation says paid plans include "a weekly
//! pool of usage for Cursor models, shared across Grok and Cursor", across
//! the editor, its CLI, Cloud Agents and the Grok apps. The record says
//! whether the account is on that unified billing (`isUnifiedBillingUser`);
//! only then is the window labelled as the shared allowance. Nothing is added
//! up: the pool is one number. Prepaid credits, on-demand spending and API
//! limits are different things and never feed the ring.
//!
//! Account: the record carries no account, but the process that wrote it
//! logged its signed-in user first. The record is shown only when that user is
//! the one signed in now (`auth.json`), so a record left by a previous account
//! is never presented as the current one, and alerts stay bound to a verified
//! account. The user id is compared in memory and never leaves this module.
//!
//! The endpoint behind `/usage`
//! (`GET https://cli-chat-proxy.grok.com/v1/billing?format=credits`) is internal
//! to Grok Build. xAI's terms could not be read (HTTP 403), so a query with the
//! CLI's session token stays off ([`LIVE_QUERY_AUTHORIZED`]); the parser and the
//! request are kept, tested, behind that switch. The ACP extension
//! `x.ai/billing` of `grok agent stdio` is not used either: it is private, not
//! advertised by `initialize`, and starts a whole agent per query.
//!
//! API-key logins (`auth_mode: api_key`) are incompatible accounts: xAI API
//! limits belong to a team, not to the subscription.
//!
//! The adapter never refreshes, writes or copies the token: Grok Build renews
//! it in the background and rewrites `auth.json` itself.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, TimeDelta, Utc};
use serde_json::Value;

use super::adapter::{
    FetchError, Fetched, Outcome, UsageAdapter, display_label, mask_account, percent, rfc3339,
    schema_anomaly,
};
use super::grok_log::{self, BillingRecord, Scan, Stamp};
use super::http::{Transport, Ureq, check_status};
use super::local::{ReadError, env_dir, home_dir, read_limited};
use super::model::{DiagnosticRow, DiagnosticState, Origin, Provider, Quota, QuotaKind};
use super::session::Session;
use super::time;

/// Off until xAI's terms are verified for third-party use of the Grok Build
/// session (see `docs/cycle-04/grok-cursor-research.md`). The local record
/// does not depend on it.
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
    /// The last scan of the local log, kept while the file is unchanged.
    log: Mutex<Option<(Stamp, Scan)>>,
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
            log: Mutex::new(None),
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
        team: field(entry, "team_name").and_then(|name| display_label(&name)),
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

/// Label and length of the allowance period. On unified billing the period
/// is the pool shared by Grok and Cursor.
fn period(
    kind: Option<&str>,
    unified: bool,
    start: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
) -> (String, Option<u32>) {
    let measured = start.zip(end).and_then(|(start, end)| {
        let seconds = end.signed_duration_since(start).num_seconds();
        time::window_minutes(seconds as f64)
    });
    match (kind, unified) {
        (Some("USAGE_PERIOD_TYPE_WEEKLY"), true) => (
            "Franquia semanal compartilhada".into(),
            measured.or(Some(WEEK_MINUTES)),
        ),
        (Some("USAGE_PERIOD_TYPE_WEEKLY"), false) => {
            ("Créditos semanais".into(), measured.or(Some(WEEK_MINUTES)))
        }
        (Some("USAGE_PERIOD_TYPE_MONTHLY"), true) => (
            "Franquia mensal compartilhada".into(),
            measured.or(Some(MONTH_MINUTES)),
        ),
        (Some("USAGE_PERIOD_TYPE_MONTHLY"), false) => {
            ("Créditos mensais".into(), measured.or(Some(MONTH_MINUTES)))
        }
        _ => ("Créditos do período".into(), measured),
    }
}

/// A normalized credits config.
#[derive(Debug, PartialEq)]
pub struct Billing {
    pub quotas: Vec<Quota>,
    /// The account is on the unified billing shared by Grok and Cursor.
    /// `None` when the response does not say.
    pub unified: Option<bool>,
    /// Display name of the subscription (for example "SuperGrok Lite").
    pub plan: Option<String>,
    /// End of the current period, as reported.
    pub period_end: Option<DateTime<Utc>>,
}

/// Normalizes the Grok Build credits response (`{config, onDemandEnabled,
/// subscriptionTier}`), from the endpoint or from the CLI's own log record.
/// The included allowance is the ring's metric; on-demand spending against
/// its cap is listed as credits. `creditUsagePercent` wins over the
/// deprecated `used`/`monthlyLimit`. Renewal times are checked against
/// `observed`, the moment the numbers were fetched.
pub fn parse_response(value: &Value, observed: DateTime<Utc>) -> Result<Billing, FetchError> {
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
    // An absent figure is not 0 %: without it there is no reading.
    let Some(used) = used else {
        return Err(FetchError::Malformed("no credit usage".into()));
    };

    let unified = config["isUnifiedBillingUser"].as_bool();
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
    let (label, minutes) = period(kind, unified == Some(true), start, end);
    let resets_at = time::plausible_reset(end, observed);
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
    Ok(Billing {
        quotas,
        unified,
        plan: value["subscriptionTier"].as_str().and_then(display_label),
        period_end: end,
    })
}

/// [`parse_response`] for a response body.
#[cfg(test)]
pub fn parse_billing(body: &str, now: DateTime<Utc>) -> Result<Vec<Quota>, FetchError> {
    let value: Value =
        serde_json::from_str(body).map_err(|_| FetchError::Malformed("billing json".into()))?;
    parse_response(&value, now).map(|billing| billing.quotas)
}

/// Why the local record gives no reading for the signed-in account.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Gap {
    /// No log file, or no billing record in it yet.
    NoRecord,
    /// The log could not be read.
    Unreadable,
    /// The record's writer could not be tied to an account.
    Unattributed,
    /// The newest record belongs to another account.
    OtherAccount,
    /// The record's period is over: its numbers describe a past week.
    Expired,
    /// The record has no trustworthy numbers.
    Invalid,
}

impl Gap {
    fn error(self) -> FetchError {
        match self {
            Self::Invalid => FetchError::Malformed("grok log record".into()),
            _ => FetchError::AwaitingClient,
        }
    }

    fn text(self) -> &'static str {
        match self {
            Self::NoRecord => "Nenhum registro de cobrança ainda. Abra o Grok Build ou use /usage.",
            Self::Unreadable => "O registro local do Grok Build não pôde ser lido",
            Self::Unattributed => {
                "O registro mais recente não identifica a conta. Abra o Grok Build para gerar outro."
            }
            Self::OtherAccount => {
                "O registro mais recente é de outra conta. Abra o Grok Build com a conta atual."
            }
            Self::Expired => {
                "O registro mais recente é de um período encerrado. Abra o Grok Build para atualizá-lo."
            }
            Self::Invalid => "O registro mais recente não tem um percentual válido",
        }
    }
}

/// The local record's reading for the signed-in account.
struct LogReading {
    fetched: Fetched,
    unified: Option<bool>,
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
        let value: Value = serde_json::from_str(&response.body)
            .map_err(|_| FetchError::Malformed("billing json".into()))?;
        let billing = parse_response(&value, now)?;
        Ok(Fetched {
            identity: sign_in.session.identity(),
            account: sign_in.account.clone(),
            plan: billing.plan,
            origin: Origin::Api,
            quotas: billing.quotas,
            observed_at: now,
        })
    }

    /// The newest billing record, scanned again only when the file changed.
    fn scan(&self, home: &Path, now: DateTime<Utc>) -> Scan {
        let path = home.join("logs").join("unified.jsonl");
        let Some(stamp) = grok_log::stamp(&path) else {
            *crate::sync::lock(&self.log) = None;
            return Scan::Missing;
        };
        let mut cache = crate::sync::lock(&self.log);
        if let Some((seen, scan)) = cache.as_ref()
            && *seen == stamp
        {
            return scan.clone();
        }
        let scan = grok_log::latest(&path, now);
        *cache = Some((stamp, scan.clone()));
        scan
    }

    fn read_log(
        &self,
        home: &Path,
        sign_in: &SignIn,
        now: DateTime<Utc>,
    ) -> Result<LogReading, (Gap, Option<DateTime<Utc>>)> {
        let record: BillingRecord = match self.scan(home, now) {
            Scan::Found(record) => record,
            Scan::Missing | Scan::Empty { .. } => return Err((Gap::NoRecord, None)),
            Scan::Unreadable => return Err((Gap::Unreadable, None)),
        };
        let at = Some(record.observed_at);
        match record.user_id.as_deref() {
            None => return Err((Gap::Unattributed, at)),
            Some(user) if user != sign_in.user_id => return Err((Gap::OtherAccount, at)),
            Some(_) => {}
        }
        let billing =
            parse_response(&record.response, record.observed_at).map_err(|_| (Gap::Invalid, at))?;
        // A period that is over says nothing about the current one.
        if billing.period_end.is_some_and(|end| end <= now) {
            return Err((Gap::Expired, at));
        }
        Ok(LogReading {
            unified: billing.unified,
            fetched: Fetched {
                identity: sign_in.session.identity(),
                account: sign_in.account.clone(),
                plan: billing.plan,
                origin: Origin::ClientLog,
                quotas: billing.quotas,
                observed_at: record.observed_at,
            },
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
            self.read_log(home, &sign_in, now)
                .map(|reading| reading.fetched)
                .map_err(|(gap, _)| gap.error())
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
        let sign_in = match read_sign_in(home, now) {
            Ok(sign_in) => {
                rows.push(DiagnosticRow::new(
                    "Sessão",
                    DiagnosticState::Ok,
                    "Login do Grok Build com conta grok.com",
                ));
                if let Some(account) = sign_in.account.clone().or(sign_in.team.clone()) {
                    rows.push(DiagnosticRow::new("Conta", DiagnosticState::Ok, account));
                }
                Some(sign_in)
            }
            Err(failure) => {
                rows.push(match failure.error {
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
                });
                None
            }
        };
        rows.push(DiagnosticRow::new(
            "Fonte de cotas",
            DiagnosticState::Info,
            if self.live {
                "Créditos do Grok Build (endpoint interno)"
            } else {
                "Registro local do Grok Build (logs/unified.jsonl), escrito pelo próprio CLI a cada consulta dele"
            },
        ));
        if let (false, Some(sign_in)) = (self.live, &sign_in) {
            match self.read_log(home, sign_in, now) {
                Ok(reading) => {
                    rows.push(DiagnosticRow {
                        at: Some(rfc3339(reading.fetched.observed_at)),
                        ..DiagnosticRow::new(
                            "Último registro",
                            DiagnosticState::Ok,
                            "Franquia lida pelo Grok Build para a conta atual",
                        )
                    });
                    rows.push(match reading.unified {
                        Some(true) => DiagnosticRow::new(
                            "Franquia",
                            DiagnosticState::Ok,
                            "Compartilhada entre Grok e Cursor (cobrança unificada da assinatura)",
                        ),
                        Some(false) => DiagnosticRow::new(
                            "Franquia",
                            DiagnosticState::Info,
                            "Só do Grok: a conta não usa a cobrança unificada, e o Cursor não entra nesta cota",
                        ),
                        None => DiagnosticRow::new(
                            "Franquia",
                            DiagnosticState::Info,
                            "O Grok Build não informou se a cobrança é unificada com o Cursor",
                        ),
                    });
                }
                Err((gap, at)) => rows.push(DiagnosticRow {
                    at: at.map(rfc3339),
                    ..DiagnosticRow::new(
                        "Último registro",
                        if gap == Gap::Invalid || gap == Gap::Unreadable {
                            DiagnosticState::Warning
                        } else {
                            DiagnosticState::Info
                        },
                        gap.text(),
                    )
                }),
            }
        }
        rows.push(DiagnosticRow::new(
            "Consulta direta",
            DiagnosticState::Info,
            if self.live {
                "Ativa"
            } else {
                "Desativada: os termos de uso da xAI não puderam ser verificados"
            },
        ));
        Some(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::http::{Response, TransportError};
    use crate::usage::local::testing::TempDir;

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

    /// One billing record and its writer's identity, as Grok Build 1.0.92
    /// logged them on 2026-11-24 (user id replaced).
    fn log_lines(user: &str, ts: &str, percent: f64, unified: bool, end: &str) -> String {
        format!(
            concat!(
                r#"{{"ts":"2026-11-24T05:04:36.000Z","src":"shell","pid":24352,"ver":"1.0.92","lvl":"info","msg":"auth init user_info check","ctx":{{"user_id":"{user}","needs_user_info":false,"key_prefix":"sentinel-prefix","rt_prefix":"sentinel-rt"}}}}"#,
                "\n",
                r#"{{"ts":"{ts}","src":"shell","pid":24352,"ver":"1.0.92","lvl":"info","msg":"billing: fetched credits config","ctx":{{"config":{{"creditUsagePercent":{percent},"currentPeriod":{{"type":"USAGE_PERIOD_TYPE_WEEKLY","start":"2026-11-20T03:30:32.116+00:00","end":"{end}"}},"prepaidBalance":{{"val":0}},"isUnifiedBillingUser":{unified},"billingPeriodStart":"2026-11-20T03:30:32.116+00:00","billingPeriodEnd":"{end}","historyLen":0}},"onDemandEnabled":null,"subscriptionTier":"SuperGrok Lite"}}}}"#,
                "\n"
            ),
            user = user,
            ts = ts,
            percent = percent,
            unified = unified,
            end = end
        )
    }

    const END: &str = "2026-11-27T03:30:32.116+00:00";

    fn with_log(user: &str, lines: &str) -> TempDir {
        let dir = signed_in(user);
        dir.write("logs/unified.jsonl", lines);
        dir
    }

    fn result(outcome: &Outcome) -> &Result<Fetched, FetchError> {
        outcome.result.as_ref().unwrap()
    }

    #[test]
    fn the_local_record_of_the_signed_in_account_is_the_shared_weekly_allowance() {
        let dir = with_log(
            "u1",
            &log_lines("u1", "2026-11-24T11:37:08.435Z", 100.0, true, END),
        );
        let grok = Grok::new(Some(dir.path().into()), Scripted::ok(WEEKLY), false);
        let outcome = grok.fetch(now());
        let fetched = result(&outcome).as_ref().unwrap();
        assert_eq!(fetched.origin, Origin::ClientLog);
        // Attributed to the signed-in account: alerts may use it.
        assert_eq!(fetched.identity, outcome.session.identity());
        assert!(fetched.identity.is_some());
        // The time is the CLI's fetch, not this read.
        assert_eq!(rfc3339(fetched.observed_at), "2026-11-24T11:37:08Z");
        assert_eq!(fetched.plan.as_deref(), Some("SuperGrok Lite"));
        let credits = &fetched.quotas[0];
        assert_eq!(
            (
                credits.id.as_str(),
                credits.label.as_str(),
                credits.percent_used
            ),
            ("grok-credits", "Franquia semanal compartilhada", 100)
        );
        assert_eq!(credits.duration_minutes, Some(WEEK_MINUTES));
        assert_eq!(credits.resets_at.as_deref(), Some("2026-11-27T03:30:32Z"));
        // Prepaid balance is not a quota, and nothing reached xAI.
        assert_eq!(fetched.quotas.len(), 1);
        assert!(grok.transport.calls.lock().unwrap().is_empty());
        assert!(!format!("{outcome:?}").contains("sentinel"));
        const { assert!(!LIVE_QUERY_AUTHORIZED) };
    }

    #[test]
    fn without_unified_billing_the_window_is_not_called_shared() {
        let dir = with_log(
            "u1",
            &log_lines("u1", "2026-11-24T11:00:00Z", 12.0, false, END),
        );
        let grok = Grok::new(Some(dir.path().into()), Scripted::ok(WEEKLY), false);
        let fetched = result(&grok.fetch(now())).clone().unwrap();
        assert_eq!(fetched.quotas[0].label, "Créditos semanais");
        let rows = format!("{:?}", grok.diagnostics(now()).unwrap());
        assert!(rows.contains("o Cursor não entra nesta cota"), "{rows}");
    }

    #[test]
    fn a_record_of_another_account_is_never_shown_as_the_current_one() {
        // Signed in as u2; the newest record was written by u1's process.
        let dir = with_log(
            "u2",
            &log_lines("u1", "2026-11-24T11:00:00Z", 88.0, true, END),
        );
        let grok = Grok::new(Some(dir.path().into()), Scripted::ok(WEEKLY), false);
        let outcome = grok.fetch(now());
        assert_eq!(result(&outcome), &Err(FetchError::AwaitingClient));
        assert!(outcome.session.identity().is_some());
        let rows = format!("{:?}", grok.diagnostics(now()).unwrap());
        assert!(rows.contains("outra conta"), "{rows}");
        assert!(!rows.contains("u1") && !rows.contains("u2@"), "{rows}");
    }

    #[test]
    fn a_record_without_an_identity_is_not_attributed() {
        let dir = signed_in("u1");
        let lines = log_lines("u1", "2026-11-24T11:00:00Z", 30.0, true, END);
        // The identity line was trimmed away by Grok's log cap.
        let billing_only = lines.lines().nth(1).unwrap().to_owned() + "\n";
        dir.write("logs/unified.jsonl", &billing_only);
        let grok = Grok::new(Some(dir.path().into()), Scripted::ok(WEEKLY), false);
        assert_eq!(result(&grok.fetch(now())), &Err(FetchError::AwaitingClient));
    }

    #[test]
    fn a_record_of_an_expired_period_is_rejected() {
        let dir = with_log(
            "u1",
            &log_lines(
                "u1",
                "2026-11-18T11:00:00Z",
                95.0,
                true,
                "2026-11-20T03:30:32.116+00:00",
            ),
        );
        let grok = Grok::new(Some(dir.path().into()), Scripted::ok(WEEKLY), false);
        assert_eq!(result(&grok.fetch(now())), &Err(FetchError::AwaitingClient));
        let rows = format!("{:?}", grok.diagnostics(now()).unwrap());
        assert!(rows.contains("período encerrado"), "{rows}");
    }

    #[test]
    fn no_log_or_no_record_means_no_reading_not_zero_percent() {
        let dir = signed_in("u1");
        let grok = Grok::new(Some(dir.path().into()), Scripted::ok(WEEKLY), false);
        assert_eq!(result(&grok.fetch(now())), &Err(FetchError::AwaitingClient));
        dir.write("logs/unified.jsonl", "{\"msg\":\"startup timing\"}\n");
        assert_eq!(result(&grok.fetch(now())), &Err(FetchError::AwaitingClient));
        // A record without a percentage is not a 0 % reading.
        let lines = log_lines("u1", "2026-11-24T11:00:00Z", 1.0, true, END)
            .replace("\"creditUsagePercent\":1,", "");
        dir.write("logs/unified.jsonl", &lines);
        assert!(matches!(
            result(&grok.fetch(now())),
            Err(FetchError::Malformed(_))
        ));
    }

    #[test]
    fn a_new_record_is_picked_up_and_an_unchanged_log_is_not_rescanned() {
        let dir = with_log(
            "u1",
            &log_lines("u1", "2026-11-24T05:04:38Z", 71.0, true, END),
        );
        let grok = Grok::new(Some(dir.path().into()), Scripted::ok(WEEKLY), false);
        let first = result(&grok.fetch(now())).clone().unwrap();
        assert_eq!(first.quotas[0].percent_used, 71);
        let cached = grok.log.lock().unwrap().clone();
        assert!(cached.is_some());
        // Same file: served from the cache.
        let again = result(&grok.fetch(now())).clone().unwrap();
        assert_eq!(again, first);
        // Grok Build fetched again: the newer record wins.
        let mut lines = log_lines("u1", "2026-11-24T05:04:38Z", 71.0, true, END);
        lines.push_str(&log_lines("u1", "2026-11-24T11:37:08Z", 100.0, true, END));
        dir.write("logs/unified.jsonl", &lines);
        let later = result(&grok.fetch(now())).clone().unwrap();
        assert_eq!(later.quotas[0].percent_used, 100);
        assert!(later.observed_at > first.observed_at);
    }

    #[test]
    fn grok_home_is_where_the_log_is_read() {
        // `detect` honors GROK_HOME; here the home is injected directly.
        let dir = with_log(
            "u1",
            &log_lines("u1", "2026-11-24T11:00:00Z", 5.0, true, END),
        );
        let grok = Grok::new(Some(dir.path().into()), Scripted::ok(WEEKLY), false);
        assert!(result(&grok.fetch(now())).is_ok());
        let elsewhere = signed_in("u1");
        let grok = Grok::new(Some(elsewhere.path().into()), Scripted::ok(WEEKLY), false);
        assert_eq!(result(&grok.fetch(now())), &Err(FetchError::AwaitingClient));
    }

    #[test]
    fn the_live_query_when_enabled_yields_the_weekly_allowance_and_on_demand_credits() {
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
            ("grok-credits", "Franquia semanal compartilhada", 42)
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
    fn signing_out_hides_the_record_even_if_the_log_remains() {
        let dir = with_log(
            "u1",
            &log_lines("u1", "2026-11-24T11:00:00Z", 50.0, true, END),
        );
        let grok = Grok::new(Some(dir.path().into()), Scripted::ok(WEEKLY), false);
        assert!(result(&grok.fetch(now())).is_ok());
        std::fs::remove_file(dir.path().join("auth.json")).unwrap();
        let outcome = grok.fetch(now());
        assert_eq!(result(&outcome), &Err(FetchError::AuthMissing));
        assert_eq!(outcome.session, Session::SignedOut);
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
    fn diagnostics_describe_the_source_without_account_data() {
        let dir = with_log(
            "u1",
            &log_lines("u1", "2026-11-24T11:00:00Z", 40.0, true, END),
        );
        let rows = Grok::new(Some(dir.path().into()), Scripted::ok(WEEKLY), false)
            .diagnostics(now())
            .unwrap();
        let text = format!("{rows:?}");
        assert!(text.contains("unified.jsonl"), "{text}");
        assert!(text.contains("Compartilhada entre Grok e Cursor"), "{text}");
        assert!(text.contains("termos de uso da xAI"), "{text}");
        assert!(
            !text.contains("sentinel") && !text.contains("u1@"),
            "{text}"
        );
        let last = rows
            .iter()
            .find(|row| row.label == "Último registro")
            .unwrap();
        assert_eq!(last.at.as_deref(), Some("2026-11-24T11:00:00Z"));
    }

    /// Reads the real Grok Build log and sign-in of this machine. Opt-in:
    /// `cargo test --locked live_grok_log -- --ignored --nocapture`. Prints
    /// only the normalized reading (no account data exists in it).
    #[test]
    #[ignore = "reads the installed Grok Build's files"]
    fn live_grok_log() {
        let grok = Grok::detect();
        let started = std::time::Instant::now();
        let outcome = grok.fetch(Utc::now());
        println!("scan {} ms", started.elapsed().as_millis());
        match result(&outcome) {
            Ok(fetched) => {
                for quota in &fetched.quotas {
                    println!(
                        "{} | {} | {}% used | {:?} min | resets {:?}",
                        quota.id,
                        quota.label,
                        quota.percent_used,
                        quota.duration_minutes,
                        quota.resets_at
                    );
                }
                println!(
                    "origin {:?}, observed {}, plan {:?}, attributed {}",
                    fetched.origin,
                    rfc3339(fetched.observed_at),
                    fetched.plan,
                    fetched.identity.is_some()
                );
            }
            Err(error) => println!("no reading: {error:?}"),
        }
        for row in grok.diagnostics(Utc::now()).unwrap() {
            println!(
                "{} | {:?} | {} | {:?}",
                row.label, row.state, row.detail, row.at
            );
        }
    }
}

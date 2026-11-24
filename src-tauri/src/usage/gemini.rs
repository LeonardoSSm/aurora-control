//! Google Gemini adapter: local sign-in detection, no quota query.
//!
//! The Gemini CLI reads its quotas from `POST https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota`
//! with the OAuth token of "Login with Google". Its terms
//! (`docs/resources/tos-privacy.md` in google-gemini/gemini-cli) state that
//! directly accessing the services powering Gemini CLI with third-party
//! software, using Gemini CLI OAuth, violates the applicable terms and may lead
//! to account suspension. This adapter therefore never contacts Google and
//! never uses the token: it only reports the local sign-in state and marks the
//! quota source as blocked (`sourceBlocked: providerTerms`). See
//! `docs/provider-compatibility.md` for the investigation and alternatives.
//!
//! Files read (all owned by the Gemini CLI, read-only, bounded):
//! - `settings.json`: `security.auth.selectedType` (or the older top-level
//!   `selectedAuthType`), to tell Google sign-in from API key or Vertex AI.
//! - `oauth_creds.json`: presence of a credential only. Values are not kept.
//! - `google_accounts.json`: the active e-mail, masked for display and
//!   reduced to a process-local fingerprint for account isolation.
//!
//! The folder is `~/.gemini`, or `$GEMINI_CLI_HOME/.gemini` when the CLI's
//! home override is set.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::adapter::{FetchError, Outcome, UsageAdapter, mask_account};
use super::local::{ReadError, env_dir, home_dir, read_limited};
use super::model::{BlockReason, DiagnosticRow, DiagnosticState, Provider};
use super::session::Session;

const MAX_SETTINGS: u64 = 256 * 1024;
const MAX_CREDENTIALS: u64 = 256 * 1024;
const MAX_ACCOUNTS: u64 = 64 * 1024;
/// Set in the CLI's environment, it moves the credential to the system
/// keychain or an encrypted file, which this app never reads.
const ENCRYPTED_STORAGE_ENV: &str = "GEMINI_FORCE_ENCRYPTED_FILE_STORAGE";

pub struct Gemini {
    home: Option<PathBuf>,
    /// The encrypted-storage switch is set in this process's environment.
    encrypted_storage: bool,
}

impl Gemini {
    pub fn detect() -> Self {
        let home = env_dir("GEMINI_CLI_HOME")
            .or_else(home_dir)
            .map(|home| home.join(".gemini"));
        let encrypted_storage =
            std::env::var(ENCRYPTED_STORAGE_ENV).is_ok_and(|value| value == "true");
        Self::new(home, encrypted_storage)
    }

    pub fn new(home: Option<PathBuf>, encrypted_storage: bool) -> Self {
        Self {
            home,
            encrypted_storage,
        }
    }
}

/// How the Gemini CLI is signed in, from `settings.json`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AuthType {
    /// "Login with Google" (`oauth-personal`): Code Assist for individuals,
    /// Google AI Pro/Ultra or Workspace Code Assist.
    Google,
    /// `gemini-api-key`: Gemini API (AI Studio) limits, per project.
    ApiKey,
    /// `vertex-ai`: Google Cloud project quotas.
    VertexAi,
    /// `compute-default-credentials`, `cloud-shell` or `gateway`.
    Other,
    /// No choice recorded yet.
    Unknown,
}

impl AuthType {
    fn parse(value: Option<&str>) -> Self {
        match value {
            Some("oauth-personal") => Self::Google,
            Some("gemini-api-key") => Self::ApiKey,
            Some("vertex-ai") => Self::VertexAi,
            Some(_) => Self::Other,
            None => Self::Unknown,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Google => "Conta Google",
            Self::ApiKey => "Chave da Gemini API",
            Self::VertexAi => "Vertex AI",
            Self::Other => "Credenciais de nuvem ou gateway",
            Self::Unknown => "Não definido",
        }
    }
}

/// `settings.json` may carry comments (the CLI strips them). An unparsable
/// file only means the method is unknown: it never blocks detection.
fn read_auth_type(home: &Path) -> AuthType {
    let Ok(text) = read_limited(&home.join("settings.json"), MAX_SETTINGS) else {
        return AuthType::Unknown;
    };
    let Ok(value) = serde_json::from_str::<Value>(&strip_line_comments(&text)) else {
        return AuthType::Unknown;
    };
    let selected = value["security"]["auth"]["selectedType"]
        .as_str()
        .or_else(|| value["selectedAuthType"].as_str());
    AuthType::parse(selected)
}

/// Drops `//` comments that start a line. Enough for the CLI's own files;
/// anything else simply fails to parse and reads as "unknown".
fn strip_line_comments(text: &str) -> String {
    text.lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn active_account(home: &Path) -> Option<String> {
    let text = read_limited(&home.join("google_accounts.json"), MAX_ACCOUNTS).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    value["active"]
        .as_str()
        .map(str::trim)
        .filter(|email| email.contains('@'))
        .map(str::to_owned)
}

/// What the local files say about the Google sign-in.
#[derive(Debug, Eq, PartialEq)]
enum Credential {
    Present,
    Missing,
    /// Unreadable, oversized or without any token.
    Invalid,
}

fn read_credential(home: &Path) -> Credential {
    match read_limited(&home.join("oauth_creds.json"), MAX_CREDENTIALS) {
        Ok(text) => {
            let Ok(value) = serde_json::from_str::<Value>(&text) else {
                return Credential::Invalid;
            };
            // Presence only: the values are dropped with `value`.
            let has_token = ["refresh_token", "access_token"]
                .iter()
                .any(|key| value[*key].as_str().is_some_and(|token| !token.is_empty()));
            if has_token {
                Credential::Present
            } else {
                Credential::Invalid
            }
        }
        Err(ReadError::Missing) => Credential::Missing,
        Err(ReadError::TooLarge | ReadError::Unreadable) => Credential::Invalid,
    }
}

/// The local state, decided from files only.
struct Local {
    auth: AuthType,
    credential: Credential,
    email: Option<String>,
}

impl Local {
    fn read(home: &Path) -> Self {
        Self {
            auth: read_auth_type(home),
            credential: read_credential(home),
            email: active_account(home),
        }
    }

    fn session(&self) -> Session {
        match (&self.credential, &self.email) {
            _ if matches!(
                self.auth,
                AuthType::ApiKey | AuthType::VertexAi | AuthType::Other
            ) =>
            {
                Session::SignedOut
            }
            (Credential::Present, Some(email)) => {
                // The e-mail identifies the account; its digest is the
                // credential version too, as the token itself is never used.
                Session::account(&["gemini", &email.to_ascii_lowercase()], email)
            }
            (Credential::Missing, _) if self.auth == AuthType::Unknown => Session::SignedOut,
            _ => Session::Unverified,
        }
    }
}

impl Gemini {
    fn outcome(&self, local: &Local) -> Outcome {
        let account = local.email.as_deref().map(mask_account);
        let error = match (local.auth, &local.credential) {
            (AuthType::ApiKey | AuthType::VertexAi | AuthType::Other, _) => FetchError::Unsupported,
            (_, Credential::Present) => FetchError::SourceBlocked(BlockReason::ProviderTerms),
            (_, Credential::Invalid) => FetchError::CredentialsInvalid,
            // Google sign-in chosen, but no credential file: the CLI keeps it
            // in the keychain or an encrypted file.
            (AuthType::Google, Credential::Missing) => FetchError::CredentialsUnavailable,
            (_, Credential::Missing) if self.encrypted_storage => {
                FetchError::CredentialsUnavailable
            }
            (_, Credential::Missing) => FetchError::AuthMissing,
        };
        let session = local.session();
        Outcome {
            account: account.filter(|_| session != Session::SignedOut),
            plan: None,
            session,
            ..Outcome::failed(error)
        }
    }
}

impl UsageAdapter for Gemini {
    fn provider(&self) -> Provider {
        Provider::Gemini
    }

    fn session(&self, _now: DateTime<Utc>) -> Option<Session> {
        Some(match &self.home {
            Some(home) => Local::read(home).session(),
            None => Session::SignedOut,
        })
    }

    /// Reads the local files and reports the sign-in. Never touches the
    /// network: there is no quota source this app may use.
    fn fetch(&self, _now: DateTime<Utc>) -> Outcome {
        match &self.home {
            Some(home) => self.outcome(&Local::read(home)),
            None => Outcome::signed_out(FetchError::AuthMissing),
        }
    }

    fn diagnostics(&self, _now: DateTime<Utc>) -> Option<Vec<DiagnosticRow>> {
        let Some(home) = &self.home else {
            return Some(vec![DiagnosticRow::new(
                "Pasta do Gemini CLI",
                DiagnosticState::Error,
                "Pasta pessoal do usuário não encontrada",
            )]);
        };
        if !home.is_dir() {
            return Some(vec![DiagnosticRow::new(
                "Pasta do Gemini CLI",
                DiagnosticState::Info,
                "Não encontrada. Instale o Gemini CLI e execute “gemini”.",
            )]);
        }
        let local = Local::read(home);
        let mut rows = vec![DiagnosticRow::new(
            "Método de login",
            match local.auth {
                AuthType::Google => DiagnosticState::Ok,
                AuthType::Unknown => DiagnosticState::Info,
                _ => DiagnosticState::Warning,
            },
            local.auth.label(),
        )];
        rows.push(match local.credential {
            Credential::Present => DiagnosticRow::new(
                "Credencial local",
                DiagnosticState::Ok,
                "Encontrada no arquivo do CLI (não é lida nem enviada)",
            ),
            Credential::Missing if local.auth == AuthType::Google || self.encrypted_storage => {
                DiagnosticRow::new(
                    "Credencial local",
                    DiagnosticState::Info,
                    "No cofre do sistema ou em arquivo cifrado",
                )
            }
            Credential::Missing => {
                DiagnosticRow::new("Credencial local", DiagnosticState::Info, "Não encontrada")
            }
            Credential::Invalid => DiagnosticRow::new(
                "Credencial local",
                DiagnosticState::Warning,
                "Arquivo ilegível ou sem token",
            ),
        });
        if let Some(email) = &local.email {
            rows.push(DiagnosticRow::new(
                "Conta",
                DiagnosticState::Ok,
                mask_account(email),
            ));
        }
        rows.push(DiagnosticRow::new(
            "Fonte de cotas",
            DiagnosticState::Warning,
            match local.auth {
                AuthType::ApiKey | AuthType::VertexAi | AuthType::Other => {
                    "Limites de API e de projeto não são cotas de assinatura"
                }
                _ => "Bloqueada pelos termos do Gemini CLI para software de terceiros",
            },
        ));
        Some(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::local::testing::TempDir;
    use crate::usage::model::Issue;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-11-24T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    const CREDS: &str = r#"{"access_token":"ya29.sentinel-access","refresh_token":"1//sentinel-refresh","expiry_date":1795000000000,"token_type":"Bearer"}"#;
    const GOOGLE: &str = r#"{"security":{"auth":{"selectedType":"oauth-personal"}}}"#;

    fn adapter(dir: &TempDir) -> Gemini {
        Gemini::new(Some(dir.path().to_path_buf()), false)
    }

    fn error(outcome: &Outcome) -> &FetchError {
        outcome.result.as_ref().unwrap().as_ref().unwrap_err()
    }

    #[test]
    fn a_google_sign_in_is_detected_and_the_quota_source_is_blocked() {
        let dir = TempDir::new("gemini-google");
        dir.write("settings.json", GOOGLE);
        dir.write("oauth_creds.json", CREDS);
        dir.write(
            "google_accounts.json",
            r#"{"active":"dev@example.com","old":[]}"#,
        );
        let outcome = adapter(&dir).fetch(now());
        assert_eq!(
            error(&outcome),
            &FetchError::SourceBlocked(BlockReason::ProviderTerms)
        );
        assert_eq!(outcome.account.as_deref(), Some("d•••@example.com"));
        assert!(outcome.session.identity().is_some());
        assert!(outcome.fallback.is_none() && outcome.activity.is_none());
        // Nothing of the credential reaches the outcome.
        let debug = format!("{outcome:?}");
        assert!(!debug.contains("sentinel"), "{debug}");
    }

    #[test]
    fn missing_settings_with_a_credential_still_count_as_google() {
        let dir = TempDir::new("gemini-nosettings");
        dir.write("oauth_creds.json", CREDS);
        let outcome = adapter(&dir).fetch(now());
        assert_eq!(
            error(&outcome),
            &FetchError::SourceBlocked(BlockReason::ProviderTerms)
        );
        // Without the active e-mail, the account cannot be verified.
        assert_eq!(outcome.session, Session::Unverified);
    }

    #[test]
    fn without_any_sign_in_the_session_is_signed_out() {
        let dir = TempDir::new("gemini-empty");
        let outcome = adapter(&dir).fetch(now());
        assert_eq!(error(&outcome), &FetchError::AuthMissing);
        assert_eq!(outcome.session, Session::SignedOut);
        let outcome = Gemini::new(None, false).fetch(now());
        assert_eq!(error(&outcome), &FetchError::AuthMissing);
    }

    #[test]
    fn api_key_and_vertex_logins_are_incompatible_accounts() {
        for kind in ["gemini-api-key", "vertex-ai", "cloud-shell", "gateway"] {
            let dir = TempDir::new("gemini-apikey");
            dir.write(
                "settings.json",
                &format!(r#"{{"security":{{"auth":{{"selectedType":"{kind}"}}}}}}"#),
            );
            // A leftover Google credential does not change the declared method.
            dir.write("oauth_creds.json", CREDS);
            dir.write("google_accounts.json", r#"{"active":"dev@example.com"}"#);
            let outcome = adapter(&dir).fetch(now());
            assert_eq!(error(&outcome), &FetchError::Unsupported, "{kind}");
            assert_eq!(outcome.session, Session::SignedOut);
            assert_eq!(outcome.account, None);
        }
        // The older top-level key is honored too.
        let dir = TempDir::new("gemini-legacy-key");
        dir.write("settings.json", r#"{"selectedAuthType":"gemini-api-key"}"#);
        assert_eq!(error(&adapter(&dir).fetch(now())), &FetchError::Unsupported);
    }

    #[test]
    fn a_google_choice_without_a_file_points_to_the_system_store() {
        let dir = TempDir::new("gemini-keychain");
        dir.write("settings.json", GOOGLE);
        let outcome = adapter(&dir).fetch(now());
        assert_eq!(error(&outcome), &FetchError::CredentialsUnavailable);
        assert_eq!(outcome.session, Session::Unverified);
        // Same when the encrypted storage switch is set.
        let dir = TempDir::new("gemini-encrypted");
        let outcome = Gemini::new(Some(dir.path().into()), true).fetch(now());
        assert_eq!(error(&outcome), &FetchError::CredentialsUnavailable);
    }

    #[test]
    fn malformed_or_oversized_files_are_invalid_not_a_logout() {
        for contents in ["{\"access_tok", "[]", r#"{"access_token":""}"#] {
            let dir = TempDir::new("gemini-invalid");
            dir.write("settings.json", GOOGLE);
            dir.write("oauth_creds.json", contents);
            let outcome = adapter(&dir).fetch(now());
            assert_eq!(
                error(&outcome),
                &FetchError::CredentialsInvalid,
                "{contents}"
            );
            assert_eq!(outcome.session, Session::Unverified);
        }
        let dir = TempDir::new("gemini-large");
        dir.write("oauth_creds.json", &" ".repeat(300 * 1024));
        assert_eq!(
            error(&adapter(&dir).fetch(now())),
            &FetchError::CredentialsInvalid
        );
        // Comments and garbage in settings only make the method unknown.
        let dir = TempDir::new("gemini-comments");
        dir.write(
            "settings.json",
            "// user notes\n{\"security\":{\"auth\":{\"selectedType\":\"oauth-personal\"}}}",
        );
        assert_eq!(read_auth_type(dir.path()), AuthType::Google);
        dir.write("settings.json", "{ not json");
        assert_eq!(read_auth_type(dir.path()), AuthType::Unknown);
    }

    #[test]
    fn switching_the_google_account_changes_the_identity() {
        let dir = TempDir::new("gemini-switch");
        dir.write("oauth_creds.json", CREDS);
        dir.write("google_accounts.json", r#"{"active":"a@example.com"}"#);
        let gemini = adapter(&dir);
        let first = gemini.session(now()).unwrap();
        dir.write(
            "google_accounts.json",
            r#"{"active":"b@example.com","old":["a@example.com"]}"#,
        );
        let second = gemini.session(now()).unwrap();
        assert!(first.identity().is_some() && second.identity().is_some());
        assert_ne!(first.identity(), second.identity());
        // Logged out: the CLI clears the active account and the credential.
        dir.write(
            "google_accounts.json",
            r#"{"active":null,"old":["b@example.com"]}"#,
        );
        std::fs::remove_file(dir.path().join("oauth_creds.json")).unwrap();
        assert_eq!(gemini.session(now()), Some(Session::SignedOut));
    }

    #[test]
    fn the_blocked_source_reaches_the_wire_as_a_tagged_issue() {
        let issue = Issue::SourceBlocked {
            reason: BlockReason::ProviderTerms,
        };
        assert_eq!(
            serde_json::to_value(issue).unwrap(),
            serde_json::json!({ "code": "sourceBlocked", "reason": "providerTerms" })
        );
    }

    #[test]
    fn diagnostics_never_show_credentials() {
        let dir = TempDir::new("gemini-diag");
        dir.write("settings.json", GOOGLE);
        dir.write("oauth_creds.json", CREDS);
        dir.write("google_accounts.json", r#"{"active":"dev@example.com"}"#);
        let rows = adapter(&dir).diagnostics(now()).unwrap();
        let text = format!("{rows:?}");
        assert!(
            !text.contains("sentinel") && !text.contains("dev@"),
            "{text}"
        );
        assert!(text.contains("Conta Google"));
        assert!(text.contains("Bloqueada"));
    }
}

//! Minimal HTTP transport. Adapters depend on the `Transport` trait so tests
//! can replay status codes, headers and bodies without network access.
//! Request headers are never logged: they carry credentials.

use std::time::Duration;

use chrono::{DateTime, Utc};

use super::adapter::FetchError;

pub const TIMEOUT: Duration = Duration::from_secs(15);
pub const USER_AGENT: &str = concat!("AuroraControl/", env!("CARGO_PKG_VERSION"), " (Windows)");
/// Upper bound for response bodies. Usage payloads are a few kilobytes.
const MAX_BODY: u64 = 1024 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct Response {
    pub status: u16,
    pub retry_after: Option<String>,
    pub body: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TransportError {
    Network,
    Timeout,
}

pub trait Transport: Send + Sync + 'static {
    fn get(&self, url: &str, headers: &[(&str, &str)]) -> Result<Response, TransportError>;
}

pub struct Ureq {
    agent: ureq::Agent,
}

impl Ureq {
    pub fn new() -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            .http_status_as_error(false)
            .max_redirects(0)
            .user_agent(USER_AGENT)
            .tls_config(
                ureq::tls::TlsConfig::builder()
                    .provider(ureq::tls::TlsProvider::NativeTls)
                    .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                    .build(),
            )
            .build();
        Self {
            agent: config.into(),
        }
    }
}

impl Transport for Ureq {
    fn get(&self, url: &str, headers: &[(&str, &str)]) -> Result<Response, TransportError> {
        let mut request = self.agent.get(url);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let mut response = request.call().map_err(|error| match error {
            ureq::Error::Timeout(_) => TransportError::Timeout,
            _ => TransportError::Network,
        })?;
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let body = response
            .body_mut()
            .with_config()
            .limit(MAX_BODY)
            .read_to_string()
            .map_err(|error| match error {
                ureq::Error::Timeout(_) => TransportError::Timeout,
                _ => TransportError::Network,
            })?;
        Ok(Response {
            status,
            retry_after,
            body,
        })
    }
}

/// `Retry-After` accepts delta-seconds or an HTTP date (RFC 9110 §10.2.3).
pub fn parse_retry_after(value: &str, now: DateTime<Utc>) -> Option<Duration> {
    let value = value.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let date = DateTime::parse_from_rfc2822(value)
        .ok()?
        .with_timezone(&Utc);
    Some((date - now).to_std().unwrap_or(Duration::ZERO))
}

/// Maps transport and status failures shared by every provider.
pub fn check_status(
    result: Result<Response, TransportError>,
    now: DateTime<Utc>,
) -> Result<Response, FetchError> {
    let response = result.map_err(|error| match error {
        TransportError::Network => FetchError::Network,
        TransportError::Timeout => FetchError::Timeout,
    })?;
    match response.status {
        200..=299 => Ok(response),
        401 | 403 => Err(FetchError::AuthRejected),
        429 => Err(FetchError::RateLimited {
            retry_after: response
                .retry_after
                .as_deref()
                .and_then(|value| parse_retry_after(value, now)),
        }),
        status => Err(FetchError::Http(status)),
    }
}

#[cfg(test)]
pub mod fake {
    use std::sync::Mutex;

    use super::*;

    /// Replays scripted responses and records requested URLs and header names.
    pub struct Script {
        responses: Mutex<Vec<Result<Response, TransportError>>>,
        pub requests: Mutex<Vec<(String, Vec<String>)>>,
    }

    impl Script {
        pub fn new(responses: Vec<Result<Response, TransportError>>) -> Self {
            Self {
                responses: Mutex::new(responses),
                requests: Mutex::new(Vec::new()),
            }
        }

        pub fn ok(status: u16, body: &str) -> Result<Response, TransportError> {
            Ok(Response {
                status,
                retry_after: None,
                body: body.into(),
            })
        }

        pub fn request_count(&self) -> usize {
            self.requests.lock().unwrap().len()
        }
    }

    impl Transport for Script {
        fn get(&self, url: &str, headers: &[(&str, &str)]) -> Result<Response, TransportError> {
            self.requests.lock().unwrap().push((
                url.into(),
                headers.iter().map(|(name, _)| name.to_string()).collect(),
            ));
            let mut responses = self.responses.lock().unwrap();
            assert!(!responses.is_empty(), "unexpected request to {url}");
            responses.remove(0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn retry_after_accepts_seconds_and_http_dates() {
        let now = Utc.with_ymd_and_hms(2026, 11, 23, 18, 0, 0).unwrap();
        assert_eq!(
            parse_retry_after("120", now),
            Some(Duration::from_secs(120))
        );
        assert_eq!(
            parse_retry_after("Mon, 23 Nov 2026 18:02:30 GMT", now),
            Some(Duration::from_secs(150))
        );
        assert_eq!(
            parse_retry_after("Mon, 23 Nov 2026 17:00:00 GMT", now),
            Some(Duration::ZERO)
        );
        assert_eq!(parse_retry_after("soon", now), None);
    }

    #[test]
    fn status_mapping_distinguishes_auth_limits_and_server_errors() {
        let now = Utc::now();
        let response = |status, retry_after: Option<&str>| {
            Ok(Response {
                status,
                retry_after: retry_after.map(str::to_owned),
                body: String::new(),
            })
        };
        assert!(check_status(response(200, None), now).is_ok());
        assert_eq!(
            check_status(response(401, None), now),
            Err(FetchError::AuthRejected)
        );
        assert_eq!(
            check_status(response(429, Some("30")), now),
            Err(FetchError::RateLimited {
                retry_after: Some(Duration::from_secs(30))
            })
        );
        assert_eq!(
            check_status(response(503, None), now),
            Err(FetchError::Http(503))
        );
        assert_eq!(
            check_status(Err(TransportError::Timeout), now),
            Err(FetchError::Timeout)
        );
    }
}

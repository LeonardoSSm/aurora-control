//! Minimal HTTP transport. Adapters depend on the `Transport` trait so tests
//! can replay status codes, headers and bodies without network access.
//! Request headers are never logged: they carry credentials.

use std::time::Duration;

use chrono::{DateTime, Utc};

use super::adapter::FetchError;
use super::time::bounded;

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
        let mut response = request.call().map_err(transport_error)?;
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        respond(status, retry_after, || {
            response
                .body_mut()
                .with_config()
                .limit(MAX_BODY)
                .read_to_string()
                .map_err(transport_error)
        })
    }
}

fn transport_error(error: ureq::Error) -> TransportError {
    match error {
        ureq::Error::Timeout(_) => TransportError::Timeout,
        _ => TransportError::Network,
    }
}

/// Builds the response from what was already received. The body is read only
/// for success statuses: an error status and its `Retry-After` stand on their
/// own, and a body that fails to arrive cannot erase them (audit I01).
pub fn respond(
    status: u16,
    retry_after: Option<String>,
    read_body: impl FnOnce() -> Result<String, TransportError>,
) -> Result<Response, TransportError> {
    let body = if (200..300).contains(&status) {
        read_body()?
    } else {
        String::new()
    };
    Ok(Response {
        status,
        retry_after,
        body,
    })
}

/// `Retry-After` accepts delta-seconds or an HTTP date (RFC 9110 §10.2.3).
/// The wait is bounded by [`super::time::MAX_WAIT`]; past dates mean now.
pub fn parse_retry_after(value: &str, now: DateTime<Utc>) -> Option<Duration> {
    let value = value.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(bounded(Duration::from_secs(seconds)));
    }
    let date = DateTime::parse_from_rfc2822(value)
        .ok()?
        .with_timezone(&Utc);
    Some(bounded(
        date.signed_duration_since(now)
            .to_std()
            .unwrap_or(Duration::ZERO),
    ))
}

/// Maps transport and status failures shared by every provider. Decided from
/// the status and headers only.
pub fn check_status(
    result: Result<Response, TransportError>,
    now: DateTime<Utc>,
) -> Result<Response, FetchError> {
    let response = result.map_err(|error| match error {
        TransportError::Network => FetchError::Network,
        TransportError::Timeout => FetchError::Timeout,
    })?;
    let retry_after = || {
        response
            .retry_after
            .as_deref()
            .and_then(|value| parse_retry_after(value, now))
    };
    match response.status {
        200..=299 => Ok(response),
        401 | 403 => Err(FetchError::AuthRejected),
        429 => Err(FetchError::RateLimited {
            retry_after: retry_after(),
        }),
        status => Err(FetchError::Http {
            status,
            retry_after: retry_after(),
        }),
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
            Err(FetchError::Http {
                status: 503,
                retry_after: None
            })
        );
        assert_eq!(
            check_status(Err(TransportError::Timeout), now),
            Err(FetchError::Timeout)
        );
    }

    #[test]
    fn i01_service_unavailable_keeps_its_retry_after() {
        let now = Utc::now();
        let response = Ok(Response {
            status: 503,
            retry_after: Some("300".into()),
            body: String::new(),
        });
        assert_eq!(
            check_status(response, now),
            Err(FetchError::Http {
                status: 503,
                retry_after: Some(Duration::from_secs(300))
            })
        );
    }

    #[test]
    fn i01_a_failing_body_never_hides_the_status_and_headers_of_an_error() {
        let now = Utc::now();
        for (status, expected) in [
            (
                429,
                FetchError::RateLimited {
                    retry_after: Some(Duration::from_secs(120)),
                },
            ),
            (401, FetchError::AuthRejected),
            (
                503,
                FetchError::Http {
                    status: 503,
                    retry_after: Some(Duration::from_secs(120)),
                },
            ),
        ] {
            let mut read = false;
            let response = respond(status, Some("120".into()), || {
                read = true;
                Err(TransportError::Network)
            });
            assert!(!read, "the body of a {status} is not read");
            assert_eq!(check_status(response, now), Err(expected));
        }
        // A success whose body fails is a transport failure.
        assert_eq!(
            respond(200, None, || Err(TransportError::Timeout)),
            Err(TransportError::Timeout)
        );
    }

    #[test]
    fn b02_retry_after_is_bounded() {
        let now = Utc.with_ymd_and_hms(2026, 11, 23, 18, 0, 0).unwrap();
        assert_eq!(
            parse_retry_after("18446744073709551615", now),
            Some(super::super::time::MAX_WAIT)
        );
        assert_eq!(
            parse_retry_after("Fri, 01 Jan 2100 00:00:00 GMT", now),
            Some(super::super::time::MAX_WAIT)
        );
        // Above the backoff ceiling but valid: preserved.
        assert_eq!(
            parse_retry_after("7200", now),
            Some(Duration::from_secs(7200))
        );
        assert_eq!(parse_retry_after("-5", now), None);
    }
}

use axum::http::header::{
    CACHE_CONTROL, CONTENT_SECURITY_POLICY, HOST, HeaderName, HeaderValue, ORIGIN, REFERRER_POLICY,
    X_CONTENT_TYPE_OPTIONS,
};
use axum::http::{HeaderMap, Request};
use getrandom::fill;

use crate::{Error, Result};

const PERMISSIONS_POLICY: HeaderName = HeaderName::from_static("permissions-policy");
const NONCE_BYTES: usize = 16;

/// Result of checking an HTTP request's browser origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OriginDecision {
    Allowed,
    Rejected,
}

/// One session's Content-Security-Policy nonce.
///
/// A Provider that inlines a script needs the header and the `<script nonce=…>`
/// attribute to agree. AFUI owns the credential-grade randomness; the Provider
/// composes its own policy string around it, because only the Provider knows
/// what its page loads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiCspNonce(String);

impl UiCspNonce {
    pub fn generate() -> Result<Self> {
        let mut bytes = [0_u8; NONCE_BYTES];
        fill(&mut bytes)
            .map_err(|error| Error::Random(std::io::Error::other(error.to_string())))?;
        Ok(Self(hex::encode(bytes)))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Security headers and request-provenance checks shared by Provider UIs.
#[derive(Debug, Clone)]
pub struct UiSecurityPolicy {
    allowed_origin: Option<HeaderValue>,
    expected_host: Option<String>,
    content_security_policy: HeaderValue,
}

impl UiSecurityPolicy {
    pub fn isolated() -> Self {
        Self {
            allowed_origin: None,
            expected_host: None,
            content_security_policy: HeaderValue::from_static(
                "default-src 'self'; base-uri 'none'; frame-ancestors 'none'; \
                 form-action 'self'; object-src 'none'",
            ),
        }
    }

    pub fn with_allowed_origin(mut self, origin: HeaderValue) -> Self {
        self.allowed_origin = Some(origin);
        self
    }

    /// Reject requests whose `Host` is not this exact authority.
    ///
    /// This is the DNS-rebinding guard: a name that resolves to the loopback
    /// address reaches the session, but arrives with its own `Host`. `serve`
    /// sets this from the address it actually advertises, so Providers do not
    /// have to remember to.
    pub fn with_expected_host(mut self, host: impl Into<String>) -> Self {
        self.expected_host = Some(host.into());
        self
    }

    pub fn with_content_security_policy(mut self, policy: HeaderValue) -> Self {
        self.content_security_policy = policy;
        self
    }

    #[must_use]
    pub fn check_origin<B>(&self, request: &Request<B>) -> OriginDecision {
        if let Some(expected_host) = &self.expected_host {
            let host_matches = request
                .headers()
                .get(HOST)
                .and_then(|host| host.to_str().ok())
                .is_some_and(|host| host == expected_host);
            if !host_matches {
                return OriginDecision::Rejected;
            }
        }
        let Some(origin) = request.headers().get(ORIGIN) else {
            return OriginDecision::Allowed;
        };
        if self
            .allowed_origin
            .as_ref()
            .is_some_and(|allowed| origin == allowed)
        {
            return OriginDecision::Allowed;
        }
        let same_http_origin = request
            .headers()
            .get(HOST)
            .and_then(|host| host.to_str().ok())
            .is_some_and(|host| origin.as_bytes() == format!("http://{host}").as_bytes());
        if same_http_origin {
            OriginDecision::Allowed
        } else {
            OriginDecision::Rejected
        }
    }

    pub fn apply_headers(&self, headers: &mut HeaderMap) {
        headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
        headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
        headers.insert(REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
        headers.insert(
            PERMISSIONS_POLICY,
            HeaderValue::from_static(
                "camera=(), microphone=(), geolocation=(), payment=(), usb=()",
            ),
        );
        headers.insert(
            CONTENT_SECURITY_POLICY,
            self.content_security_policy.clone(),
        );
    }
}

impl Default for UiSecurityPolicy {
    fn default() -> Self {
        Self::isolated()
    }
}

#[cfg(test)]
mod tests {
    use axum::http::Request;

    use super::*;

    #[test]
    fn rejects_unconfigured_cross_origin_requests() {
        let request = Request::builder()
            .header(ORIGIN, "https://other.example")
            .body(())
            .unwrap();
        assert_eq!(
            UiSecurityPolicy::isolated().check_origin(&request),
            OriginDecision::Rejected
        );
    }

    #[test]
    fn allows_same_origin_requests() {
        let request = Request::builder()
            .header(HOST, "127.0.0.1:1234")
            .header(ORIGIN, "http://127.0.0.1:1234")
            .body(())
            .unwrap();
        assert_eq!(
            UiSecurityPolicy::isolated().check_origin(&request),
            OriginDecision::Allowed
        );
    }

    #[test]
    fn applies_shared_security_headers() {
        let mut headers = HeaderMap::new();
        UiSecurityPolicy::isolated().apply_headers(&mut headers);
        assert_eq!(headers[CACHE_CONTROL], "no-store");
        assert_eq!(headers[X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert!(headers.contains_key(CONTENT_SECURITY_POLICY));
        assert!(headers.contains_key(PERMISSIONS_POLICY));
    }

    #[test]
    fn an_expected_host_rejects_a_rebound_name_that_reaches_the_same_socket() {
        let policy = UiSecurityPolicy::isolated().with_expected_host("127.0.0.1:1234");
        // Same socket, attacker-controlled name: no Origin header is sent on a
        // top-level navigation, so the Host check is the only thing that fires.
        let rebound = Request::builder()
            .header(HOST, "attacker.example:1234")
            .body(())
            .unwrap();
        assert_eq!(policy.check_origin(&rebound), OriginDecision::Rejected);
        let genuine = Request::builder()
            .header(HOST, "127.0.0.1:1234")
            .body(())
            .unwrap();
        assert_eq!(policy.check_origin(&genuine), OriginDecision::Allowed);
    }

    #[test]
    fn nonces_are_unique_per_generation() {
        let first = UiCspNonce::generate().unwrap();
        let second = UiCspNonce::generate().unwrap();
        assert_ne!(first, second);
        assert_eq!(first.as_str().len(), NONCE_BYTES * 2);
        assert!(first.as_str().chars().all(|c| c.is_ascii_hexdigit()));
    }
}

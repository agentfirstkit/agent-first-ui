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

/// Which scripts a Provider page may run.
///
/// Closed on purpose, and closed at exactly the three shapes a page in this
/// kit has: none at all, one script the Provider splices in under this
/// session's nonce, or script files served from this session's own origin.
/// There is no fourth arm because there is no fourth trustworthy source — a
/// page whose script could come from anywhere else is a page a replacement
/// frontend can make run code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiPageScript {
    /// No script. The page has no behaviour, and neither a Provider nor a
    /// frontend can give it any.
    None,
    /// Exactly one script, carrying this session's nonce. A frontend cannot
    /// supply a script file (refused by name) or hide one in a template
    /// (refused by content), and could not run it if it did: it does not know
    /// the nonce.
    Nonce(UiCspNonce),
    /// Script files served from this session's own origin — the Provider's
    /// own runtime as a file rather than an inline block. A frontend still
    /// cannot supply one: the file names an override may write are refused
    /// before they ever reach a route.
    SameOrigin,
}

/// What one Provider page is allowed to load, as switches rather than a
/// string.
///
/// A page of this shape denies everything by default and then names a small,
/// closed set of exceptions. Written by hand, that is a `format!` over nine
/// directives in every Provider, where a typo produces a policy that is
/// merely *different* rather than invalid — and where the `HeaderValue::from_str`
/// at the end has a failure arm that is very easy to answer by quietly falling
/// back to some other policy, which is the one outcome a security header must
/// never have. Assembled here, the string is built from values that are already
/// valid header bytes, so there is no parse step and no failure arm to answer.
///
/// This is deliberately not a general CSP builder. It covers the directives
/// that actually vary across pages of this shape; anything outside it goes
/// through [`UiSecurityPolicy::with_content_security_policy`], where getting
/// the string wrong is the caller's error and must be reported as one.
#[derive(Debug, Clone)]
pub struct UiPagePolicy {
    script: UiPageScript,
    images: bool,
    fonts: bool,
    runtime: bool,
    form_submission: bool,
    inline_styles: bool,
}

impl UiPagePolicy {
    /// A page that loads its own stylesheet and nothing else.
    ///
    /// Everything below is an addition to this floor: `default-src 'none'`,
    /// a stylesheet from this session's own origin, and no base URI, framing,
    /// plugin, form target, or network destination at all.
    #[must_use]
    pub fn new(script: UiPageScript) -> Self {
        Self {
            script,
            images: false,
            fonts: false,
            runtime: false,
            form_submission: false,
            inline_styles: false,
        }
    }

    /// Replace which scripts this page may run, keeping every other switch.
    ///
    /// For a Provider whose pages share one floor and differ in whether they
    /// carry behaviour — the common shape, and the one that otherwise gets
    /// written as two nearly identical policy strings.
    #[must_use]
    pub fn with_script(mut self, script: UiPageScript) -> Self {
        self.script = script;
        self
    }

    /// Images from this session's own origin — a frontend's shipped assets, or
    /// something the Provider serves on one of its own routes. Same origin,
    /// same credential, no network.
    #[must_use]
    pub fn allow_images(mut self) -> Self {
        self.images = true;
        self
    }

    /// Fonts from this session's own origin, for a frontend that ships them.
    #[must_use]
    pub fn allow_fonts(mut self) -> Self {
        self.fonts = true;
        self
    }

    /// Network requests back to this session.
    ///
    /// Required by [`crate::UiSessionRuntime`]. AFUI's runtime is the one
    /// connection a first-party page needs; the switch describes the browser
    /// capability rather than asking the Provider to name a transport.
    #[must_use]
    pub fn allow_runtime(mut self) -> Self {
        self.runtime = true;
        self
    }

    /// Form submission back to this session. This is what a page carrying a
    /// decision needs: the answer travels as a navigation to the Provider's
    /// own route, which needs no network destination opened for it.
    #[must_use]
    pub fn allow_form_submission(mut self) -> Self {
        self.form_submission = true;
        self
    }

    /// Inline `style` attributes and `<style>` blocks, for a page whose markup
    /// carries presentation the stylesheet cannot express.
    #[must_use]
    pub fn allow_inline_styles(mut self) -> Self {
        self.inline_styles = true;
        self
    }

    /// Whether this page may connect to its AFUI runtime.
    #[must_use]
    pub fn allows_runtime(&self) -> bool {
        self.runtime
    }

    /// The assembled policy.
    ///
    /// Infallible: every part is either a fixed ASCII string or a hex nonce,
    /// so there is nothing here that could fail to be a header value and
    /// therefore nothing to fall back from.
    #[must_use]
    pub fn header_value(&self) -> HeaderValue {
        let style = if self.inline_styles {
            "'self' 'unsafe-inline'"
        } else {
            "'self'"
        };
        let mut policy = format!("default-src 'none'; style-src {style}");
        match &self.script {
            UiPageScript::None => {}
            UiPageScript::Nonce(nonce) => {
                policy.push_str("; script-src 'nonce-");
                policy.push_str(nonce.as_str());
                policy.push('\'');
            }
            UiPageScript::SameOrigin => policy.push_str("; script-src 'self'"),
        }
        if self.images {
            policy.push_str("; img-src 'self'");
        }
        if self.fonts {
            policy.push_str("; font-src 'self'");
        }
        policy.push_str(if self.runtime {
            "; connect-src 'self'"
        } else {
            "; connect-src 'none'"
        });
        policy.push_str(if self.form_submission {
            "; form-action 'self'"
        } else {
            "; form-action 'none'"
        });
        policy.push_str("; base-uri 'none'; frame-ancestors 'none'; object-src 'none'");
        HeaderValue::from_str(&policy).unwrap_or_else(|_| {
            // Unreachable: everything appended above is fixed ASCII or a hex
            // nonce. A silent fallback to a *different* policy is the failure
            // this whole type exists to remove, so the answer here is the
            // strictest thing a header can say, not a looser one.
            HeaderValue::from_static("default-src 'none'")
        })
    }

    /// The full security posture this page is served under: these directives
    /// plus the response headers every AFUI-served page carries.
    #[must_use]
    pub fn into_security_policy(self) -> UiSecurityPolicy {
        UiSecurityPolicy::from(self)
    }
}

impl From<UiPagePolicy> for UiSecurityPolicy {
    fn from(page: UiPagePolicy) -> Self {
        Self {
            allowed_origin: None,
            expected_host: None,
            content_security_policy: page.header_value(),
            page_allows_runtime: Some(page.allows_runtime()),
        }
    }
}

/// Security headers and request-provenance checks shared by Provider UIs.
#[derive(Debug, Clone)]
pub struct UiSecurityPolicy {
    allowed_origin: Option<HeaderValue>,
    expected_host: Option<String>,
    content_security_policy: HeaderValue,
    /// Whether this page may reach its own session, when that is knowable.
    ///
    /// `Some` for a policy assembled from [`UiPagePolicy`], where the answer is
    /// a switch AFUI set itself. `None` for a hand-written policy, where the
    /// only way to answer would be to parse a string a caller wrote — so AFUI
    /// does not guess, and the caller keeps the whole of that decision.
    page_allows_runtime: Option<bool>,
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
            page_allows_runtime: None,
        }
    }

    /// Whether a page served under this policy may reach its own session, or
    /// `None` when the policy came from a caller-written string.
    pub(crate) fn page_allows_runtime(&self) -> Option<bool> {
        self.page_allows_runtime
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

    /// Replace the whole policy string with one the caller wrote.
    ///
    /// The escape hatch, for a page whose needs [`UiPagePolicy`] does not
    /// cover. Taking it has consequences. The string is yours, so
    /// getting it wrong is your error and must be reported as one — never
    /// answered by quietly serving some other policy, which turns a mistake in
    /// a security header into a page that looks like it worked. And AFUI can
    /// no longer tell whether the page may reach its own session, so the check
    /// that catches a runtime attached to a page that cannot connect to it
    /// stops applying.
    pub fn with_content_security_policy(mut self, policy: HeaderValue) -> Self {
        self.content_security_policy = policy;
        self.page_allows_runtime = None;
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

    /// The floor a page starts from: nothing at all except its own stylesheet.
    #[test]
    fn a_page_that_asked_for_nothing_can_load_nothing_but_its_own_stylesheet() {
        let policy = UiPagePolicy::new(UiPageScript::None);
        let value = policy.header_value();
        let policy = value.to_str().unwrap();
        assert!(
            policy.starts_with("default-src 'none'; style-src 'self'"),
            "{policy}"
        );
        for denied in [
            "connect-src 'none'",
            "form-action 'none'",
            "base-uri 'none'",
            "frame-ancestors 'none'",
            "object-src 'none'",
        ] {
            assert!(policy.contains(denied), "{policy}");
        }
        // A directive that was never asked for is absent, and `default-src
        // 'none'` is what governs it — naming it would be the same policy
        // written twice.
        assert!(!policy.contains("script-src"), "{policy}");
        assert!(!policy.contains("img-src"), "{policy}");
        assert!(!policy.contains("font-src"), "{policy}");
    }

    #[test]
    fn each_switch_changes_exactly_the_directive_it_names() {
        let nonce = UiCspNonce::generate().unwrap();
        let cases: [(UiPagePolicy, &str); 6] = [
            (
                UiPagePolicy::new(UiPageScript::Nonce(nonce.clone())),
                &format!("script-src 'nonce-{}'", nonce.as_str()),
            ),
            (
                UiPagePolicy::new(UiPageScript::SameOrigin),
                "script-src 'self'",
            ),
            (
                UiPagePolicy::new(UiPageScript::None).allow_images(),
                "img-src 'self'",
            ),
            (
                UiPagePolicy::new(UiPageScript::None).allow_fonts(),
                "font-src 'self'",
            ),
            (
                UiPagePolicy::new(UiPageScript::None).allow_runtime(),
                "connect-src 'self'",
            ),
            (
                UiPagePolicy::new(UiPageScript::None).allow_form_submission(),
                "form-action 'self'",
            ),
        ];
        for (policy, expected) in cases {
            let value = policy.header_value();
            let policy = value.to_str().unwrap();
            assert!(
                policy.contains(expected),
                "{expected} missing from {policy}"
            );
        }

        let inline = UiPagePolicy::new(UiPageScript::None).allow_inline_styles();
        let value = inline.header_value();
        assert!(
            value
                .to_str()
                .unwrap()
                .contains("style-src 'self' 'unsafe-inline'"),
            "{value:?}"
        );
    }

    /// The whole point of assembling rather than formatting: a nonce is hex and
    /// every other part is a fixed literal, so there is no parse step that
    /// could fail and therefore no failure a caller could answer by serving a
    /// different policy.
    #[test]
    fn an_assembled_policy_is_always_a_valid_header_value() {
        for script in [
            UiPageScript::None,
            UiPageScript::SameOrigin,
            UiPageScript::Nonce(UiCspNonce::generate().unwrap()),
        ] {
            let policy = UiPagePolicy::new(script)
                .allow_images()
                .allow_fonts()
                .allow_runtime()
                .allow_form_submission()
                .allow_inline_styles();
            let mut headers = HeaderMap::new();
            policy.into_security_policy().apply_headers(&mut headers);
            assert!(headers[CONTENT_SECURITY_POLICY].to_str().is_ok());
        }
    }

    /// The response headers every AFUI-served page carries do not depend on
    /// which directives the page asked for.
    #[test]
    fn a_page_policy_still_carries_the_shared_response_headers() {
        let mut headers = HeaderMap::new();
        UiPagePolicy::new(UiPageScript::None)
            .into_security_policy()
            .apply_headers(&mut headers);
        assert_eq!(headers[CACHE_CONTROL], "no-store");
        assert_eq!(headers[X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert!(headers.contains_key(PERMISSIONS_POLICY));
    }

    /// A page that may reach its own session is answerable; one written by
    /// hand is not, and AFUI does not guess by reading the caller's string.
    #[test]
    fn only_an_assembled_policy_can_answer_whether_the_page_reaches_its_session() {
        assert_eq!(
            UiPagePolicy::new(UiPageScript::None)
                .into_security_policy()
                .page_allows_runtime(),
            Some(false)
        );
        assert_eq!(
            UiPagePolicy::new(UiPageScript::None)
                .allow_runtime()
                .into_security_policy()
                .page_allows_runtime(),
            Some(true)
        );
        assert_eq!(UiSecurityPolicy::isolated().page_allows_runtime(), None);
        // Taking the escape hatch takes the answer with it: the string is the
        // caller's now, and a guess at what it permits would be worse than no
        // answer at all.
        assert_eq!(
            UiPagePolicy::new(UiPageScript::None)
                .allow_runtime()
                .into_security_policy()
                .with_content_security_policy(HeaderValue::from_static("default-src 'none'"))
                .page_allows_runtime(),
            None
        );
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

use std::fmt;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;

use axum::Router;
use getrandom::fill;
use tokio::sync::{Mutex, Notify};

use crate::frontend::validate_identifier;
use crate::host::{SessionRegistration, UiHost, UiMount};
use crate::registry::{self, RegistryHandle};
use crate::{Error, Result, UiAccessToken, UiExpiry, UiSecurityPolicy, UiWindow, UiWindowConfig};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct UiSessionId(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiSessionMetadata {
    pub provider_id: String,
    pub ui_kind: String,
    pub session_id: UiSessionId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome<T> {
    Completed(T),
    Closed,
    Expired,
}

/// How a managed URL is exposed, and when its credential stops working.
///
/// Window delivery has no equivalent: the window is the bound. This type only
/// exists for the case where AFUI hands out a URL and nobody is watching it.
#[derive(Debug, Clone)]
pub struct UiUrlDelivery {
    expiry: UiExpiry,
    bind_ip: IpAddr,
    advertised_host: Option<String>,
}

impl UiUrlDelivery {
    /// Expiry is required, not defaulted: a URL nobody is watching must say
    /// when it stops working.
    #[must_use]
    pub fn new(expiry: UiExpiry) -> Self {
        Self {
            expiry,
            bind_ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
            advertised_host: None,
        }
    }

    #[must_use]
    pub fn with_bind_ip(mut self, bind_ip: IpAddr) -> Self {
        self.bind_ip = bind_ip;
        self
    }

    /// Advertise a different authority than the one bound.
    ///
    /// Binding `0.0.0.0` is how a session becomes reachable from the LAN, but
    /// `0.0.0.0` is not an address anyone can open. The caller knows which of
    /// its interfaces to name; AFUI cannot guess. `host` carries the port when
    /// the port is not implied, for example `192.168.1.4:8080`.
    #[must_use]
    pub fn with_advertised_host(mut self, host: impl Into<String>) -> Self {
        self.advertised_host = Some(host.into());
        self
    }
}

struct CompletionState<T> {
    outcome: Mutex<Option<Outcome<T>>>,
    notify: Notify,
}

/// Cloneable handle passed to Provider state so a typed handler can finish a session.
pub struct UiCompletion<T> {
    state: Arc<CompletionState<T>>,
}

pub struct UiSession<T> {
    metadata: UiSessionMetadata,
    completion: UiCompletion<T>,
    security_policy: UiSecurityPolicy,
    subject: Option<String>,
}

pub struct ActiveUiSession<T> {
    metadata: UiSessionMetadata,
    access_url: String,
    access_token: UiAccessToken,
    completion: UiCompletion<T>,
    /// Deregisters this session from its host when dropped.
    _registration: SessionRegistration,
    /// Withdraws this session from the cross-process registry when dropped.
    _announcement: RegistryHandle,
    /// Set only when this session created its own single-session host, which
    /// must then outlive it. A session mounted on a caller-owned host holds
    /// nothing here — the caller decides when that host stops.
    _owned_host: Option<Arc<UiHost>>,
}

impl<T> Clone for UiCompletion<T> {
    fn clone(&self) -> Self {
        Self {
            state: Arc::clone(&self.state),
        }
    }
}

impl<T> UiCompletion<T> {
    /// Complete once. Returns false when another terminal outcome already won.
    pub async fn complete(&self, value: T) -> bool {
        self.finish(Outcome::Completed(value)).await
    }

    async fn finish(&self, outcome: Outcome<T>) -> bool {
        let mut stored = self.state.outcome.lock().await;
        if stored.is_some() {
            return false;
        }
        *stored = Some(outcome);
        drop(stored);
        self.state.notify.notify_waiters();
        true
    }

    async fn wait(&self) -> Outcome<T> {
        loop {
            let notified = self.state.notify.notified();
            if let Some(outcome) = self.state.outcome.lock().await.take() {
                return outcome;
            }
            notified.await;
        }
    }
}

impl<T> UiSession<T>
where
    T: Send + 'static,
{
    pub fn new(provider_id: impl Into<String>, ui_kind: impl Into<String>) -> Result<Self> {
        let provider_id = provider_id.into();
        let ui_kind = ui_kind.into();
        validate_identifier("provider_id", &provider_id)?;
        validate_identifier("ui_kind", &ui_kind)?;
        Ok(Self {
            metadata: UiSessionMetadata {
                provider_id,
                ui_kind,
                session_id: UiSessionId::generate()?,
            },
            completion: UiCompletion {
                state: Arc::new(CompletionState {
                    outcome: Mutex::new(None),
                    notify: Notify::new(),
                }),
            },
            security_policy: UiSecurityPolicy::isolated(),
            subject: None,
        })
    }

    /// Name what this session is about, in the Provider's own words — a table,
    /// a case reference, a wallet.
    ///
    /// This is identity, not presentation: AFUI never parses it and never
    /// decides anything from it. It exists so a person looking at a list of
    /// their open sessions can tell two of the same kind apart.
    #[must_use]
    pub fn with_subject(mut self, subject: impl Into<String>) -> Self {
        self.subject = Some(subject.into());
        self
    }

    #[must_use]
    pub fn with_security_policy(mut self, security_policy: UiSecurityPolicy) -> Self {
        self.security_policy = security_policy;
        self
    }

    #[must_use]
    pub fn metadata(&self) -> &UiSessionMetadata {
        &self.metadata
    }

    #[must_use]
    pub fn completion(&self) -> UiCompletion<T> {
        self.completion.clone()
    }

    /// Serve on loopback and open an isolated window onto it.
    ///
    /// The window is the bound, so the credential never expires: a person who
    /// leaves the window open is still using it, and `Outcome::Expired` cannot
    /// occur here. The session ends when a handler completes it or the person
    /// closes the window.
    pub async fn window(self, router: Router, config: &UiWindowConfig) -> Result<Outcome<T>> {
        let active = self
            .serve(router, UiUrlDelivery::new(UiExpiry::Never))
            .await?;
        active.open_window(config).await
    }

    /// Serve a managed URL for a person who is not at this machine.
    ///
    /// This gives the session a host of its own. When several sessions belong
    /// together — several attachments to one runtime, several reviews from one
    /// workspace — bind a [`UiHost`] once and use [`UiHost::attach`] instead, so
    /// they share a port. A caller that already has a listener and a router of
    /// its own wants [`UiMount`] and neither of these.
    pub async fn serve(
        self,
        router: Router,
        delivery: UiUrlDelivery,
    ) -> Result<ActiveUiSession<T>> {
        let mut host = UiHost::bind(delivery.bind_ip).await?;
        if let Some(advertised) = delivery.advertised_host {
            host = host.with_advertised_host(advertised);
        }
        let host = Arc::new(host);
        let mut active = host.attach(self, router, delivery.expiry)?;
        active._owned_host = Some(host);
        Ok(active)
    }
}

impl UiMount {
    /// Mount a session and return the handle its caller waits on.
    ///
    /// Every session on a mount is reached by its own credential, so a person
    /// can have several open at once and each answer still returns to the call
    /// that is waiting for it.
    ///
    /// The session announces itself to the cross-process registry here, whether
    /// AFUI owns the listener or the caller does — a session `afui session
    /// list` cannot see is a session `afui session serve` cannot reach.
    pub fn attach<T>(
        &self,
        session: UiSession<T>,
        router: Router,
        expiry: UiExpiry,
    ) -> Result<ActiveUiSession<T>>
    where
        T: Send + 'static,
    {
        let access_token = UiAccessToken::issue(expiry)?;
        let (access_url, registration) =
            self.register(&access_token, session.security_policy.clone(), router)?;
        // Announce automatically rather than on request. The `afui` CLI can
        // only schedule what it can see, and a feature that depends on every
        // Provider remembering to opt in is a feature that silently has holes.
        let announcement = registry::publish(&session.metadata, session.subject, &access_url);
        Ok(ActiveUiSession {
            metadata: session.metadata,
            access_url,
            access_token,
            completion: session.completion,
            _registration: registration,
            _announcement: announcement,
            _owned_host: None,
        })
    }
}

impl UiHost {
    /// Mount a session on this host and return the handle its caller waits on.
    ///
    /// The same call as [`UiMount::attach`], on the mount this host owns.
    pub fn attach<T>(
        &self,
        session: UiSession<T>,
        router: Router,
        expiry: UiExpiry,
    ) -> Result<ActiveUiSession<T>>
    where
        T: Send + 'static,
    {
        self.mount().attach(session, router, expiry)
    }
}

impl<T> ActiveUiSession<T>
where
    T: Send + 'static,
{
    #[must_use]
    pub fn metadata(&self) -> &UiSessionMetadata {
        &self.metadata
    }

    #[must_use]
    pub fn access_url(&self) -> &str {
        &self.access_url
    }

    #[must_use]
    pub fn access_token(&self) -> &UiAccessToken {
        &self.access_token
    }

    /// A handle that finishes this session.
    ///
    /// Usually the Provider takes this from [`UiSession::completion`] before
    /// building its router, because the handler needs it. This accessor is for
    /// completing a session from outside its own pages — a supervisor
    /// cancelling it, or a caller that mounted a router it did not build.
    #[must_use]
    pub fn completion(&self) -> UiCompletion<T> {
        self.completion.clone()
    }

    pub async fn wait(mut self) -> Result<Outcome<T>> {
        let outcome = tokio::select! {
            outcome = self.completion.wait() => outcome,
            () = expiry_elapsed(&self.access_token) => Outcome::Expired,
        };
        self.end();
        Ok(outcome)
    }

    pub async fn open_window(mut self, config: &UiWindowConfig) -> Result<Outcome<T>> {
        let mut window = UiWindow::launch(&self.access_url, config)?;
        let outcome = tokio::select! {
            outcome = self.completion.wait() => outcome,
            closed = window.wait_closed() => {
                closed?;
                Outcome::Closed
            },
            () = expiry_elapsed(&self.access_token) => Outcome::Expired,
        };
        drop(window);
        self.end();
        Ok(outcome)
    }

    /// End the session: revoke the credential so the host stops routing to it.
    ///
    /// The host itself keeps running — other sessions may still be mounted on
    /// it. A session that owns its host takes the host down with it when the
    /// handle drops.
    fn end(&mut self) {
        self.access_token.revoke();
    }
}

impl<T> Drop for ActiveUiSession<T> {
    fn drop(&mut self) {
        // `_registration` removes the session from its host right after this.
        self.access_token.revoke();
    }
}

/// Resolve when the credential expires, or never when it cannot.
///
/// An idle expiry slides forward from the request path, so this re-reads the
/// deadline rather than sleeping once against a deadline captured up front.
async fn expiry_elapsed(token: &UiAccessToken) {
    loop {
        match token.remaining_ttl() {
            None => std::future::pending::<()>().await,
            Some(remaining) if remaining.is_zero() => return,
            Some(remaining) => tokio::time::sleep(remaining).await,
        }
    }
}

impl UiSessionId {
    pub(crate) fn generate() -> Result<Self> {
        let mut bytes = [0_u8; 16];
        fill(&mut bytes)
            .map_err(|error| Error::Random(std::io::Error::other(error.to_string())))?;
        Ok(Self(hex::encode(bytes)))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for UiSessionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use axum::http::StatusCode;

    use super::*;

    #[tokio::test]
    async fn completed_value_returns_to_the_session() {
        let session = UiSession::<u32>::new("reviewer", "case_review").unwrap();
        let completion = session.completion();
        assert!(completion.complete(42).await);
        assert_eq!(session.completion.wait().await, Outcome::Completed(42));
    }

    #[tokio::test]
    async fn concurrent_sessions_route_results_independently() {
        let first = UiSession::<u8>::new("reviewer", "case_review").unwrap();
        let second = UiSession::<u8>::new("ledger", "plan_confirm").unwrap();
        let first_completion = first.completion();
        let second_completion = second.completion();
        assert!(second_completion.complete(2).await);
        assert!(first_completion.complete(1).await);
        assert_eq!(first.completion.wait().await, Outcome::Completed(1));
        assert_eq!(second.completion.wait().await, Outcome::Completed(2));
    }

    #[test]
    fn identifiers_are_stable_path_segments() {
        assert!(UiSession::<()>::new("reviewer", "case_review").is_ok());
        assert!(UiSession::<()>::new("AfMail", "case_review").is_err());
        assert!(UiSession::<()>::new("reviewer", "../review").is_err());
    }

    #[tokio::test]
    async fn a_url_delivery_advertises_the_named_host_rather_than_the_bound_wildcard() {
        let session = UiSession::<()>::new("reviewer", "case_review").unwrap();
        let active = session
            .serve(
                Router::new(),
                UiUrlDelivery::new(UiExpiry::After(Duration::from_secs(900)))
                    .with_bind_ip(IpAddr::V4(Ipv4Addr::UNSPECIFIED))
                    .with_advertised_host("192.168.1.4"),
            )
            .await
            .unwrap();
        let url = active.access_url().to_owned();
        assert!(url.starts_with("http://192.168.1.4:"), "{url}");
        assert!(!url.contains("0.0.0.0"), "{url}");
        // The credential lives in the path, and the trailing slash is what
        // makes a page's relative asset references resolve.
        assert!(url.ends_with('/'), "{url}");
    }

    #[tokio::test]
    async fn a_window_session_can_never_expire() {
        let session = UiSession::<()>::new("console", "terminal").unwrap();
        let active = session
            .serve(Router::new(), UiUrlDelivery::new(UiExpiry::Never))
            .await
            .unwrap();
        assert_eq!(active.access_token().remaining_ttl(), None);
        // The expiry race must not be resolvable, or a watch session would die
        // with its window still open.
        assert!(
            tokio::time::timeout(
                Duration::from_millis(50),
                expiry_elapsed(active.access_token()),
            )
            .await
            .is_err()
        );
    }

    #[tokio::test]
    async fn an_expired_url_delivery_stops_serving_before_the_waiter_notices() {
        let session = UiSession::<()>::new("reviewer", "case_review").unwrap();
        let active = session
            .serve(
                Router::new(),
                UiUrlDelivery::new(UiExpiry::After(Duration::from_millis(30))),
            )
            .await
            .unwrap();
        let url = active.access_url().to_owned();
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(active.access_token().is_expired());
        let status = reqwest_status(&url).await;
        assert_eq!(status, Some(StatusCode::NOT_FOUND.as_u16()));
    }

    /// Minimal HTTP GET so the test does not add a client dependency.
    async fn reqwest_status(url: &str) -> Option<u16> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let rest = url.strip_prefix("http://")?;
        let (authority, path) = rest.split_once('/')?;
        let mut stream = tokio::net::TcpStream::connect(authority).await.ok()?;
        stream
            .write_all(
                format!("GET /{path} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n")
                    .as_bytes(),
            )
            .await
            .ok()?;
        let mut response = String::new();
        stream.read_to_string(&mut response).await.ok()?;
        response.split_whitespace().nth(1)?.parse().ok()
    }
}

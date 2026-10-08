use std::any::TypeId;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;

use axum::Router;
use getrandom::fill;
use tokio::sync::{Mutex, Notify};

use crate::frontend::validate_identifier;
use crate::host::{SessionRegistration, UiHost, UiMount};
use crate::registry::{self, RegistryHandle};
use crate::runtime::{UiSessionRuntimeCloser, UiSessionRuntimeMount};
use crate::{
    DEFAULT_UI_RUNTIME_CALL_CAPACITY, DEFAULT_UI_RUNTIME_EVENT_CAPACITY, Error, Result,
    UiAccessToken, UiAppIcon, UiBlobPolicy, UiExpiry, UiSecurityPolicy, UiSessionEntry,
    UiSessionRuntime, UiWindow, UiWindowConfig,
};

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

impl<T> Outcome<T> {
    /// The one word for how this ended: `completed`, `closed`, or `expired`.
    ///
    /// A Provider almost always has a richer word of its own for the completed
    /// case — what the person actually decided — and should use it. What this
    /// is for is the other two, which carry no domain meaning at all and which
    /// every Provider was otherwise spelling for itself: whether a person
    /// closing a window reads back as `closed`, `window_closed`, or
    /// `cancelled` is not a decision anyone benefits from making twice.
    #[must_use]
    pub fn ending(&self) -> &'static str {
        match self {
            Self::Completed(_) => "completed",
            Self::Closed => "closed",
            Self::Expired => "expired",
        }
    }
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

/// A session's one terminal outcome, and whether it has been handed over yet.
///
/// Terminal means terminal: once a slot leaves `Pending` it never returns, so a
/// `UiCompletion` retained past the end of a session cannot complete it again.
/// The outcome is taken out exactly once, by the waiter that owns the session.
enum TerminalSlot<T> {
    Pending,
    Settled(Outcome<T>),
    Taken,
}

struct CompletionState<T> {
    outcome: Mutex<TerminalSlot<T>>,
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
    app_icon: Option<UiAppIcon>,
    subject: Option<String>,
    waits_for_decision: bool,
    runtime: Option<UiSessionRuntimeMount>,
}

pub struct ActiveUiSession<T> {
    metadata: UiSessionMetadata,
    subject: Option<String>,
    waits_for_decision: bool,
    access_url: String,
    window_url: String,
    access_token: UiAccessToken,
    completion: UiCompletion<T>,
    /// Signalled by the reserved end route this session answers under its own
    /// credential, under every delivery — see [`crate::SESSION_END_PATH`].
    /// [`Self::wait`] and [`Self::open_window`] race on it exactly the way
    /// they race on expiry, and either one turns it into `Outcome::Closed`.
    close: Arc<Notify>,
    /// Deregisters this session from its host when dropped.
    _registration: SessionRegistration,
    /// Withdraws this session from the cross-process registry when dropped.
    _announcement: RegistryHandle,
    /// Set only when this session created its own single-session host, which
    /// must then outlive it. A session mounted on a caller-owned host holds
    /// nothing here — the caller decides when that host stops.
    _owned_host: Option<Arc<UiHost>>,
    /// Ends the AFUI-owned runtime with the session.
    runtime_closer: Option<UiSessionRuntimeCloser>,
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
        if !matches!(*stored, TerminalSlot::Pending) {
            return false;
        }
        *stored = TerminalSlot::Settled(outcome);
        drop(stored);
        self.state.notify.notify_waiters();
        true
    }

    /// Offer `outcome` as the session's ending, and return the one that won.
    ///
    /// The close, expiry and window-close branches used to build an outcome and
    /// return it directly, so a `Completed(T)` that became ready in the same
    /// moment could lose to `Closed` or `Expired` on nothing more than which
    /// arm `select!` happened to pick — and a caller would be told the person
    /// closed the window when they had in fact just answered. Every ending goes
    /// through the same slot now, and whoever got there first is the answer.
    async fn settle(&self, outcome: Outcome<T>) -> Outcome<T> {
        self.finish(outcome).await;
        self.wait().await
    }

    /// Take the terminal outcome. Called once, by the waiter that owns the
    /// session — `wait` and `wait_with_window` both consume `self`.
    async fn wait(&self) -> Outcome<T> {
        loop {
            let notified = self.state.notify.notified();
            {
                let mut stored = self.state.outcome.lock().await;
                if let TerminalSlot::Settled(_) = *stored {
                    let TerminalSlot::Settled(outcome) =
                        std::mem::replace(&mut *stored, TerminalSlot::Taken)
                    else {
                        unreachable!("checked immediately above while holding the lock")
                    };
                    return outcome;
                }
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
                    outcome: Mutex::new(TerminalSlot::Pending),
                    notify: Notify::new(),
                }),
            },
            security_policy: UiSecurityPolicy::isolated(),
            app_icon: None,
            subject: None,
            // A session that hands a value back is, by definition, waiting on
            // whoever supplies it. `T = ()` cannot say that on its own — see
            // `awaiting_person` for the session that has nothing to hand back
            // yet is still, unmistakably, waiting on a person.
            waits_for_decision: TypeId::of::<T>() != TypeId::of::<()>(),
            runtime: None,
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

    /// Declare that this session is waiting on a person, for the one shape the
    /// automatic rule above cannot see: `T = ()`, nothing is ever handed back,
    /// and it is still unmistakably a person being waited on — several
    /// unrelated items settled one at a time, each written to disk as it is
    /// decided, with the window closing the only thing that ends it.
    ///
    /// Every session whose `T` is not `()` already reports this on its own;
    /// call this only for the `T = ()` session it would otherwise miss. Not
    /// calling it on a session that is genuinely watched costs nothing but a
    /// badge somewhere upstream not lighting up — the direction that matters
    /// is the other one, so this defaults to off rather than on.
    #[must_use]
    pub fn awaiting_person(mut self) -> Self {
        self.waits_for_decision = true;
        self
    }

    #[must_use]
    pub fn with_security_policy(mut self, security_policy: UiSecurityPolicy) -> Self {
        self.security_policy = security_policy;
        self
    }

    /// Carry this Provider frontend's resolved icon through every delivery.
    ///
    /// AFUI serves it at [`crate::APP_ICON_PATH`] for browser favicons and for
    /// windows opened later with `afui session open`. The icon stays optional
    /// for backwards compatibility; absence means no Provider icon, never the
    /// AFUI icon.
    #[must_use]
    pub fn with_app_icon(mut self, app_icon: UiAppIcon) -> Self {
        self.app_icon = Some(app_icon);
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

    /// Attach AFUI's typed, reconnecting session runtime.
    ///
    /// State is latest-value and retained, calls are correlated by AFUI, and
    /// ordered events have bounded replay. Provider payload types contain none
    /// of those transport fields.
    pub fn with_runtime<Action, Reply, SurfaceState>(
        self,
    ) -> Result<(Self, UiSessionRuntime<Action, Reply, SurfaceState>)> {
        self.with_runtime_capacity(
            DEFAULT_UI_RUNTIME_CALL_CAPACITY,
            DEFAULT_UI_RUNTIME_EVENT_CAPACITY,
        )
    }

    /// Attach the AFUI runtime with explicit unacknowledged-call and event-replay
    /// bounds.
    pub fn with_runtime_capacity<Action, Reply, SurfaceState>(
        self,
        call_capacity: usize,
        event_capacity: usize,
    ) -> Result<(Self, UiSessionRuntime<Action, Reply, SurfaceState>)> {
        self.attach_runtime(call_capacity, event_capacity, None)
    }

    /// Attach the runtime with AFUI-managed, policy-bounded byte bodies.
    pub fn with_runtime_and_blobs<Action, Reply, SurfaceState>(
        self,
        blob_policy: UiBlobPolicy,
    ) -> Result<(Self, UiSessionRuntime<Action, Reply, SurfaceState>)> {
        self.with_runtime_capacity_and_blobs(
            DEFAULT_UI_RUNTIME_CALL_CAPACITY,
            DEFAULT_UI_RUNTIME_EVENT_CAPACITY,
            blob_policy,
        )
    }

    /// Attach a blob-enabled runtime with explicit call and event bounds.
    pub fn with_runtime_capacity_and_blobs<Action, Reply, SurfaceState>(
        self,
        call_capacity: usize,
        event_capacity: usize,
        blob_policy: UiBlobPolicy,
    ) -> Result<(Self, UiSessionRuntime<Action, Reply, SurfaceState>)> {
        self.attach_runtime(call_capacity, event_capacity, Some(blob_policy))
    }

    fn attach_runtime<Action, Reply, SurfaceState>(
        mut self,
        call_capacity: usize,
        event_capacity: usize,
        blob_policy: Option<UiBlobPolicy>,
    ) -> Result<(Self, UiSessionRuntime<Action, Reply, SurfaceState>)> {
        if self.runtime.is_some() {
            return Err(Error::SessionRuntimeAlreadyAttached);
        }
        let (controller, mount) = match blob_policy {
            Some(policy) => {
                UiSessionRuntime::bounded_with_blob_policy(call_capacity, event_capacity, policy)?
            }
            None => UiSessionRuntime::bounded(call_capacity, event_capacity)?,
        };
        self.runtime = Some(mount);
        Ok((self, controller))
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
    /// can have several open at once and each runtime and terminal outcome stay
    /// bound to the session that owns it.
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
        // A runtime the page cannot reach is a session that opens, connects to
        // nothing, and reports no error. Both halves of that mistake are known
        // here for the first time, so this is where it is caught. Only for a
        // policy AFUI assembled: a hand-written one would have to be parsed to
        // answer, and AFUI does not guess at a caller-owned string.
        if session.runtime.is_some() && session.security_policy.page_allows_runtime() == Some(false)
        {
            return Err(Error::SessionRuntimeUnreachable);
        }
        let access_token = UiAccessToken::issue(expiry)?;
        let registered = self.register(
            &access_token,
            session.security_policy.clone(),
            session.app_icon.clone(),
            session.runtime,
            router,
        )?;
        // Announce automatically rather than on request. The `afui` CLI can
        // only schedule what it can see, and a feature that depends on every
        // Provider remembering to opt in is a feature that silently has holes.
        let subject = session.subject;
        let waits_for_decision = session.waits_for_decision;
        let announcement = registry::publish(
            &session.metadata,
            subject.clone(),
            waits_for_decision,
            &registered.access_url,
            (registered.window_url != registered.access_url)
                .then_some(registered.window_url.as_str()),
        );
        Ok(ActiveUiSession {
            metadata: session.metadata,
            subject,
            waits_for_decision,
            access_url: registered.access_url,
            window_url: registered.window_url,
            access_token,
            completion: session.completion,
            close: registered.close,
            _registration: registered.registration,
            _announcement: announcement,
            _owned_host: None,
            runtime_closer: registered.runtime_closer,
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

    /// The URL AFUI's own local window opens: the access URL plus the window
    /// key that lets that window, and only it, ask this machine to open a link.
    pub(crate) fn window_url(&self) -> &str {
        &self.window_url
    }

    #[must_use]
    pub fn access_token(&self) -> &UiAccessToken {
        &self.access_token
    }

    /// Describe this live session to an AFUI-owned remote page without reading
    /// the cross-process registry. Direct links must still work when registry
    /// publication is disabled, and must expose only the session they own.
    pub(crate) fn remote_entry(&self) -> UiSessionEntry {
        UiSessionEntry {
            session_id: self.metadata.session_id.as_str().to_owned(),
            provider_id: self.metadata.provider_id.clone(),
            ui_kind: self.metadata.ui_kind.clone(),
            subject: self.subject.clone(),
            waits_for_decision: self.waits_for_decision,
            access_url_secret: self.access_url.clone(),
            window_url_secret: None,
            announced_at_epoch_ms: 0,
            owner_pid: std::process::id(),
        }
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
            // The reserved end route this session answers under its own
            // credential — a person acting through the served shell's "End"
            // control, `afui session close`, or a raw request against the
            // URL itself. It offers `Closed` rather than returning it: if the
            // handler answered in the same moment, that answer is the outcome.
            () = self.close.notified() => self.completion.settle(Outcome::Closed).await,
            () = expiry_elapsed(&self.access_token) => {
                self.completion.settle(Outcome::Expired).await
            }
        };
        self.end();
        Ok(outcome)
    }

    pub async fn open_window(self, config: &UiWindowConfig) -> Result<Outcome<T>> {
        let window = UiWindow::launch(&self.window_url, config)?;
        self.wait_with_window(window).await
    }

    pub(crate) async fn wait_with_window(mut self, mut window: UiWindow) -> Result<Outcome<T>> {
        let outcome = tokio::select! {
            outcome = self.completion.wait() => outcome,
            () = self.close.notified() => self.completion.settle(Outcome::Closed).await,
            closed = window.wait_closed() => {
                closed?;
                self.completion.settle(Outcome::Closed).await
            },
            () = expiry_elapsed(&self.access_token) => {
                self.completion.settle(Outcome::Expired).await
            }
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
        if let Some(runtime) = &self.runtime_closer {
            runtime.close();
        }
    }
}

impl<T> Drop for ActiveUiSession<T> {
    fn drop(&mut self) {
        // `_registration` removes the session from its host right after this.
        self.access_token.revoke();
        if let Some(runtime) = &self.runtime_closer {
            runtime.close();
        }
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
    async fn an_answer_that_arrives_with_the_close_is_still_the_answer() {
        // Close, expiry and window-close used to build an outcome inside their
        // own `select!` arm, so an answer that became ready in the same moment
        // could be reported as `Closed` on nothing but which arm was polled
        // first — telling a caller the person walked away when they had just
        // decided. Every ending goes through one slot now.
        let session = UiSession::<u32>::new("reviewer", "case_review").unwrap();
        let completion = session.completion();
        assert!(completion.complete(7).await);

        assert_eq!(
            session.completion.settle(Outcome::Closed).await,
            Outcome::Completed(7),
            "a completed answer outranks a close that arrives after it"
        );
    }

    #[tokio::test]
    async fn a_session_has_exactly_one_ending() {
        let session = UiSession::<u32>::new("reviewer", "case_review").unwrap();
        let completion = session.completion();

        // Whoever reaches the slot first wins, and says so.
        assert_eq!(
            session.completion.settle(Outcome::Closed).await,
            Outcome::Closed
        );

        // A handle held past the end cannot reopen the question. This used to
        // return `true`, because waiting took the outcome out and left the slot
        // looking untouched.
        assert!(
            !completion.complete(9).await,
            "a retained completion must not be able to answer a finished session"
        );
        assert!(!completion.complete(10).await);
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
        assert!(UiSession::<()>::new("Reviewer", "case_review").is_err());
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

    #[tokio::test]
    async fn a_session_serves_its_provider_icon_ahead_of_the_provider_router() {
        let icon = UiAppIcon::from_svg(
            "<svg xmlns=\"http://www.w3.org/2000/svg\"><circle r=\"1\"/></svg>",
        )
        .unwrap();
        let session = UiSession::<()>::new("reviewer", "case_review")
            .unwrap()
            .with_app_icon(icon);
        let active = session
            .serve(Router::new(), UiUrlDelivery::new(UiExpiry::Never))
            .await
            .unwrap();
        let response = http_get(&format!("{}{}", active.access_url(), crate::APP_ICON_PATH))
            .await
            .unwrap();

        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(
            response.contains("content-type: image/svg+xml"),
            "{response}"
        );
        assert!(response.contains("<circle"), "{response}");
    }

    /// A session that brought no router at all still serves the floor its
    /// page stands on. That is the whole point of the route: a frontend
    /// override replaces the Provider's stylesheet, and what a replacement
    /// cannot take away is what a replacement is allowed to omit.
    #[tokio::test]
    async fn a_session_serves_the_baseline_stylesheet_whatever_the_provider_routed() {
        let session = UiSession::<()>::new("reviewer", "case_review").unwrap();
        let active = session
            .serve(Router::new(), UiUrlDelivery::new(UiExpiry::Never))
            .await
            .unwrap();
        let response = http_get(&format!(
            "{}{}",
            active.access_url(),
            crate::BASE_STYLE_PATH
        ))
        .await
        .unwrap();

        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(
            response.contains("content-type: text/css; charset=utf-8"),
            "{response}"
        );
        assert!(response.contains("--afui-focus"), "{response}");
    }

    /// Minimal HTTP GET so the test does not add a client dependency.
    async fn reqwest_status(url: &str) -> Option<u16> {
        let response = http_get(url).await?;
        response.split_whitespace().nth(1)?.parse().ok()
    }

    /// Minimal HTTP GET so the test does not add a client dependency.
    async fn http_get(url: &str) -> Option<String> {
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
        Some(response)
    }
}

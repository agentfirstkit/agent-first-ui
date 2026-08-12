//! One executor for the three ways a session reaches a person.
//!
//! [`crate::UiSession::window`] and [`crate::UiSession::serve`] already exist;
//! this lets a Provider say "I have a session" once and get all three
//! deliveries, with the choice coming from one environment-aware place instead
//! of being reinvented around each UI. `link` is the direct URL shape another
//! device can open without an `afui` binary in the handoff path.

use std::net::{IpAddr, Ipv4Addr};

use axum::Router;

use crate::lan::primary_lan_ipv4;
use crate::{
    ActiveUiSession, Error, Outcome, Result, UiExpiry, UiSession, UiSessionMetadata, UiUrlDelivery,
    UiWindow, UiWindowConfig,
};

/// The default delivery mode when a Provider's own `--mode` flag was not
/// given. See [`UiDeliveryPlan::resolve`] for the priority this participates
/// in.
pub const DELIVERY_ENV: &str = "AFUI_DELIVERY";

/// One of the three ways a session reaches a person.
///
/// A closed set on purpose: three words a Provider's own `--mode` flag can
/// reuse verbatim, and the same three words [`DELIVERY_ENV`] accepts. A
/// Provider that needs a second axis — which network a listener binds,
/// which of several tools to run — needs a second flag, not a fourth
/// variant here; reusing this one to mean two things at once is how a runner
/// ends up needing a compatibility matrix instead of a name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiDeliveryMode {
    /// A local, isolated browser window. Never expires: the window is the
    /// bound.
    Window,
    /// A URL another device can open directly. The Provider must declare an
    /// expiry for it with [`UiDeliveryPlan::with_link_expiry`] — there is no
    /// default, and a Provider that never declares one does not offer this
    /// mode.
    Link,
    /// Registered only, with no window opened and no address advertised.
    /// Reached later through `afui session open` or `afui session serve`,
    /// whose own idle policy bounds the view rather than the credential
    /// itself expiring.
    Session,
}

impl UiDeliveryMode {
    /// Parse one of the three delivery words.
    ///
    /// Anything else is an error naming all three, rather than a `None` that
    /// looks the same as "not specified" — a caller resolving `--mode`
    /// against this needs to tell "the person typed something wrong" from
    /// "the person typed nothing" apart.
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "window" => Ok(Self::Window),
            "link" => Ok(Self::Link),
            "session" => Ok(Self::Session),
            other => Err(Error::InvalidDeliveryMode {
                value: other.to_owned(),
            }),
        }
    }

    /// [`DELIVERY_ENV`], read once: unset is `None`; set to one of the three
    /// words is `Some`; set to anything else — including an empty string — is
    /// an error.
    ///
    /// Deliberately not folded into a plain `Option<Self>` that also treats a
    /// bad value as absent. The reason this variable exists at all is to stop
    /// an unattended machine from popping a window nobody is there to see;
    /// silently defaulting a typo to `window` is exactly the failure this is
    /// supposed to prevent.
    pub fn from_env() -> Result<Option<Self>> {
        match std::env::var_os(DELIVERY_ENV) {
            None => Ok(None),
            Some(value) => Self::parse(&value.to_string_lossy()).map(Some),
        }
    }
}

/// The three deliveries, collapsed into one call.
///
/// ```no_run
/// use agent_first_ui::{UiDeliveryMode, UiDeliveryPlan, UiExpiry, UiSession};
/// use axum::Router;
/// use std::time::Duration;
///
/// # async fn example(explicit_mode: Option<UiDeliveryMode>, router: Router) -> agent_first_ui::Result<()> {
/// let session = UiSession::<()>::new("console", "terminal")?;
/// let outcome = UiDeliveryPlan::resolve(explicit_mode)?
///     // Omit this call to not offer `link` at all.
///     .with_link_expiry(UiExpiry::After(Duration::from_secs(900)))
///     .deliver(session, router)
///     .await?;
/// # let _ = outcome;
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct UiDeliveryPlan {
    mode: UiDeliveryMode,
    window_config: UiWindowConfig,
    link_expiry: Option<UiExpiry>,
}

/// A delivery that is already reachable but has not yet reached its terminal
/// outcome.
///
/// Splitting start from wait matters for `link`: the URL is the thing a caller
/// must hand to a person, so it has to be available before waiting for that
/// person to finish.  [`UiDeliveryPlan::deliver`] remains the one-call form for
/// callers that do not need this handoff point.
pub struct ActiveUiDelivery<T> {
    mode: UiDeliveryMode,
    session: ActiveUiSession<T>,
    window: Option<UiWindow>,
}

impl<T> ActiveUiDelivery<T>
where
    T: Send + 'static,
{
    /// The resolved delivery mode.
    #[must_use]
    pub fn mode(&self) -> UiDeliveryMode {
        self.mode
    }

    /// Stable identity shared with the registry entry.
    #[must_use]
    pub fn metadata(&self) -> &UiSessionMetadata {
        self.session.metadata()
    }

    /// The bearer-capability URL for a link delivery, available before wait.
    ///
    /// Anyone holding this URL can use the session until it expires.  Do not
    /// put it in logs, diagnostics, or ordinary persistent fields.  This
    /// accessor is deliberately limited to `link`; window/session credentials
    /// are implementation details and are never handed out by the plan.
    #[must_use]
    pub fn link_url_secret(&self) -> Option<&str> {
        (self.mode == UiDeliveryMode::Link).then(|| self.session.access_url())
    }

    /// Wait for completion, explicit close, window close, or expiry.
    pub async fn wait(mut self) -> Result<Outcome<T>> {
        match self.window.take() {
            Some(window) => self.session.wait_with_window(window).await,
            None => self.session.wait().await,
        }
    }
}

impl UiDeliveryPlan {
    /// Resolve which of the three deliveries this call uses.
    ///
    /// Priority is fixed and the same for every Provider:
    /// `explicit > `[`DELIVERY_ENV`]` > window`.
    ///
    /// `explicit` is a person's own `--mode` flag, already parsed with
    /// [`UiDeliveryMode::parse`] — pass `None` when the flag was not given.
    /// **Do not** give that flag a default value of its own: a default there
    /// is indistinguishable from a person having typed `--mode window`, so
    /// [`DELIVERY_ENV`] would never be consulted and an unattended run would
    /// always pop a window.
    pub fn resolve(explicit: Option<UiDeliveryMode>) -> Result<Self> {
        let mode = match explicit {
            Some(mode) => mode,
            None => UiDeliveryMode::from_env()?.unwrap_or(UiDeliveryMode::Window),
        };
        Ok(Self {
            mode,
            window_config: UiWindowConfig::default(),
            link_expiry: None,
        })
    }

    /// Declare that this Provider offers `link` delivery, and how long its
    /// credential lasts while nobody is watching it.
    ///
    /// Not calling this is how a Provider opts out of `link`: [`Self::deliver`]
    /// then refuses a resolved `link` mode instead of inventing an expiry for
    /// it. There is no default here because delivery contexts legitimately
    /// need different answers: no URL at all, a fixed timeout, or a sliding
    /// idle window. A session-level default cannot infer which boundary the
    /// caller promised.
    #[must_use]
    pub fn with_link_expiry(mut self, expiry: UiExpiry) -> Self {
        self.link_expiry = Some(expiry);
        self
    }

    /// Make the selected delivery reachable and return before waiting for its
    /// terminal outcome.  A link caller reads
    /// [`ActiveUiDelivery::link_url_secret`] here, hands it to the intended
    /// person through an explicitly sensitive channel, then calls
    /// [`ActiveUiDelivery::wait`].
    pub async fn start<T>(
        self,
        session: UiSession<T>,
        router: Router,
    ) -> Result<ActiveUiDelivery<T>>
    where
        T: Send + 'static,
    {
        let mode = self.mode;
        let (session, window) = match mode {
            UiDeliveryMode::Window => {
                let active = session
                    .serve(router, UiUrlDelivery::new(UiExpiry::Never))
                    .await?;
                let window = UiWindow::launch(active.access_url(), &self.window_config)?;
                (active, Some(window))
            }
            UiDeliveryMode::Link => {
                let expiry = self.link_expiry.ok_or(Error::LinkDeliveryNotDeclared)?;
                let advertised = primary_lan_ipv4().map_err(Error::LinkAddressUnavailable)?;
                let delivery = UiUrlDelivery::new(expiry)
                    .with_bind_ip(IpAddr::V4(Ipv4Addr::UNSPECIFIED))
                    .with_advertised_host(advertised.to_string());
                (session.serve(router, delivery).await?, None)
            }
            UiDeliveryMode::Session => {
                // Loopback, the default `UiUrlDelivery` bind: this mode
                // advertises no address of its own. Reaching it is `afui
                // session open` or `afui session serve`'s job, both of which
                // find it through the registry `serve` already publishes to.
                (
                    session
                        .serve(router, UiUrlDelivery::new(UiExpiry::Never))
                        .await?,
                    None,
                )
            }
        };
        Ok(ActiveUiDelivery {
            mode,
            session,
            window,
        })
    }

    /// Hand the session to a person and wait, whichever of the three ways this
    /// plan resolved to.  Equivalent to [`Self::start`] followed by
    /// [`ActiveUiDelivery::wait`].
    pub async fn deliver<T>(self, session: UiSession<T>, router: Router) -> Result<Outcome<T>>
    where
        T: Send + 'static,
    {
        self.start(session, router).await?.wait().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::ENV_LOCK as TEST_LOCK;

    fn set_env(value: &str) {
        // SAFETY: serialised by `TEST_LOCK`.
        unsafe { std::env::set_var(DELIVERY_ENV, value) };
    }

    fn clear_env() {
        // SAFETY: serialised by `TEST_LOCK`.
        unsafe { std::env::remove_var(DELIVERY_ENV) };
    }

    #[test]
    fn parse_accepts_only_the_three_words() {
        assert_eq!(
            UiDeliveryMode::parse("window").unwrap(),
            UiDeliveryMode::Window
        );
        assert_eq!(UiDeliveryMode::parse("link").unwrap(), UiDeliveryMode::Link);
        assert_eq!(
            UiDeliveryMode::parse("session").unwrap(),
            UiDeliveryMode::Session
        );
        let error = UiDeliveryMode::parse("lan").unwrap_err().to_string();
        assert!(error.contains("window"), "{error}");
        assert!(error.contains("link"), "{error}");
        assert!(error.contains("session"), "{error}");
    }

    #[test]
    fn an_unset_env_var_resolves_to_none_not_an_error() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        clear_env();
        assert_eq!(UiDeliveryMode::from_env().unwrap(), None);
    }

    #[test]
    fn an_unrecognised_env_value_is_rejected_rather_than_silently_defaulted() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        set_env("lan");
        let error = UiDeliveryMode::from_env().unwrap_err().to_string();
        clear_env();
        // The one failure mode this variable exists to prevent: a typo must
        // not read the same as "unset", which resolves to `window`.
        assert!(error.contains("window"), "{error}");
        assert!(error.contains("link"), "{error}");
        assert!(error.contains("session"), "{error}");
    }

    #[test]
    fn an_empty_env_value_is_rejected_rather_than_treated_as_unset() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        set_env("");
        assert!(UiDeliveryMode::from_env().is_err());
        clear_env();
    }

    #[test]
    fn resolve_prefers_the_explicit_mode_over_the_environment() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        set_env("session");
        let plan = UiDeliveryPlan::resolve(Some(UiDeliveryMode::Window)).unwrap();
        clear_env();
        assert_eq!(plan.mode, UiDeliveryMode::Window);
    }

    #[test]
    fn resolve_falls_back_to_the_environment_when_nothing_explicit_was_given() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        set_env("session");
        let plan = UiDeliveryPlan::resolve(None).unwrap();
        clear_env();
        assert_eq!(plan.mode, UiDeliveryMode::Session);
    }

    #[test]
    fn resolve_defaults_to_window_when_nothing_was_said_at_all() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        clear_env();
        let plan = UiDeliveryPlan::resolve(None).unwrap();
        assert_eq!(plan.mode, UiDeliveryMode::Window);
    }

    #[test]
    fn resolve_propagates_an_unrecognised_environment_value() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        set_env("lan");
        let error = UiDeliveryPlan::resolve(None).unwrap_err().to_string();
        clear_env();
        assert!(error.contains("window"), "{error}");
        assert!(error.contains("link"), "{error}");
        assert!(error.contains("session"), "{error}");
    }

    #[tokio::test]
    async fn link_delivery_is_refused_when_no_expiry_was_ever_declared() {
        let session = UiSession::<()>::new("console", "terminal").unwrap();
        let plan = UiDeliveryPlan::resolve(Some(UiDeliveryMode::Link)).unwrap();
        let error = plan.deliver(session, Router::new()).await.unwrap_err();
        assert!(matches!(error, Error::LinkDeliveryNotDeclared), "{error}");
    }

    #[tokio::test]
    async fn session_start_returns_before_the_session_is_finished() {
        let session = UiSession::<u8>::new("console", "terminal").unwrap();
        let completion = session.completion();
        let active = UiDeliveryPlan::resolve(Some(UiDeliveryMode::Session))
            .unwrap()
            .start(session, Router::new())
            .await
            .unwrap();
        assert_eq!(active.mode(), UiDeliveryMode::Session);
        assert!(active.link_url_secret().is_none());
        assert!(completion.complete(7).await);
        assert_eq!(active.wait().await.unwrap(), Outcome::Completed(7));
    }
}

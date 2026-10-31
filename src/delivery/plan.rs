//! One executor for the three ways a session reaches a person.
//!
//! [`crate::UiSession::window`] and [`crate::UiSession::serve`] already exist;
//! this lets a Provider say "I have a session" once and get all three
//! deliveries, with the choice coming from one environment-aware place instead
//! of being reinvented around each UI. `link` embeds AFUI's remote shell around
//! exactly this session, so another device can open it without an `afui` binary
//! in the handoff path.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use axum::Router;
use serde::Serialize;

use crate::lan::primary_lan_ipv4;
use crate::serve::ActiveRemoteView;
use crate::{
    ActiveUiSession, AnnouncedUiUpstream, Error, Outcome, Result, SessionServer, UiAttentionPolicy,
    UiDeliveryMode, UiExpiry, UiSession, UiSessionEntry, UiSessionMetadata, UiUpstream,
    UiUrlDelivery, UiWindow, UiWindowConfig,
};

/// The three deliveries, collapsed into one call.
///
/// ```no_run
/// use agent_first_ui::{UiDeliveryMode, UiDeliveryPlan, UiSession};
/// use axum::Router;
///
/// # async fn example(explicit_mode: Option<UiDeliveryMode>, router: Router) -> agent_first_ui::Result<()> {
/// let session = UiSession::<()>::new("console", "terminal")?;
/// // `resolve` for a Provider that stays local; `resolve_allowing_link` for
/// // one that offers the LAN page as well.
/// let outcome = UiDeliveryPlan::resolve_allowing_link(explicit_mode)?
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
}

/// Everything AFUI knows about a delivery that is already reachable, as data.
///
/// This is what a Provider's readiness event is built from. It is deliberately
/// facts and nothing else: no sentence to print, no notion of which field
/// matters more, no opinion on whether a field belongs in an event at all.
/// Those are the Provider's, because only the Provider knows what publishing
/// one of these means for its own domain.
///
/// AFUI knows nothing about how a Provider serializes its output, so this hands
/// over values rather than emitting anything. The names still follow the
/// conventions a structured-output layer reads — a `_s` suffix on a duration in
/// seconds, a `_secret` suffix on a bearer capability — so a Provider that
/// splices these straight into an event gets correct handling by default, and
/// one that deliberately publishes the URL renames that single field itself.
///
/// `Debug` prints `***` for [`Self::link_url_secret`].
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct UiDeliveryFacts {
    /// The resolved delivery, spelled the way a person types it.
    pub mode: UiDeliveryMode,
    /// Stable identity, shared with this delivery's registry entry.
    pub session_id: String,
    /// The bearer-capability URL, present for `link` and nothing else.
    ///
    /// Anyone holding it can use the session until it lapses. Handing it to
    /// the intended person is the whole point of `link` delivery, and doing
    /// so is an explicit act — not a side effect of logging this structure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link_url_secret: Option<String>,
    /// How long this delivery may sit unattended before the warning starts.
    ///
    /// Absent when nothing lapses: either this delivery has no attention clock
    /// at all (`window` and `session`), or its policy is configured never to
    /// expire. [`Self::grace_period_s`] tells those two apart — a delivery with
    /// a clock always reports one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idle_timeout_s: Option<u64>,
    /// How long the warning runs before the delivery actually lapses. Present
    /// exactly when this delivery has an attention clock.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grace_period_s: Option<u64>,
}

impl std::fmt::Debug for UiDeliveryFacts {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("UiDeliveryFacts")
            .field("mode", &self.mode)
            .field("session_id", &self.session_id)
            .field(
                "link_url_secret",
                &self.link_url_secret.as_ref().map(|_| "***"),
            )
            .field("idle_timeout_s", &self.idle_timeout_s)
            .field("grace_period_s", &self.grace_period_s)
            .finish()
    }
}

fn facts(
    mode: UiDeliveryMode,
    metadata: &UiSessionMetadata,
    remote: Option<&ActiveRemoteView>,
) -> UiDeliveryFacts {
    let policy = remote.map(ActiveRemoteView::attention_policy);
    UiDeliveryFacts {
        mode,
        session_id: metadata.session_id.to_string(),
        link_url_secret: remote.map(|remote| remote.access_url().to_owned()),
        idle_timeout_s: policy
            .and_then(|policy| policy.idle_timeout())
            .map(|idle| idle.as_secs()),
        grace_period_s: policy.map(|policy| policy.grace_period().as_secs()),
    }
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
    remote: Option<ActiveRemoteView>,
}

/// An upstream delivery that is reachable but has not ended.
///
/// The page itself is served elsewhere, but the three delivery meanings are
/// still AFUI's: a local window, an AFUI-owned fixed-session Link page, or a
/// registry-only session. Holding this value also holds the announcement, so
/// cancelling its wait withdraws the session in every mode.
pub struct ActiveUiUpstreamDelivery {
    mode: UiDeliveryMode,
    upstream: AnnouncedUiUpstream,
    window: Option<UiWindow>,
    remote: Option<ActiveRemoteView>,
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
        self.remote.as_ref().map(ActiveRemoteView::access_url)
    }

    /// The AFUI attention policy applied to a direct link.
    ///
    /// This is report-only: the Provider can tell the person how the URL
    /// behaves, but does not choose either interval. Window and registered
    /// session delivery return `None` because neither has a remote view yet.
    #[must_use]
    pub fn attention_policy(&self) -> Option<UiAttentionPolicy> {
        self.remote.as_ref().map(ActiveRemoteView::attention_policy)
    }

    /// Everything above, in one value a readiness event is built from.
    ///
    /// The accessors remain for a caller that wants one fact; this exists
    /// because most callers want all of them at the same moment — the point
    /// between "reachable" and "waiting" — and assembling them by hand is how
    /// two tools end up reporting the same delivery with different field
    /// names.
    #[must_use]
    pub fn facts(&self) -> UiDeliveryFacts {
        facts(self.mode, self.metadata(), self.remote.as_ref())
    }

    /// Wait for completion, explicit close, window close, or expiry.
    pub async fn wait(mut self) -> Result<Outcome<T>> {
        if let Some(window) = self.window.take() {
            return self.session.wait_with_window(window).await;
        }
        if let Some(mut remote) = self.remote.take() {
            return self
                .session
                .wait_or_lapse(remote.wait_until_expired())
                .await;
        }
        self.session.wait().await
    }
}

impl ActiveUiUpstreamDelivery {
    /// The resolved delivery mode.
    #[must_use]
    pub fn mode(&self) -> UiDeliveryMode {
        self.mode
    }

    /// Stable identity shared with the registry entry and direct Link page.
    #[must_use]
    pub fn metadata(&self) -> &UiSessionMetadata {
        self.upstream.metadata()
    }

    /// The AFUI-owned bearer URL for a Link delivery.
    ///
    /// The upstream credential is never returned here. The URL points at
    /// AFUI's fixed-session shell, which keeps that credential behind its
    /// streaming proxy.
    #[must_use]
    pub fn link_url_secret(&self) -> Option<&str> {
        self.remote.as_ref().map(ActiveRemoteView::access_url)
    }

    /// The global AFUI attention policy applied to a direct Link.
    #[must_use]
    pub fn attention_policy(&self) -> Option<UiAttentionPolicy> {
        self.remote.as_ref().map(ActiveRemoteView::attention_policy)
    }

    /// Everything above, in one value a readiness event is built from — the
    /// same shape and the same field names an AFUI-served delivery reports, so
    /// a Provider that hands its page over and one that keeps its own listener
    /// describe a ready UI identically.
    #[must_use]
    pub fn facts(&self) -> UiDeliveryFacts {
        facts(self.mode, self.metadata(), self.remote.as_ref())
    }

    /// Wait for the ending intrinsic to this delivery.
    ///
    /// A window ends when it closes and a Link ends when its AFUI attention
    /// policy lapses. A registered session has no delivery clock and therefore
    /// waits forever; its Provider normally races this future against its own
    /// shutdown signal. Cancelling this future drops the announcement.
    pub async fn wait(mut self) -> Result<Outcome<()>> {
        if let Some(mut window) = self.window.take() {
            window.wait_closed().await?;
            return Ok(Outcome::Closed);
        }
        if let Some(mut remote) = self.remote.take() {
            remote.wait_until_expired().await?;
            return Ok(Outcome::Expired);
        }
        std::future::pending().await
    }
}

impl UiDeliveryPlan {
    /// Resolve which delivery this call uses, with
    /// [`UiDeliveryMode::resolve`]'s fixed priority, for a Provider that stays
    /// local.
    ///
    /// `window` and `session` both keep the listener on loopback; `link` is the
    /// one mode that widens it, so it is the one mode a Provider has to ask
    /// for. Asking is [`Self::resolve_allowing_link`] — a plan built here
    /// refuses a resolved `link` **now**, not once delivery starts. That
    /// timing is the point: a Provider normally announces the resolved mode to
    /// whoever is waiting before it delivers, and a refusal that arrives after
    /// that announcement produces a readiness report that was already false
    /// when it was written.
    pub fn resolve(explicit: Option<UiDeliveryMode>) -> Result<Self> {
        Self::resolve_offering(explicit, false)
    }

    /// The same resolution for a Provider that also offers AFUI-owned `link`
    /// delivery.
    ///
    /// Choosing this constructor is the opt-in to that network exposure, and
    /// it enables exposure only: AFUI loads the remote attention and warning
    /// policy from its own global configuration, and the Provider never picks
    /// either interval.
    pub fn resolve_allowing_link(explicit: Option<UiDeliveryMode>) -> Result<Self> {
        Self::resolve_offering(explicit, true)
    }

    fn resolve_offering(explicit: Option<UiDeliveryMode>, link: bool) -> Result<Self> {
        let mode = UiDeliveryMode::resolve(explicit)?;
        if mode == UiDeliveryMode::Link && !link {
            return Err(Error::DeliveryModeNotOffered { requested: mode });
        }
        Ok(Self {
            mode,
            window_config: UiWindowConfig::default(),
        })
    }

    /// The delivery this plan resolved to, before anything is served.
    ///
    /// A Provider that reports which delivery a run used — in a readiness
    /// event, a log line, a status field — reads it here rather than running
    /// [`UiDeliveryMode::resolve`] a second time against the same inputs. Two
    /// reads of one decision are two chances to disagree.
    #[must_use]
    pub fn mode(&self) -> UiDeliveryMode {
        self.mode
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
        let (session, window, remote) = match mode {
            UiDeliveryMode::Window => {
                let active = session
                    .serve(router, UiUrlDelivery::new(UiExpiry::Never))
                    .await?;
                let window = active.launch_window(&self.window_config)?;
                (active, Some(window), None)
            }
            UiDeliveryMode::Link => {
                // Whether this Provider offers `link` at all was settled at
                // `resolve`; reaching here means it does.
                let advertised = primary_lan_ipv4().map_err(Error::LinkAddressUnavailable)?;
                // The Provider session remains loopback-only and has no clock
                // of its own. The AFUI shell below is the sole public URL and
                // the sole owner of remote lifetime.
                let active = session
                    .serve(router, UiUrlDelivery::new(UiExpiry::Never))
                    .await?;
                let remote = start_link(active.remote_entry(), advertised).await?;
                (active, None, Some(remote))
            }
            UiDeliveryMode::Session => {
                // Loopback, the default `UiUrlDelivery` bind: this mode
                // advertises no address of its own. Reaching it is `afui
                // session open` or `afui session serve`'s job, both of which
                // find it through the registry `serve` already publishes to —
                // so a write that failed there is this delivery failing.
                let mut active = session
                    .serve(router, UiUrlDelivery::new(UiExpiry::Never))
                    .await?;
                if let Some(error) = active.take_listing_failure() {
                    return Err(error);
                }
                (active, None, None)
            }
        };
        Ok(ActiveUiDelivery {
            mode,
            session,
            window,
            remote,
        })
    }

    /// Deliver a page that already answers on another listener.
    ///
    /// This is the upstream counterpart to [`Self::start`]. AFUI still owns
    /// every delivery decision: `window` opens the upstream locally, `link`
    /// places exactly that upstream behind AFUI's remote shell and attention
    /// policy, and `session` only announces it. The upstream owner remains
    /// responsible for keeping its private URL valid until this value drops.
    pub async fn start_upstream(self, upstream: UiUpstream) -> Result<ActiveUiUpstreamDelivery> {
        let mode = self.mode;
        // Only `session` depends entirely on the registry. Window and Link are
        // already deliveries of their own, so their listing remains best effort
        // exactly as it is for an AFUI-served session.
        let upstream = upstream.announce_for_delivery(mode)?;
        let (window, remote) = match mode {
            UiDeliveryMode::Window => (
                Some(UiWindow::launch(
                    upstream.access_url(),
                    &self.window_config,
                )?),
                None,
            ),
            UiDeliveryMode::Link => {
                let advertised = primary_lan_ipv4().map_err(Error::LinkAddressUnavailable)?;
                (
                    None,
                    Some(start_link(upstream.remote_entry(), advertised).await?),
                )
            }
            UiDeliveryMode::Session => (None, None),
        };
        Ok(ActiveUiUpstreamDelivery {
            mode,
            upstream,
            window,
            remote,
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

/// Start AFUI's one-session remote page for either kind of session source.
async fn start_link(
    entry: UiSessionEntry,
    advertised: std::net::Ipv4Addr,
) -> Result<ActiveRemoteView> {
    let served = SessionServer::new(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0))
        .with_advertised_host(advertised.to_string())
        .with_fixed_sessions(vec![entry])
        .bind()
        .await?;
    Ok(served.start())
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::ENV_LOCK as TEST_LOCK;

    fn set_env(value: &str) {
        // SAFETY: serialised by `TEST_LOCK`.
        unsafe { std::env::set_var(crate::DELIVERY_ENV, value) };
    }

    fn clear_env() {
        // SAFETY: serialised by `TEST_LOCK`.
        unsafe { std::env::remove_var(crate::DELIVERY_ENV) };
    }

    /// The executor resolves through the same call as the bare word list, so a
    /// Provider that hands its page over and one that keeps its own listener
    /// cannot end up reading the flag and the variable in two different
    /// orders.
    #[test]
    fn the_plan_resolves_the_mode_the_same_way() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        set_env("session");
        let plan = UiDeliveryPlan::resolve(None).unwrap();
        let explicit = UiDeliveryPlan::resolve_allowing_link(Some(UiDeliveryMode::Link)).unwrap();
        clear_env();
        assert_eq!(plan.mode(), UiDeliveryMode::Session);
        assert_eq!(explicit.mode(), UiDeliveryMode::Link);
    }

    /// The refusal lands while the plan is being built, so a Provider that
    /// reports its resolved mode before delivering never reports one that was
    /// already refused.
    #[test]
    fn link_is_refused_at_resolve_when_this_provider_does_not_offer_it() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        clear_env();
        let error = UiDeliveryPlan::resolve(Some(UiDeliveryMode::Link)).unwrap_err();
        assert!(
            matches!(error, Error::DeliveryModeNotOffered { .. }),
            "{error}"
        );
        // The refusal names what is on offer, in the words a person types.
        let message = error.to_string();
        assert!(message.contains("`window`"), "{message}");
        assert!(message.contains("`session`"), "{message}");
    }

    /// The same refusal fires for a mode that arrived through the environment
    /// rather than a flag: neither source is more trusted than the other.
    #[test]
    fn link_from_the_environment_is_refused_the_same_way() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        set_env("link");
        let refused = UiDeliveryPlan::resolve(None);
        let allowed = UiDeliveryPlan::resolve_allowing_link(None);
        clear_env();
        assert!(matches!(
            refused.unwrap_err(),
            Error::DeliveryModeNotOffered { .. }
        ));
        assert_eq!(allowed.unwrap().mode(), UiDeliveryMode::Link);
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

    /// A delivery with no remote view has no URL to hand out and no clock, and
    /// says so by leaving those fields out rather than by reporting zero.
    #[tokio::test]
    async fn a_local_delivery_reports_only_the_facts_it_has() {
        let session = UiSession::<()>::new("console", "terminal").unwrap();
        let active = UiDeliveryPlan::resolve(Some(UiDeliveryMode::Session))
            .unwrap()
            .start(session, Router::new())
            .await
            .unwrap();
        let facts = active.facts();
        assert_eq!(facts.mode, UiDeliveryMode::Session);
        assert_eq!(facts.session_id, active.metadata().session_id.to_string());
        assert!(facts.link_url_secret.is_none());
        assert!(facts.idle_timeout_s.is_none());
        assert!(facts.grace_period_s.is_none());

        let serialized = serde_json::to_value(&facts).unwrap();
        let object = serialized.as_object().unwrap();
        assert_eq!(object["mode"], "session");
        // Absent, not null: a Provider splicing this into an event must not
        // have to filter out fields that mean nothing for this delivery.
        for absent in ["link_url_secret", "idle_timeout_s", "grace_period_s"] {
            assert!(!object.contains_key(absent), "{absent} was serialized");
        }
    }

    /// The three endings are one word each, and `Completed` reports the ending
    /// rather than anything about the value it carries.
    #[test]
    fn every_outcome_has_exactly_one_ending_word() {
        assert_eq!(Outcome::Completed(7_u8).ending(), "completed");
        assert_eq!(Outcome::<u8>::Closed.ending(), "closed");
        assert_eq!(Outcome::<u8>::Expired.ending(), "expired");
    }
}

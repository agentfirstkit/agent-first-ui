//! One rendered page, handed to a person, waited on.
//!
//! Everything under this crate composes: a page, a policy, a session, a
//! delivery, the facts to report, the outcome to wait for. Composing them is
//! nine steps, and every Provider here wrote the same nine — resolve a
//! stylesheet, build a router, construct a session, attach a subject and an
//! icon and a policy, start the delivery, read the facts, wait, and turn the
//! outcome into whatever it says next.
//!
//! Measured on the thinnest watch panel in this kit, that came to a hundred
//! lines of which about eighty had nothing to do with the panel. This is those
//! eighty.
//!
//! It is deliberately the *last* layer, not the first. Everything it does is
//! reachable directly, and a Provider whose page does not fit this shape should
//! use the pieces rather than bend to it — the pieces are the library, and this
//! is a convenience over them.
//!
//! # What this will not grow
//!
//! A submission arrives as `BTreeMap<String, String>` and is parsed by a
//! function the Provider wrote. AFUI does not learn what a field means, and
//! therefore will never gain validation, field types, conditional display,
//! layout, or grouping. Anyone who needs those needs a Router, which they have
//! always had: this layer exists to remove plumbing, and a layer that started
//! deciding what a form means would be adding some.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::Router;
use axum::extract::Form;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::post;

use crate::frontend::UiFrontend;
use crate::{
    ActiveUiDelivery, Outcome, Result, UiAppIcon, UiBlobPolicy, UiDeliveryFacts, UiDeliveryPlan,
    UiPagePolicy, UiSecurityPolicy, UiSession, UiSessionMetadata, UiSessionRuntime, page_routes,
};

/// What one submission means, and what the person who submitted it sees.
type Answer<T> = Arc<dyn Fn(BTreeMap<String, String>) -> T + Send + Sync>;
type AnswerPage<T> = Arc<dyn Fn(&T, bool) -> String + Send + Sync>;

/// A page, its routes, and the session it ends with.
///
/// ```no_run
/// use agent_first_ui::{UiDeliveryPlan, UiPagePolicy, UiPageScript, UiSurface};
///
/// # async fn example(
/// #     frontend: &agent_first_ui::UiFrontend,
/// #     page: String,
/// #     stylesheet: Vec<u8>,
/// #     plan: UiDeliveryPlan,
/// # ) -> agent_first_ui::Result<()> {
/// let active = UiSurface::<()>::new("inspector", "table_inspect")?
///     .with_subject("orders")
///     .with_page_policy(UiPagePolicy::new(UiPageScript::None).allow_images())
///     .start(plan, page, stylesheet, frontend)
///     .await?;
/// let facts = active.facts();
/// let outcome = active.wait().await?;
/// # let _ = (facts, outcome);
/// # Ok(())
/// # }
/// ```
pub struct UiSurface<T> {
    session: UiSession<T>,
    routes: Vec<(String, Answer<T>)>,
    answer_page: Option<AnswerPage<T>>,
    extra: Option<Router>,
}

/// A surface that is reachable but has not ended.
pub struct ActiveUiSurface<T> {
    delivery: ActiveUiDelivery<T>,
}

impl<T> UiSurface<T>
where
    T: Clone + Send + Sync + 'static,
{
    /// A surface for one Provider and one kind of UI.
    pub fn new(provider_id: impl Into<String>, ui_kind: impl Into<String>) -> Result<Self> {
        Ok(Self {
            session: UiSession::new(provider_id, ui_kind)?,
            routes: Vec::new(),
            answer_page: None,
            extra: None,
        })
    }

    /// What this surface is about, in the Provider's own words — a table, a
    /// case reference, a wallet. Identity, not presentation: it is what tells
    /// two open sessions of the same kind apart in a list.
    #[must_use]
    pub fn with_subject(mut self, subject: impl Into<String>) -> Self {
        self.session = self.session.with_subject(subject);
        self
    }

    /// The Provider's own application icon, from its resolved frontend.
    #[must_use]
    pub fn with_app_icon(mut self, app_icon: UiAppIcon) -> Self {
        self.session = self.session.with_app_icon(app_icon);
        self
    }

    /// What this page is allowed to load.
    #[must_use]
    pub fn with_page_policy(mut self, policy: UiPagePolicy) -> Self {
        self.session = self
            .session
            .with_security_policy(policy.into_security_policy());
        self
    }

    /// A policy this surface did not assemble, for a page the switches do not
    /// cover — the same escape hatch, and the same cost, as everywhere else.
    #[must_use]
    pub fn with_security_policy(mut self, policy: UiSecurityPolicy) -> Self {
        self.session = self.session.with_security_policy(policy);
        self
    }

    /// One named submission: `POST <name>`, whose fields this function reads.
    ///
    /// The name is a route relative to the session, so it is the same string a
    /// decide runtime posts to. The fields arrive as strings because that is
    /// what a form sends; `answer` is the Provider's own parse, and AFUI never
    /// looks inside the map.
    ///
    /// Submitting completes the session with what `answer` returned. Only the
    /// first submission wins — a second arrives after the outcome is already
    /// decided, and is told so rather than quietly recorded.
    #[must_use]
    pub fn on<F>(mut self, name: impl Into<String>, answer: F) -> Self
    where
        F: Fn(BTreeMap<String, String>) -> T + Send + Sync + 'static,
    {
        self.routes.push((name.into(), Arc::new(answer)));
        self
    }

    /// What the person who submitted now sees.
    ///
    /// Called with what they answered and whether theirs was the answer that
    /// counted — `false` when the session had already ended, which is the only
    /// honest thing to show somebody whose click arrived second.
    #[must_use]
    pub fn with_answer_page<F>(mut self, page: F) -> Self
    where
        F: Fn(&T, bool) -> String + Send + Sync + 'static,
    {
        self.answer_page = Some(Arc::new(page));
        self
    }

    /// The Provider's own routes, merged alongside the page's.
    ///
    /// For static assets or domain HTTP routes this layer does not know about.
    /// Live state, actions, replies, events, and large response bodies belong
    /// to the AFUI runtime attached below, not to a second page protocol.
    #[must_use]
    pub fn merge(mut self, routes: Router) -> Self {
        self.extra = Some(match self.extra {
            Some(existing) => existing.merge(routes),
            None => routes,
        });
        self
    }

    /// Attach AFUI's typed, reconnecting runtime to this surface.
    pub fn with_runtime<Action, Reply, SurfaceState>(
        mut self,
    ) -> Result<(Self, UiSessionRuntime<Action, Reply, SurfaceState>)> {
        let (session, runtime) = self.session.with_runtime()?;
        self.session = session;
        Ok((self, runtime))
    }

    /// Attach the runtime with explicit unacknowledged-call and event-replay bounds.
    pub fn with_runtime_capacity<Action, Reply, SurfaceState>(
        mut self,
        call_capacity: usize,
        event_capacity: usize,
    ) -> Result<(Self, UiSessionRuntime<Action, Reply, SurfaceState>)> {
        let (session, runtime) = self
            .session
            .with_runtime_capacity(call_capacity, event_capacity)?;
        self.session = session;
        Ok((self, runtime))
    }

    /// Attach the runtime with AFUI-managed, policy-bounded byte bodies.
    pub fn with_runtime_and_blobs<Action, Reply, SurfaceState>(
        mut self,
        blob_policy: UiBlobPolicy,
    ) -> Result<(Self, UiSessionRuntime<Action, Reply, SurfaceState>)> {
        let (session, runtime) = self.session.with_runtime_and_blobs(blob_policy)?;
        self.session = session;
        Ok((self, runtime))
    }

    /// Attach a blob-enabled runtime with explicit call and event bounds.
    pub fn with_runtime_capacity_and_blobs<Action, Reply, SurfaceState>(
        mut self,
        call_capacity: usize,
        event_capacity: usize,
        blob_policy: UiBlobPolicy,
    ) -> Result<(Self, UiSessionRuntime<Action, Reply, SurfaceState>)> {
        let (session, runtime) = self.session.with_runtime_capacity_and_blobs(
            call_capacity,
            event_capacity,
            blob_policy,
        )?;
        self.session = session;
        Ok((self, runtime))
    }

    /// Make this surface reachable, and return before waiting for the person.
    ///
    /// The split is the same one [`UiDeliveryPlan::start`] makes and for the
    /// same reason: every delivery fact is known here, and nobody is waiting
    /// yet, so this is where a readiness report describes something that is
    /// already true.
    pub async fn start(
        self,
        plan: UiDeliveryPlan,
        page: impl Into<String>,
        stylesheet: impl Into<Vec<u8>>,
        frontend: &UiFrontend,
    ) -> Result<ActiveUiSurface<T>> {
        let completion = self.session.completion();
        let mut router = page_routes(page, stylesheet, frontend);
        for (name, answer) in self.routes {
            let completion = completion.clone();
            let answer_page = self.answer_page.clone();
            router = router.route(
                &format!("/{}", name.trim_start_matches('/')),
                post(move |Form(fields): Form<BTreeMap<String, String>>| {
                    // Cloned per request: the handler itself is reused, and
                    // Axum requires it to stay cloneable.
                    let completion = completion.clone();
                    let answer = Arc::clone(&answer);
                    let answer_page = answer_page.clone();
                    async move {
                        let value = answer(fields);
                        // Cloned before completing, because completing
                        // consumes it and what the person sees depends on
                        // what they answered.
                        let shown = value.clone();
                        let recorded = completion.complete(value).await;
                        match answer_page {
                            Some(page) => Html(page(&shown, recorded)).into_response(),
                            // A Provider with nothing to show still gets the
                            // answer recorded; the delivery is what ends.
                            None => axum::http::StatusCode::NO_CONTENT.into_response(),
                        }
                    }
                }),
            );
        }
        if let Some(extra) = self.extra {
            router = router.merge(extra);
        }
        Ok(ActiveUiSurface {
            delivery: plan.start(self.session, router).await?,
        })
    }
}

impl<T> ActiveUiSurface<T>
where
    T: Send + 'static,
{
    /// Stable identity, shared with this surface's registry entry.
    #[must_use]
    pub fn metadata(&self) -> &UiSessionMetadata {
        self.delivery.metadata()
    }

    /// Everything AFUI knows about how this surface is reaching a person.
    #[must_use]
    pub fn facts(&self) -> UiDeliveryFacts {
        self.delivery.facts()
    }

    /// Wait for a submission, an explicit close, a closed window, or expiry.
    pub async fn wait(self) -> Result<Outcome<T>> {
        self.delivery.wait().await
    }
}

/// So a Provider can hand one route's response back without importing axum's
/// response types for it.
#[must_use]
pub fn surface_page(html: impl Into<String>) -> Response {
    Html(html.into()).into_response()
}

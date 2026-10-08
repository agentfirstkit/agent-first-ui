//! One AFUI-owned remote view, whether it carries one direct session or the
//! sessions registered on this machine.
//!
//! A direct `link` supplies exactly the session its delivery just started;
//! `afui session serve` follows [`crate::list_sessions`]. Both enter the same
//! shell, proxy, attention clock, warning grace, and credential boundary. This
//! is not a new kind of session: the Provider is still the only thing that
//! understands its own page, and the remote layer asks no domain field of it.
//!
//! Two things follow from the person not being at this machine.
//!
//! **Loopback is not reachable.** `127.0.0.1` on a phone is the phone. So every
//! session is reverse-proxied onto a path this listener answers, and the proxy
//! streams: a terminal is Server-Sent Events, and a proxy that collects a body
//! before passing it on turns a live terminal into a page that never loads.
//!
//! **Loopback is not authentication either.** The listener mints one credential
//! at startup and requires it for the page and its data. That credential is
//! deliberately absent from everything a Provider UI touches: a proxied UI is
//! reached by its own mount credential governed by the remote attention
//! policy, is framed so that it cannot read the page it is in, and the requests
//! it makes are forwarded upstream through a header allowlist that has no room
//! for the page credential.
//!
//! There are two ways to frame it, and they differ in where the isolation comes
//! from. By default every session is a path on the one origin this listener
//! owns, and the frames are sandboxed without `allow-same-origin` — an opaque
//! origin, which is both what isolates them and what denies them
//! `localStorage`. With [`SessionServer::with_session_origin_host`] each session
//! instead gets an origin of its own, `s-<credential>.<host>`, and the isolation
//! is the browser's own origin boundary; the frame may then be same-origin with
//! itself, so a Provider UI that reads browser storage works.
//!
//! What this is not is a transport. The listener binds a port on this machine.
//! Getting a phone to that port is WireGuard, Tailscale, an SSH tunnel, or a
//! TLS reverse proxy — mature things that already exist and are not this.

use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{Response, StatusCode};
use axum::response::{Html, IntoResponse};
use serde_json::Value;
use tokio::net::TcpListener;

use crate::host::split_first_segment;
use crate::{Error, Result, UiAccessToken, UiExpiry, UiSecurityPolicy, UiSessionEntry};

mod addressing;
mod attention;
mod frontend;
mod mounts;
mod proxy;
pub(crate) use proxy::AFUI_REMOTE_VIEW;
mod shell;

use addressing::{
    ORIGIN_LABEL_PREFIX, PROXY_SEGMENT, advertised_authority, hostname_of, mount_secret_of,
    requested_authority, session_origin_base,
};
pub use attention::UiAttentionPolicy;
use attention::{AttentionClock, Phase};
use mounts::Mounts;
use proxy::{Framing, Proxied};

/// What an address this listener does not recognise renders in a browser.
///
/// The body is deliberately identical for a malformed, unknown, expired, or
/// revoked credential: a 404 must not become an oracle for what is running.
/// Naming the response as HTML matters on mobile Safari, which otherwise
/// downloads Axum's empty, untyped 404 as `document.txt` on a top-level
/// navigation instead of showing an error page.
const NOT_FOUND_HTML: &str =
    "<!doctype html><meta charset=\"utf-8\"><title>Not Found</title><h1>Not Found</h1>";

/// What this page's own credential renders after the page has lapsed.
///
/// Only somebody holding that credential ever sees it, which is what makes
/// saying this much safe: they held the page, so being told it is over reveals
/// nothing they did not already have. Everyone else keeps the 404 above,
/// including anyone who arrives at this port without a credential — a listener
/// that announced "an AFUI page expired here" to a passing scan would be
/// answering the question the 404 exists to refuse.
///
/// Three facts, because arriving at a dead URL raises three questions and a
/// bare 404 answers none of them: the address was right, the work is still
/// there, and getting back to it is one command on the other machine.
const GONE_HTML: &str = "<!doctype html><meta charset=\"utf-8\"><title>Page Expired</title>\
     <h1>This page expired</h1>\
     <p>Nobody was attending it, so it stopped answering and its credential was \
     revoked. The address was correct — this is not a mistyped link, and \
     reloading will not bring it back.</p>\
     <p>The sessions themselves are untouched. They are still running on the \
     machine that published them, and still listed there by \
     <code>afui session list</code>.</p>\
     <p>To reach them again, run <code>afui session serve</code> on that machine. \
     It prints a new URL; this one will never work again.</p>";

/// Serve the live sessions on this machine to a browser that is not on it.
pub struct SessionServer {
    listen: SocketAddr,
    advertised_host: Option<String>,
    session_origin_host: Option<String>,
    attention_policy: Option<UiAttentionPolicy>,
    workspace_root: Option<PathBuf>,
    source: SessionSource,
}

/// Which sessions this page is allowed to expose.
///
/// The long-running CLI page follows the cross-process registry. A direct link
/// is deliberately narrower: it receives exactly the one session the caller
/// just created, so handing out that link never reveals an unrelated session
/// running on the same machine.
enum SessionSource {
    Registry,
    Fixed(Vec<UiSessionEntry>),
}

/// Where a framed session is reached, which is the only thing that differs
/// between the two shapes this listener can take.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Addressing {
    /// Every session is a path on the listener's own origin. One name, no DNS
    /// to arrange, and frames that are isolated by being given an opaque origin
    /// — which costs them browser storage.
    Path,
    /// Every session is `s-<credential>.<base_host>`, an origin of its own.
    /// Isolation is then the browser's origin boundary rather than an attribute
    /// on a frame, and a framed UI keeps its own storage.
    PerSession { base_host: String },
}

/// A bound listener with its credential minted, not yet answering requests.
///
/// Binding and running are separate so the caller can tell the person where to
/// go before the call blocks.
pub struct ServedSessions {
    listener: TcpListener,
    state: Arc<ServeState>,
    access_url: String,
    local_addr: SocketAddr,
}

/// A direct remote page that is already answering requests.
///
/// Kept crate-private because Providers interact with it through
/// [`crate::ActiveUiDelivery`]. That is where a lapsed page becomes the typed
/// terminal outcome for the one session it carries.
pub(crate) struct ActiveRemoteView {
    access_url: String,
    state: Arc<ServeState>,
    server_task: tokio::task::JoinHandle<Result<()>>,
}

impl SessionServer {
    #[must_use]
    pub fn new(listen: SocketAddr) -> Self {
        Self {
            listen,
            advertised_host: None,
            session_origin_host: None,
            attention_policy: None,
            workspace_root: None,
            source: SessionSource::Registry,
        }
    }

    /// Where a replacement page is looked for, if the caller does not want the
    /// current directory — which is what `afui session serve` uses, the same
    /// place every Provider looks for its own.
    #[must_use]
    pub fn with_workspace_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.workspace_root = Some(root.into());
        self
    }

    /// When an unattended page lapses, and how much warning it gets.
    ///
    /// Applies to the page and to every session it frames, from the one signal:
    /// the page saying somebody is looking at it, and traffic through a frame.
    /// One policy rather than two, because a person attending the page is
    /// attending what is on it, and splitting the two would ask them to keep a
    /// terminal alive twice.
    #[must_use]
    pub fn with_attention_policy(mut self, policy: UiAttentionPolicy) -> Self {
        self.attention_policy = Some(policy);
        self
    }

    /// Restrict this listener to sessions supplied by its owning delivery.
    pub(crate) fn with_fixed_sessions(mut self, sessions: Vec<UiSessionEntry>) -> Self {
        self.source = SessionSource::Fixed(sessions);
        self
    }

    /// Advertise a different authority than the one bound.
    ///
    /// Binding `0.0.0.0` is how this becomes reachable from anywhere but here,
    /// and `0.0.0.0` is not an address anyone can open. A host without a port
    /// keeps the bound one.
    #[must_use]
    pub fn with_advertised_host(mut self, host: impl Into<String>) -> Self {
        self.advertised_host = Some(host.into());
        self
    }

    /// Give every session an origin of its own, under `host`.
    ///
    /// A session then answers at `s-<credential>.<host>` rather than at
    /// `<host>/s/<credential>/`, and the page frames it there. What that buys is
    /// browser storage: isolation stops being "the frame has no origin at all"
    /// and becomes "the frame has a different origin", which is a boundary the
    /// browser draws for free and which `localStorage` lives on the right side
    /// of.
    ///
    /// What it costs is names. `*.<host>` has to resolve to this listener, and
    /// behind TLS the certificate has to cover `*.<host>` too. `localhost` costs
    /// nothing — Chrome, Firefox and Safari all resolve `*.localhost` to the
    /// loopback address with no DNS at all — which is what makes this usable on
    /// the machine doing the work and worth arranging anywhere else.
    ///
    /// This is also the authority the page itself is advertised at, unless
    /// [`Self::with_advertised_host`] has already named one: the page and its
    /// frames have to agree on where the page is, or the frames refuse to load.
    #[must_use]
    pub fn with_session_origin_host(mut self, host: impl Into<String>) -> Self {
        let host = host.into();
        self.advertised_host.get_or_insert_with(|| host.clone());
        self.session_origin_host = Some(host);
        self
    }

    /// Bind the listener and mint the credential the person will use.
    pub async fn bind(self) -> Result<ServedSessions> {
        let attention_policy = match self.attention_policy {
            Some(policy) => policy,
            None => UiAttentionPolicy::load_global()?,
        };
        // Resolve the page before binding. An unusable trusted override should
        // cost the caller an error, not leave a port allocated for a page that
        // can never be served.
        let workspace_root = match self.workspace_root {
            Some(root) => root,
            None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        };
        let shell = frontend::ShellUi::resolve(&workspace_root)?;
        let listener = TcpListener::bind(self.listen).await.map_err(Error::Io)?;
        let local_addr = listener.local_addr().map_err(Error::Io)?;
        let addressing = match &self.session_origin_host {
            Some(host) => Addressing::PerSession {
                base_host: session_origin_base(host)?,
            },
            None => Addressing::Path,
        };
        let authority = match self.advertised_host {
            Some(host) if host.contains(':') => host,
            Some(host) => format!("{host}:{}", local_addr.port()),
            None => advertised_authority(local_addr),
        };
        // The page and its frames are one arrangement or they are nothing: a
        // frame at `s-….<base>` names `<base>` as the only page allowed to
        // embed it, so a page reached at any other name would be refused by the
        // browser with nothing on this side to explain it. Said here, once, and
        // out loud.
        if let Addressing::PerSession { base_host } = &addressing
            && hostname_of(&authority).to_ascii_lowercase() != *base_host
        {
            return Err(Error::InvalidSessionOriginHost {
                value: authority,
                reason: "the page is advertised at a host its sessions are not named under, so \
                         every frame it builds would be refused",
            });
        }
        // The credential itself never expires; what expires is the page's
        // attention (§`UiAttentionPolicy`), and that is a decision this listener
        // makes rather than one the token's own clock could make for it. A
        // token that slid forward on every authorized request would be renewed
        // by the page polling — which is the page asking a question, not a
        // person being there.
        let token = UiAccessToken::issue(UiExpiry::Never)?;
        let access_url = format!("http://{authority}/{}/", token.secret().expose_secret());
        Ok(ServedSessions {
            listener,
            state: Arc::new(ServeState {
                token,
                attention_policy,
                attention: Arc::new(AttentionClock::new()),
                shell,
                mounts: Mutex::new(Mounts::default()),
                addressing,
                source: self.source,
            }),
            access_url,
            local_addr,
        })
    }
}

impl ServedSessions {
    #[must_use]
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// The URL the person opens. It carries the credential — give it to them,
    /// not to a log.
    #[must_use]
    pub fn access_url(&self) -> &str {
        &self.access_url
    }

    /// The attention policy this listener loaded or was explicitly given.
    #[must_use]
    pub fn attention_policy(&self) -> UiAttentionPolicy {
        self.state.attention_policy
    }

    /// The workspace frontend serving this page, or `None` for AFUI's own.
    ///
    /// Reported rather than assumed: an untrusted override is skipped in
    /// silence, so this is how a person tells "my page is running" from "my page
    /// is sitting there inert waiting to be enabled".
    #[must_use]
    pub fn frontend_id(&self) -> Option<&str> {
        self.state.shell.frontend_id()
    }

    /// Answer requests until `shutdown` resolves.
    ///
    /// Deliberately not a graceful drain: a terminal's event stream has no end
    /// of its own, so waiting for in-flight requests to finish would be waiting
    /// for the person to close the page they are no longer looking at.
    pub async fn run(self, shutdown: impl Future<Output = ()> + Send) -> Result<()> {
        let app = Router::new()
            .fallback(dispatch)
            .with_state(Arc::clone(&self.state));
        let served = axum::serve(self.listener, app);
        tokio::select! {
            result = served => result.map_err(Error::Io),
            () = shutdown => Ok(()),
        }
    }

    /// Start answering in the background for one direct delivery.
    pub(crate) fn start(self) -> ActiveRemoteView {
        let access_url = self.access_url.clone();
        let state = Arc::clone(&self.state);
        let server_task = tokio::spawn(self.run(std::future::pending::<()>()));
        ActiveRemoteView {
            access_url,
            state,
            server_task,
        }
    }
}

impl ActiveRemoteView {
    pub(crate) fn access_url(&self) -> &str {
        &self.access_url
    }

    pub(crate) fn attention_policy(&self) -> UiAttentionPolicy {
        self.state.attention_policy
    }

    /// Wait until the shared attention clock lapses or the listener fails.
    pub(crate) async fn wait_until_expired(&mut self) -> Result<()> {
        tokio::select! {
            () = self.state.attention.wait_until_dead(&self.state.attention_policy) => Ok(()),
            result = &mut self.server_task => match result {
                Ok(result) => result,
                Err(error) => Err(Error::Io(std::io::Error::other(format!(
                    "remote UI listener task failed: {error}"
                )))),
            },
        }
    }
}

impl Drop for ActiveRemoteView {
    fn drop(&mut self) {
        self.server_task.abort();
    }
}

/// Everything one running listener holds.
struct ServeState {
    /// What the person proves to reach the page and its data. Never forwarded,
    /// never rendered, never in a URL a Provider UI can see.
    token: UiAccessToken,
    attention_policy: UiAttentionPolicy,
    /// The page's own clock. Its ending is final: this URL stops answering, and
    /// the way back is a new `session serve` URL from the machine, which is
    /// exactly the property that makes it a deadman rather than a nuisance.
    attention: Arc<AttentionClock>,
    /// The page this listener serves, resolved once before the port was bound.
    /// A replacement AFUI cannot use is an error at `bind`, never a listener
    /// quietly serving the built-in page to somebody expecting theirs.
    shell: frontend::ShellUi,
    mounts: Mutex<Mounts>,
    /// Fixed when the listener was bound, because it decides what the page's
    /// own address means: in one shape everything answers on it, in the other
    /// it answers for the page and nothing else.
    addressing: Addressing,
    source: SessionSource,
}

impl ServeState {
    /// The live sessions, each mounted and described for the page.
    ///
    /// Reading the registry probes each session's host over TCP, so it goes to
    /// a blocking thread rather than stalling the runtime that is serving a
    /// terminal stream.
    ///
    /// `shell_authority` is where the page asking this question is, and it is
    /// `Some` exactly when every session has an origin of its own: the frame
    /// addresses are built as names *under* it, so they have to be built from
    /// the authority the browser really used — port, or a TLS terminator's lack
    /// of one, included.
    async fn sessions(&self, shell_authority: Option<&str>) -> Result<Vec<Value>> {
        let entries =
            match &self.source {
                SessionSource::Registry => tokio::task::spawn_blocking(crate::list_sessions)
                    .await
                    .map_err(|error| Error::Io(std::io::Error::other(error.to_string())))??,
                SessionSource::Fixed(entries) => entries.clone(),
            };
        self.mount_live_sessions(&entries, shell_authority)
    }

    /// Bring the mount table in line with what is actually running, and
    /// describe the result for the page.
    ///
    /// Reading this list is deliberately **not** attending to anything. The
    /// page polls every couple of seconds whether or not a person is in front
    /// of it, so a poll that renewed what it reported would be a clock that
    /// never ran down.
    fn mount_live_sessions(
        &self,
        entries: &[UiSessionEntry],
        shell_authority: Option<&str>,
    ) -> Result<Vec<Value>> {
        self.mounts.lock().map_err(|_| poisoned())?.reconcile(
            entries,
            &self.attention_policy,
            shell_authority,
        )
    }

    /// The page's own clock, advanced.
    fn shell_phase(&self) -> Phase {
        self.attention.phase(&self.attention_policy)
    }

    /// Somebody is at the page, and looking at these sessions.
    ///
    /// The page is renewed whatever it names, because saying this at all is
    /// evidence somebody is there. A session already lapsed is skipped rather
    /// than revived — coming back is [`Self::reopen`], an act a person performs
    /// on one card, not something a heartbeat does to everything it can reach.
    fn attend(&self, session_ids: &[String]) {
        self.attention.attend();
        if let Ok(mut mounts) = self.mounts.lock() {
            mounts.attend(session_ids, &self.attention_policy);
        }
    }

    /// Put a lapsed session back on the page, with a credential of its own.
    ///
    /// The old one is not resurrected — it was revoked when the view lapsed and
    /// stays revoked. Returns whether there was a lapse to end, so the page can
    /// tell "done" from "there was nothing there".
    fn reopen(&self, session_id: &str) -> bool {
        // Mounting is `mount_live_sessions`' job — it is the only place that
        // knows the session is still running and where it answers. This only
        // takes the refusal away, and the next poll does the rest.
        let removed = match self.mounts.lock() {
            Ok(mut mounts) => mounts.revive(session_id),
            Err(_) => false,
        };
        if removed {
            self.attention.attend();
        }
        removed
    }

    fn mount_session(&self, session_id: &str) -> Option<Arc<Proxied>> {
        let proxied = self
            .mounts
            .lock()
            .ok()?
            .authorize_session(session_id, &self.attention_policy)?;
        self.attention.attend();
        Some(proxied)
    }

    /// The mount a proxy credential names, if it is still live.
    ///
    /// Traffic through a frame is the other half of attention: it is the only
    /// evidence this listener has that somebody is using a Provider UI, since
    /// what happens inside one is on an origin it cannot read. So a request
    /// that authorizes here renews both this session and the page around it.
    fn mount(&self, secret: &str) -> Option<Arc<Proxied>> {
        let proxied = self
            .mounts
            .lock()
            .ok()?
            .authorize(secret, &self.attention_policy)?;
        self.attention.attend();
        Some(proxied)
    }
}

/// Route one request to the page, or to a proxied Provider UI.
///
/// A wrong credential is a 404 in every direction, for the same reason the
/// session host answers that way: there is nothing there to authenticate
/// against, and saying otherwise would confirm that this port hosts something.
///
/// The page's own lapse ends everything this listener answers, frames included.
/// A page that stopped answering while the terminal it framed kept going would
/// leave the part that shows a person's work as the one part that outlived them
/// walking away. What the lapse changes is only what the *page's* credential is
/// answered with — a `410` that says so, rather than a 404 indistinguishable
/// from a typo. Everything else about a dead listener is unchanged.
async fn dispatch(State(state): State<Arc<ServeState>>, request: Request) -> Response<Body> {
    let lapsed = state.shell_phase() == Phase::Dead;
    match &state.addressing {
        Addressing::Path => on_one_origin(&state, lapsed, request).await,
        Addressing::PerSession { base_host } => {
            across_origins(&state, base_host, lapsed, request).await
        }
    }
}

/// The page and every session it frames, on the one authority this listener
/// answers for. What tells them apart is the first path segment.
async fn on_one_origin(state: &Arc<ServeState>, lapsed: bool, request: Request) -> Response<Body> {
    let path = request.uri().path().to_owned();
    let Some((head, remainder)) = split_first_segment(&path) else {
        return not_found();
    };
    if head == PROXY_SEGMENT {
        // A frame is not where a person is told anything. It holds a Provider's
        // UI, on an origin the page cannot read, and explaining AFUI's lifetime
        // inside one would put this listener's words where a Provider's belong.
        // The page around it says what happened.
        if lapsed {
            return not_found();
        }
        let Some((secret, rest)) = split_first_segment(&remainder) else {
            return not_found();
        };
        let Some(proxied) = state.mount(&secret) else {
            return not_found();
        };
        let mounted_at = format!("/{PROXY_SEGMENT}/{secret}/");
        return proxy::forward(
            proxied,
            Arc::clone(&state.attention),
            state.attention_policy,
            Framing::Path {
                mounted_at: &mounted_at,
            },
            &rest,
            request,
        )
        .await;
    }
    if !state.token.authorize(&head) {
        return not_found();
    }
    if lapsed {
        return gone();
    }
    shell::respond(state, &remainder, &path, None, request).await
}

/// One origin per session, and one more for the page. What tells them apart is
/// the name the browser asked for.
///
/// Nothing else is answered. A request that arrives under any other name is a
/// 404 whatever its path says — including the path a session would have had in
/// the other shape, because two addresses for one session would put a Provider
/// UI back on the page's own origin, which is the thing this shape exists to
/// stop.
async fn across_origins(
    state: &Arc<ServeState>,
    base_host: &str,
    lapsed: bool,
    request: Request,
) -> Response<Body> {
    let Some(authority) = requested_authority(&request) else {
        return not_found();
    };
    // A session's name is one label under the page's: everything after the
    // first dot is the page's own authority, as the browser wrote it.
    if let Some((label, shell_authority)) = authority.split_once('.')
        && let Some(label) = label.strip_prefix(ORIGIN_LABEL_PREFIX)
        && hostname_of(shell_authority) == base_host
    {
        // As on the one-origin shape: a frame goes quiet, the page explains.
        if lapsed {
            return not_found();
        }
        let Some(secret) = mount_secret_of(label) else {
            return not_found();
        };
        let Some(proxied) = state.mount(&secret) else {
            return not_found();
        };
        let path = request.uri().path().to_owned();
        return proxy::forward(
            proxied,
            Arc::clone(&state.attention),
            state.attention_policy,
            Framing::Origin { shell_authority },
            &path,
            request,
        )
        .await;
    }
    if hostname_of(&authority) != base_host {
        return not_found();
    }
    let path = request.uri().path().to_owned();
    let Some((head, remainder)) = split_first_segment(&path) else {
        return not_found();
    };
    if !state.token.authorize(&head) {
        return not_found();
    }
    if lapsed {
        return gone();
    }
    shell::respond(state, &remainder, &path, Some(&authority), request).await
}

fn not_found() -> Response<Body> {
    let mut response = (StatusCode::NOT_FOUND, Html(NOT_FOUND_HTML)).into_response();
    UiSecurityPolicy::isolated().apply_headers(response.headers_mut());
    response
}

/// The page's own credential, after the page is over.
///
/// `410` rather than `404` because the two say different things and a person
/// acts on the difference: gone is the address that was right and is finished,
/// which is exactly the fact a lapsed page has to convey. The page's script
/// reads the same status, so what a still-open tab reports and what a reload
/// finds cannot drift apart.
///
/// Carries no stylesheet. Every route that could serve one belongs to a shell
/// that is no longer answering, and a page whose whole content is three
/// sentences does not need one badly enough to reopen a route for it.
fn gone() -> Response<Body> {
    let mut response = (StatusCode::GONE, Html(GONE_HTML)).into_response();
    UiSecurityPolicy::isolated().apply_headers(response.headers_mut());
    response
}

fn poisoned() -> Error {
    Error::Io(std::io::Error::other("served session registry is poisoned"))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::addressing::LABEL_LENGTH;
    use super::*;

    fn entry(session_id: &str, url: &str) -> UiSessionEntry {
        UiSessionEntry {
            session_id: session_id.to_owned(),
            provider_id: "console".to_owned(),
            ui_kind: "terminal".to_owned(),
            subject: None,
            waits_for_decision: false,
            access_url_secret: url.to_owned(),
            window_url_secret: None,
            announced_at_epoch_ms: 0,
            owner_pid: 1,
        }
    }

    fn state() -> Result<ServeState> {
        state_with(UiAttentionPolicy::never())
    }

    fn state_with(attention_policy: UiAttentionPolicy) -> Result<ServeState> {
        Ok(ServeState {
            token: UiAccessToken::issue(UiExpiry::Never)?,
            attention_policy,
            attention: Arc::new(AttentionClock::new()),
            shell: frontend::ShellUi::resolve(std::path::Path::new("/nonexistent"))?,
            mounts: Mutex::new(Mounts::default()),
            addressing: Addressing::Path,
            source: SessionSource::Registry,
        })
    }

    /// The proxy credential inside a framed address, which is what the mount
    /// table keys on and what a leaked frame URL would carry.
    fn proxy_secret(card: &Value) -> String {
        card["proxy_url_secret"]
            .as_str()
            .unwrap()
            .trim_matches('/')
            .split('/')
            .nth(1)
            .unwrap()
            .to_owned()
    }

    #[test]
    fn a_sessions_own_credential_never_reaches_the_page() {
        let state = state().unwrap();
        for authority in [None, Some("phone.example:8787")] {
            let cards = state
                .mount_live_sessions(&[entry("one", "http://127.0.0.1:9/aabbccdd/")], authority)
                .unwrap();
            let rendered = serde_json::to_string(&cards).unwrap();
            assert!(
                !rendered.contains("aabbccdd"),
                "the session's own credential must not be described to the page: {rendered}"
            );
            assert!(!rendered.contains("access_url"), "{rendered}");
        }
    }

    #[test]
    fn a_session_keeps_its_path_across_refreshes_and_loses_it_when_it_ends() {
        let state = state().unwrap();
        let live = [entry("one", "http://127.0.0.1:9/aabbccdd/")];
        let first = state.mount_live_sessions(&live, None).unwrap();
        let second = state.mount_live_sessions(&live, None).unwrap();
        // A path that changed on every poll would reload the iframe every two
        // seconds, which for a terminal means losing the terminal.
        assert_eq!(first, second);

        let secret = proxy_secret(&first[0]);
        assert!(state.mount(&secret).is_some());

        assert!(state.mount_live_sessions(&[], None).unwrap().is_empty());
        assert!(
            state.mount(&secret).is_none(),
            "a proxy credential must stop working when its session ends"
        );
    }

    /// Short enough to run, long enough that a loaded machine cannot cross it
    /// between two statements.
    const IDLE: Duration = Duration::from_millis(300);
    const GRACE: Duration = Duration::from_millis(300);

    fn lapsing() -> Result<ServeState> {
        state_with(
            UiAttentionPolicy::default()
                .with_idle_timeout(IDLE)
                .with_grace_period(GRACE),
        )
    }

    #[test]
    fn a_warning_is_a_warning_and_not_the_outage_arriving_early() {
        let state = lapsing().unwrap();
        let live = [entry("one", "http://127.0.0.1:9/aabbccdd/")];
        let card = state.mount_live_sessions(&live, None).unwrap()[0].clone();
        assert_eq!(card["state"], "live");
        let secret = proxy_secret(&card);

        std::thread::sleep(IDLE);
        let warned = state.mount_live_sessions(&live, None).unwrap()[0].clone();
        assert_eq!(warned["state"], "expiring");
        assert!(warned["expires_in_ms"].as_u64().unwrap() > 0);
        // The address is the same one, so the page has no reason to rebuild the
        // frame — losing a half-filled form is what the warning exists to
        // prevent, not something it does on the way.
        assert_eq!(warned["proxy_url_secret"], card["proxy_url_secret"]);
        // And it still works, which is the whole difference between a warning
        // and an outage.
        assert!(state.mount(&secret).is_some());
    }

    #[test]
    fn nobody_attending_ends_the_view_and_revokes_the_address_it_was_reached_at() {
        let state = lapsing().unwrap();
        let live = [entry("one", "http://127.0.0.1:9/aabbccdd/")];
        let secret = proxy_secret(&state.mount_live_sessions(&live, None).unwrap()[0]);

        std::thread::sleep(IDLE);
        // Enters the warning, which is what starts the grace clock.
        let _warned = state.mount_live_sessions(&live, None).unwrap();
        std::thread::sleep(GRACE);
        let lapsed = state.mount_live_sessions(&live, None).unwrap()[0].clone();
        assert_eq!(lapsed["state"], "dead");
        // No address at all: the page has nothing to frame, rather than an
        // iframe left holding the last thing it painted.
        assert!(lapsed["proxy_url_secret"].is_null());
        assert!(
            state.mount(&secret).is_none(),
            "a frame address that leaked has to stop working, which is the only \
             thing this expiry can actually promise"
        );

        // And a poll two seconds later must not quietly undo it.
        let still = state.mount_live_sessions(&live, None).unwrap()[0].clone();
        assert_eq!(still["state"], "dead");
        assert!(still["proxy_url_secret"].is_null());
    }

    #[test]
    fn attending_during_the_warning_keeps_the_very_same_address() {
        let state = lapsing().unwrap();
        let live = [entry("one", "http://127.0.0.1:9/aabbccdd/")];
        let card = state.mount_live_sessions(&live, None).unwrap()[0].clone();

        std::thread::sleep(IDLE);
        assert_eq!(
            state.mount_live_sessions(&live, None).unwrap()[0]["state"],
            "expiring"
        );
        state.attend(&["one".to_owned()]);
        let kept = state.mount_live_sessions(&live, None).unwrap()[0].clone();
        assert_eq!(kept["state"], "live");
        assert_eq!(kept["proxy_url_secret"], card["proxy_url_secret"]);
        assert!(state.mount(&proxy_secret(&kept)).is_some());
    }

    #[test]
    fn a_lapsed_view_comes_back_only_when_asked_and_never_at_its_old_address() {
        let state = lapsing().unwrap();
        let live = [entry("one", "http://127.0.0.1:9/aabbccdd/")];
        let was = proxy_secret(&state.mount_live_sessions(&live, None).unwrap()[0]);
        std::thread::sleep(IDLE + GRACE);
        assert_eq!(
            state.mount_live_sessions(&live, None).unwrap()[0]["state"],
            "dead"
        );

        // Attending is not reviving: a heartbeat that brought back everything it
        // could name would undo the lapse on the next tick.
        state.attend(&["one".to_owned()]);
        assert_eq!(
            state.mount_live_sessions(&live, None).unwrap()[0]["state"],
            "dead"
        );

        assert!(state.reopen("one"));
        let back = state.mount_live_sessions(&live, None).unwrap()[0].clone();
        assert_eq!(back["state"], "live");
        // Reopen lands back on the very same session — the frame just inside
        // it rebuilds from scratch, which is why `docs/providers.md` asks a
        // Provider's page to be able to reconstruct itself from its own URL
        // alone — at a new address, never the revoked one.
        assert_eq!(back["session_id"], "one");
        let now = proxy_secret(&back);
        assert_ne!(now, was);
        assert!(state.mount(&now).is_some());
        assert!(
            state.mount(&was).is_none(),
            "reopening mints a credential; it does not resurrect the revoked one"
        );
    }

    #[test]
    fn traffic_through_a_frame_is_attention_and_the_page_polling_is_not() {
        let state = lapsing().unwrap();
        let live = [entry("one", "http://127.0.0.1:9/aabbccdd/")];
        let secret = proxy_secret(&state.mount_live_sessions(&live, None).unwrap()[0]);

        // Half an idle window of nothing but polls, then a request through the
        // frame: the polls must not have renewed anything and the request must.
        std::thread::sleep(IDLE / 2);
        assert_eq!(state.shell_phase(), Phase::Live);
        assert!(state.mount(&secret).is_some());
        std::thread::sleep(IDLE / 2 + IDLE / 4);
        assert_eq!(
            state.mount_live_sessions(&live, None).unwrap()[0]["state"],
            "live",
            "a request through the frame is the only evidence there is that a \
             person is using a Provider UI, and it has to count"
        );
        // The page's own clock was renewed by that same request, for the same
        // reason: something is talking to a frame this page is holding.
        assert_eq!(state.shell_phase(), Phase::Live);
    }

    /// Names the trade-off `attention.rs` documents for a desktop with
    /// several cards open: a self-refreshing panel's own traffic keeps the
    /// *page* alive, not only the card it came through — even for a
    /// neighbour whose own view lapses right on schedule because nothing
    /// ever touched it.
    #[test]
    fn traffic_through_one_session_renews_the_whole_page() {
        let state = lapsing().unwrap();
        let live = [
            entry("chatty", "http://127.0.0.1:9/aaaaaaaa/"),
            entry("quiet", "http://127.0.0.1:9/bbbbbbbb/"),
        ];
        let cards = state.mount_live_sessions(&live, None).unwrap();
        let chatty_secret = proxy_secret(&cards[0]);

        // "chatty" keeps generating frame traffic on its own; "quiet" is
        // never proxied to and never named to `attend`. Five touches well
        // under `IDLE` apart, spanning past `IDLE + GRACE` in total, so
        // "chatty" itself never comes close to lapsing while "quiet" ages
        // the whole time untouched.
        for _ in 0..5 {
            std::thread::sleep(IDLE / 2);
            assert!(state.mount(&chatty_secret).is_some());
        }

        let cards = state.mount_live_sessions(&live, None).unwrap();
        assert_eq!(
            cards[1]["state"], "dead",
            "an untouched session's own view lapses on its own schedule"
        );
        // But the page around it did not: "chatty"'s traffic alone kept the
        // shared clock renewed the whole time.
        assert_eq!(state.shell_phase(), Phase::Live);
    }

    #[test]
    fn the_page_itself_lapses_and_a_policy_of_never_does_not() {
        let lapsing = lapsing().unwrap();
        assert_eq!(lapsing.shell_phase(), Phase::Live);
        std::thread::sleep(IDLE);
        assert!(matches!(
            lapsing.shell_phase(),
            Phase::Expiring { remaining } if remaining > Duration::ZERO
        ));
        lapsing.attend(&[]);
        assert_eq!(lapsing.shell_phase(), Phase::Live);
        std::thread::sleep(IDLE + GRACE);
        assert_eq!(lapsing.shell_phase(), Phase::Dead);
        // Final: attending a dead page does not bring it back, because the
        // person who could attend it is the one who is no longer there.
        lapsing.attend(&[]);
        assert_eq!(lapsing.shell_phase(), Phase::Dead);

        let forever = state_with(UiAttentionPolicy::never()).unwrap();
        let live = [entry("one", "http://127.0.0.1:9/aabbccdd/")];
        let secret = proxy_secret(&forever.mount_live_sessions(&live, None).unwrap()[0]);
        std::thread::sleep(IDLE + GRACE);
        assert_eq!(forever.shell_phase(), Phase::Live);
        assert_eq!(
            forever.mount_live_sessions(&live, None).unwrap()[0]["state"],
            "live"
        );
        assert!(forever.mount(&secret).is_some());
    }

    #[test]
    fn a_grace_of_nothing_lapses_without_a_warning() {
        let state = state_with(
            UiAttentionPolicy::default()
                .with_idle_timeout(IDLE)
                .with_grace_period(Duration::ZERO),
        )
        .unwrap();
        std::thread::sleep(IDLE);
        assert_eq!(state.shell_phase(), Phase::Dead);
    }

    #[test]
    fn a_credential_that_names_no_mount_is_refused() {
        let state = state().unwrap();
        state
            .mount_live_sessions(&[entry("one", "http://127.0.0.1:9/aabbccdd/")], None)
            .unwrap();
        assert!(state.mount("deadbeef").is_none());
        assert!(state.mount("").is_none());
    }

    #[test]
    fn a_session_given_its_own_origin_is_one_label_under_the_pages() {
        let state = state().unwrap();
        let cards = state
            .mount_live_sessions(
                &[entry("one", "http://127.0.0.1:9/aabbccdd/")],
                Some("phone.example:8787"),
            )
            .unwrap();
        let address = cards[0]["proxy_url_secret"].as_str().unwrap();
        // Scheme-relative, so a TLS terminator in front is not mixed content.
        let label = address
            .strip_prefix("//s-")
            .and_then(|rest| rest.split_once('.'))
            .map(|(label, rest)| {
                assert_eq!(rest, "phone.example:8787/");
                label.to_owned()
            })
            .unwrap();
        // A DNS label may have 63 characters, and the credential is 32 bytes.
        // Hex would spell it in 64 — which is why it is spelled in base32.
        assert_eq!(label.len(), LABEL_LENGTH);
        assert!(label.len() <= 63, "{label}");
        // And the name round-trips to the credential the mount table keys on.
        let secret = mount_secret_of(&label).unwrap();
        assert!(state.mount(&secret).is_some());
    }
}

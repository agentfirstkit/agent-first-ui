use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{Method, Response, StatusCode};
use axum::response::IntoResponse;
use tokio::net::TcpListener;
use tokio::sync::{Notify, watch};
use tokio::task::JoinHandle;
use tower::ServiceExt;

use crate::channel::{UiSessionChannelCloser, UiSessionChannelMount};
use crate::{
    APP_ICON_PATH, Error, OriginDecision, Result, UiAccessToken, UiAppIcon, UiSecurityPolicy,
};

/// The prefix every reserved AFUI route lives under, on every session's own
/// credential — checked before the Provider's own router is ever reached, so
/// nothing a Provider routes there is reachable, shadowed or not. A Provider
/// must never claim any name under it: today those routes end a session and
/// serve its Provider-owned application icon. Reserving the whole prefix
/// rather than only today's paths is what lets AFUI add another one later
/// without it silently colliding with something a Provider already routed
/// there.
const RESERVED_PATH_PREFIX: &str = "__afui/";

/// Reserved for every session, under its own credential, on every delivery —
/// `window`, `link`, and `session` alike, because they all share this one
/// dispatch. `POST <access_url>SESSION_END_PATH` ends the session: whoever created it
/// gets `Outcome::Closed`, the same as if a person had closed its window.
///
/// Written without a leading slash because every caller appends it to a
/// credential that already ends in one —
/// `format!("{access_url}{SESSION_END_PATH}")`.
pub const SESSION_END_PATH: &str = "__afui/end";

/// The optional full-duplex channel attached to a session.
///
/// A surface opens a WebSocket at `format!("{access_url}{SESSION_CHANNEL_PATH}")`.
/// The session credential and same-origin checks are applied before the
/// upgrade, exactly as they are for every Provider route.
pub const SESSION_CHANNEL_PATH: &str = "__afui/channel";

/// Many UI sessions behind one entry point, with no listener of its own.
///
/// Sessions and ports are orthogonal. A person may have several interfaces open
/// at once — two reviews, a terminal and a monitor — and those are several
/// sessions, not several ports. Each session is reached by its own credential
/// as the first path segment; a mount routes on that and nothing else.
///
/// A Provider that already runs an Axum server does not need AFUI to bind
/// anything. It has the port; what it lacks is the routing, the expiry, the
/// revocation and the security policy that go with a session. That is this.
/// [`UiHost`] is a mount plus a listener, and both route through the same
/// dispatch — there is one implementation of credential routing in this crate,
/// not two.
///
/// The base URL is the one a browser can reach, prefix included, because that
/// is what every session URL is built from and what the cross-process registry
/// publishes. AFUI cannot guess it: only the caller knows which of its
/// interfaces it named and where in its own route table it put the mount.
///
/// ```no_run
/// use agent_first_ui::{UiExpiry, UiMount, UiSession};
/// use axum::Router;
///
/// # fn example(provider_routes: Router, session_routes: Router) -> agent_first_ui::Result<()> {
/// let mount = UiMount::new("http://127.0.0.1:8080/ui/")?;
/// // The Provider's own server, with AFUI sessions living under `/ui/`.
/// let app = provider_routes.nest_service("/ui", mount.router());
/// let session = UiSession::<()>::new("console", "terminal")?;
/// let active = mount.attach(session, session_routes, UiExpiry::Never)?;
/// assert!(active.access_url().starts_with("http://127.0.0.1:8080/ui/"));
/// # let _ = app;
/// # Ok(())
/// # }
/// ```
pub struct UiMount {
    /// Where a browser reaches this mount, prefix included. Always ends in `/`,
    /// because that is what makes a page's relative asset references resolve.
    base_url: String,
    /// The authority in `base_url`, which is also the `Host` a session accepts.
    authority: String,
    sessions: SessionRegistry,
}

/// One listener that hosts many UI sessions.
///
/// This is a [`UiMount`] with a port of its own: AFUI binds, AFUI serves, and
/// the caller does nothing but attach sessions. A Provider that already has a
/// listener and a router wants the mount by itself.
///
/// A single port survives the places a session actually has to live: a
/// container with one mapped port, a machine behind a firewall, a tunnel.
pub struct UiHost {
    mount: UiMount,
    local_addr: SocketAddr,
    shutdown_tx: watch::Sender<bool>,
    server_task: Mutex<Option<JoinHandle<Result<()>>>>,
}

type SessionRegistry = Arc<Mutex<HashMap<String, MountedSession>>>;

/// What the host needs to serve one session. Deliberately not generic over the
/// session's result type: the host routes bytes, and only the waiting caller
/// cares what a completed session returns.
struct MountedSession {
    token: UiAccessToken,
    policy: UiSecurityPolicy,
    router: Router,
    /// The Provider frontend's resolved icon. AFUI serves the bytes but never
    /// substitutes its own identity for a Provider that did not supply one.
    app_icon: Option<UiAppIcon>,
    /// Notified by [`SESSION_END_PATH`]. `notify_one` rather than `notify_waiters`:
    /// there is exactly one waiter per session — whoever called
    /// [`crate::ActiveUiSession::wait`] or `open_window` — and `notify_one`
    /// stores a permit when nobody is listening yet, so an end request that
    /// arrives before that call is not lost, and a repeat while nobody has
    /// consumed the first one is not a second, redundant permit either.
    close: Arc<Notify>,
    channel_endpoint: Option<crate::channel::UiSessionChannelEndpoint>,
    channel_closer: Option<UiSessionChannelCloser>,
}

/// Removes a session from its host when the session ends.
///
/// Held by `ActiveUiSession`, so a session that is dropped, completed, closed,
/// or expired stops being reachable without the host having to poll for it.
pub(crate) struct SessionRegistration {
    sessions: SessionRegistry,
    secret: String,
}

impl Drop for SessionRegistration {
    fn drop(&mut self) {
        if let Ok(mut sessions) = self.sessions.lock() {
            sessions.remove(&self.secret);
        }
    }
}

impl UiMount {
    /// A mount reachable at `base_url`.
    ///
    /// `base_url` is where the caller's own server answers for this mount —
    /// authority and path prefix, for example `http://127.0.0.1:8080/ui/`. A
    /// missing trailing slash is added; a query or fragment is refused, because
    /// a session URL is built by appending to this.
    ///
    /// Only `http://` is accepted. A session is reached over the loopback or
    /// LAN address of the machine that created it, and both the registry's
    /// liveness probe and `afui session serve` read that URL back on the
    /// assumption that it is plain HTTP. A TLS terminator belongs in front of
    /// the caller's listener, not in this string.
    pub fn new(base_url: impl AsRef<str>) -> Result<Self> {
        let supplied = base_url.as_ref();
        let invalid = || Error::InvalidBaseUrl {
            value: supplied.to_owned(),
        };
        let rest = supplied.strip_prefix("http://").ok_or_else(invalid)?;
        let (authority, path) = match rest.find('/') {
            Some(boundary) => (&rest[..boundary], &rest[boundary..]),
            None => (rest, "/"),
        };
        if authority.is_empty() || path.contains('?') || path.contains('#') {
            return Err(invalid());
        }
        Ok(Self {
            base_url: format!("http://{authority}{}/", path.trim_end_matches('/')),
            authority: authority.to_owned(),
            sessions: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// The routes that serve every session on this mount.
    ///
    /// Nest it at the prefix `base_url` names — `nest_service` rather than
    /// `nest`, so the whole subtree reaches this mount's dispatch rather than
    /// the caller's own 404:
    ///
    /// ```no_run
    /// # fn example(mount: &agent_first_ui::UiMount) -> axum::Router {
    /// axum::Router::new().nest_service("/ui", mount.router())
    /// # }
    /// ```
    ///
    /// Nothing outside the mount's prefix is touched: the caller's own routes,
    /// its own middleware, and its own authentication are unaffected, because a
    /// session credential is checked here and never leaves.
    pub fn router(&self) -> Router {
        Router::new()
            .fallback(dispatch)
            .with_state(Arc::clone(&self.sessions))
    }

    /// The URL every session on this mount is built from. Always ends in `/`.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// The authority sessions on this mount answer to, and refuse to answer
    /// under any other name.
    #[must_use]
    pub fn authority(&self) -> &str {
        &self.authority
    }

    /// How many sessions are currently reachable on this mount.
    #[must_use]
    pub fn session_count(&self) -> usize {
        self.sessions
            .lock()
            .map(|sessions| sessions.len())
            .unwrap_or(0)
    }

    pub(crate) fn register(
        &self,
        token: &UiAccessToken,
        policy: UiSecurityPolicy,
        app_icon: Option<UiAppIcon>,
        channel: Option<UiSessionChannelMount>,
        router: Router,
    ) -> Result<(
        String,
        SessionRegistration,
        Arc<Notify>,
        Option<UiSessionChannelCloser>,
    )> {
        let secret = token.secret().expose_secret().to_owned();
        // Pin the authority the URL actually names, so a rebound DNS name
        // pointing at this socket is refused without the Provider opting in.
        let policy = policy.with_expected_host(self.authority.clone());
        let access_url = format!("{}{secret}/", self.base_url);
        let close = Arc::new(Notify::new());
        let (channel_endpoint, channel_closer) = match channel {
            Some(channel) => (Some(channel.endpoint), Some(channel.closer)),
            None => (None, None),
        };
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| Error::Io(std::io::Error::other("UI host registry is poisoned")))?;
        sessions.insert(
            secret.clone(),
            MountedSession {
                token: token.clone(),
                policy,
                router,
                app_icon,
                close: Arc::clone(&close),
                channel_endpoint,
                channel_closer: channel_closer.clone(),
            },
        );
        drop(sessions);
        Ok((
            access_url,
            SessionRegistration {
                sessions: Arc::clone(&self.sessions),
                secret,
            },
            close,
            channel_closer,
        ))
    }
}

impl UiHost {
    /// Bind a host on `bind_ip`, letting the OS choose the port.
    pub async fn bind(bind_ip: IpAddr) -> Result<Self> {
        Self::bind_to(SocketAddr::new(bind_ip, 0)).await
    }

    /// Bind a host on loopback with an OS-chosen port.
    ///
    /// This is what a Provider opening a window wants: reachable from this
    /// machine and nowhere else.
    pub async fn bind_local() -> Result<Self> {
        Self::bind(IpAddr::V4(Ipv4Addr::LOCALHOST)).await
    }

    pub async fn bind_to(address: SocketAddr) -> Result<Self> {
        let listener = TcpListener::bind(address).await.map_err(Error::Io)?;
        let local_addr = listener.local_addr().map_err(Error::Io)?;
        let mount = UiMount::new(format!("http://{local_addr}/"))?;
        let app = mount.router();
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        let server_task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    while !*shutdown_rx.borrow() {
                        if shutdown_rx.changed().await.is_err() {
                            break;
                        }
                    }
                })
                .await
                .map_err(Error::Io)
        });
        Ok(Self {
            mount,
            local_addr,
            shutdown_tx,
            server_task: Mutex::new(Some(server_task)),
        })
    }

    /// Advertise a different authority than the one bound.
    ///
    /// Binding `0.0.0.0` is how a host becomes reachable from the LAN, but
    /// `0.0.0.0` is not an address anyone can open. The caller knows which of
    /// its interfaces to name; AFUI cannot guess. A host without a port keeps
    /// the bound one.
    #[must_use]
    pub fn with_advertised_host(mut self, host: impl Into<String>) -> Self {
        let host = host.into();
        let authority = if host.contains(':') {
            host
        } else {
            format!("{host}:{}", self.local_addr.port())
        };
        self.mount.base_url = format!("http://{authority}/");
        self.mount.authority = authority;
        self
    }

    #[must_use]
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// The authority that appears in every session URL this host hands out.
    #[must_use]
    pub fn authority(&self) -> &str {
        self.mount.authority()
    }

    /// How many sessions are currently reachable on this host.
    #[must_use]
    pub fn session_count(&self) -> usize {
        self.mount.session_count()
    }

    /// The mount this host serves. A host owns exactly one.
    pub(crate) fn mount(&self) -> &UiMount {
        &self.mount
    }

    /// Stop the listener. Mounted sessions stop being reachable.
    pub async fn shutdown(&self) -> Result<()> {
        let _ignored = self.shutdown_tx.send(true);
        let task = self
            .server_task
            .lock()
            .ok()
            .and_then(|mut task| task.take());
        let Some(task) = task else {
            return Ok(());
        };
        match task.await {
            Ok(result) => result,
            Err(join_error) if join_error.is_cancelled() => Ok(()),
            Err(join_error) => Err(Error::Io(std::io::Error::other(join_error.to_string()))),
        }
    }
}

impl Drop for UiHost {
    fn drop(&mut self) {
        let _ignored = self.shutdown_tx.send(true);
        if let Ok(mut task) = self.server_task.lock()
            && let Some(task) = task.take()
        {
            task.abort();
        }
    }
}

/// Route one request to the session named by its first path segment.
///
/// A wrong, revoked, or expired credential is a 404 rather than a 401: there is
/// no session there to authenticate against, and saying otherwise would confirm
/// that this port hosts one.
async fn dispatch(State(sessions): State<SessionRegistry>, mut request: Request) -> Response<Body> {
    let Some((secret, remainder)) = split_first_segment(request.uri().path()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(mounted) = lookup(&sessions, &secret) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    // The real authorization check: constant time, and honouring expiry and
    // revocation. It also slides an idle expiry forward.
    if !mounted.token.authorize(&secret) {
        return StatusCode::NOT_FOUND.into_response();
    }
    if mounted.policy.check_origin(&request) == OriginDecision::Rejected {
        return with_headers(StatusCode::FORBIDDEN.into_response(), &mounted.policy);
    }
    // Everything under `RESERVED_PATH_PREFIX` is this dispatch's own, ahead of
    // the Provider's router: reached here, a Provider's own route at the same
    // name is never even tried, so there is nothing to shadow. Ending and
    // Provider identity use this path under every delivery, window included,
    // rather than being shapes only some of them can produce. The whole
    // prefix is refused rather than only today's paths, so a name AFUI
    // reserves later cannot already belong to a Provider that happened to
    // route there first.
    if let Some(reserved) = reserved_route(&remainder) {
        let response = match (request.method(), reserved) {
            (&Method::GET, APP_ICON_PATH) => match mounted.app_icon {
                Some(icon) => (
                    [(
                        axum::http::header::CONTENT_TYPE,
                        axum::http::HeaderValue::from_static("image/svg+xml"),
                    )],
                    icon.as_svg().to_owned(),
                )
                    .into_response(),
                None => StatusCode::NOT_FOUND.into_response(),
            },
            (&Method::POST, SESSION_END_PATH) => {
                // `ActiveUiSession::wait` and `open_window` are the only
                // readers of `close`, so this never manufactures an outcome
                // on its own — it only wakes whichever of them is waiting.
                if let Some(channel) = &mounted.channel_closer {
                    channel.close();
                }
                mounted.close.notify_one();
                StatusCode::NO_CONTENT.into_response()
            }
            (&Method::GET, SESSION_CHANNEL_PATH) => match mounted.channel_endpoint {
                Some(endpoint) => {
                    rewrite_path(&mut request, "/");
                    match endpoint.router().oneshot(request).await {
                        Ok(response) => response,
                        Err(infallible) => match infallible {},
                    }
                }
                None => StatusCode::NOT_FOUND.into_response(),
            },
            _ => StatusCode::NOT_FOUND.into_response(),
        };
        return with_headers(response, &mounted.policy);
    }
    rewrite_path(&mut request, &remainder);
    let response = match mounted.router.oneshot(request).await {
        Ok(response) => response,
        Err(infallible) => match infallible {},
    };
    with_headers(response, &mounted.policy)
}

/// A path already stripped of its session credential, as the reserved route
/// name it names — `SESSION_END_PATH` included, its prefix — or `None` when
/// this request is not this dispatch's to answer.
fn reserved_route(remainder: &str) -> Option<&str> {
    let rest = remainder.strip_prefix('/')?;
    rest.starts_with(RESERVED_PATH_PREFIX).then_some(rest)
}

/// Clone what serving one request needs, so the registry lock is not held
/// across the Provider's handler.
fn lookup(sessions: &SessionRegistry, secret: &str) -> Option<MountedSession> {
    let sessions = sessions.lock().ok()?;
    let mounted = sessions.get(secret)?;
    Some(MountedSession {
        token: mounted.token.clone(),
        policy: mounted.policy.clone(),
        router: mounted.router.clone(),
        app_icon: mounted.app_icon.clone(),
        close: Arc::clone(&mounted.close),
        channel_endpoint: mounted.channel_endpoint.clone(),
        channel_closer: mounted.channel_closer.clone(),
    })
}

/// Split the leading segment off a request path.
///
/// Here that segment is a session credential. Returns the path the Provider's
/// router should see, so that `/{secret}`, `/{secret}/`, and
/// `/{secret}/style.css` all resolve the way the person's browser expects — the
/// trailing slash is what makes a page's relative asset references resolve
/// inside the session.
pub(crate) fn split_first_segment(path: &str) -> Option<(String, String)> {
    let rest = path.strip_prefix('/')?;
    let (candidate, remainder) = match rest.find('/') {
        Some(boundary) => (&rest[..boundary], &rest[boundary..]),
        None => (rest, ""),
    };
    if candidate.is_empty() {
        return None;
    }
    Some((
        candidate.to_owned(),
        if remainder.is_empty() {
            "/".to_owned()
        } else {
            remainder.to_owned()
        },
    ))
}

fn rewrite_path(request: &mut Request, path: &str) {
    let mut parts = request.uri().clone().into_parts();
    let path_and_query = match request.uri().query() {
        Some(query) => format!("{path}?{query}"),
        None => path.to_owned(),
    };
    if let Ok(value) = path_and_query.parse()
        && let Ok(uri) = {
            parts.path_and_query = Some(value);
            axum::http::Uri::from_parts(parts)
        }
    {
        *request.uri_mut() = uri;
    }
}

fn with_headers(mut response: Response<Body>, policy: &UiSecurityPolicy) -> Response<Body> {
    policy.apply_headers(response.headers_mut());
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_whole_reserved_prefix_is_taken_not_only_the_one_route_defined_today() {
        assert_eq!(reserved_route("/__afui/end"), Some("__afui/end"));
        assert_eq!(
            reserved_route("/__afui/app-icon.svg"),
            Some("__afui/app-icon.svg")
        );
        // A name under the prefix this dispatch does not yet recognise is
        // still this dispatch's to answer — 404, not the Provider's router —
        // which is what lets AFUI add another reserved route later without
        // it silently already belonging to a Provider that routed there.
        assert_eq!(reserved_route("/__afui/watch"), Some("__afui/watch"));
        assert_eq!(reserved_route("/__afui/"), Some("__afui/"));
        assert_eq!(
            reserved_route("__afui/end"),
            None,
            "must still carry the leading slash"
        );
        assert_eq!(reserved_route("/__afuistuff"), None, "no prefix matching");
        assert_eq!(reserved_route("/style.css"), None);
        assert_eq!(reserved_route("/"), None);
    }

    #[test]
    fn a_credential_segment_splits_off_whatever_follows_it() {
        assert_eq!(
            split_first_segment("/abc"),
            Some(("abc".to_owned(), "/".to_owned()))
        );
        assert_eq!(
            split_first_segment("/abc/"),
            Some(("abc".to_owned(), "/".to_owned()))
        );
        assert_eq!(
            split_first_segment("/abc/style.css"),
            Some(("abc".to_owned(), "/style.css".to_owned()))
        );
        assert_eq!(split_first_segment("/"), None);
        assert_eq!(split_first_segment(""), None);
    }

    #[tokio::test]
    async fn an_advertised_host_without_a_port_keeps_the_bound_one() {
        let host = UiHost::bind_local()
            .await
            .unwrap()
            .with_advertised_host("192.168.1.4");
        assert_eq!(
            host.authority(),
            format!("192.168.1.4:{}", host.local_addr().port())
        );
        assert_eq!(
            host.mount.base_url(),
            format!("http://192.168.1.4:{}/", host.local_addr().port())
        );
    }

    #[test]
    fn a_mount_url_is_normalised_to_the_prefix_sessions_are_appended_to() {
        for supplied in [
            "http://127.0.0.1:8080/ui",
            "http://127.0.0.1:8080/ui/",
            "http://127.0.0.1:8080/ui///",
        ] {
            let mount = UiMount::new(supplied).unwrap();
            assert_eq!(mount.base_url(), "http://127.0.0.1:8080/ui/", "{supplied}");
            assert_eq!(mount.authority(), "127.0.0.1:8080", "{supplied}");
        }
        let root = UiMount::new("http://127.0.0.1:8080").unwrap();
        assert_eq!(root.base_url(), "http://127.0.0.1:8080/");
    }

    #[test]
    fn a_mount_url_that_could_not_carry_a_session_is_refused() {
        // Every one of these would silently produce a session URL that either
        // does not resolve or is not the one the caller thinks it published.
        for supplied in [
            "https://example.com/ui/",
            "http:///ui/",
            "127.0.0.1:8080/ui/",
            "http://127.0.0.1:8080/ui/?tenant=1",
            "http://127.0.0.1:8080/ui/#top",
        ] {
            assert!(UiMount::new(supplied).is_err(), "{supplied}");
        }
    }
}

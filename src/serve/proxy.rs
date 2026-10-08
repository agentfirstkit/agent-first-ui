//! Reverse-proxy one Provider UI, without learning anything about it.
//!
//! Four rules shape this file, and each of them is a rule about what does
//! *not* cross.
//!
//! **Nothing of the listener's goes up.** Request headers are copied by
//! allowlist, not filtered by denylist. A cookie, an `Authorization`, an
//! invented `X-Anything` — none of them has a line here, so no credential the
//! listener holds can reach a Provider even if a page tries to send it one.
//!
//! **Nothing of the Provider's leaks sideways.** A `Set-Cookie` handed to the
//! browser is a cookie the browser decides where to send, and in the shape
//! where every proxied UI shares one origin the answer is "to the next one". So
//! none is handed to the browser, in either shape. The cookies a Provider sets
//! are kept in a jar that belongs to that session's mount and is attached to
//! that mount's requests only — see [`CookieJar`]. Isolation is then a property
//! of this file rather than of a browser honouring an attribute, which is worth
//! keeping even once the browser would honour one.
//!
//! **The body is never collected.** A terminal is Server-Sent Events. A proxy
//! that waits for a response to end before forwarding it would work perfectly
//! on every page and hang on the one that matters, so the response body is
//! handed on frame by frame, and so is the request body a person's keystrokes
//! arrive in.
//!
//! **An upgraded connection stops being HTTP, and is still relayed.** A VNC
//! canvas is RFB over WebSocket. Once both ends have agreed to switch, there
//! are no more requests to filter — so everything above happens during the
//! handshake, and after it the two connections are simply spliced.

use std::borrow::Cow;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use axum::body::{Body, Bytes, HttpBody};
use axum::extract::Request;
use axum::http::header::{
    ACCEPT, ACCEPT_LANGUAGE, CACHE_CONTROL, CONNECTION, CONTENT_LENGTH, CONTENT_SECURITY_POLICY,
    CONTENT_TYPE, COOKIE, HOST, HeaderName, HeaderValue, IF_MODIFIED_SINCE, IF_NONE_MATCH,
    LOCATION, ORIGIN, RANGE, SEC_WEBSOCKET_EXTENSIONS, SEC_WEBSOCKET_KEY, SEC_WEBSOCKET_PROTOCOL,
    SEC_WEBSOCKET_VERSION, SET_COOKIE, UPGRADE, USER_AGENT, VARY,
};
use axum::http::{HeaderMap, Method, Response, StatusCode, Uri};
use axum::response::IntoResponse;
use hyper::upgrade::OnUpgrade;
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;
use tokio::sync::watch;

use super::attention::{AttentionClock, UiAttentionPolicy};

const UPSTREAM_STEP_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_COOKIE_NAMES: usize = 256;
/// The longest lifetime a cookie is kept for, whatever `Max-Age` asks: the
/// 400 days RFC 6265bis lets a user agent cap it at. Without a cap, whether a
/// huge `Max-Age` overflows the clock depends on the platform's time type, so
/// one Provider's cookie would be dropped on one OS and kept on another.
const MAX_COOKIE_AGE: Duration = Duration::from_secs(400 * 24 * 60 * 60);

type Unavailable = Pin<Box<dyn Future<Output = ()> + Send>>;

const ACCESS_CONTROL_ALLOW_HEADERS: HeaderName =
    HeaderName::from_static("access-control-allow-headers");
const ACCESS_CONTROL_ALLOW_METHODS: HeaderName =
    HeaderName::from_static("access-control-allow-methods");
const ACCESS_CONTROL_ALLOW_ORIGIN: HeaderName =
    HeaderName::from_static("access-control-allow-origin");
const ACCESS_CONTROL_MAX_AGE: HeaderName = HeaderName::from_static("access-control-max-age");
const ACCESS_CONTROL_REQUEST_HEADERS: HeaderName =
    HeaderName::from_static("access-control-request-headers");
const ACCESS_CONTROL_REQUEST_METHOD: HeaderName =
    HeaderName::from_static("access-control-request-method");
const CONTENT_SECURITY_POLICY_REPORT_ONLY: HeaderName =
    HeaderName::from_static("content-security-policy-report-only");
const LAST_EVENT_ID: HeaderName = HeaderName::from_static("last-event-id");
const X_FRAME_OPTIONS: HeaderName = HeaderName::from_static("x-frame-options");
/// Set on every request this proxy forwards, so a Provider's runtime can tell
/// a remote view from the local window. Browsers cannot set it on a WebSocket
/// upgrade, and the allowlist above never copies a client's copy of it, so
/// the only way it reaches a Provider is through this hop.
pub(crate) const AFUI_REMOTE_VIEW: HeaderName = HeaderName::from_static("afui-remote-view");

/// The only request headers that reach a Provider.
///
/// Everything a browser adds that carries authority — `Cookie`,
/// `Authorization`, `Origin`, `Referer` — is absent by construction. `Origin`
/// and `Referer` are absent for a second reason: they describe the page the
/// Provider is framed in, and where a Provider UI sits is not a Provider's
/// business.
static FORWARDED_REQUEST_HEADERS: [HeaderName; 10] = [
    ACCEPT,
    ACCEPT_LANGUAGE,
    CACHE_CONTROL,
    CONTENT_LENGTH,
    CONTENT_TYPE,
    IF_MODIFIED_SINCE,
    IF_NONE_MATCH,
    // How an event stream says where it was cut off. Without it a reconnecting
    // terminal silently loses whatever arrived during the gap.
    LAST_EVENT_ID,
    RANGE,
    USER_AGENT,
];

/// The handshake headers an upgrade needs, forwarded only when the request is
/// one.
///
/// They carry no authority and identify nobody: a version, an extension list, a
/// subprotocol name, and a nonce the client invented for this handshake.
/// `Sec-WebSocket-Key` crosses verbatim because the `Sec-WebSocket-Accept` the
/// Provider derives from it is what the client checks — a proxy that minted its
/// own key would have to mint its own accept, and would be a second WebSocket
/// implementation rather than a relay.
static FORWARDED_UPGRADE_HEADERS: [HeaderName; 4] = [
    SEC_WEBSOCKET_EXTENSIONS,
    SEC_WEBSOCKET_KEY,
    SEC_WEBSOCKET_PROTOCOL,
    SEC_WEBSOCKET_VERSION,
];

/// Response headers this listener answers for, so the Provider's own values are
/// dropped rather than passed on.
static REWRITTEN_RESPONSE_HEADERS: [HeaderName; 6] = [
    // Rewritten: the Provider says `frame-ancestors 'none'`, which is right for
    // a window and wrong for a page that is nothing but frames.
    CONTENT_SECURITY_POLICY,
    CONTENT_SECURITY_POLICY_REPORT_ONLY,
    // Kept, not passed on: where every proxied UI shares one origin, a cookie
    // the browser held for one is a cookie it would send to the next. It goes
    // into this mount's jar instead — in both shapes, so that which one is
    // running is never what a session's cookies depend on.
    SET_COOKIE,
    // Dropped: it cannot express "only the page that framed me", and the
    // rewritten CSP says exactly that.
    X_FRAME_OPTIONS,
    // Answered here, from the request that arrived.
    ACCESS_CONTROL_ALLOW_ORIGIN,
    VARY,
];

/// Connection-level headers, which belong to one hop and not the next.
static HOP_BY_HOP_HEADERS: [HeaderName; 8] = [
    HeaderName::from_static("connection"),
    HeaderName::from_static("keep-alive"),
    HeaderName::from_static("proxy-authenticate"),
    HeaderName::from_static("proxy-authorization"),
    HeaderName::from_static("proxy-connection"),
    HeaderName::from_static("te"),
    HeaderName::from_static("trailer"),
    HeaderName::from_static("upgrade"),
];

/// Where one proxied session is answered, from the point of view of the browser
/// that asked.
///
/// The two shapes differ in exactly two places, and both are here: what a
/// Provider's own redirect has to be rewritten to, and which origin is allowed
/// to frame it. Everything else about proxying a session is the same either way.
#[derive(Debug, Clone, Copy)]
pub(super) enum Framing<'a> {
    /// A path on the listener's own origin. The page framing it is served from
    /// that same origin, so `'self'` names it exactly.
    Path { mounted_at: &'a str },
    /// An origin of this session's own, one label under the page's. The page is
    /// therefore *not* `'self'` here — it is the authority left when this
    /// session's own label is taken off the front, which is where the browser
    /// just came from.
    Origin { shell_authority: &'a str },
}

impl Framing<'_> {
    /// The address the browser reaches this session at, as a path. A session
    /// with an origin of its own owns the root of it.
    fn mounted_at(&self) -> &str {
        match self {
            Self::Path { mounted_at } => mounted_at,
            Self::Origin { .. } => "/",
        }
    }

    /// Who may frame this session, as a `frame-ancestors` value.
    ///
    /// Both schemes, because a person who put a TLS terminator in front — which
    /// is what §2.3 tells them to do — is reached over the other one, and this
    /// listener never learns which.
    fn frame_ancestors(&self) -> Cow<'static, str> {
        match self {
            Self::Path { .. } => Cow::Borrowed("'self'"),
            Self::Origin { shell_authority } => Cow::Owned(format!(
                "http://{shell_authority} https://{shell_authority}"
            )),
        }
    }
}

/// One session as this listener proxies it.
///
/// The upstream and the jar are one value because they live and die together: a
/// jar that outlived its mount would hand one session's cookies to whatever
/// took its place on the same path.
pub(super) struct Proxied {
    pub(super) upstream: Upstream,
    pub(super) attention: AttentionClock,
    cookies: CookieJar,
    revoked: watch::Sender<bool>,
}

impl Proxied {
    pub(super) fn new(upstream: Upstream) -> Self {
        Self {
            upstream,
            attention: AttentionClock::new(),
            cookies: CookieJar::default(),
            revoked: watch::channel(false).0,
        }
    }

    pub(super) fn revoke(&self) {
        self.revoked.send_replace(true);
    }

    /// One owned waiter follows the same resettable clocks as new requests.
    /// Revocation also wakes streams when reconciliation removes the mount.
    fn unavailable(
        self: &Arc<Self>,
        shell_attention: Arc<AttentionClock>,
        policy: UiAttentionPolicy,
    ) -> Unavailable {
        let proxied = Arc::clone(self);
        let mut revoked = self.revoked.subscribe();
        Box::pin(async move {
            let changed = async {
                while !*revoked.borrow_and_update() {
                    if revoked.changed().await.is_err() {
                        return;
                    }
                }
            };
            tokio::select! {
                biased;
                () = changed => {}
                () = proxied.attention.wait_until_dead(&policy) => proxied.revoke(),
                () = shell_attention.wait_until_dead(&policy) => proxied.revoke(),
            }
        })
    }
}

/// The driver belongs to this response, including an upgraded connection.
/// Dropping a timed-out request or its body closes the otherwise detached task.
struct ConnectionTask(tokio::task::JoinHandle<()>);

impl Drop for ConnectionTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Stream frames without collecting them, checking expiry before each frame.
struct ScopedBody {
    incoming: Option<Pin<Box<hyper::body::Incoming>>>,
    unavailable: Unavailable,
    connection: Option<ConnectionTask>,
}

impl HttpBody for ScopedBody {
    type Data = Bytes;
    type Error = hyper::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<hyper::body::Frame<Self::Data>, Self::Error>>> {
        let body = self.get_mut();
        if body.incoming.is_none() {
            return Poll::Ready(None);
        }
        if body.unavailable.as_mut().poll(context).is_ready() {
            body.incoming = None;
            body.connection = None;
            return Poll::Ready(None);
        }
        let Some(incoming) = body.incoming.as_mut() else {
            return Poll::Ready(None);
        };
        let frame = incoming.as_mut().poll_frame(context);
        if matches!(frame, Poll::Ready(None)) {
            body.incoming = None;
            body.connection = None;
        }
        frame
    }

    fn is_end_stream(&self) -> bool {
        self.incoming
            .as_ref()
            .is_none_or(|incoming| incoming.is_end_stream())
    }

    fn size_hint(&self) -> hyper::body::SizeHint {
        self.incoming
            .as_ref()
            .map_or_else(hyper::body::SizeHint::default, |incoming| {
                incoming.size_hint()
            })
    }
}

/// One proxied session's cookies, kept here rather than in the browser.
///
/// A cookie exists so a Provider can recognise the same visitor on the next
/// request. A panel that hands a credential over as `?handoff_secret=…` once
/// and a cookie every time after depends on exactly that, so a proxy that
/// drops cookies cannot carry it at all.
///
/// It cannot be a browser cookie in the default shape. One origin answers for
/// every framed UI there, so a cookie the browser held for one session would be
/// attached to the next session's requests, and neither `Path` scoping nor a
/// name prefix would be worth trusting: a sandboxed document has an opaque
/// origin, which is exactly the case browsers treat least consistently.
///
/// So the jar is on this side and belongs to one mount. `Set-Cookie` is still
/// dropped on the way out and `Cookie` is still absent from the request
/// allowlist, so the browser neither receives nor sends one; the only cookies a
/// Provider is ever sent are the ones it set itself, through the mount it set
/// them on. Isolation is then a property of this structure rather than of a
/// browser honouring an attribute.
///
/// Giving each session its own origin would make the browser's own cookie
/// scoping sound, and this still does not hand them back. A session's cookies
/// would then be isolated by whichever mechanism the person's shape happened to
/// select, which is a worse thing to have to reason about than one mechanism
/// that always holds — and this one costs nothing to keep.
///
/// Only `Max-Age` is honoured, because it is the attribute that decides
/// anything here: `Domain`, `Path`, `Secure`, `SameSite` and `HttpOnly` all
/// instruct a browser that is not holding this cookie. `Expires` is not
/// honoured — it would need a date parser this crate has no other use for, and
/// RFC 6265 already has `Max-Age` win wherever both appear.
#[derive(Default)]
struct CookieJar {
    cookies: Mutex<HashMap<String, Stored>>,
}

struct Stored {
    value: String,
    expires_at: Option<Instant>,
}

impl CookieJar {
    /// Take one `Set-Cookie` the Provider wrote, instead of the browser taking
    /// it.
    fn store(&self, directive: &HeaderValue) {
        let Ok(directive) = directive.to_str() else {
            return;
        };
        let mut attributes = directive.split(';');
        let Some((name, value)) = attributes
            .next()
            .and_then(|pair| pair.split_once('='))
            .map(|(name, value)| (name.trim(), value.trim()))
        else {
            return;
        };
        if name.is_empty() {
            return;
        }
        let max_age = attributes
            .filter_map(|attribute| attribute.split_once('='))
            .find(|(key, _)| key.trim().eq_ignore_ascii_case("max-age"))
            .and_then(|(_, seconds)| seconds.trim().parse::<i64>().ok());
        let Ok(mut cookies) = self.cookies.lock() else {
            return;
        };
        let now = Instant::now();
        cookies.retain(|_, stored| stored.expires_at.is_none_or(|expiry| now < expiry));
        match max_age {
            // How a Provider deletes one.
            Some(seconds) if seconds <= 0 => {
                cookies.remove(name);
            }
            _ => {
                let expires_at = match max_age {
                    Some(seconds) => {
                        let Ok(seconds) = u64::try_from(seconds) else {
                            return;
                        };
                        let Some(expiry) =
                            now.checked_add(Duration::from_secs(seconds).min(MAX_COOKIE_AGE))
                        else {
                            return;
                        };
                        Some(expiry)
                    }
                    None => None,
                };
                if !cookies.contains_key(name) && cookies.len() >= MAX_COOKIE_NAMES {
                    return;
                }
                cookies.insert(
                    name.to_owned(),
                    Stored {
                        value: value.to_owned(),
                        expires_at,
                    },
                );
            }
        }
    }

    /// What this mount, and only this mount, sends upstream.
    fn header(&self) -> Option<HeaderValue> {
        let mut cookies = self.cookies.lock().ok()?;
        let now = Instant::now();
        cookies.retain(|_, stored| stored.expires_at.is_none_or(|expiry| now < expiry));
        if cookies.is_empty() {
            return None;
        }
        // Sorted so the same jar produces the same header twice running.
        let mut pairs: Vec<String> = cookies
            .iter()
            .map(|(name, stored)| format!("{name}={}", stored.value))
            .collect();
        pairs.sort();
        HeaderValue::from_str(&pairs.join("; ")).ok()
    }
}

/// Where one proxied session actually lives.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Upstream {
    authority: String,
    /// The prefix this session's host routes on — for a session AFUI serves,
    /// that is its credential. Read from the registry, never leaves this
    /// process.
    base_path: String,
    /// The part of the address that is not a path.
    ///
    /// A session AFUI serves never has one. A session that only *names* an
    /// upstream may: a panel that puts its handoff credential in
    /// `?handoff_secret=…` makes that as much a part of the address as the
    /// path is. It is reproduced on every request to this mount rather than
    /// interpreted — the proxy neither reads these parameters nor knows what
    /// they mean, exactly as it neither reads nor knows the path prefix.
    base_query: Option<String>,
}

impl Upstream {
    pub(super) fn parse(access_url: &str) -> Option<Self> {
        let rest = access_url.strip_prefix("http://")?;
        let boundary = rest.find(['/', '?']).unwrap_or(rest.len());
        let (authority, remainder) = rest.split_at(boundary);
        if authority.is_empty() {
            return None;
        }
        let (path, query) = match remainder.split_once('?') {
            Some((path, query)) => (path, Some(query.to_owned()).filter(|q| !q.is_empty())),
            None => (remainder, None),
        };
        Some(Self {
            authority: authority.to_owned(),
            base_path: path.trim_end_matches('/').to_owned(),
            base_query: query,
        })
    }

    /// The address one forwarded request is asked for upstream.
    ///
    /// The session's own query comes first so that a Provider reading the first
    /// occurrence of a parameter reads the announced one — the address a
    /// person was given wins over anything a page later put in a link.
    fn target(&self, remainder: &str, query: Option<&str>) -> String {
        match (&self.base_query, query) {
            (None, None) => format!("{}{remainder}", self.base_path),
            (None, Some(query)) => format!("{}{remainder}?{query}", self.base_path),
            (Some(base), None) => format!("{}{remainder}?{base}", self.base_path),
            (Some(base), Some(query)) => {
                format!("{}{remainder}?{base}&{query}", self.base_path)
            }
        }
    }
}

/// Pass one request to a Provider UI and stream its answer back.
///
/// `framing` is where the browser reaches this session, and it is here for two
/// reasons. A Provider that redirects names its own address, and its own
/// address contains its own credential: sending that to the browser would both
/// hand out a credential that never leaves this process and point a phone at a
/// loopback port that, on a phone, is the phone. And a Provider that says
/// `frame-ancestors 'none'` has to be told which page may frame it, which is a
/// different origin in each shape.
pub(super) async fn forward(
    proxied: Arc<Proxied>,
    shell_attention: Arc<AttentionClock>,
    attention_policy: UiAttentionPolicy,
    framing: Framing<'_>,
    remainder: &str,
    mut request: Request,
) -> Response<Body> {
    let upstream = &proxied.upstream;
    if !origin_allowed(request.headers()) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if is_preflight(&request) {
        // Answered here rather than upstream. A Provider that serves a window
        // has no reason to have written a CORS route, and being framed is not a
        // reason to make it write one.
        return preflight_response(request.headers());
    }
    let origin = request.headers().get(ORIGIN).cloned();
    // Taken before the request is pulled apart: the handle onto the connection
    // this request arrived on lives in its extensions, and it is the only way
    // back to the socket once the exchange stops being HTTP.
    let protocol = upgrade_requested(request.headers());
    let downstream = protocol.as_ref().map(|_| hyper::upgrade::on(&mut request));
    let (parts, body) = request.into_parts();
    let target = upstream.target(remainder, parts.uri.query());
    let Ok(uri) = target.parse::<Uri>() else {
        return bad_gateway();
    };
    let Ok(host) = HeaderValue::from_str(&upstream.authority) else {
        return bad_gateway();
    };
    let mut outbound = hyper::Request::builder().method(parts.method).uri(uri);
    if let Some(headers) = outbound.headers_mut() {
        headers.insert(HOST, host);
        headers.insert(AFUI_REMOTE_VIEW, HeaderValue::from_static("1"));
        for name in &FORWARDED_REQUEST_HEADERS {
            for value in parts.headers.get_all(name) {
                headers.append(name, value.clone());
            }
        }
        // Not the browser's cookies — it has none for this origin and its
        // `Cookie` header has no line in the allowlist above. These are the
        // ones this Provider set on this mount, coming back to it.
        if let Some(cookie) = proxied.cookies.header() {
            headers.insert(COOKIE, cookie);
        }
        if let Some(protocol) = &protocol {
            // Both are hop-by-hop, and this proxy is the hop asking.
            headers.insert(CONNECTION, HeaderValue::from_static("upgrade"));
            headers.insert(UPGRADE, protocol.clone());
            for name in &FORWARDED_UPGRADE_HEADERS {
                for value in parts.headers.get_all(name) {
                    headers.append(name, value.clone());
                }
            }
        }
    }
    let Ok(outbound) = outbound.body(body) else {
        return bad_gateway();
    };
    let mut unavailable = proxied.unavailable(Arc::clone(&shell_attention), attention_policy);
    let connected = tokio::select! {
        biased;
        () = &mut unavailable => return super::not_found(),
        result = tokio::time::timeout(UPSTREAM_STEP_TIMEOUT, TcpStream::connect(&upstream.authority)) => result,
    };
    let Ok(Ok(stream)) = connected else {
        return bad_gateway();
    };
    let handshaken = tokio::select! {
        biased;
        () = &mut unavailable => return super::not_found(),
        result = tokio::time::timeout(UPSTREAM_STEP_TIMEOUT, hyper::client::conn::http1::handshake(TokioIo::new(stream))) => result,
    };
    let Ok(Ok((mut sender, connection))) = handshaken else {
        return bad_gateway();
    };
    // One connection per proxied request, driven for as long as the body is
    // still arriving. Nothing is pooled: an event stream that shares a
    // connection with a form post is an event stream waiting behind it.
    //
    // `with_upgrades` is what lets that connection stop being HTTP part way
    // through and be handed back as a socket instead.
    // The driver also listens independently: a downstream socket under
    // backpressure may stop polling its response body for a while.
    let mut connection_unavailable = proxied.unavailable(shell_attention, attention_policy);
    let connection = ConnectionTask(tokio::spawn(async move {
        tokio::select! {
            biased;
            () = &mut connection_unavailable => {}
            result = connection.with_upgrades() => { let _ignored = result; }
        }
    }));
    let answered = tokio::select! {
        biased;
        () = &mut unavailable => return super::not_found(),
        result = tokio::time::timeout(UPSTREAM_STEP_TIMEOUT, sender.send_request(outbound)) => result,
    };
    let Ok(Ok(mut response)) = answered else {
        return bad_gateway();
    };
    for directive in response.headers().get_all(SET_COOKIE) {
        proxied.cookies.store(directive);
    }
    let ancestors = framing.frame_ancestors();
    if response.status() == StatusCode::SWITCHING_PROTOCOLS
        && let Some(downstream) = downstream
    {
        return splice(
            &mut response,
            downstream,
            &ancestors,
            unavailable,
            connection,
        );
    }
    let (parts, incoming) = response.into_parts();
    // `Body::new` wraps rather than reads: the frames the Provider produces are
    // the frames the person receives, when the Provider produces them.
    let mut response = Response::new(Body::new(ScopedBody {
        incoming: Some(Box::pin(incoming)),
        unavailable,
        connection: Some(connection),
    }));
    *response.status_mut() = parts.status;
    copy_response_headers(&parts.headers, response.headers_mut(), &ancestors);
    relocate(response.headers_mut(), upstream, framing.mounted_at());
    allow_reading(response.headers_mut(), origin.as_ref());
    response
}

/// Whether this request is asking to stop speaking HTTP, and what it wants to
/// speak instead.
///
/// `Connection` is a comma-separated list of the headers that belong to this
/// hop only, so `upgrade` has to be looked for inside it rather than compared
/// against the whole value.
fn upgrade_requested(headers: &HeaderMap) -> Option<HeaderValue> {
    let asked = headers.get_all(CONNECTION).iter().any(|value| {
        value.to_str().is_ok_and(|value| {
            value
                .split(',')
                .any(|token| token.trim().eq_ignore_ascii_case("upgrade"))
        })
    });
    asked.then(|| headers.get(UPGRADE).cloned()).flatten()
}

/// Join the two connections once both ends have agreed to switch.
///
/// After a `101` there is no more HTTP to inspect in either direction — a VNC
/// canvas is RFB frames, and framing them again would be inventing a protocol.
/// Everything this proxy decides was decided during the handshake above; from
/// here it copies bytes until one side stops.
fn splice(
    response: &mut hyper::Response<hyper::body::Incoming>,
    downstream: OnUpgrade,
    ancestors: &str,
    mut unavailable: Unavailable,
    connection: ConnectionTask,
) -> Response<Body> {
    let upstream = hyper::upgrade::on(&mut *response);
    tokio::spawn(async move {
        let _connection = connection;
        let relay = async move {
            let (Ok(downstream), Ok(upstream)) = (downstream.await, upstream.await) else {
                return;
            };
            let mut downstream = TokioIo::new(downstream);
            let mut upstream = TokioIo::new(upstream);
            let _ignored = tokio::io::copy_bidirectional(&mut downstream, &mut upstream).await;
        };
        tokio::select! {
            biased;
            () = &mut unavailable => {}
            () = relay => {}
        }
    });
    let mut switching = Response::new(Body::empty());
    *switching.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
    let headers = switching.headers_mut();
    // `Sec-WebSocket-Accept` is among these: it is the Provider's answer to the
    // client's own key, and the client is the one that checks it.
    copy_response_headers(response.headers(), headers, ancestors);
    // Dropped as hop-by-hop on the way through, and this is the hop they
    // describe, so they are written rather than copied.
    headers.insert(CONNECTION, HeaderValue::from_static("upgrade"));
    if let Some(protocol) = response.headers().get(UPGRADE) {
        headers.insert(UPGRADE, protocol.clone());
    }
    switching
}

/// Point a Provider's redirect back at the path the browser can reach.
///
/// A `Location` the Provider wrote names its own host and its own credentialed
/// base path. Rewritten, it names this listener and the proxy path instead; a
/// `Location` that points somewhere else entirely is left alone, because a
/// Provider sending someone off its own site is not this proxy's business.
fn relocate(headers: &mut HeaderMap, upstream: &Upstream, mounted_at: &str) {
    let Some(location) = headers.get(LOCATION).and_then(|value| value.to_str().ok()) else {
        return;
    };
    let absolute = format!("http://{}{}", upstream.authority, upstream.base_path);
    let rewritten = if let Some(rest) = location.strip_prefix(&absolute) {
        format!("{mounted_at}{}", rest.trim_start_matches('/'))
    } else if let Some(rest) = location.strip_prefix(&upstream.base_path)
        && (rest.is_empty() || rest.starts_with('/'))
    {
        format!("{mounted_at}{}", rest.trim_start_matches('/'))
    } else {
        return;
    };
    if let Ok(value) = HeaderValue::from_str(&rewritten) {
        headers.insert(LOCATION, value);
    }
}

/// Who may talk to a proxied UI at all.
///
/// `null` is the framed UI itself: a sandboxed document has an opaque origin,
/// so every request it makes — including to the URL it was loaded from — says
/// `null`. The scheme pair covers the person having put a TLS terminator in
/// front, which is exactly what §2.3 tells them to do.
fn origin_allowed(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(ORIGIN).and_then(|value| value.to_str().ok()) else {
        return true;
    };
    if origin == "null" {
        return true;
    }
    headers
        .get(HOST)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|host| {
            origin == format!("http://{host}") || origin == format!("https://{host}")
        })
}

fn is_preflight(request: &Request) -> bool {
    request.method() == Method::OPTIONS
        && request
            .headers()
            .contains_key(ACCESS_CONTROL_REQUEST_METHOD)
}

fn preflight_response(headers: &HeaderMap) -> Response<Body> {
    let mut response = StatusCode::NO_CONTENT.into_response();
    let out = response.headers_mut();
    if let Some(method) = headers.get(ACCESS_CONTROL_REQUEST_METHOD) {
        out.insert(ACCESS_CONTROL_ALLOW_METHODS, method.clone());
    }
    if let Some(requested) = headers.get(ACCESS_CONTROL_REQUEST_HEADERS) {
        out.insert(ACCESS_CONTROL_ALLOW_HEADERS, requested.clone());
    }
    out.insert(ACCESS_CONTROL_MAX_AGE, HeaderValue::from_static("600"));
    allow_reading(out, headers.get(ORIGIN));
    response
}

/// Let a framed UI read its own answers.
///
/// A sandboxed document is cross-origin to the URL it was served from, so
/// without this its own `fetch` and `EventSource` calls fail. Only the opaque
/// origin is answered — a real page that guessed the URL is told nothing — and
/// credentials are never allowed, because there are none to send: the proxy
/// path is the whole authorization.
fn allow_reading(headers: &mut HeaderMap, origin: Option<&HeaderValue>) {
    headers.insert(VARY, HeaderValue::from_static("Origin"));
    if origin.is_some_and(|origin| origin == "null") {
        headers.insert(
            ACCESS_CONTROL_ALLOW_ORIGIN,
            HeaderValue::from_static("null"),
        );
    }
}

fn copy_response_headers(from: &HeaderMap, to: &mut HeaderMap, ancestors: &str) {
    for (name, value) in from {
        if REWRITTEN_RESPONSE_HEADERS.contains(name) || HOP_BY_HOP_HEADERS.contains(name) {
            continue;
        }
        // Framing was decoded on the way in and is re-encoded on the way out;
        // repeating the upstream's answer would describe a body that no longer
        // exists in that shape.
        if name == axum::http::header::TRANSFER_ENCODING {
            continue;
        }
        to.append(name, value.clone());
    }
    if let Some(policy) = from
        .get(CONTENT_SECURITY_POLICY)
        .and_then(|value| value.to_str().ok())
        .map(|policy| framed_policy(policy, ancestors))
        .and_then(|policy| HeaderValue::from_str(&policy).ok())
    {
        to.insert(CONTENT_SECURITY_POLICY, policy);
    }
}

/// Let the page that framed this UI frame it, and nothing else.
///
/// A Provider writes `frame-ancestors 'none'` because its own delivery is a
/// window nobody should be able to embed. Framed, that directive is the one
/// thing standing between the person and their own page, so it is replaced by
/// the narrowest value that names that page — and which value that is depends
/// on the shape: `'self'` when the page and this session share an origin, and
/// the page's own authority when they no longer do. Getting this wrong does not
/// leak anything; it blanks every frame.
fn framed_policy(policy: &str, ancestors: &str) -> String {
    let mut directives: Vec<String> = Vec::new();
    let mut replaced = false;
    for directive in policy.split(';') {
        let directive = directive.trim();
        if directive.is_empty() {
            continue;
        }
        let name = directive.split_whitespace().next().unwrap_or_default();
        if name.eq_ignore_ascii_case("frame-ancestors") {
            directives.push(format!("frame-ancestors {ancestors}"));
            replaced = true;
        } else {
            directives.push(directive.to_owned());
        }
    }
    if !replaced {
        // A Provider that said nothing about framing gets the same answer as
        // one that did: only the page in front of it.
        directives.push(format!("frame-ancestors {ancestors}"));
    }
    directives.join("; ")
}

fn bad_gateway() -> Response<Body> {
    StatusCode::BAD_GATEWAY.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_upstream_keeps_the_sessions_credential_as_its_path_prefix() {
        let upstream = Upstream::parse("http://127.0.0.1:8080/abc123/").unwrap();
        assert_eq!(upstream.authority, "127.0.0.1:8080");
        assert_eq!(upstream.base_path, "/abc123");
        assert_eq!(
            Upstream::parse("http://127.0.0.1:8080/abc123"),
            Some(Upstream {
                authority: "127.0.0.1:8080".to_owned(),
                base_path: "/abc123".to_owned(),
                base_query: None,
            })
        );
        assert!(Upstream::parse("https://example.com/abc/").is_none());
        assert!(Upstream::parse("http:///abc/").is_none());
    }

    #[test]
    fn an_announced_upstream_keeps_the_query_its_credential_lives_in() {
        // A handoff credential carried in the query. Dropping it would proxy
        // every request to a listener that answers 401.
        let upstream =
            Upstream::parse("http://127.0.0.1:9222/takeover/panel?handoff_secret=deadbeef")
                .unwrap();
        assert_eq!(upstream.base_path, "/takeover/panel");
        assert_eq!(
            upstream.base_query.as_deref(),
            Some("handoff_secret=deadbeef")
        );
        // Every request to this mount is asked for with the announced address,
        // not just the first one.
        assert_eq!(
            upstream.target("/", None),
            "/takeover/panel/?handoff_secret=deadbeef"
        );
        assert_eq!(
            upstream.target("/websockify", Some("path=s%2Fabc%2Fwebsockify")),
            "/takeover/panel/websockify?handoff_secret=deadbeef&path=s%2Fabc%2Fwebsockify"
        );
        // A session AFUI serves is unchanged: no query announced, none added.
        let served = Upstream::parse("http://127.0.0.1:8080/abc123/").unwrap();
        assert_eq!(served.target("/style.css", None), "/abc123/style.css");
        assert_eq!(served.target("/x", Some("a=1")), "/abc123/x?a=1");
    }

    #[test]
    fn a_re_minted_upstream_is_a_different_upstream() {
        // The mount table reuses a proxy path only while the address behind it
        // is the same one. A fresh credential is a fresh address, so the frame
        // is pointed at a new mount rather than kept on a dead credential.
        assert_ne!(
            Upstream::parse("http://127.0.0.1:9222/takeover/panel?handoff_secret=one"),
            Upstream::parse("http://127.0.0.1:9222/takeover/panel?handoff_secret=two")
        );
    }

    #[test]
    fn framing_is_narrowed_to_the_page_in_front_rather_than_removed() {
        let ancestors = Framing::Path {
            mounted_at: "/s/abc/",
        }
        .frame_ancestors();
        assert_eq!(
            framed_policy(
                "default-src 'none'; frame-ancestors 'none'; connect-src 'self'",
                &ancestors
            ),
            "default-src 'none'; frame-ancestors 'self'; connect-src 'self'"
        );
        assert_eq!(
            framed_policy("default-src 'self'", &ancestors),
            "default-src 'self'; frame-ancestors 'self'"
        );
        assert_eq!(
            framed_policy("FRAME-ANCESTORS https://elsewhere.example", &ancestors),
            "frame-ancestors 'self'"
        );
    }

    #[test]
    fn a_session_on_its_own_origin_names_the_page_rather_than_itself() {
        // The negative control is `'self'`: it is right in the other shape and
        // catastrophic in this one, because here `'self'` is the session's own
        // origin and the page is a different one, so every frame would blank.
        let framing = Framing::Origin {
            shell_authority: "phone.example:8787",
        };
        let ancestors = framing.frame_ancestors();
        assert_eq!(
            framed_policy("default-src 'none'; frame-ancestors 'none'", &ancestors),
            "default-src 'none'; frame-ancestors http://phone.example:8787 \
             https://phone.example:8787"
        );
        assert_eq!(
            framed_policy("default-src 'self'", &ancestors),
            "default-src 'self'; frame-ancestors http://phone.example:8787 \
             https://phone.example:8787"
        );
        // And the session owns the root of its own origin, so a Provider's
        // redirect is rewritten to a path rather than under a mount prefix.
        assert_eq!(framing.mounted_at(), "/");
        let upstream = Upstream::parse("http://127.0.0.1:8080/sessionsecret/").unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(LOCATION, HeaderValue::from_static("/sessionsecret/done"));
        relocate(&mut headers, &upstream, framing.mounted_at());
        assert_eq!(headers[LOCATION], "/done");
    }

    #[test]
    fn only_the_framed_ui_is_allowed_to_read_what_it_asked_for() {
        let mut headers = HeaderMap::new();
        allow_reading(&mut headers, Some(&HeaderValue::from_static("null")));
        assert_eq!(headers[ACCESS_CONTROL_ALLOW_ORIGIN], "null");

        let mut headers = HeaderMap::new();
        allow_reading(
            &mut headers,
            Some(&HeaderValue::from_static("https://attacker.example")),
        );
        assert!(!headers.contains_key(ACCESS_CONTROL_ALLOW_ORIGIN));
        assert_eq!(headers[VARY], "Origin");
    }

    #[test]
    fn a_provider_is_never_told_which_page_framed_it() {
        for headers in [
            FORWARDED_REQUEST_HEADERS.as_slice(),
            FORWARDED_UPGRADE_HEADERS.as_slice(),
        ] {
            assert!(!headers.contains(&ORIGIN));
            assert!(!headers.contains(&axum::http::header::REFERER));
            assert!(!headers.contains(&COOKIE));
            assert!(!headers.contains(&axum::http::header::AUTHORIZATION));
        }
    }

    /// What this jar would put on the next request to its own mount.
    fn sent(jar: &CookieJar) -> Option<String> {
        jar.header()
            .and_then(|value| value.to_str().map(str::to_owned).ok())
    }

    #[test]
    fn a_jar_holds_one_mounts_cookies_and_forgets_the_ones_it_was_told_to() {
        let jar = CookieJar::default();
        assert_eq!(sent(&jar), None);

        jar.store(&HeaderValue::from_static(
            "provider_handoff=abc123; Path=/takeover; Max-Age=600; HttpOnly; SameSite=Lax",
        ));
        // Only the pair crosses. Every other attribute instructs a browser, and
        // no browser is holding this.
        assert_eq!(sent(&jar).as_deref(), Some("provider_handoff=abc123"));

        jar.store(&HeaderValue::from_static("theme=dark"));
        assert_eq!(
            sent(&jar).as_deref(),
            Some("provider_handoff=abc123; theme=dark")
        );

        // A later value for the same name replaces the earlier one rather than
        // being sent alongside it.
        jar.store(&HeaderValue::from_static("theme=light"));
        assert_eq!(
            sent(&jar).as_deref(),
            Some("provider_handoff=abc123; theme=light")
        );

        // How a Provider deletes one, and the only reason this jar reads
        // `Max-Age` at all.
        jar.store(&HeaderValue::from_static(
            "provider_handoff=; Path=/takeover; Max-Age=0",
        ));
        assert_eq!(sent(&jar).as_deref(), Some("theme=light"));
    }

    #[test]
    fn a_jar_stops_offering_a_cookie_once_its_lifetime_is_over() {
        let jar = CookieJar::default();
        jar.store(&HeaderValue::from_static("gone=1; Max-Age=1"));
        assert!(sent(&jar).is_some());
        if let Ok(mut cookies) = jar.cookies.lock()
            && let Some(stored) = cookies.get_mut("gone")
        {
            stored.expires_at = Some(Instant::now() - Duration::from_secs(1));
        }
        assert_eq!(sent(&jar), None);
    }

    #[test]
    fn a_jar_caps_a_huge_max_age_instead_of_overflowing() {
        let jar = CookieJar::default();
        jar.store(&HeaderValue::from_static(
            "huge=value; Max-Age=9223372036854775807",
        ));
        assert_eq!(sent(&jar).as_deref(), Some("huge=value"));
        let expiry = jar
            .cookies
            .lock()
            .unwrap_or_else(|error| error.into_inner())["huge"]
            .expires_at
            .expect("a Max-Age cookie expires");
        assert!(expiry <= Instant::now() + MAX_COOKIE_AGE);
    }

    #[test]
    fn a_jar_caps_new_names_but_allows_updates_and_deletions() {
        let jar = CookieJar::default();
        for index in 0..MAX_COOKIE_NAMES {
            jar.store(&HeaderValue::from_str(&format!("cookie_{index}=value")).unwrap());
        }
        jar.store(&HeaderValue::from_static("extra=rejected"));
        assert_eq!(jar.cookies.lock().unwrap().len(), MAX_COOKIE_NAMES);
        assert!(!jar.cookies.lock().unwrap().contains_key("extra"));

        jar.store(&HeaderValue::from_static("cookie_0=updated"));
        assert_eq!(
            jar.cookies.lock().unwrap().get("cookie_0").unwrap().value,
            "updated"
        );
        jar.store(&HeaderValue::from_static("cookie_1=; Max-Age=0"));
        jar.store(&HeaderValue::from_static("extra=accepted"));
        assert_eq!(jar.cookies.lock().unwrap().len(), MAX_COOKIE_NAMES);
        assert!(!jar.cookies.lock().unwrap().contains_key("cookie_1"));
        assert!(jar.cookies.lock().unwrap().contains_key("extra"));
    }

    #[test]
    fn a_jar_reuses_expired_slots_when_storing_new_cookies() {
        let jar = CookieJar::default();
        for index in 0..MAX_COOKIE_NAMES - 1 {
            jar.store(&HeaderValue::from_str(&format!("cookie_{index}=value")).unwrap());
        }
        jar.store(&HeaderValue::from_static("expired=value; Max-Age=1"));
        if let Some(stored) = jar.cookies.lock().unwrap().get_mut("expired") {
            stored.expires_at = Some(Instant::now() - Duration::from_secs(1));
        }
        jar.store(&HeaderValue::from_static("new=accepted"));
        assert_eq!(jar.cookies.lock().unwrap().len(), MAX_COOKIE_NAMES);
        assert!(!jar.cookies.lock().unwrap().contains_key("expired"));
        assert!(jar.cookies.lock().unwrap().contains_key("new"));
    }

    #[tokio::test]
    async fn a_shell_deadline_ends_a_mount_whose_own_clock_is_still_attended() {
        let proxied = Arc::new(Proxied::new(
            Upstream::parse("http://127.0.0.1:8080/panel/").unwrap(),
        ));
        let shell = Arc::new(AttentionClock::new());
        let policy = UiAttentionPolicy::never()
            .with_idle_timeout(Duration::from_millis(700))
            .with_grace_period(Duration::ZERO);
        let unavailable = proxied.unavailable(shell, policy);
        let attend_mount = async {
            loop {
                tokio::time::sleep(Duration::from_millis(50)).await;
                proxied.attention.attend();
            }
        };
        tokio::select! {
            result = tokio::time::timeout(Duration::from_secs(2), unavailable) => {
                assert!(result.is_ok(), "the shell deadline must cancel even an attended mount");
            }
            () = attend_mount => {}
        }
        assert!(*proxied.revoked.borrow());
        assert_eq!(
            proxied.attention.phase(&policy),
            super::super::attention::Phase::Live
        );
    }

    #[test]
    fn only_a_request_that_asked_to_stop_speaking_http_is_treated_as_one() {
        let mut headers = HeaderMap::new();
        assert_eq!(upgrade_requested(&headers), None);
        // An `Upgrade` nobody listed in `Connection` is advertising, not asking.
        headers.insert(UPGRADE, HeaderValue::from_static("websocket"));
        assert_eq!(upgrade_requested(&headers), None);
        // Browsers send `keep-alive, Upgrade`, so the token has to be found
        // inside the list rather than compared against it.
        headers.insert(CONNECTION, HeaderValue::from_static("keep-alive, Upgrade"));
        assert_eq!(
            upgrade_requested(&headers),
            Some(HeaderValue::from_static("websocket"))
        );
        headers.insert(CONNECTION, HeaderValue::from_static("close"));
        assert_eq!(upgrade_requested(&headers), None);
    }

    #[test]
    fn a_redirect_names_the_path_the_browser_can_reach_rather_than_a_loopback_credential() {
        let upstream = Upstream::parse("http://127.0.0.1:8080/sessionsecret/").unwrap();
        for location in [
            "http://127.0.0.1:8080/sessionsecret/done",
            "/sessionsecret/done",
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(LOCATION, HeaderValue::from_static(location));
            relocate(&mut headers, &upstream, "/s/proxysecret/");
            assert_eq!(headers[LOCATION], "/s/proxysecret/done");
        }
        // Somewhere that is not this session stays exactly as the Provider
        // wrote it.
        let mut headers = HeaderMap::new();
        headers.insert(
            LOCATION,
            HeaderValue::from_static("https://elsewhere.example/x"),
        );
        relocate(&mut headers, &upstream, "/s/proxysecret/");
        assert_eq!(headers[LOCATION], "https://elsewhere.example/x");
    }

    #[test]
    fn a_request_from_somewhere_that_is_not_this_page_is_refused() {
        let mut headers = HeaderMap::new();
        assert!(origin_allowed(&headers), "a navigation sends no Origin");
        headers.insert(HOST, HeaderValue::from_static("phone.example:8787"));
        headers.insert(ORIGIN, HeaderValue::from_static("null"));
        assert!(origin_allowed(&headers));
        headers.insert(
            ORIGIN,
            HeaderValue::from_static("https://phone.example:8787"),
        );
        assert!(
            origin_allowed(&headers),
            "a TLS terminator in front is the documented shape"
        );
        headers.insert(ORIGIN, HeaderValue::from_static("https://attacker.example"));
        assert!(!origin_allowed(&headers));
    }
}

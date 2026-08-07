//! A third delivery: hand the sessions on this machine to a browser elsewhere.
//!
//! A window and a direct URL are two ways of giving the same session to a
//! person. This is the third, and it exists for one situation: the person is
//! holding a phone and the agent is working on their laptop. Nothing here is a
//! new kind of session — [`crate::list_sessions`] is still the only source, the
//! Provider is still the only thing that understands its own page, and no new
//! field is asked of anybody.
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
//! reached by its own separate short-lived credential, is framed so that it
//! cannot read the page it is in, and the requests it makes are forwarded
//! upstream through a header allowlist that has no room for one.
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

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{Response, StatusCode};
use axum::response::{Html, IntoResponse};
use serde_json::{Value, json};
use tokio::net::TcpListener;

use crate::host::split_first_segment;
use crate::{Error, Result, UiAccessToken, UiExpiry, UiSecurityPolicy, UiSessionEntry};

mod proxy;
mod shell;

use proxy::{Framing, Proxied, Upstream};

/// The path segment every proxied Provider UI lives under.
///
/// It is not under the listener's own credential, and that is the point: the
/// URL a Provider UI is framed at must be one the Provider UI may know.
const PROXY_SEGMENT: &str = "s";

/// What marks a hostname as one session's own origin.
///
/// A label rather than a whole name, so one wildcard record and one wildcard
/// certificate cover every session there will ever be. The prefix keeps session
/// names out of the way of anything else living under the same host.
const ORIGIN_LABEL_PREFIX: &str = "s-";

/// How long a proxy credential survives with nobody using it.
///
/// Sliding rather than fixed, because a page being used is a page that keeps
/// asking: a terminal polls, a review saves. A panel nobody has touched for
/// this long is one whose credential should stop working, and the next refresh
/// mints it a new one if the session is still there.
const PROXY_IDLE_TTL: Duration = Duration::from_secs(15 * 60);

/// What an address this listener does not recognise renders in a browser.
///
/// The body is deliberately identical for a malformed, unknown, expired, or
/// revoked credential: a 404 must not become an oracle for what is running.
/// Naming the response as HTML matters on mobile Safari, which otherwise
/// downloads Axum's empty, untyped 404 as `document.txt` on a top-level
/// navigation instead of showing an error page.
const NOT_FOUND_HTML: &str =
    "<!doctype html><meta charset=\"utf-8\"><title>Not Found</title><h1>Not Found</h1>";

/// Serve the live sessions on this machine to a browser that is not on it.
pub struct SessionServer {
    listen: SocketAddr,
    advertised_host: Option<String>,
    session_origin_host: Option<String>,
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

impl SessionServer {
    #[must_use]
    pub fn new(listen: SocketAddr) -> Self {
        Self {
            listen,
            advertised_host: None,
            session_origin_host: None,
        }
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
        // The process is the bound, the way a window is the bound for a window
        // delivery: when this call ends, the credential and every proxy mount
        // it minted go with it. A person working from a phone should not have
        // their page stop answering halfway through the afternoon.
        let token = UiAccessToken::issue(UiExpiry::Never)?;
        let access_url = format!("http://{authority}/{}/", token.secret().expose_secret());
        Ok(ServedSessions {
            listener,
            state: Arc::new(ServeState {
                token,
                mounts: Mutex::new(Mounts::default()),
                addressing,
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
}

/// Everything one running listener holds.
struct ServeState {
    /// What the person proves to reach the page and its data. Never forwarded,
    /// never rendered, never in a URL a Provider UI can see.
    token: UiAccessToken,
    mounts: Mutex<Mounts>,
    /// Fixed when the listener was bound, because it decides what the page's
    /// own address means: in one shape everything answers on it, in the other
    /// it answers for the page and nothing else.
    addressing: Addressing,
}

/// The proxied sessions, indexed by the two things that look them up.
#[derive(Default)]
struct Mounts {
    by_secret: HashMap<String, Mount>,
    by_session: HashMap<String, String>,
}

/// One session, reachable through this listener.
struct Mount {
    token: UiAccessToken,
    proxied: Arc<Proxied>,
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
        let entries = tokio::task::spawn_blocking(crate::list_sessions)
            .await
            .map_err(|error| Error::Io(std::io::Error::other(error.to_string())))??;
        self.mount_live_sessions(&entries, shell_authority)
    }

    /// Bring the mount table in line with what is actually running.
    ///
    /// This is the whole lifecycle: a session that has appeared gets a
    /// credential, a session that has ended has its credential revoked and its
    /// path removed. Nothing is asked of the Provider, and nothing here
    /// survives the session it belongs to.
    fn mount_live_sessions(
        &self,
        entries: &[UiSessionEntry],
        shell_authority: Option<&str>,
    ) -> Result<Vec<Value>> {
        let mut mounts = self.mounts.lock().map_err(|_| poisoned())?;
        let Mounts {
            by_secret,
            by_session,
        } = &mut *mounts;
        let live: HashSet<&str> = entries
            .iter()
            .map(|entry| entry.session_id.as_str())
            .collect();
        by_session.retain(|session_id, secret| {
            if live.contains(session_id.as_str()) {
                return true;
            }
            if let Some(mount) = by_secret.remove(secret) {
                mount.token.revoke();
            }
            false
        });

        let mut cards = Vec::with_capacity(entries.len());
        for entry in entries {
            let Some(upstream) = Upstream::parse(&entry.access_url_secret) else {
                continue;
            };
            let usable = by_session
                .get(&entry.session_id)
                .and_then(|secret| by_secret.get(secret))
                .is_some_and(|mount| {
                    !mount.token.is_expired()
                        && !mount.token.is_revoked()
                        && mount.proxied.upstream == upstream
                });
            if !usable {
                if let Some(stale) = by_session.remove(&entry.session_id)
                    && let Some(mount) = by_secret.remove(&stale)
                {
                    mount.token.revoke();
                }
                let token = UiAccessToken::issue(UiExpiry::Idle(PROXY_IDLE_TTL))?;
                let secret = token.secret().expose_secret().to_owned();
                by_secret.insert(
                    secret.clone(),
                    Mount {
                        token,
                        proxied: Arc::new(Proxied::new(upstream)),
                    },
                );
                by_session.insert(entry.session_id.clone(), secret);
            }
            let Some(secret) = by_session.get(&entry.session_id) else {
                continue;
            };
            let Some(framed_at) = frame_address(secret, shell_authority) else {
                continue;
            };
            let mut card = json!({
                "session_id": entry.session_id,
                "provider_id": entry.provider_id,
                "ui_kind": entry.ui_kind,
                // Where this session is framed. It carries the only credential
                // the Provider UI is given, so it is named for what it is. The
                // session's own `access_url_secret` never leaves this process.
                "proxy_url_secret": framed_at,
            });
            if let Some(subject) = &entry.subject {
                card["subject"] = Value::String(subject.clone());
            }
            cards.push(card);
        }
        Ok(cards)
    }

    /// The mount a proxy credential names, if it is still live.
    fn mount(&self, secret: &str) -> Option<Arc<Proxied>> {
        let mounts = self.mounts.lock().ok()?;
        let mount = mounts.by_secret.get(secret)?;
        // The lookup above is a hash of the candidate; this is the check that
        // decides, in constant time and honouring expiry and revocation. A
        // credential that fails it never reaches this session's cookie jar,
        // which is what keeps one session's cookies out of another's requests.
        mount
            .token
            .authorize(secret)
            .then(|| Arc::clone(&mount.proxied))
    }
}

/// Route one request to the page, or to a proxied Provider UI.
///
/// A wrong credential is a 404 in every direction, for the same reason the
/// session host answers that way: there is nothing there to authenticate
/// against, and saying otherwise would confirm that this port hosts something.
async fn dispatch(State(state): State<Arc<ServeState>>, request: Request) -> Response<Body> {
    match &state.addressing {
        Addressing::Path => on_one_origin(&state, request).await,
        Addressing::PerSession { base_host } => across_origins(&state, base_host, request).await,
    }
}

/// The page and every session it frames, on the one authority this listener
/// answers for. What tells them apart is the first path segment.
async fn on_one_origin(state: &Arc<ServeState>, request: Request) -> Response<Body> {
    let path = request.uri().path().to_owned();
    let Some((head, remainder)) = split_first_segment(&path) else {
        return not_found();
    };
    if head == PROXY_SEGMENT {
        let Some((secret, rest)) = split_first_segment(&remainder) else {
            return not_found();
        };
        let Some(proxied) = state.mount(&secret) else {
            return not_found();
        };
        let mounted_at = format!("/{PROXY_SEGMENT}/{secret}/");
        return proxy::forward(
            &proxied,
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
        let Some(secret) = mount_secret_of(label) else {
            return not_found();
        };
        let Some(proxied) = state.mount(&secret) else {
            return not_found();
        };
        let path = request.uri().path().to_owned();
        return proxy::forward(
            &proxied,
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
    shell::respond(state, &remainder, &path, Some(&authority), request).await
}

fn not_found() -> Response<Body> {
    let mut response = (StatusCode::NOT_FOUND, Html(NOT_FOUND_HTML)).into_response();
    UiSecurityPolicy::isolated().apply_headers(response.headers_mut());
    response
}

/// The authority the browser asked for, lowercased.
///
/// The `Host` header is what a browser sends; the request target carries one
/// only when a proxy wrote it. Either is read, because which one arrives is not
/// this listener's decision.
fn requested_authority<B>(request: &axum::http::Request<B>) -> Option<String> {
    let host = request
        .headers()
        .get(axum::http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .or_else(|| request.uri().authority().map(|value| value.to_string()))?;
    Some(host.to_ascii_lowercase())
}

/// The name part of an authority, without the port.
fn hostname_of(authority: &str) -> &str {
    match authority.rsplit_once(':') {
        Some((host, port))
            if !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            host
        }
        _ => authority,
    }
}

/// Where the page points a frame at one session.
///
/// Scheme-relative in the per-session shape, so the frame is fetched over
/// whatever the page itself was: a TLS terminator in front would otherwise turn
/// every frame into blocked mixed content.
fn frame_address(secret: &str, shell_authority: Option<&str>) -> Option<String> {
    match shell_authority {
        Some(authority) => Some(format!(
            "//{ORIGIN_LABEL_PREFIX}{}.{authority}/",
            origin_label(secret)?
        )),
        None => Some(format!("/{PROXY_SEGMENT}/{secret}/")),
    }
}

/// The alphabet a session's own name is written in.
///
/// RFC 4648 base32, lowercased: letters and digits that are all legal in a DNS
/// label, in an encoding that is case-insensitive the way a hostname is. The
/// credential is re-encoded rather than shortened because hex would spell it in
/// 64 characters and a label may have 63.
const LABEL_ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

/// The label that names the mount this credential opens.
fn origin_label(secret: &str) -> Option<String> {
    let bytes = hex::decode(secret).ok()?;
    let mut label = String::with_capacity(bytes.len().div_ceil(5) * 8);
    let mut buffer = 0_u16;
    let mut bits = 0_u32;
    for byte in bytes {
        buffer = (buffer << 8) | u16::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            label.push(char::from(
                LABEL_ALPHABET[usize::from((buffer >> bits) & 0x1f)],
            ));
        }
    }
    if bits > 0 {
        label.push(char::from(
            LABEL_ALPHABET[usize::from((buffer << (5 - bits)) & 0x1f)],
        ));
    }
    Some(label)
}

/// The credential a session's own name carries, in the form the mount table
/// keys on.
///
/// Every step can refuse: a character outside the alphabet, or padding bits
/// that are not zero. The second one matters — without it sixteen spellings of
/// one label would open the same mount, and a session would have sixteen
/// addresses instead of one.
fn mount_secret_of(label: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(label.len() * 5 / 8);
    let mut buffer = 0_u16;
    let mut bits = 0_u32;
    for character in label.bytes() {
        let value = LABEL_ALPHABET
            .iter()
            .position(|candidate| *candidate == character)?;
        buffer = (buffer << 5) | u16::try_from(value).ok()?;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            bytes.push(u8::try_from((buffer >> bits) & 0xff).ok()?);
        }
    }
    if bits > 0 && buffer & ((1 << bits) - 1) != 0 {
        return None;
    }
    Some(hex::encode(bytes))
}

/// A host every session can be given a name under.
///
/// Returns the name itself, without whatever port came with it: the port is the
/// page's business and every session shares it.
fn session_origin_base(host: &str) -> Result<String> {
    let refuse = |reason: &'static str| Error::InvalidSessionOriginHost {
        value: host.to_owned(),
        reason,
    };
    let name = hostname_of(host.trim()).to_ascii_lowercase();
    if name.is_empty() {
        return Err(refuse("it names no host at all"));
    }
    if name.parse::<IpAddr>().is_ok() || name.starts_with('[') {
        return Err(refuse(
            "an address has no names under it, and no wildcard record can be written for one",
        ));
    }
    if !name
        .split('.')
        .all(|label| !label.is_empty() && label.len() <= 63 && is_host_label(label))
    {
        return Err(refuse(
            "a host is dot-separated labels of letters, digits and hyphens",
        ));
    }
    // 253 is the whole name a resolver will carry, and a session spends the
    // first 55 of them on `s-` and its own credential.
    if name.len() + ORIGIN_LABEL_PREFIX.len() + LABEL_LENGTH + 1 > 253 {
        return Err(refuse(
            "a session's own name is 55 characters longer, and that is past what a hostname may be",
        ));
    }
    Ok(name)
}

/// How long a credential is once it is spelled as a label: 32 bytes at five
/// bits a character.
const LABEL_LENGTH: usize = 52;

fn is_host_label(label: &str) -> bool {
    label
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        && !label.starts_with('-')
        && !label.ends_with('-')
}

fn poisoned() -> Error {
    Error::Io(std::io::Error::other("served session registry is poisoned"))
}

/// The authority to put in the URL when the caller did not name one.
///
/// A wildcard bind is reachable from elsewhere but cannot be typed into
/// anything, so it is the one case worth resolving to a real interface.
fn advertised_authority(local_addr: SocketAddr) -> String {
    if local_addr.ip().is_unspecified()
        && let Some(lan) = primary_lan_ipv4()
    {
        return format!("{lan}:{}", local_addr.port());
    }
    local_addr.to_string()
}

/// This machine's address on the local network, as far as its routing table
/// knows.
///
/// Probes the IPv4 all-hosts multicast route rather than a unicast default
/// route: VPN and overlay interfaces commonly capture the latter without owning
/// the route that reaches another device on the physical network. Nothing is
/// sent — a connected UDP socket only fixes a source address.
fn primary_lan_ipv4() -> Option<Ipv4Addr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect((Ipv4Addr::new(224, 0, 0, 1), 9)).ok()?;
    match socket.local_addr().ok()?.ip() {
        IpAddr::V4(ip)
            if !ip.is_loopback()
                && !ip.is_unspecified()
                && !ip.is_multicast()
                && !ip.is_broadcast()
                && !ip.is_link_local() =>
        {
            Some(ip)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(session_id: &str, url: &str) -> UiSessionEntry {
        UiSessionEntry {
            session_id: session_id.to_owned(),
            provider_id: "console".to_owned(),
            ui_kind: "terminal".to_owned(),
            subject: None,
            access_url_secret: url.to_owned(),
            owner_pid: 1,
        }
    }

    fn state() -> Result<ServeState> {
        Ok(ServeState {
            token: UiAccessToken::issue(UiExpiry::Never)?,
            mounts: Mutex::new(Mounts::default()),
            addressing: Addressing::Path,
        })
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

        let path = first[0]["proxy_url_secret"].as_str().unwrap().to_owned();
        let secret = path.trim_matches('/').split('/').nth(1).unwrap().to_owned();
        assert!(state.mount(&secret).is_some());

        assert!(state.mount_live_sessions(&[], None).unwrap().is_empty());
        assert!(
            state.mount(&secret).is_none(),
            "a proxy credential must stop working when its session ends"
        );
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

    #[test]
    fn a_name_that_is_not_exactly_one_sessions_opens_nothing() {
        assert_eq!(mount_secret_of("not base32: !"), None);
        // Padding bits that are not zero: a second spelling of one credential,
        // refused so that a session has one address rather than sixteen.
        let label = origin_label(&"ab".repeat(32)).unwrap();
        assert_eq!(
            mount_secret_of(&label).as_deref(),
            Some("ab".repeat(32).as_str())
        );
        let mut tampered = label.clone();
        tampered.pop();
        tampered.push('b');
        assert_eq!(mount_secret_of(&tampered), None);
    }

    #[test]
    fn a_host_nothing_can_be_named_under_is_refused_before_binding() {
        assert_eq!(session_origin_base("localhost").unwrap(), "localhost");
        assert_eq!(session_origin_base("localhost:8787").unwrap(), "localhost");
        assert_eq!(
            session_origin_base("UI.Example.COM").unwrap(),
            "ui.example.com"
        );
        // An address has nothing under it: no wildcard record, no certificate.
        assert!(session_origin_base("127.0.0.1").is_err());
        assert!(session_origin_base("127.0.0.1:8787").is_err());
        assert!(session_origin_base("[::1]:8787").is_err());
        assert!(session_origin_base("").is_err());
        assert!(session_origin_base("under_score.example").is_err());
        assert!(session_origin_base("-leading.example").is_err());
        assert!(session_origin_base("http://localhost").is_err());
        // No room left for a session's own name.
        assert!(session_origin_base(&format!("{}.example", "a".repeat(200))).is_err());
    }

    #[test]
    fn an_authority_is_read_without_its_port() {
        assert_eq!(hostname_of("localhost:8787"), "localhost");
        assert_eq!(hostname_of("localhost"), "localhost");
        assert_eq!(hostname_of("[::1]"), "[::1]");
        assert_eq!(hostname_of("ui.example.com"), "ui.example.com");
    }
}

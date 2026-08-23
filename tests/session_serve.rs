//! Serving live sessions to a browser that is not on this machine.
//!
//! Every test here drives real sockets, because every claim here is about what
//! actually crosses one: what the person's browser is refused, what a Provider
//! is told, and when bytes arrive rather than merely whether they do.
//!
//! Most of them run twice, once for each shape the listener can take: every
//! session on a path under the one origin this listener owns, or every session
//! on an origin of its own. The two differ in where isolation comes from — an
//! opaque origin the sandbox imposes, or the browser's own origin boundary —
//! and in nothing else, so an invariant that held in only one of them would be
//! an invariant this listener does not have.

#![cfg(feature = "serve")]

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use agent_first_ui::{
    ActiveUiSession, Outcome, SessionServer, UiAttentionPolicy, UiExpiry, UiSecurityPolicy,
    UiSession, UiUrlDelivery, UiWindowConfig,
};
use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::{any, get};
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio_stream::wrappers::ReceiverStream;

/// Which boundary keeps two framed sessions apart.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Shape {
    /// Every session is a path on the listener's own origin, and the frame is
    /// sandboxed into an opaque origin. The default, and the shape for someone
    /// who has no wildcard DNS.
    OneOrigin,
    /// Every session is `s-<credential>.localhost`, an origin of its own.
    /// `*.localhost` is loopback in every browser without any DNS at all, which
    /// is what makes this shape testable here and usable on a laptop.
    OriginPerSession,
}

/// One test body, run against both shapes.
///
/// The body is a plain `async fn` rather than a `#[test]`, so the clippy
/// allowances a test gets for free have to be written out — which is why they
/// are here rather than on each body.
macro_rules! in_both_shapes {
    ($(#[doc = $doc:expr])* async fn $name:ident($shape:ident: Shape $(,)?) $body:block) => {
        $(#[doc = $doc])*
        mod $name {
            use super::*;

            #[allow(clippy::expect_used, clippy::unwrap_used)]
            async fn run($shape: Shape) $body

            #[tokio::test]
            async fn on_one_origin() {
                run(Shape::OneOrigin).await;
            }

            #[tokio::test]
            async fn with_an_origin_per_session() {
                run(Shape::OriginPerSession).await;
            }
        }
    };
}

/// One registry root for this binary, so tests cannot see each other's config
/// directory. They do share the session directory, so every assertion here is
/// about a session this test created, never about the directory as a whole.
///
/// The helpers below fail loudly rather than threading a `Result` up into every
/// test: a fixture that could not be built has nothing to assert about. Clippy
/// allows that inside a `#[test]` function but not in a helper, hence the
/// scoped allowances.
#[allow(clippy::expect_used)]
fn registry_root() -> &'static tempfile::TempDir {
    static ROOT: OnceLock<tempfile::TempDir> = OnceLock::new();
    ROOT.get_or_init(|| {
        let root = tempfile::tempdir().expect("temporary registry root");
        // SAFETY: written exactly once, before any test reads it, and every
        // test enters through this function.
        unsafe { std::env::set_var(agent_first_ui::CONFIG_DIR_ENV, root.path()) };
        root
    })
}

/// A running session, as a Provider would have created it.
async fn provider_session(router: Router) -> ActiveUiSession<()> {
    provider_session_with(router, UiSecurityPolicy::isolated()).await
}

#[allow(clippy::expect_used)]
async fn provider_session_with(router: Router, policy: UiSecurityPolicy) -> ActiveUiSession<()> {
    registry_root();
    UiSession::<()>::new("inspector", "table_inspect")
        .expect("session")
        .with_subject("public.orders")
        .with_security_policy(policy)
        .serve(router, UiUrlDelivery::new(UiExpiry::Never))
        .await
        .expect("serve the provider session")
}

/// A bound listener, and everything a test needs to put a request on its wire.
struct Shell {
    /// Where every request actually connects. Always loopback: nothing in these
    /// tests resolves a name, because `*.localhost` is a rule browsers apply
    /// and not one this process has.
    wire: String,
    /// The authority the page answers to, which is what a request claims in its
    /// `Host` header.
    host: String,
    /// The page's own path, credential and trailing slash included.
    page_path: String,
    shape: Shape,
    handle: tokio::task::JoinHandle<()>,
}

/// One session as the browser reaches it: an address to connect to, a name to
/// claim, and the path that session is rooted at.
struct Framed {
    wire: String,
    host: String,
    path: String,
}

#[allow(clippy::expect_used)]
async fn serving(shape: Shape) -> Shell {
    serving_with(shape, UiAttentionPolicy::never()).await
}

#[allow(clippy::expect_used)]
async fn serving_with(shape: Shape, attention_policy: UiAttentionPolicy) -> Shell {
    registry_root();
    let mut server = SessionServer::new("127.0.0.1:0".parse().expect("address"))
        .with_attention_policy(attention_policy);
    if shape == Shape::OriginPerSession {
        server = server.with_session_origin_host("localhost");
    }
    let served = server.bind().await.expect("bind");
    let wire = served.local_addr().to_string();
    let access_url = served.access_url().to_owned();
    let handle = tokio::spawn(async move {
        let _ignored = served.run(std::future::pending::<()>()).await;
    });
    let (host, page_path) = access_url
        .strip_prefix("http://")
        .and_then(|rest| rest.split_once('/'))
        .expect("an access URL with an authority and a path");
    Shell {
        wire,
        host: host.to_owned(),
        page_path: format!("/{page_path}"),
        shape,
        handle,
    }
}

impl Shell {
    /// What the person proves to reach the page, as it appears in their URL.
    fn credential(&self) -> String {
        self.page_path.trim_matches('/').to_owned()
    }

    /// The same listener, reached by somebody who has no credential for it.
    ///
    /// Same wire and same claimed name as the page, so the only thing that
    /// differs is the one thing being tested.
    async fn wrong_credential(&self) -> Reply {
        request(
            &self.wire,
            &self.host,
            "/0000000000000000000000000000000000000000000000000000000000000000/",
            &[],
            None,
        )
        .await
    }

    /// One request to the page itself.
    async fn page(&self, suffix: &str, headers: &[(&str, &str)], body: Option<&str>) -> Reply {
        request(
            &self.wire,
            &self.host,
            &format!("{}{suffix}", self.page_path),
            headers,
            body,
        )
        .await
    }

    /// The session the page is showing, as the page sees it.
    #[allow(clippy::expect_used)]
    async fn listed(&self, session_id: &str) -> Option<serde_json::Value> {
        let reply = self.page("sessions", &[], None).await;
        assert_eq!(reply.status, 200, "{}", reply.body);
        let body: serde_json::Value = serde_json::from_str(&reply.body).expect("sessions json");
        assert_eq!(
            body["isolation"],
            match self.shape {
                Shape::OneOrigin => "opaque-origin",
                Shape::OriginPerSession => "own-origin",
            },
            "the page is told which boundary it is building frames against"
        );
        body["sessions"]
            .as_array()?
            .iter()
            .find(|session| session["session_id"] == session_id)
            .cloned()
    }

    /// The same card, insisted upon: a session the page is showing.
    #[allow(clippy::expect_used)]
    async fn card(&self, session_id: &str) -> serde_json::Value {
        self.listed(session_id)
            .await
            .expect("the session is listed")
    }

    /// The page saying somebody is in front of it, and what they can see.
    async fn attend(&self, session_ids: &[&str]) -> u16 {
        let body = serde_json::json!({ "sessions": session_ids }).to_string();
        self.page(
            "attention",
            &[("Content-Type", "application/json")],
            Some(&body),
        )
        .await
        .status
    }

    /// Where one live session is framed, as the page was told it.
    #[allow(clippy::expect_used)]
    async fn framed(&self, session: &ActiveUiSession<()>) -> Framed {
        self.framed_id(session.metadata().session_id.as_str()).await
    }

    /// The same, for a session this process announced rather than served. That
    /// the two share one lookup is the point: the page cannot tell them apart.
    #[allow(clippy::expect_used)]
    async fn framed_id(&self, session_id: &str) -> Framed {
        let card = self
            .listed(session_id)
            .await
            .expect("the session is listed");
        self.frame_of(card["proxy_url_secret"].as_str().expect("a framed address"))
    }

    /// Take apart the one address the page is given.
    ///
    /// A path in one shape and a scheme-relative URL in the other, which is the
    /// whole of the difference the page sees.
    #[allow(clippy::expect_used)]
    fn frame_of(&self, framed_at: &str) -> Framed {
        match framed_at.strip_prefix("//") {
            Some(rest) => {
                assert_eq!(
                    self.shape,
                    Shape::OriginPerSession,
                    "only a session with an origin of its own is framed at an authority: \
                     {framed_at}"
                );
                let (authority, path) = rest.split_once('/').expect("a path under the authority");
                assert!(
                    authority.starts_with("s-") && authority.ends_with(&self.host),
                    "a session is one label under the page's own authority: {framed_at}"
                );
                Framed {
                    wire: self.wire.clone(),
                    host: authority.to_owned(),
                    path: format!("/{path}"),
                }
            }
            None => {
                assert_eq!(
                    self.shape,
                    Shape::OneOrigin,
                    "a session with an origin of its own is never framed at a bare path: \
                     {framed_at}"
                );
                Framed {
                    wire: self.wire.clone(),
                    host: self.host.clone(),
                    path: framed_at.to_owned(),
                }
            }
        }
    }
}

impl Framed {
    async fn request(&self, suffix: &str, headers: &[(&str, &str)], body: Option<&str>) -> Reply {
        request(
            &self.wire,
            &self.host,
            &format!("{}{suffix}", self.path),
            headers,
            body,
        )
        .await
    }
}

struct Reply {
    status: u16,
    headers: HashMap<String, String>,
    body: String,
}

impl Reply {
    fn header(&self, name: &str) -> &str {
        self.headers.get(name).map_or("", String::as_str)
    }
}

/// One request, read to the end of the connection.
///
/// Hand-written so the test can put headers on the wire that no client library
/// would let it send — a cookie that is not for this origin, an `Authorization`
/// nobody asked for — which is exactly what has to be proven not to cross. It
/// also separates where the request goes from what name it claims, because a
/// session with an origin of its own is reached by claiming that name and
/// nothing here resolves one.
#[allow(clippy::expect_used)]
async fn request(
    wire: &str,
    host: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&str>,
) -> Reply {
    let mut stream = TcpStream::connect(wire).await.expect("connect");
    let method = if body.is_some() { "POST" } else { "GET" };
    let mut request = format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\n");
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    if let Some(body) = body {
        request.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    request.push_str("Connection: close\r\n\r\n");
    if let Some(body) = body {
        request.push_str(body);
    }
    stream
        .write_all(request.as_bytes())
        .await
        .expect("write request");
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.expect("read response");
    parse(&String::from_utf8_lossy(&raw))
}

fn parse(raw: &str) -> Reply {
    let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((raw, ""));
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    let mut headers = HashMap::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
        }
    }
    Reply {
        status,
        headers,
        body: body.to_owned(),
    }
}

// ── The proxy streams ───────────────────────────────────────────────

in_both_shapes! {
    async fn the_proxy_hands_on_each_frame_instead_of_waiting_for_the_last_one(shape: Shape) {
        let (sender, receiver) = tokio::sync::mpsc::channel::<Result<Vec<u8>, std::io::Error>>(4);
        let receiver = Arc::new(Mutex::new(Some(receiver)));
        let session = provider_session(Router::new().route(
            "/stream",
            get(move || {
                let receiver = Arc::clone(&receiver);
                async move {
                    let receiver = receiver.lock().ok().and_then(|mut slot| slot.take());
                    match receiver {
                        Some(receiver) => (
                            [(header::CONTENT_TYPE, "text/event-stream")],
                            Body::from_stream(ReceiverStream::new(receiver)),
                        )
                            .into_response(),
                        None => StatusCode::CONFLICT.into_response(),
                    }
                }
            }),
        ))
        .await;
        let shell = serving(shape).await;
        let framed = shell.framed(&session).await;

        let mut stream = TcpStream::connect(&framed.wire).await.expect("connect");
        stream
            .write_all(
                format!(
                    "GET {}stream HTTP/1.1\r\nHost: {}\r\nAccept: text/event-stream\r\n\r\n",
                    framed.path, framed.host
                )
                .as_bytes(),
            )
            .await
            .expect("write");
        let mut reader = BufReader::new(&mut stream);

        // The upstream response is open and will stay open: nothing has been sent
        // and the sender is still alive. A proxy that collects a body before
        // passing it on cannot answer at all from here.
        sender
            .send(Ok(b"data: first\n\n".to_vec()))
            .await
            .expect("send the first frame");
        let head = read_for(&mut reader, "data: first", Duration::from_secs(3)).await;
        assert!(
            head.contains("200"),
            "the stream must start before it ends: {head}"
        );

        sender
            .send(Ok(b"data: second\n\n".to_vec()))
            .await
            .expect("send the second frame");
        let more = read_for(&mut reader, "data: second", Duration::from_secs(3)).await;
        assert!(
            more.contains("data: second"),
            "a later frame must arrive on the same open stream: {more}"
        );

        drop(sender);
        drop(session);
        shell.handle.abort();
    }
}

/// Read until `wanted` shows up, or give up. A buffering proxy fails here by
/// timing out, which is the whole point of the deadline.
async fn read_for(
    reader: &mut BufReader<&mut TcpStream>,
    wanted: &str,
    within: Duration,
) -> String {
    let mut seen = String::new();
    let deadline = tokio::time::Instant::now() + within;
    while !seen.contains(wanted) {
        let mut line = String::new();
        let read = tokio::time::timeout_at(deadline, reader.read_line(&mut line)).await;
        match read {
            Ok(Ok(0)) | Err(_) => break,
            Ok(Ok(_)) => seen.push_str(&line),
            Ok(Err(_)) => break,
        }
    }
    seen
}

// ── Nothing of this listener's reaches a Provider ───────────────────

#[derive(Clone, Default)]
struct Seen(Arc<Mutex<Vec<String>>>);

in_both_shapes! {
    async fn nothing_the_person_authenticated_with_is_forwarded_to_a_provider(shape: Shape) {
        let seen = Seen::default();
        let session = provider_session(
            Router::new()
                .route(
                    "/echo",
                    any(|State(seen): State<Seen>, request: Request| async move {
                        let mut lines = vec![format!("uri {}", request.uri())];
                        for (name, value) in request.headers() {
                            lines.push(format!("{name}: {}", value.to_str().unwrap_or("?")));
                        }
                        if let Ok(mut recorded) = seen.0.lock() {
                            recorded.push(lines.join("\n"));
                        }
                        "echoed"
                    }),
                )
                .with_state(seen.clone()),
        )
        .await;
        let shell = serving(shape).await;
        let credential = shell.credential();
        let framed = shell.framed(&session).await;
        let page_url = format!("http://{}{}", shell.host, shell.page_path);

        let reply = framed
            .request(
                "echo",
                &[
                    ("Cookie", &format!("afui_session={credential}")),
                    ("Authorization", &format!("Bearer {credential}")),
                    ("X-Afui-Session", &credential),
                    ("Origin", "null"),
                    ("Referer", &page_url),
                    ("Content-Type", "application/x-www-form-urlencoded"),
                ],
                Some("decision=accept"),
            )
            .await;
        assert_eq!(reply.status, 200, "{}", reply.body);

        let recorded = seen.0.lock().expect("recorded").join("\n---\n");
        assert!(
            !recorded.contains(&credential),
            "the page's own credential must not reach a Provider:\n{recorded}"
        );
        // Not merely absent by luck: the carriers themselves have no way through.
        for header in [
            "cookie",
            "authorization",
            "x-afui-session",
            "origin",
            "referer",
        ] {
            assert!(
                !recorded
                    .to_ascii_lowercase()
                    .contains(&format!("\n{header}:")),
                "`{header}` must not be forwarded:\n{recorded}"
            );
        }
        // And what the Provider does see is its own session's path, not the
        // proxy's: the two credentials never meet.
        assert!(recorded.contains("/echo"), "{recorded}");

        drop(session);
        shell.handle.abort();
    }
}

in_both_shapes! {
    async fn a_providers_own_credential_is_never_described_to_the_page(shape: Shape) {
        let session = provider_session(Router::new().route("/", get(|| async { "panel" }))).await;
        let shell = serving(shape).await;
        let session_id = session.metadata().session_id.as_str().to_owned();
        let upstream_credential = session
            .access_url()
            .strip_prefix("http://")
            .and_then(|rest| rest.split('/').nth(1))
            .expect("the session's own credential")
            .to_owned();

        let reply = shell.page("sessions", &[], None).await;
        assert!(reply.body.contains(&session_id));
        assert!(
            !reply.body.contains(&upstream_credential),
            "a session's own credential must stay in this process: {}",
            reply.body
        );

        drop(session);
        shell.handle.abort();
    }
}

// ── An unauthenticated request is refused ───────────────────────────

in_both_shapes! {
    async fn a_request_without_the_credential_is_refused_and_told_nothing(shape: Shape) {
        let session = provider_session(Router::new().route("/", get(|| async { "panel" }))).await;
        let shell = serving(shape).await;
        let credential = shell.credential();

        for path in [
            "/".to_owned(),
            "/sessions".to_owned(),
            "/shell.js".to_owned(),
            format!("/{}/", "0".repeat(credential.len())),
            format!("/{}/sessions", &credential[..credential.len() - 2]),
            format!("/{credential}x/sessions"),
            "/s/00112233445566778899aabbccddeeff/".to_owned(),
        ] {
            let reply = request(&shell.wire, &shell.host, &path, &[], None).await;
            assert_eq!(
                reply.status, 404,
                "`{path}` must be refused: {}",
                reply.body
            );
            assert_eq!(
                reply.header("content-type"),
                "text/html; charset=utf-8",
                "`{path}` must render in a browser rather than download"
            );
            assert_eq!(reply.header("cache-control"), "no-store", "{path}");
            assert_eq!(reply.header("x-content-type-options"), "nosniff", "{path}");
            assert!(reply.body.contains("<title>Not Found</title>"), "{path}");
            assert!(!reply.body.contains("Open interfaces"), "{path}");
            assert!(!reply.body.contains("proxy_url_secret"), "{path}");
        }

        // The same listener, with the credential, is a page.
        let reply = shell.page("", &[], None).await;
        assert_eq!(reply.status, 200);
        assert!(reply.body.contains("Open interfaces"));

        // A live session is reachable at its own address and at no other. In
        // the per-session shape that is the whole claim: the path a session
        // would have had on the page's origin answers nothing, so a Provider UI
        // cannot be loaded onto the page's origin at all.
        let framed = shell.framed(&session).await;
        assert_eq!(framed.request("", &[], None).await.status, 200);
        if shape == Shape::OriginPerSession {
            let secret_label = framed
                .host
                .split('.')
                .next()
                .and_then(|label| label.strip_prefix("s-"))
                .expect("a session label")
                .to_owned();
            for (host, path) in [
                // The other shape's address, on the page's own host.
                (shell.host.clone(), format!("/s/{secret_label}/")),
                (shell.host.clone(), "/s/".to_owned()),
                // A name that is not the one the sessions are under, however
                // right the rest of the request is.
                (shell.wire.clone(), shell.page_path.clone()),
                (format!("s-{secret_label}.{}", shell.wire), "/".to_owned()),
                (format!("s-{secret_label}.localhost.example"), "/".to_owned()),
                // The right shape of name, naming no mount.
                (format!("s-{}.{}", "a".repeat(52), shell.host), "/".to_owned()),
                (format!("s-.{}", shell.host), "/".to_owned()),
            ] {
                let reply = request(&shell.wire, &host, &path, &[], None).await;
                assert_eq!(
                    reply.status, 404,
                    "`{host}{path}` must be refused: {}",
                    reply.body
                );
                assert!(!reply.body.contains("panel"), "{host}{path}");
            }
        }

        drop(session);
        shell.handle.abort();
    }
}

/// The page and its frames are one arrangement, and a listener that could not
/// build one refuses to start rather than serving a page whose every frame the
/// browser will blank.
#[tokio::test]
#[allow(clippy::expect_used)]
async fn a_page_advertised_away_from_its_sessions_names_refuses_to_bind() {
    registry_root();
    let address = "127.0.0.1:0".parse().expect("address");
    // The default advertised authority is the bound one, and an address has no
    // names under it.
    assert!(
        SessionServer::new(address)
            .with_advertised_host("127.0.0.1")
            .with_session_origin_host("localhost")
            .bind()
            .await
            .is_err()
    );
    assert!(
        SessionServer::new(address)
            .with_session_origin_host("127.0.0.1")
            .bind()
            .await
            .is_err()
    );
    // Naming the same host twice, once with the port the page is reached on, is
    // not a conflict.
    let served = SessionServer::new(address)
        .with_advertised_host("localhost:9999")
        .with_session_origin_host("localhost")
        .bind()
        .await
        .expect("bind");
    assert!(served.access_url().starts_with("http://localhost:9999/"));
}

// ── A session that ends disappears ──────────────────────────────────

in_both_shapes! {
    async fn a_session_that_ends_leaves_the_page_and_takes_its_path_with_it(shape: Shape) {
        let session = provider_session(Router::new().route("/", get(|| async { "panel" }))).await;
        let shell = serving(shape).await;
        let session_id = session.metadata().session_id.as_str().to_owned();
        let framed = shell.framed(&session).await;
        assert_eq!(framed.request("", &[], None).await.status, 200);

        // What ending a session means: the Provider's call returned.
        drop(session);

        assert!(
            shell.listed(&session_id).await.is_none(),
            "an ended session must stop being shown"
        );
        let stale = framed.request("", &[], None).await;
        assert_eq!(stale.status, 404, "its address must stop answering too");
        assert_eq!(
            stale.header("content-type"),
            "text/html; charset=utf-8",
            "a stale top-level URL must render rather than download"
        );
        assert!(stale.body.contains("<title>Not Found</title>"));

        shell.handle.abort();
    }
}

// ── What framing costs, and what it does not ────────────────────────

in_both_shapes! {
    async fn a_framed_provider_keeps_its_policy_except_for_who_may_frame_it(shape: Shape) {
        let session = provider_session_with(
            Router::new().route(
                "/",
                get(|| async { ([(header::SET_COOKIE, "provider_state=1")], "panel") }),
            ),
            UiSecurityPolicy::isolated().with_content_security_policy(
                header::HeaderValue::from_static(
                    "default-src 'none'; script-src 'self'; frame-ancestors 'none'",
                ),
            ),
        )
        .await;
        let shell = serving(shape).await;
        let framed = shell.framed(&session).await;

        let reply = framed.request("", &[("Origin", "null")], None).await;
        assert_eq!(reply.status, 200);
        let policy = reply.header("content-security-policy");
        assert!(policy.contains("script-src 'self'"), "{policy}");
        assert!(!policy.contains("frame-ancestors 'none'"), "{policy}");
        // Which origin may frame this session is the one thing the two shapes
        // disagree about, and getting it wrong blanks every frame. The negative
        // control is the other shape's answer: `'self'` is this session's own
        // origin once it has one, and the page is somewhere else.
        match shape {
            Shape::OneOrigin => assert!(policy.contains("frame-ancestors 'self'"), "{policy}"),
            Shape::OriginPerSession => {
                assert!(
                    policy.contains(&format!(
                        "frame-ancestors http://{host} https://{host}",
                        host = shell.host
                    )),
                    "{policy}"
                );
                assert!(!policy.contains("frame-ancestors 'self'"), "{policy}");
            }
        }
        // A cookie the browser held for one session would be attached to the
        // next one's requests by the browser itself in one shape, and is simply
        // not this listener's to hand out in either. It is kept on this side —
        // see `a_cookie_one_session_set_is_never_sent_to_another`.
        assert!(
            !reply.headers.contains_key("set-cookie"),
            "{:?}",
            reply.headers
        );
        // A sandboxed document is cross-origin to the URL it was served from, so
        // without this its own fetches fail.
        assert_eq!(reply.header("access-control-allow-origin"), "null");
        assert!(
            !reply
                .headers
                .contains_key("access-control-allow-credentials"),
            "the proxy path is the whole authorization; there is nothing to send"
        );

        if shape == Shape::OriginPerSession {
            // With an origin of its own the framed UI is same-origin with what
            // it fetches, so it is allowed and needs no header to be. A page
            // that is neither is refused outright.
            let own = format!("http://{}", framed.host);
            let same_origin = framed.request("", &[("Origin", &own)], None).await;
            assert_eq!(same_origin.status, 200);
            assert_eq!(same_origin.header("access-control-allow-origin"), "");
            let foreign = framed
                .request("", &[("Origin", "https://attacker.example")], None)
                .await;
            assert_eq!(foreign.status, 403, "{}", foreign.body);
        }

        drop(session);
        shell.handle.abort();
    }
}

/// A preflight is a cost of the opaque origin: a sandboxed document is
/// cross-origin to the URL it was served from, so its own `fetch` becomes a
/// CORS request. It is answered here rather than upstream, and this test stays
/// in the shape that has the problem.
#[tokio::test]
#[allow(clippy::expect_used)]
async fn a_preflight_is_answered_here_rather_than_asked_of_a_provider() {
    let session = provider_session(Router::new().route("/input", get(|| async { "unused" }))).await;
    let shell = serving(Shape::OneOrigin).await;
    let framed = shell.framed(&session).await;

    let mut stream = TcpStream::connect(&framed.wire).await.expect("connect");
    stream
        .write_all(
            format!(
                "OPTIONS {}input HTTP/1.1\r\nHost: {}\r\nOrigin: null\r\n\
                 Access-Control-Request-Method: POST\r\n\
                 Access-Control-Request-Headers: content-type\r\nConnection: close\r\n\r\n",
                framed.path, framed.host
            )
            .as_bytes(),
        )
        .await
        .expect("write");
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.expect("read");
    let reply = parse(&String::from_utf8_lossy(&raw));
    assert_eq!(reply.status, 204, "{:?}", reply.headers);
    assert_eq!(reply.header("access-control-allow-origin"), "null");
    assert_eq!(reply.header("access-control-allow-methods"), "POST");
    assert_eq!(reply.header("access-control-allow-headers"), "content-type");

    drop(session);
    shell.handle.abort();
}

// ── A session's cookies are its own ─────────────────────────────────

/// A Provider that hands a credential over the way a proxied third-party panel does:
/// once in a query, and in a cookie on every request after it.
///
/// Both sessions in the test below use the same cookie *name*, because a
/// namespace that only worked while two Providers happened to disagree about
/// naming would not be one.
#[allow(clippy::expect_used)]
fn handoff_provider(label: &'static str, seen: Seen) -> Router {
    Router::new()
        .route(
            "/set",
            get(move || async move {
                (
                    [(
                        header::SET_COOKIE,
                        format!("handoff={label}; Path=/; Max-Age=600; HttpOnly; SameSite=Lax"),
                    )],
                    "handed off",
                )
            }),
        )
        .route(
            "/whoami",
            get(|State(seen): State<Seen>, request: Request| async move {
                let cookie = request
                    .headers()
                    .get(header::COOKIE)
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("<none>")
                    .to_owned();
                if let Ok(mut recorded) = seen.0.lock() {
                    recorded.push(cookie.clone());
                }
                cookie
            }),
        )
        .with_state(seen)
}

/// The last `Cookie` header this Provider was sent.
#[allow(clippy::expect_used)]
fn last_seen(seen: &Seen) -> String {
    seen.0
        .lock()
        .expect("recorded")
        .last()
        .cloned()
        .unwrap_or_default()
}

in_both_shapes! {
    async fn a_cookie_one_session_set_is_never_sent_to_another(shape: Shape) {
        let alpha_seen = Seen::default();
        let beta_seen = Seen::default();
        let alpha = provider_session(handoff_provider("alpha", alpha_seen.clone())).await;
        let beta = provider_session(handoff_provider("beta", beta_seen.clone())).await;
        let shell = serving(shape).await;
        let alpha_framed = shell.framed(&alpha).await;
        let beta_framed = shell.framed(&beta).await;
        assert_ne!(
            (&alpha_framed.host, &alpha_framed.path),
            (&beta_framed.host, &beta_framed.path)
        );

        // Before anything is set, neither session is sent a cookie.
        alpha_framed.request("whoami", &[], None).await;
        assert_eq!(last_seen(&alpha_seen), "<none>");

        // A hands its credential over. The browser is still given nothing —
        // holding it is not the browser's job in either shape, and in one of
        // them a cookie it held for A would be attached to B's requests by the
        // browser itself.
        let handed = alpha_framed.request("set", &[], None).await;
        assert_eq!(handed.status, 200, "{}", handed.body);
        assert!(
            !handed.headers.contains_key("set-cookie"),
            "a proxied cookie must never reach the browser: {:?}",
            handed.headers
        );

        // A's own next request carries it, which is the whole point.
        alpha_framed.request("whoami", &[], None).await;
        assert_eq!(last_seen(&alpha_seen), "handoff=alpha");

        // B, on the same listener, is sent nothing.
        beta_framed.request("whoami", &[], None).await;
        assert_eq!(
            last_seen(&beta_seen),
            "<none>",
            "one session's cookie must not reach another"
        );

        // Nor when someone puts it on the wire by hand: `Cookie` has no line in
        // the request allowlist, so a browser or a script cannot promote itself
        // into any session's jar.
        beta_framed
            .request("whoami", &[("Cookie", "handoff=alpha")], None)
            .await;
        assert_eq!(last_seen(&beta_seen), "<none>");

        // Nor by claiming to be the other session while asking for this one:
        // the name decides which mount answers, and the jar belongs to the
        // mount rather than to the request.
        request(
            &shell.wire,
            &alpha_framed.host,
            &format!("{}whoami", beta_framed.path),
            &[],
            None,
        )
        .await;
        assert_eq!(last_seen(&beta_seen), "<none>");

        // B sets its own under the same name. Neither ends up with the other's.
        beta_framed.request("set", &[], None).await;
        beta_framed.request("whoami", &[], None).await;
        assert_eq!(last_seen(&beta_seen), "handoff=beta");
        alpha_framed.request("whoami", &[], None).await;
        assert_eq!(last_seen(&alpha_seen), "handoff=alpha");

        drop(alpha);
        drop(beta);
        shell.handle.abort();
    }
}

// ── A session AFUI does not serve ───────────────────────────────────

/// A UI served by something that is not AFUI, behind a credential of its own.
///
/// Shaped like a proxied third-party panel: the credential arrives as a query
/// parameter, a request without it is refused, and the address a person is
/// given carries it. Nothing here is minted by or known to AFUI.
#[allow(clippy::expect_used)]
fn foreign_panel(seen: Seen) -> Router {
    Router::new()
        .fallback(any(
            |State(seen): State<Seen>, request: Request| async move {
                let query = request.uri().query().unwrap_or_default().to_owned();
                if let Ok(mut recorded) = seen.0.lock() {
                    recorded.push(format!("{}?{query}", request.uri().path()));
                }
                if query
                    .split('&')
                    .any(|pair| pair == "handoff_secret=deadbeef")
                {
                    (StatusCode::OK, "panel").into_response()
                } else {
                    StatusCode::UNAUTHORIZED.into_response()
                }
            },
        ))
        .with_state(seen)
}

in_both_shapes! {
    async fn a_session_that_is_only_named_is_framed_exactly_like_one_that_is_served(shape: Shape) {
        use agent_first_ui::UiUpstream;

        registry_root();
        let seen = Seen::default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the foreign panel");
        let upstream_addr = listener.local_addr().expect("addr");
        let app = foreign_panel(seen.clone());
        let panel = tokio::spawn(async move {
            let _ignored = axum::serve(listener, app).await;
        });
        let panel_url = format!("http://{upstream_addr}/takeover/panel?handoff_secret=deadbeef");
        let announced = UiUpstream::new("remote", "takeover", &panel_url)
            .expect("upstream")
            .with_subject("captcha wall")
            .announce()
            .expect("announce");

        let shell = serving(shape).await;
        let session_id = announced.metadata().session_id.as_str().to_owned();
        let card = shell.listed(&session_id).await.expect("listed");
        // The page is handed exactly the fields a served session gives it, and no
        // way of telling the two apart.
        assert_eq!(card["provider_id"], "remote");
        assert_eq!(card["ui_kind"], "takeover");
        assert_eq!(card["subject"], "captcha wall");
        assert!(card.get("access_url_secret").is_none());
        assert!(
            !serde_json::to_string(&card)
                .expect("json")
                .contains("deadbeef"),
            "a credential AFUI does not own is still a credential: {card}"
        );

        // Framing it reaches the panel, which means the announced address — query
        // and all — was reproduced upstream. Without that the panel answers 401.
        let framed = shell.framed_id(&session_id).await;
        let reply = framed.request("", &[], None).await;
        assert_eq!(reply.status, 200, "{}", reply.body);
        assert_eq!(last_seen(&seen), "/takeover/panel/?handoff_secret=deadbeef");

        // And on every request after it, not only the first — the panel's own
        // assets and its WebSocket carry the same credential, with the page's own
        // parameters after it.
        let asset = framed.request("app.js?resize=scale", &[], None).await;
        assert_eq!(asset.status, 200, "{}", asset.body);
        assert_eq!(
            last_seen(&seen),
            "/takeover/panel/app.js?handoff_secret=deadbeef&resize=scale"
        );

        // Negative control for the announced query: a mount built from the same
        // address without it is refused by the panel, which is what proves the 200s
        // above came from reproducing the credential rather than from the panel
        // being open.
        let unannounced = UiUpstream::new(
            "remote",
            "takeover",
            format!("http://{upstream_addr}/takeover/panel"),
        )
        .expect("upstream")
        .announce()
        .expect("announce");
        let bare = shell
            .framed_id(unannounced.metadata().session_id.as_str())
            .await;
        let refused = bare.request("", &[], None).await;
        assert_eq!(refused.status, 401, "{}", refused.body);

        drop(unannounced);
        drop(announced);
        shell.handle.abort();
        panel.abort();
    }
}

// ── An upgraded connection is relayed ───────────────────────────────

/// A WebSocket echo, as a Provider would serve one.
///
/// Hand-rolled onto a raw upgrade rather than taken from a framework, because
/// it has to be a route on a real session router — but the protocol on top of
/// it is a real implementation at both ends, which is what makes the test
/// evidence rather than a restatement of the proxy's own code.
#[allow(clippy::expect_used)]
fn websocket_echo() -> Router {
    use axum::response::Response;
    use hyper_util::rt::TokioIo;
    use tokio_tungstenite::WebSocketStream;
    use tokio_tungstenite::tungstenite::Message;
    use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
    use tokio_tungstenite::tungstenite::protocol::Role;

    Router::new().route(
        "/socket",
        any(|mut upgrade: Request| async move {
            let Some(key) = upgrade.headers().get(header::SEC_WEBSOCKET_KEY).cloned() else {
                return StatusCode::BAD_REQUEST.into_response();
            };
            let accept = derive_accept_key(key.as_bytes());
            tokio::spawn(async move {
                let Ok(upgraded) = hyper::upgrade::on(&mut upgrade).await else {
                    return;
                };
                let mut socket =
                    WebSocketStream::from_raw_socket(TokioIo::new(upgraded), Role::Server, None)
                        .await;
                // Spoken without being asked, so the test can tell a frame that
                // travelled server-to-client from one that is merely an echo.
                let _ignored = socket.send(Message::text("hello from the provider")).await;
                while let Some(Ok(message)) = socket.next().await {
                    if message.is_text() && socket.send(message).await.is_err() {
                        return;
                    }
                }
            });
            Response::builder()
                .status(StatusCode::SWITCHING_PROTOCOLS)
                .header(header::CONNECTION, "upgrade")
                .header(header::UPGRADE, "websocket")
                .header(header::SEC_WEBSOCKET_ACCEPT, accept)
                .body(Body::empty())
                .map_or_else(
                    |_| StatusCode::INTERNAL_SERVER_ERROR.into_response(),
                    IntoResponse::into_response,
                )
        }),
    )
}

in_both_shapes! {
    /// §2.5 step 3 needs this: a remote-framebuffer panel is RFB over WebSocket, and
    /// an upgrade the proxy refuses to perform cannot be framed at all.
    async fn frames_cross_the_proxy_in_both_directions_once_the_connection_is_upgraded(
        shape: Shape,
    ) {
        use tokio_tungstenite::tungstenite::Message;

        let session = provider_session(websocket_echo()).await;
        let shell = serving(shape).await;
        let framed = shell.framed(&session).await;

        let stream = TcpStream::connect(&framed.wire).await.expect("connect");
        // tungstenite mints the key, and checks the `Sec-WebSocket-Accept` derived
        // from it. Both have to have crossed intact for this to return at all.
        // The address is the session's own — which in one shape is a name this
        // process never resolves, because the connection above is already open.
        let (mut socket, response) = tokio_tungstenite::client_async(
            format!("ws://{}{}socket", framed.host, framed.path),
            stream,
        )
        .await
        .expect("the handshake must complete through the proxy");
        assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);

        // Provider → person, unprompted.
        let greeting = tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .expect("a frame from the provider")
            .expect("a frame")
            .expect("a readable frame");
        assert_eq!(greeting, Message::text("hello from the provider"));

        // Person → Provider → person, twice, on the same connection: one frame that
        // happened to work would not distinguish a relay from a fluke.
        for keystroke in ["a keystroke", "another keystroke"] {
            socket
                .send(Message::text(keystroke))
                .await
                .expect("send a frame");
            let echoed = tokio::time::timeout(Duration::from_secs(5), socket.next())
                .await
                .expect("an echo")
                .expect("a frame")
                .expect("a readable frame");
            assert_eq!(echoed, Message::text(keystroke));
        }

        socket.close(None).await.expect("close");
        drop(session);
        shell.handle.abort();
    }
}

// ── Walking away ────────────────────────────────────────────────────

/// Short enough to run, long enough that a loaded machine cannot cross one of
/// these windows between two requests on a real socket.
const IDLE: Duration = Duration::from_millis(700);
const GRACE: Duration = Duration::from_millis(700);

fn lapsing() -> UiAttentionPolicy {
    UiAttentionPolicy::default()
        .with_idle_timeout(IDLE)
        .with_grace_period(GRACE)
}

/// Wait out one card's whole window with somebody still at the page.
///
/// This is the shape of a person who took that card off their page and carried
/// on working: the heartbeat keeps arriving and stops naming the session, which
/// is the only way one view can end while the rest of the page does not.
async fn away_from_one_card(shell: &Shell) {
    for _ in 0..5 {
        tokio::time::sleep(IDLE / 2).await;
        assert_eq!(shell.attend(&[]).await, 204);
    }
}

in_both_shapes! {
    /// The whole arc, through the wire the browser uses: warned while still
    /// working, kept open by a person, then left alone until it is not.
    async fn a_view_warns_then_ends_and_the_address_it_was_reached_at_stops_working(shape: Shape) {
        let session = provider_session(Router::new().route("/", get(|| async { "panel" }))).await;
        let shell = serving_with(shape, lapsing()).await;
        let session_id = session.metadata().session_id.as_str().to_owned();
        let framed = shell.framed(&session).await;
        assert_eq!(framed.request("", &[], None).await.status, 200);
        assert_eq!(shell.card(&session_id).await["state"], "live");

        // Warned — and completely usable, which is the difference between a
        // warning and the outage arriving five minutes early.
        tokio::time::sleep(IDLE).await;
        assert_eq!(shell.attend(&[]).await, 204);
        let warned = shell.card(&session_id).await;
        assert_eq!(warned["state"], "expiring");
        assert!(warned["expires_in_ms"].as_u64().expect("a countdown") > 0);
        assert_eq!(framed.request("", &[], None).await.status, 200);

        // A person presses the button. The address does not move, so the frame
        // is not rebuilt and whatever it was holding survives.
        tokio::time::sleep(IDLE).await;
        assert_eq!(shell.attend(&[]).await, 204);
        assert_eq!(shell.card(&session_id).await["state"], "expiring");
        assert_eq!(shell.attend(&[&session_id]).await, 204);
        let kept = shell.card(&session_id).await;
        assert_eq!(kept["state"], "live");
        assert_eq!(kept["proxy_url_secret"], warned["proxy_url_secret"]);

        // Then nobody comes back to this one.
        away_from_one_card(&shell).await;
        let lapsed = shell.card(&session_id).await;
        assert_eq!(lapsed["state"], "dead");
        assert!(
            lapsed["proxy_url_secret"].is_null(),
            "a lapsed card has no address, so the page has nothing to frame: {lapsed}"
        );
        assert_eq!(
            framed.request("", &[], None).await.status,
            404,
            "the address a frame was reached at is what a leaked URL would be, \
             and it has to stop working"
        );
        // The session itself is untouched: this expiry is a remote view ending,
        // never a Provider being shut down from a phone.
        assert!(!session.access_token().is_revoked());

        drop(session);
        shell.handle.abort();
    }
}

in_both_shapes! {
    /// Coming back is a person acting on one card, and it never resurrects the
    /// credential that lapsed.
    async fn a_lapsed_view_is_reopened_at_an_address_it_never_had_before(shape: Shape) {
        let session = provider_session(Router::new().route("/", get(|| async { "panel" }))).await;
        let shell = serving_with(shape, lapsing()).await;
        let session_id = session.metadata().session_id.as_str().to_owned();
        let was = shell.framed(&session).await;
        away_from_one_card(&shell).await;
        assert_eq!(shell.card(&session_id).await["state"], "dead");

        // The heartbeat names it, as the page names everything it is showing.
        // That must not undo the lapse, or nothing would ever stay lapsed.
        assert_eq!(shell.attend(&[&session_id]).await, 204);
        assert_eq!(shell.card(&session_id).await["state"], "dead");

        let reopened = shell
            .page(&format!("sessions/{session_id}/reopen"), &[], Some(""))
            .await;
        assert_eq!(reopened.status, 204, "{}", reopened.body);
        let back = shell.card(&session_id).await;
        assert_eq!(back["state"], "live");
        let now = shell.frame_of(back["proxy_url_secret"].as_str().expect("an address"));
        assert_eq!(now.request("", &[], None).await.status, 200);
        assert_eq!(
            was.request("", &[], None).await.status,
            404,
            "reopening mints a credential rather than resurrecting the revoked one"
        );

        drop(session);
        shell.handle.abort();
    }
}

in_both_shapes! {
    /// The page's own ending, which takes its frames with it.
    async fn the_page_stops_answering_for_everything_once_it_has_lapsed(shape: Shape) {
        let session = provider_session(Router::new().route("/", get(|| async { "panel" }))).await;
        let shell = serving_with(shape, lapsing()).await;
        let framed = shell.framed(&session).await;

        tokio::time::sleep(IDLE + GRACE).await;
        // Gone, not missing. Somebody arriving here holds the page's own
        // credential — they had this page — so being told it is finished
        // reveals nothing they did not already have, and it is the one thing
        // that tells them apart from a person who mistyped a URL.
        let refused = shell.page("sessions", &[], None).await;
        assert_eq!(refused.status, 410, "{}", refused.body);
        let landing = shell.page("", &[], None).await;
        assert_eq!(landing.status, 410, "{}", landing.body);
        // The three questions arriving at a dead URL raises, each answered:
        // was the address wrong, is the work lost, and what do I do now.
        assert!(landing.body.contains("expired"), "{}", landing.body);
        assert!(
            landing.body.contains("not a mistyped link"),
            "a lapsed page must rule out the explanation a 404 implies: {}",
            landing.body
        );
        assert!(
            landing.body.contains("still running"),
            "a lapsed page must say the sessions outlived it: {}",
            landing.body
        );
        assert!(
            landing.body.contains("afui session serve"),
            "a lapsed page must name the way back: {}",
            landing.body
        );
        assert_eq!(
            framed.request("", &[], None).await.status,
            404,
            "a page that has stopped answering must not leave the frame showing \
             a person's work to whoever picks the device up"
        );
        // And the page cannot talk its own way back: the person who could
        // attend it is the one who is no longer there.
        assert_eq!(shell.attend(&[]).await, 410);
        assert_eq!(shell.page("sessions", &[], None).await.status, 410);

        drop(session);
        shell.handle.abort();
    }
}

in_both_shapes! {
    /// What a lapse must not turn into: a way to learn this port hosts AFUI.
    ///
    /// The explanation above is owed to whoever held the page. It is owed to
    /// nobody else, and a listener that answered a passing scan with "an AFUI
    /// page expired here" would be answering the question the 404 exists to
    /// refuse — before and after the lapse alike.
    async fn a_lapsed_listener_still_says_nothing_to_a_credential_it_does_not_know(shape: Shape) {
        let session = provider_session(Router::new().route("/", get(|| async { "panel" }))).await;
        let shell = serving_with(shape, lapsing()).await;

        let before = shell.wrong_credential().await;
        assert_eq!(before.status, 404, "{}", before.body);

        tokio::time::sleep(IDLE + GRACE).await;
        let after = shell.wrong_credential().await;
        assert_eq!(after.status, 404, "{}", after.body);
        assert_eq!(
            after.body, before.body,
            "a lapse must not make a stranger's 404 read differently"
        );
        assert!(
            !after.body.contains("expired"),
            "a credential this listener never issued must not be told anything: {}",
            after.body
        );

        drop(session);
        shell.handle.abort();
    }
}

in_both_shapes! {
    /// Traffic through a frame is the only evidence there is that somebody is
    /// using a Provider UI, since what happens inside one is on an origin this
    /// listener cannot read.
    async fn a_frame_being_used_keeps_itself_and_the_page_alive(shape: Shape) {
        let session = provider_session(Router::new().route("/", get(|| async { "panel" }))).await;
        let shell = serving_with(shape, lapsing()).await;
        let session_id = session.metadata().session_id.as_str().to_owned();
        let framed = shell.framed(&session).await;

        // Well past the idle window in total, with nothing but frame traffic and
        // the page's own polling — which is not attention and must not count.
        for _ in 0..4 {
            tokio::time::sleep(IDLE / 2).await;
            assert_eq!(framed.request("", &[], None).await.status, 200);
            assert_eq!(shell.card(&session_id).await["state"], "live");
        }

        drop(session);
        shell.handle.abort();
    }
}

// ── The window delivery is untouched ────────────────────────────────

#[cfg(unix)]
#[tokio::test]
#[allow(clippy::expect_used)]
async fn a_window_still_opens_when_nothing_is_being_served() {
    use std::os::unix::fs::PermissionsExt;

    registry_root();
    // §2.6: the delivery a Provider uses when nobody is remote must not depend
    // on this file existing. Nothing is serving in this test, on purpose.
    let directory = tempfile::tempdir().expect("stub directory");
    let script = directory.path().join("browser.sh");
    let log = directory.path().join("argv");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > {}\n",
            log.to_string_lossy()
        ),
    )
    .expect("write stub");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");

    let session = UiSession::<()>::new("reviewer", "case_review").expect("session");
    let outcome = session
        .window(
            Router::new().route("/", get(|| async { "review" })),
            &UiWindowConfig::default().with_browser_binary(&script),
        )
        .await
        .expect("window delivery");
    assert!(matches!(outcome, Outcome::Closed));
    let launched = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(
        launched.contains("--app=http://127.0.0.1:"),
        "a window must still be opened onto the session's own loopback URL: {launched:?}"
    );
}

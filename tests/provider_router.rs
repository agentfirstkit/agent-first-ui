//! A Provider that already has a listener hosts AFUI sessions inside it.
//!
//! afterminal serves its UI from its own API server, so it cannot let AFUI bind
//! anything — and until there was a mount, it had to hand-write a credential
//! store and a `/ui/{token}/` prefix to get many windows onto one port. This
//! drives the affordance that replaces that: the Provider owns the router, the
//! listener and every other route on it, and AFUI owns the sessions under one
//! prefix.
//!
//! In its own test binary because it sets `AFUI_CONFIG_DIR`, which is process
//! global — sharing a binary with other tests would let them race over it.

// Every session affordance this drives is behind `session`, so a guard-tier
// build has nothing here to compile.
#![cfg(feature = "session")]

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

use agent_first_ui::{Outcome, UiExpiry, UiMount, UiSession, list_sessions};
use axum::Router;
use axum::extract::State;
use axum::response::Html;
use axum::routing::{get, post};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// The Provider's own API, which knows nothing about AFUI.
fn provider_api() -> Router {
    Router::new().route("/v1/health", get(|| async { "ok" }))
}

/// One session's page, plus the route a person submits from.
fn review_router(completion: agent_first_ui::UiCompletion<String>, label: &'static str) -> Router {
    Router::new()
        .route(
            "/",
            get(move || async move { Html(format!("<h1>{label}</h1>")) }),
        )
        .route("/asset.css", get(move || async move { format!("/* {label} */") }))
        .route(
            "/submit",
            post(
                move |State(completion): State<Arc<agent_first_ui::UiCompletion<String>>>| async move {
                    completion.complete(format!("submitted:{label}")).await;
                    "done"
                },
            ),
        )
        .with_state(Arc::new(completion))
}

struct Reply {
    status: u16,
    body: String,
}

/// One request against the Provider's own listener, read to end of connection.
///
/// Fails loudly rather than threading a `Result` up into the test: a fixture
/// that could not reach the listener has nothing to assert about. Clippy allows
/// that inside a `#[test]` function but not in a helper, hence the scope.
#[allow(clippy::expect_used)]
async fn call(url: &str, method: &str) -> Reply {
    let rest = url.strip_prefix("http://").expect("http url");
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let mut stream = tokio::net::TcpStream::connect(authority)
        .await
        .expect("connect to the provider's listener");
    stream
        .write_all(
            format!(
                "{method} /{path} HTTP/1.1\r\nHost: {authority}\r\n\
                 Content-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .expect("write request");
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.expect("read response");
    let raw = String::from_utf8_lossy(&raw).into_owned();
    let status = raw
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    Reply { status, body: raw }
}

#[tokio::test]
async fn a_provider_hosts_afui_sessions_inside_the_router_and_listener_it_already_owns() {
    let root = tempfile::tempdir().expect("registry root");
    // SAFETY: this binary contains exactly one test, so nothing races here.
    unsafe { std::env::set_var("AFUI_CONFIG_DIR", root.path()) };

    // The Provider binds. AFUI does not.
    let listener = tokio::net::TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .await
        .expect("the provider's own listener");
    let address = listener.local_addr().expect("local address");
    let mount = UiMount::new(format!("http://{address}/ui/")).expect("mount");
    assert_eq!(mount.base_url(), format!("http://{address}/ui/"));

    let app = provider_api().nest_service("/ui", mount.router());
    let server = tokio::spawn(async move {
        let _ignored = axum::serve(listener, app).await;
    });

    let review = UiSession::<String>::new("afmail", "case_review")
        .expect("session")
        .with_subject("case-42");
    let review_completion = review.completion();
    let review = mount
        .attach(
            review,
            review_router(review_completion, "review"),
            UiExpiry::Never,
        )
        .expect("attach the review");
    let terminal = UiSession::<String>::new("afterminal", "terminal").expect("session");
    let terminal_completion = terminal.completion();
    let terminal = mount
        .attach(
            terminal,
            review_router(terminal_completion, "terminal"),
            UiExpiry::Never,
        )
        .expect("attach the terminal");

    assert_eq!(mount.session_count(), 2);

    // Both sessions live on the Provider's port, under the Provider's prefix.
    for active in [&review, &terminal] {
        let url = active.access_url();
        assert!(url.starts_with(&format!("http://{address}/ui/")), "{url}");
        assert!(url.ends_with('/'), "{url}");
    }
    assert_ne!(review.access_url(), terminal.access_url());

    // Each credential reaches its own page and its own assets.
    for (active, label) in [(&review, "review"), (&terminal, "terminal")] {
        let page = call(active.access_url(), "GET").await;
        assert_eq!(page.status, 200, "{}", page.body);
        assert!(
            page.body.contains(&format!("<h1>{label}</h1>")),
            "{}",
            page.body
        );
        let asset = call(&format!("{}asset.css", active.access_url()), "GET").await;
        assert_eq!(asset.status, 200, "{}", asset.body);
        assert!(
            asset.body.contains(&format!("/* {label} */")),
            "{}",
            asset.body
        );
        // AFUI's security headers are applied by the mount, not by the
        // Provider: nothing about them changes because the listener did.
        assert!(
            asset
                .body
                .to_ascii_lowercase()
                .contains("content-security-policy:"),
            "{}",
            asset.body
        );
    }

    // The Provider's own routes are untouched by what is nested beside them.
    let health = call(&format!("http://{address}/v1/health"), "GET").await;
    assert_eq!(health.status, 200, "{}", health.body);
    assert!(health.body.ends_with("ok"), "{}", health.body);

    // A wrong credential under the prefix is a 404, not a hint that the
    // Provider hosts anything here.
    let guessed = call(&format!("http://{address}/ui/{}/", "0".repeat(64)), "GET").await;
    assert_eq!(guessed.status, 404, "{}", guessed.body);

    // §4.2.1: a session mounted this way is still a session `afui` can see, or
    // `afui session serve` would have nothing to reach.
    let listed = list_sessions().expect("list");
    assert_eq!(listed.len(), 2, "{listed:#?}");
    let announced = listed
        .iter()
        .find(|entry| entry.provider_id == "afmail")
        .expect("the review is announced");
    assert_eq!(announced.ui_kind, "case_review");
    assert_eq!(announced.subject.as_deref(), Some("case-42"));
    assert_eq!(
        announced.access_url_secret,
        review.access_url(),
        "the announced URL must be the one the Provider's listener actually serves"
    );

    // Driving one session to its end returns its own answer to its own waiter,
    // and leaves the other one serving.
    let submitted = call(&format!("{}submit", review.access_url()), "POST").await;
    assert_eq!(submitted.status, 200, "{}", submitted.body);
    let review_url = review.access_url().to_owned();
    assert_eq!(
        review.wait().await.expect("outcome"),
        Outcome::Completed("submitted:review".to_owned())
    );

    // Ending it revokes the credential and removes it from the mount and the
    // registry, without the Provider's listener noticing.
    assert_eq!(mount.session_count(), 1);
    assert_eq!(call(&review_url, "GET").await.status, 404);
    let remaining = list_sessions().expect("list");
    assert_eq!(remaining.len(), 1, "{remaining:#?}");
    assert_eq!(remaining[0].provider_id, "afterminal");
    assert_eq!(call(terminal.access_url(), "GET").await.status, 200);
    assert_eq!(
        call(&format!("http://{address}/v1/health"), "GET")
            .await
            .status,
        200
    );

    drop(terminal);
    server.abort();
}

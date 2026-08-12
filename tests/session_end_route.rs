//! Every delivery can be ended from outside it, not only a window.
//!
//! `Outcome::Closed` used to exist for window delivery alone — closing the
//! window was the only signal AFUI could see (§1.3 of the design notes). This
//! drives the reserved route every session answers under its own credential,
//! on every delivery this crate has: `POST <access_url>SESSION_END_PATH`.
//! `afui session close` and the served shell's own "End" control both reach
//! it; this file drives the route directly, the same way they do.

// Every session affordance this drives is behind `session`, so a guard-tier
// build has nothing here to compile.
#![cfg(feature = "session")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use agent_first_ui::{Outcome, SESSION_END_PATH, UiExpiry, UiHost, UiSession, UiWindowConfig};
use axum::Router;
use axum::response::IntoResponse;
use axum::routing::post;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// `POST <url>SESSION_END_PATH`, by hand — the same request `afui session
/// close` and the shell's "End" control both send, minus the machinery that
/// finds the URL in the first place. Returns the raw response, so a caller
/// that only wants the status can read it and a caller checking what
/// answered can read the body too.
async fn post_end_raw(url: &str) -> String {
    let rest = url.strip_prefix("http://").expect("http url");
    let (authority, path) = rest.split_once('/').expect("url has a path");
    let mut stream = tokio::net::TcpStream::connect(authority)
        .await
        .expect("connect to the session");
    stream
        .write_all(
            format!(
                "POST /{path}{SESSION_END_PATH} HTTP/1.1\r\nHost: {authority}\r\n\
                 Content-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .expect("write request");
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .await
        .expect("read response");
    response
}

async fn post_end(url: &str) -> u16 {
    post_end_raw(url)
        .await
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .expect("a status line")
}

#[tokio::test]
async fn ending_a_session_from_outside_it_is_closed_never_completed() {
    // A completion type that could carry an answer, and a route that would
    // supply one — so a bug that let this path manufacture `Completed` from
    // nothing would have somewhere real to manufacture it from.
    let router = Router::new().route(
        "/submit",
        post(|| async { "never called by this test".into_response() }),
    );
    let host = UiHost::bind_local().await.unwrap();
    let session = UiSession::<String>::new("reviewer", "case_review").unwrap();
    let active = host.attach(session, router, UiExpiry::Never).unwrap();
    let url = active.access_url().to_owned();

    assert_eq!(post_end(&url).await, 204);
    assert_eq!(active.wait().await.unwrap(), Outcome::Closed);

    // Once the session has actually ended, the same request is a 404 —
    // indistinguishable from a credential that never existed, so a 404 here
    // can never be read as "there used to be a session".
    let fake = format!("http://{}/{}/", host.local_addr(), "0".repeat(64));
    assert_eq!(post_end(&url).await, post_end(&fake).await);
    assert_eq!(post_end(&url).await, 404);
}

#[tokio::test]
async fn the_end_route_is_answered_before_the_providers_own_router_and_cannot_be_shadowed() {
    // A Provider that (incorrectly, but the point is this cannot happen even
    // by accident) routes on the same reserved name itself. If dispatch ever
    // reached the Provider's router first, this handler would answer and its
    // distinctive body would show up below.
    let shadow_path = format!("/{SESSION_END_PATH}");
    let router = Router::new().route(
        &shadow_path,
        post(|| async { "provider answered, not afui".into_response() }),
    );
    let host = UiHost::bind_local().await.unwrap();
    let session = UiSession::<()>::new("console", "terminal").unwrap();
    let active = host.attach(session, router, UiExpiry::Never).unwrap();
    let url = active.access_url().to_owned();

    let response = post_end_raw(&url).await;
    assert!(response.starts_with("HTTP/1.1 204"), "{response}");
    assert!(
        !response.contains("provider answered"),
        "the Provider's own route must never be reached: {response}"
    );
    assert_eq!(active.wait().await.unwrap(), Outcome::Closed);
}

#[tokio::test]
async fn the_whole_reserved_prefix_is_unreachable_not_only_the_end_route() {
    // `SESSION_END_PATH` is the one reserved route that exists today, but the
    // whole `__afui/` prefix is refused to the Provider, so a name AFUI
    // reserves later cannot already belong to a Provider that routed there
    // first. A GET here is not even the right method for `end`, which is
    // itself proof this is a namespace reservation and not a coincidence of
    // matching one exact path.
    let router = Router::new().route(
        "/__afui/watch",
        axum::routing::get(|| async { "provider answered, not afui".into_response() }),
    );
    let host = UiHost::bind_local().await.unwrap();
    let session = UiSession::<()>::new("console", "terminal").unwrap();
    let active = host.attach(session, router, UiExpiry::Never).unwrap();
    let url = active.access_url().to_owned();

    let rest = url.strip_prefix("http://").unwrap();
    let (authority, path) = rest.split_once('/').unwrap();
    let mut stream = tokio::net::TcpStream::connect(authority).await.unwrap();
    stream
        .write_all(
            format!(
                "GET /{path}__afui/watch HTTP/1.1\r\nHost: {authority}\r\n\
                 Connection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    assert!(response.starts_with("HTTP/1.1 404"), "{response}");
    assert!(
        !response.contains("provider answered"),
        "nothing under the reserved prefix may reach the Provider's router: {response}"
    );
}

#[tokio::test]
async fn posting_end_while_the_session_is_still_registered_is_idempotent() {
    let host = UiHost::bind_local().await.unwrap();
    let session = UiSession::<()>::new("reviewer", "case_review").unwrap();
    let active = host
        .attach(session, Router::new(), UiExpiry::Never)
        .unwrap();
    let url = active.access_url().to_owned();

    // Two requests before anything has drained the signal: both see a session
    // that is still there, so both get the same answer.
    assert_eq!(post_end(&url).await, 204);
    assert_eq!(post_end(&url).await, 204);

    assert_eq!(active.wait().await.unwrap(), Outcome::Closed);
    // And once it has actually ended, the same request is a 404.
    assert_eq!(post_end(&url).await, 404);
}

fn write_script(path: &Path, body: &str) -> io::Result<PathBuf> {
    let mut file = std::fs::File::create(path)?;
    write!(file, "{body}")?;
    drop(file);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(path.to_path_buf())
}

#[tokio::test]
async fn window_delivery_ends_through_the_same_route_with_no_special_case() {
    // Rule 4 of §D: window delivery needs no special handling, because it is
    // the same mount and the same dispatch as every other delivery. A stub
    // browser that just sits there proves the `Closed` this test sees came
    // from the HTTP route and not from a window process exiting on its own.
    let workdir = tempfile::tempdir().unwrap();
    let script = write_script(
        &workdir.path().join("idle-browser.sh"),
        "#!/bin/sh\nsleep 30\n",
    )
    .unwrap();

    let host = UiHost::bind_local().await.unwrap();
    let session = UiSession::<()>::new("console", "terminal").unwrap();
    let active = host
        .attach(session, Router::new(), UiExpiry::Never)
        .unwrap();
    let url = active.access_url().to_owned();

    let config = UiWindowConfig::default().with_browser_binary(&script);
    let ender = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        post_end(&url).await
    });

    let outcome = tokio::time::timeout(Duration::from_secs(5), active.open_window(&config))
        .await
        .expect("the end route must resolve the wait, not the never-exiting stub browser")
        .unwrap();
    assert_eq!(outcome, Outcome::Closed);
    assert_eq!(ender.await.unwrap(), 204);
}

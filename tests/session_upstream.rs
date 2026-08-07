//! A session that only names a UI something else already serves.
//!
//! In its own test binary because it sets `AFUI_CONFIG_DIR`, which is process
//! global — sharing a binary with other tests would let them race over it.

// Every session affordance this drives is behind `session`, so a guard-tier
// build has nothing here to compile.
#![cfg(feature = "session")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use agent_first_ui::{UiUpstream, list_sessions};
use axum::Router;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Something that is not AFUI, serving a UI behind a credential of its own.
///
/// Shaped like afhttp's takeover panel: the credential arrives as a query
/// parameter, and a request without it is refused. Nothing here is minted by,
/// known to, or revocable by AFUI.
async fn foreign_provider(secret: &'static str) -> String {
    let app = Router::new().fallback(move |request: Request| async move {
        let authorized = request.uri().query().is_some_and(|query| {
            query
                .split('&')
                .any(|pair| pair == format!("handoff_secret={secret}"))
        });
        if authorized {
            (StatusCode::OK, "panel").into_response()
        } else {
            StatusCode::UNAUTHORIZED.into_response()
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ignored = axum::serve(listener, app).await;
    });
    format!("http://{address}")
}

/// One GET, by hand, so the test adds no HTTP client.
async fn status(url: &str) -> u16 {
    let rest = url.strip_prefix("http://").unwrap();
    let (authority, path_and_query) = rest.split_once('/').unwrap();
    let mut stream = tokio::net::TcpStream::connect(authority).await.unwrap();
    stream
        .write_all(
            format!(
                "GET /{path_and_query} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    response.split_whitespace().nth(1).unwrap().parse().unwrap()
}

#[tokio::test]
async fn an_upstream_is_listed_like_any_session_and_keeps_working_after_it_is_withdrawn() {
    let root = tempfile::tempdir().unwrap();
    // SAFETY: this binary contains exactly one test, so nothing races here.
    unsafe { std::env::set_var("AFUI_CONFIG_DIR", root.path()) };

    assert!(list_sessions().unwrap().is_empty());

    let base = foreign_provider("deadbeef").await;
    let panel_url = format!("{base}/takeover/panel?handoff_secret=deadbeef");
    assert_eq!(
        status(&panel_url).await,
        200,
        "the fixture must start usable"
    );

    let announced = UiUpstream::new("afhttp", "takeover", &panel_url)
        .unwrap()
        .with_subject("captcha wall")
        .announce()
        .unwrap();

    let listed = list_sessions().unwrap();
    assert_eq!(listed.len(), 1, "{listed:#?}");
    let entry = &listed[0];
    assert_eq!(entry.provider_id, "afhttp");
    assert_eq!(entry.ui_kind, "takeover");
    assert_eq!(entry.subject.as_deref(), Some("captcha wall"));
    assert_eq!(entry.session_id, announced.metadata().session_id.as_str());
    // The announced address, query included: the credential lives there, and an
    // entry that dropped it would name a URL that answers 401.
    assert_eq!(entry.access_url_secret, panel_url);
    // The announcing process, which is the only thing bounding this session's
    // life — its host belongs to somebody else and outlives it.
    assert_eq!(entry.owner_pid, std::process::id());

    drop(announced);
    assert!(
        list_sessions().unwrap().is_empty(),
        "an ended announcement must stop being listed"
    );
    // And the half AFUI has no business touching: the credential was not its to
    // mint and is not its to revoke, so withdrawing the listing left the panel
    // exactly as usable as its owner says it is.
    assert_eq!(
        status(&panel_url).await,
        200,
        "withdrawing an announcement must not revoke a credential AFUI never minted"
    );
}

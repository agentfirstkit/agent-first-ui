//! Many sessions, one listener.
//!
//! Sessions and ports are orthogonal. A person may have several interfaces open
//! at once, and that is several sessions — not several ports. This matters
//! beyond tidiness: a container with one mapped port, a machine behind a
//! firewall, or a single tunnel all have exactly one way in.

// Every session affordance this drives is behind `session`, so a guard-tier
// build has nothing here to compile.
#![cfg(feature = "session")]

use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

use agent_first_ui::{Outcome, UiExpiry, UiHost, UiSession};
use axum::Router;
use axum::response::Html;
use axum::routing::get;

/// A page that reports which session served it, so a mix-up is visible.
fn labelled_router(label: &'static str) -> Router {
    Router::new()
        .route(
            "/",
            get(move || async move { Html(format!("<h1>{label}</h1>")) }),
        )
        .route(
            "/asset.css",
            get(move || async move { format!("/* {label} */") }),
        )
}

async fn body_of(url: &str) -> (u16, String) {
    let Some(rest) = url.strip_prefix("http://") else {
        return (0, String::new());
    };
    let Some((authority, path)) = rest.split_once('/') else {
        return (0, String::new());
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let Ok(mut stream) = tokio::net::TcpStream::connect(authority).await else {
        return (0, String::new());
    };
    let request = format!("GET /{path} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n");
    if stream.write_all(request.as_bytes()).await.is_err() {
        return (0, String::new());
    }
    let mut response = String::new();
    if stream.read_to_string(&mut response).await.is_err() {
        return (0, String::new());
    }
    let status = response
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    (status, response)
}

#[tokio::test]
async fn three_sessions_share_one_port_and_stay_isolated() {
    let host = UiHost::bind_local().await.unwrap();
    let port = host.local_addr().port();

    let review = host
        .attach(
            UiSession::<&str>::new("reviewer", "case_review").unwrap(),
            labelled_router("review"),
            UiExpiry::Never,
        )
        .unwrap();
    let terminal = host
        .attach(
            UiSession::<&str>::new("console", "terminal").unwrap(),
            labelled_router("terminal"),
            UiExpiry::Never,
        )
        .unwrap();
    let panel = host
        .attach(
            UiSession::<&str>::new("inspector", "table_inspect").unwrap(),
            labelled_router("panel"),
            UiExpiry::Never,
        )
        .unwrap();

    assert_eq!(host.session_count(), 3);

    // One port, three credentials.
    for active in [&review, &terminal, &panel] {
        assert!(
            active.access_url().contains(&format!("127.0.0.1:{port}/")),
            "session did not share the host port: {}",
            active.access_url()
        );
    }
    let urls = [
        review.access_url(),
        terminal.access_url(),
        panel.access_url(),
    ];
    assert_ne!(urls[0], urls[1]);
    assert_ne!(urls[1], urls[2]);

    // Each credential reaches its own page, and its own assets.
    for (active, label) in [
        (&review, "review"),
        (&terminal, "terminal"),
        (&panel, "panel"),
    ] {
        let (status, body) = body_of(active.access_url()).await;
        assert_eq!(status, 200, "{label} page: {body}");
        assert!(
            body.contains(&format!("<h1>{label}</h1>")),
            "{label}: {body}"
        );
        let (asset_status, asset_body) =
            body_of(&format!("{}asset.css", active.access_url())).await;
        assert_eq!(asset_status, 200, "{label} asset: {asset_body}");
        assert!(
            asset_body.contains(&format!("/* {label} */")),
            "{asset_body}"
        );
    }

    // An unknown credential on a live host is a 404, not a leak of what is here.
    let (status, _) = body_of(&format!(
        "http://127.0.0.1:{port}/0000000000000000000000000000000000000000000000000000000000000000/"
    ))
    .await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn results_route_back_to_the_call_that_is_waiting_for_them() {
    // Plan §2.5 step 4: several concurrent UIs must be independently
    // completable, in any order, with each answer reaching its own waiter.
    let host = UiHost::bind_local().await.unwrap();

    let first = host
        .attach(
            UiSession::<&str>::new("reviewer", "case_review").unwrap(),
            Router::new(),
            UiExpiry::Never,
        )
        .unwrap();
    let second = host
        .attach(
            UiSession::<&str>::new("ledger", "plan_confirm").unwrap(),
            Router::new(),
            UiExpiry::Never,
        )
        .unwrap();

    let first_completion = first.completion();
    let second_completion = second.completion();

    // Complete them in the opposite order to the one they were created in.
    second_completion.complete("second-answer").await;
    first_completion.complete("first-answer").await;

    assert_eq!(
        first.wait().await.unwrap(),
        Outcome::Completed("first-answer")
    );
    assert_eq!(
        second.wait().await.unwrap(),
        Outcome::Completed("second-answer")
    );
}

#[tokio::test]
async fn ending_one_session_leaves_its_neighbours_serving() {
    let host = UiHost::bind_local().await.unwrap();
    let leaving = host
        .attach(
            UiSession::<()>::new("reviewer", "case_review").unwrap(),
            labelled_router("leaving"),
            UiExpiry::Never,
        )
        .unwrap();
    let staying = host
        .attach(
            UiSession::<()>::new("console", "terminal").unwrap(),
            labelled_router("staying"),
            UiExpiry::Never,
        )
        .unwrap();
    let leaving_url = leaving.access_url().to_owned();
    let staying_url = staying.access_url().to_owned();

    assert_eq!(body_of(&leaving_url).await.0, 200);
    drop(leaving);

    // The departed session is gone from the host…
    assert_eq!(host.session_count(), 1);
    assert_eq!(body_of(&leaving_url).await.0, 404);
    // …and the listener is still up for everyone else.
    let (status, body) = body_of(&staying_url).await;
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("<h1>staying</h1>"), "{body}");
}

#[tokio::test]
async fn a_lan_host_advertises_a_reachable_authority_for_every_session() {
    let host = UiHost::bind(IpAddr::V4(Ipv4Addr::UNSPECIFIED))
        .await
        .unwrap()
        .with_advertised_host("192.168.1.4");
    let session = host
        .attach(
            UiSession::<()>::new("reviewer", "case_review").unwrap(),
            Router::new(),
            UiExpiry::After(Duration::from_secs(900)),
        )
        .unwrap();
    let url = session.access_url();
    // 0.0.0.0 is how you become reachable, never an address anyone can open.
    assert!(url.starts_with("http://192.168.1.4:"), "{url}");
    assert!(!url.contains("0.0.0.0"), "{url}");
    assert!(url.ends_with('/'), "{url}");
}

#[tokio::test]
async fn shutting_the_host_down_takes_every_session_with_it() {
    let host = UiHost::bind_local().await.unwrap();
    let session = host
        .attach(
            UiSession::<()>::new("inspector", "schema_inspect").unwrap(),
            labelled_router("panel"),
            UiExpiry::Never,
        )
        .unwrap();
    let url = session.access_url().to_owned();
    assert_eq!(body_of(&url).await.0, 200);

    host.shutdown().await.unwrap();
    // Connection refused, not a 404: the listener itself is gone.
    assert_eq!(body_of(&url).await.0, 0);
}

//! Credentials a host process issues itself, through the same dispatch a
//! session's credential goes through.
//!
//! The point of this file is what a Provider does *not* have to reimplement.
//! Every assertion below is something a hand-written copy of credential
//! routing has to remember, and the versions this replaced had each forgotten
//! at least one: the 404 that does not confirm a UI is here, the `Host` check
//! that stops a rebound name reaching the same socket, the response headers,
//! and the reserved prefix that keeps a Provider's own routes out of the way
//! of names AFUI adds later.

#![cfg(feature = "session")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

use agent_first_ui::{
    APP_ICON_PATH, SESSION_END_PATH, UiAppIcon, UiBlobPolicy, UiExpiry, UiHost, UiPagePolicy,
    UiPageScript,
};
use axum::Router;
use axum::routing::get;

const ICON: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 1 1\"></svg>";

async fn host() -> UiHost {
    UiHost::bind(IpAddr::V4(Ipv4Addr::LOCALHOST))
        .await
        .expect("a loopback host binds")
}

fn provider_routes() -> Router {
    Router::new().route("/panel", get(|| async { "the provider page" }))
}

/// Every request names the authority the URL actually advertises and connects
/// to loopback regardless, exactly as a browser opening that URL would.
async fn http_get(url: &str, authority: &str) -> (u16, String, axum::http::HeaderMap) {
    let (_, rest) = url.split_once("://").expect("an http url");
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let port = host.rsplit(':').next().expect("a port");
    let request = format!("GET /{path} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n");
    raw(port, &request).await
}

async fn http_post(url: &str, authority: &str) -> (u16, String, axum::http::HeaderMap) {
    let (_, rest) = url.split_once("://").expect("an http url");
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let port = host.rsplit(':').next().expect("a port");
    let request = format!(
        "POST /{path} HTTP/1.1\r\nHost: {authority}\r\nContent-Length: 0\r\nConnection: \
         close\r\n\r\n"
    );
    raw(port, &request).await
}

async fn raw(port: &str, request: &str) -> (u16, String, axum::http::HeaderMap) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("the host is listening");
    stream
        .write_all(request.as_bytes())
        .await
        .expect("the request is written");
    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .await
        .expect("the response is read");
    let text = String::from_utf8_lossy(&raw).into_owned();
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    let mut headers = axum::http::HeaderMap::new();
    for line in text.lines().skip(1) {
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && let (Ok(name), Ok(value)) = (
                name.trim().parse::<axum::http::HeaderName>(),
                value.trim().parse::<axum::http::HeaderValue>(),
            )
        {
            headers.insert(name, value);
        }
    }
    (status, text, headers)
}

#[tokio::test]
async fn a_credential_this_process_issued_reaches_the_provider_router() {
    let host = host().await;
    let access = host.external_access(
        provider_routes(),
        UiPagePolicy::new(UiPageScript::SameOrigin).into_security_policy(),
        Some(UiAppIcon::from_svg(ICON).expect("an svg")),
    );
    let credential = access
        .issue(UiExpiry::Idle(Duration::from_secs(60)))
        .expect("a credential is issued");

    let (status, body, headers) = http_get(
        &format!("{}panel", credential.access_url_secret()),
        host.authority(),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("the provider page"), "{body}");
    // The headers a session gets, without the Provider having applied any.
    assert_eq!(headers["cache-control"], "no-store");
    assert_eq!(headers["x-content-type-options"], "nosniff");
    assert!(headers.contains_key("content-security-policy"));

    // The Provider's identity, from the reserved route — the one thing a
    // Provider was previously forced to answer under the reserved prefix
    // itself, because its own dispatch did not.
    let (status, _, _) = http_get(
        &format!("{}{APP_ICON_PATH}", credential.access_url_secret()),
        host.authority(),
    )
    .await;
    assert_eq!(status, 200);
}

/// There is no UI here to authenticate against, and saying otherwise would
/// confirm this port hosts one.
#[tokio::test]
async fn a_credential_that_was_never_issued_is_absent_rather_than_refused() {
    let host = host().await;
    let _access = host.external_access(
        provider_routes(),
        UiPagePolicy::new(UiPageScript::SameOrigin).into_security_policy(),
        None,
    );
    let (status, _, _) = http_get(
        &format!("http://{}/{}/panel", host.authority(), "0".repeat(64)),
        host.authority(),
    )
    .await;
    assert_eq!(status, 404);
}

/// The DNS-rebinding guard, which every hand-written copy of this dispatch had
/// lost: a name that resolves to loopback reaches the same socket, but arrives
/// under its own `Host`.
#[tokio::test]
async fn a_rebound_name_reaching_the_same_socket_is_refused() {
    let host = host().await;
    let access = host.external_access(
        provider_routes(),
        UiPagePolicy::new(UiPageScript::SameOrigin).into_security_policy(),
        None,
    );
    let credential = access
        .issue(UiExpiry::Idle(Duration::from_secs(60)))
        .expect("a credential is issued");
    let (status, _, _) = http_get(
        &format!("{}panel", credential.access_url_secret()),
        "attacker.example",
    )
    .await;
    assert_eq!(status, 403);
}

#[tokio::test]
async fn revoking_stops_exactly_the_one_credential_it_names() {
    let host = host().await;
    let access = host.external_access(
        provider_routes(),
        UiPagePolicy::new(UiPageScript::SameOrigin).into_security_policy(),
        None,
    );
    let first = access
        .issue(UiExpiry::Idle(Duration::from_secs(60)))
        .expect("a credential is issued");
    let second = access
        .issue(UiExpiry::Idle(Duration::from_secs(60)))
        .expect("a second credential is issued");
    assert_eq!(access.live_count(), 2);

    assert!(access.revoke(first.secret()));
    assert!(
        !access.revoke(first.secret()),
        "revoking twice is not a second revocation"
    );
    assert!(!access.revoke(&"0".repeat(64)));
    assert_eq!(access.live_count(), 1);

    let (gone, _, _) = http_get(
        &format!("{}panel", first.access_url_secret()),
        host.authority(),
    )
    .await;
    assert_eq!(gone, 404);
    let (alive, _, _) = http_get(
        &format!("{}panel", second.access_url_secret()),
        host.authority(),
    )
    .await;
    assert_eq!(alive, 200);
}

#[tokio::test]
async fn revoking_an_external_credential_closes_its_typed_runtime() {
    let host = host().await;
    let access = host.external_access(
        provider_routes(),
        UiPagePolicy::new(UiPageScript::SameOrigin)
            .allow_runtime()
            .into_security_policy(),
        None,
    );
    let (credential, runtime) = access
        .issue_with_runtime::<serde_json::Value, serde_json::Value, serde_json::Value>(
            UiExpiry::Idle(Duration::from_secs(60)),
        )
        .expect("a runtime-backed credential is issued");
    runtime
        .publish_state(&serde_json::json!({"ready": true}))
        .expect("opening state");

    assert!(access.revoke(credential.secret()));
    assert!(matches!(runtime.recv().await, Ok(None)));
}

/// Nobody renews, nobody issues, nobody counts: the listener has simply gone
/// quiet. The credential's runtime still closes when the credential lapses,
/// because what a Provider spun up per credential must not outlive it by
/// however long the next unrelated call takes to arrive.
#[tokio::test]
async fn a_lapsed_external_credential_closes_its_typed_runtime_unprompted() {
    let host = host().await;
    let access = host.external_access(
        provider_routes(),
        UiPagePolicy::new(UiPageScript::SameOrigin)
            .allow_runtime()
            .into_security_policy(),
        None,
    );
    let (_credential, runtime) = access
        .issue_with_runtime::<serde_json::Value, serde_json::Value, serde_json::Value>(
            UiExpiry::Idle(Duration::from_millis(80)),
        )
        .expect("a runtime-backed credential is issued");
    runtime
        .publish_state(&serde_json::json!({"ready": true}))
        .expect("opening state");

    let closed = tokio::time::timeout(Duration::from_secs(2), runtime.recv())
        .await
        .expect("the runtime closes when its credential lapses");
    assert!(matches!(closed, Ok(None)));
}

#[tokio::test]
async fn an_external_runtime_can_bound_calls_events_and_blobs_together() {
    let host = host().await;
    let access = host.external_access(
        provider_routes(),
        UiPagePolicy::new(UiPageScript::SameOrigin)
            .allow_runtime()
            .allow_images()
            .into_security_policy(),
        None,
    );
    let (_credential, runtime) = access
        .issue_with_runtime_capacities_and_blobs::<
            serde_json::Value,
            serde_json::Value,
            serde_json::Value,
        >(
            UiExpiry::Idle(Duration::from_secs(60)),
            2,
            3,
            UiBlobPolicy::new(8, ["image/png"]),
        )
        .expect("a bounded blob-enabled runtime credential is issued");

    let blob = runtime
        .store_blob("image/png", vec![1, 2, 3])
        .expect("the external runtime owns managed blobs");
    assert_eq!(blob.media_type(), "image/png");
    assert_eq!(blob.size_bytes(), 3);
}

#[test]
fn a_runtime_is_refused_when_the_page_policy_cannot_reach_it() {
    let mount = agent_first_ui::UiMount::new("http://127.0.0.1:8080/ui/").expect("mount");
    let access = mount.external_access(
        provider_routes(),
        UiPagePolicy::new(UiPageScript::SameOrigin).into_security_policy(),
        None,
    );
    let result = access
        .issue_with_runtime::<serde_json::Value, serde_json::Value, serde_json::Value>(
            UiExpiry::Never,
        );
    assert!(matches!(
        result,
        Err(agent_first_ui::Error::SessionRuntimeUnreachable)
    ));
}

/// Ending means the same thing here as everywhere else — the way in closes —
/// but there is no session waiting on an outcome, so it revokes rather than
/// waking a waiter that does not exist.
#[tokio::test]
async fn ending_an_externally_issued_credential_revokes_it() {
    let host = host().await;
    let access = host.external_access(
        provider_routes(),
        UiPagePolicy::new(UiPageScript::SameOrigin).into_security_policy(),
        None,
    );
    let credential = access
        .issue(UiExpiry::Idle(Duration::from_secs(60)))
        .expect("a credential is issued");

    let (status, _, _) = http_post(
        &format!("{}{SESSION_END_PATH}", credential.access_url_secret()),
        host.authority(),
    )
    .await;
    assert_eq!(status, 204);

    let (after, _, _) = http_get(
        &format!("{}panel", credential.access_url_secret()),
        host.authority(),
    )
    .await;
    assert_eq!(after, 404);
    assert_eq!(access.live_count(), 0);
}

/// A credential nobody uses stops working on its own, which is what makes this
/// crash cleanup rather than a lifetime somebody has to remember to end.
#[tokio::test]
async fn an_unused_credential_lapses_and_is_reaped() {
    let host = host().await;
    let access = host.external_access(
        provider_routes(),
        UiPagePolicy::new(UiPageScript::SameOrigin).into_security_policy(),
        None,
    );
    let credential = access
        .issue(UiExpiry::Idle(Duration::from_millis(80)))
        .expect("a credential is issued");
    tokio::time::sleep(Duration::from_millis(200)).await;

    let (status, _, _) = http_get(
        &format!("{}panel", credential.access_url_secret()),
        host.authority(),
    )
    .await;
    assert_eq!(status, 404);
    assert_eq!(access.live_count(), 0);
}

/// The holder of one of these credentials is somewhere else, so what keeps it
/// alive is a renewal arriving over the Provider's own API rather than a
/// request on the credential itself.
#[tokio::test]
async fn renewing_holds_a_credential_open_past_its_idle_window() {
    let host = host().await;
    let access = host.external_access(
        provider_routes(),
        UiPagePolicy::new(UiPageScript::SameOrigin).into_security_policy(),
        None,
    );
    let credential = access
        .issue(UiExpiry::Idle(Duration::from_millis(150)))
        .expect("a credential is issued");
    let secret = credential
        .access_url_secret()
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .expect("the credential is the last path segment")
        .to_owned();

    // Two idle windows' worth of waiting, renewed inside each one.
    for _ in 0..4 {
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(access.renew(&secret), "a live credential renews");
    }
    let (status, body, _) = http_get(
        &format!("{}panel", credential.access_url_secret()),
        host.authority(),
    )
    .await;
    assert_eq!(status, 200, "{body}");

    // Nothing else renews: not a credential this mount never issued, and not
    // one that has been given back.
    assert!(!access.renew(&"0".repeat(64)));
    assert!(access.revoke(&secret));
    assert!(!access.renew(&secret));
    assert_eq!(access.live_count(), 0);
}

/// A Provider must not be able to claim a name AFUI reserves, whether or not
/// AFUI answers that exact name today.
#[tokio::test]
async fn the_reserved_prefix_is_not_the_providers_to_route_under() {
    let host = host().await;
    let access = host.external_access(
        Router::new().route("/__afui/mine", get(|| async { "the provider's" })),
        UiPagePolicy::new(UiPageScript::SameOrigin).into_security_policy(),
        None,
    );
    let credential = access
        .issue(UiExpiry::Idle(Duration::from_secs(60)))
        .expect("a credential is issued");
    let (status, body, _) = http_get(
        &format!("{}__afui/mine", credential.access_url_secret()),
        host.authority(),
    )
    .await;
    assert_eq!(status, 404);
    assert!(!body.contains("the provider's"), "{body}");
}

/// Revoking stops more than new requests: a response the Provider is still
/// streaming under that credential ends too, or an open event stream would
/// keep handing the page's data to whoever holds a credential that no longer
/// works.
#[tokio::test]
async fn revoking_ends_a_response_that_is_still_streaming() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let host = host().await;
    let (frames, receiver) =
        tokio::sync::mpsc::channel::<Result<axum::body::Bytes, std::io::Error>>(4);
    let receiver = std::sync::Arc::new(std::sync::Mutex::new(Some(receiver)));
    let routes = Router::new().route(
        "/events",
        get(move || {
            let receiver = receiver.lock().unwrap().take().expect("streamed once");
            async move {
                axum::body::Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(receiver))
            }
        }),
    );
    let access = host.external_access(
        routes,
        UiPagePolicy::new(UiPageScript::SameOrigin).into_security_policy(),
        None,
    );
    let credential = access
        .issue(UiExpiry::Idle(Duration::from_secs(60)))
        .expect("a credential is issued");
    let url = format!("{}events", credential.access_url_secret());
    let (_, rest) = url.split_once("://").unwrap();
    let (authority, path) = rest.split_once('/').unwrap();
    let mut stream = tokio::net::TcpStream::connect(authority).await.unwrap();
    stream
        .write_all(
            format!(
                "GET /{path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
                host.authority()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    frames.send(Ok("first".into())).await.unwrap();
    let mut received = Vec::new();
    while !String::from_utf8_lossy(&received).contains("first") {
        let mut chunk = [0_u8; 1024];
        let read = stream.read(&mut chunk).await.unwrap();
        assert!(read > 0, "the stream ended before its first frame");
        received.extend_from_slice(&chunk[..read]);
    }

    assert!(access.revoke(credential.secret()));
    // The Provider still has more to say; the credential no longer does.
    let _ignored = frames.send(Ok("after".into())).await;
    let mut rest = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut rest))
        .await
        .expect("the response ends once its credential is revoked")
        .unwrap();
    assert!(
        !String::from_utf8_lossy(&rest).contains("after"),
        "{}",
        String::from_utf8_lossy(&rest)
    );
}

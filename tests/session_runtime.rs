//! The AFUI-owned UI protocol through a real hosted WebSocket.

#![cfg(feature = "session")]
#![allow(clippy::unwrap_used)]

use std::time::Duration;

use agent_first_ui::{UiBlobPolicy, UiExpiry, UiSession, UiSessionRuntime, UiUrlDelivery};
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;

const TEST_RUNTIME_PATH: &str = "__afui/runtime";
const TEST_RUNTIME_PROTOCOL: &str = "afui-session-v1";

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Action {
    Inspect { target: String },
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Reply {
    Started,
    Complete { rows: usize },
}

#[derive(Debug, Serialize)]
struct SurfaceState<'a> {
    phase: &'a str,
}

async fn connect(access_url: &str) -> tokio_tungstenite::WebSocketStream<TcpStream> {
    let endpoint = format!(
        "{}{}",
        access_url.replacen("http://", "ws://", 1),
        TEST_RUNTIME_PATH
    );
    let authority = access_url
        .strip_prefix("http://")
        .and_then(|rest| rest.split('/').next())
        .unwrap_or_default();
    let stream = TcpStream::connect(authority).await.unwrap();
    tokio_tungstenite::client_async(endpoint, stream)
        .await
        .unwrap()
        .0
}

#[tokio::test]
async fn a_socket_without_hello_closes_when_its_session_ends() {
    let host = agent_first_ui::UiHost::bind_local().await.unwrap();
    let (session, _runtime) = UiSession::<()>::new("inspector", "console")
        .unwrap()
        .with_runtime::<Action, Reply, SurfaceState<'static>>()
        .unwrap();
    let active = host
        .attach(session, Router::new(), UiExpiry::Never)
        .unwrap();
    let mut socket = connect(active.access_url()).await;
    drop(active);
    let closed = tokio::time::timeout(Duration::from_secs(1), socket.next())
        .await
        .expect("ending a session must release sockets that never send hello");
    assert!(matches!(
        closed,
        None | Some(Err(_)) | Some(Ok(Message::Close(_)))
    ));
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_socket_without_hello_has_a_deadline_while_the_session_stays_open() {
    let host = agent_first_ui::UiHost::bind_local().await.unwrap();
    let (session, _runtime) = UiSession::<()>::new("inspector", "console")
        .unwrap()
        .with_runtime::<Action, Reply, SurfaceState<'static>>()
        .unwrap();
    let active = host
        .attach(session, Router::new(), UiExpiry::Never)
        .unwrap();
    let mut socket = connect(active.access_url()).await;
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(11)).await;
    tokio::time::resume();
    let closed = tokio::time::timeout(Duration::from_secs(1), socket.next())
        .await
        .expect("a silent upgraded socket must expire while its session remains open");
    assert!(matches!(
        closed,
        None | Some(Err(_)) | Some(Ok(Message::Close(_)))
    ));
    assert_eq!(host.session_count(), 1);
    drop(active);
    host.shutdown().await.unwrap();
}

async fn send(socket: &mut tokio_tungstenite::WebSocketStream<TcpStream>, value: Value) {
    socket
        .send(Message::Text(value.to_string().into()))
        .await
        .unwrap();
}

async fn receive(socket: &mut tokio_tungstenite::WebSocketStream<TcpStream>) -> Value {
    let message = socket.next().await.unwrap().unwrap();
    serde_json::from_str(&message.into_text().unwrap()).unwrap()
}

async fn hello(socket: &mut tokio_tungstenite::WebSocketStream<TcpStream>, client_id: &str) {
    send(
        socket,
        json!({
            "kind": "hello",
            "hello": {
                "protocol": TEST_RUNTIME_PROTOCOL,
                "client_id": client_id,
                "last_event_sequence": 0,
            }
        }),
    )
    .await;
}

async fn http_get(url: &str) -> (u16, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let rest = url.strip_prefix("http://").unwrap();
    let (authority, path) = rest.split_once('/').unwrap();
    let mut stream = TcpStream::connect(authority).await.unwrap();
    stream
        .write_all(
            format!("GET /{path} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await
        .unwrap();
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).await.unwrap();
    let response = String::from_utf8_lossy(&bytes).into_owned();
    let status = response
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse().ok())
        .unwrap_or_default();
    (status, response)
}

#[tokio::test]
async fn state_and_calls_cross_only_inside_the_afui_envelope() {
    let session = UiSession::<()>::new("inspector", "live_state").unwrap();
    let (session, runtime): (
        UiSession<()>,
        UiSessionRuntime<Action, Reply, SurfaceState<'static>>,
    ) = session.with_runtime().unwrap();
    runtime
        .publish_state(&SurfaceState { phase: "old" })
        .unwrap();
    runtime
        .publish_state(&SurfaceState { phase: "current" })
        .unwrap();
    let active = session
        .serve(Router::new(), UiUrlDelivery::new(UiExpiry::Never))
        .await
        .unwrap();
    let mut surface = connect(active.access_url()).await;
    hello(&mut surface, "surface-1").await;

    let state = receive(&mut surface).await;
    assert_eq!(state["kind"], "state");
    assert_eq!(state["state"]["revision"], 2);
    assert_eq!(state["state"]["payload"]["phase"], "current");
    assert_eq!(receive(&mut surface).await["kind"], "ready");

    send(
        &mut surface,
        json!({
            "kind": "call",
            "call": {
                "request_id": "r1",
                "payload": {"type": "inspect", "target": "orders"},
            }
        }),
    )
    .await;
    let call = runtime.recv().await.unwrap().unwrap();
    assert_eq!(
        call.action(),
        &Action::Inspect {
            target: "orders".to_string()
        }
    );
    call.progress(&Reply::Started).await.unwrap();
    assert_eq!(
        receive(&mut surface).await["call_progress"]["request_id"],
        "r1"
    );
    call.finish(&Reply::Complete { rows: 3 }).await.unwrap();
    let result = receive(&mut surface).await;
    assert_eq!(result["call_result"]["request_id"], "r1");
    assert_eq!(result["call_result"]["payload"]["rows"], 3);
    assert!(result["call_result"]["payload"].get("request_id").is_none());
}

#[tokio::test]
async fn reconnect_replays_one_call_without_dispatching_it_twice() {
    let session = UiSession::<()>::new("inspector", "live_state").unwrap();
    let (session, runtime): (
        UiSession<()>,
        UiSessionRuntime<Action, Reply, SurfaceState<'static>>,
    ) = session.with_runtime().unwrap();
    runtime
        .publish_state(&SurfaceState { phase: "current" })
        .unwrap();
    let active = session
        .serve(Router::new(), UiUrlDelivery::new(UiExpiry::Never))
        .await
        .unwrap();

    let mut first = connect(active.access_url()).await;
    hello(&mut first, "surface-1").await;
    assert_eq!(receive(&mut first).await["kind"], "state");
    assert_eq!(receive(&mut first).await["kind"], "ready");
    send(
        &mut first,
        json!({
            "kind": "call",
            "call": {
                "request_id": "r1",
                "payload": {"type": "inspect", "target": "orders"},
            }
        }),
    )
    .await;
    let call = runtime.recv().await.unwrap().unwrap();
    call.progress(&Reply::Started).await.unwrap();
    assert_eq!(receive(&mut first).await["kind"], "call_progress");
    drop(first);

    let mut second = connect(active.access_url()).await;
    hello(&mut second, "surface-1").await;
    assert_eq!(receive(&mut second).await["kind"], "state");
    assert_eq!(receive(&mut second).await["kind"], "ready");
    send(
        &mut second,
        json!({
            "kind": "call",
            "call": {
                "request_id": "r1",
                "payload": {"type": "inspect", "target": "orders"},
            }
        }),
    )
    .await;
    assert_eq!(receive(&mut second).await["kind"], "call_progress");

    assert!(
        tokio::time::timeout(Duration::from_millis(50), runtime.recv())
            .await
            .is_err(),
        "the same AFUI call was dispatched to Provider code twice"
    );
}

/// Retained state is a value, but publishing one is an occurrence: nothing
/// here compares a state to the last one. That is what lets a Provider whose
/// page only needs to know *that* something changed publish one unit-like
/// state as an invalidation and let the page pull what it needs — a shape two
/// of the pages in this kit are built on, which until now was a property of
/// the implementation rather than a promise anyone had made.
#[tokio::test]
async fn republishing_an_equal_state_still_reaches_the_page() {
    let session = UiSession::<()>::new("inspector", "live_state").unwrap();
    let (session, runtime): (
        UiSession<()>,
        UiSessionRuntime<Action, Reply, SurfaceState<'static>>,
    ) = session.with_runtime().unwrap();
    runtime
        .publish_state(&SurfaceState { phase: "changed" })
        .unwrap();
    let active = session
        .serve(Router::new(), UiUrlDelivery::new(UiExpiry::Never))
        .await
        .unwrap();
    let mut surface = connect(active.access_url()).await;
    hello(&mut surface, "surface-1").await;
    assert_eq!(receive(&mut surface).await["state"]["revision"], 1);
    assert_eq!(receive(&mut surface).await["kind"], "ready");

    runtime
        .publish_state(&SurfaceState { phase: "changed" })
        .unwrap();
    let repeat = receive(&mut surface).await;
    assert_eq!(repeat["kind"], "state");
    assert_eq!(repeat["state"]["revision"], 2);
    assert_eq!(repeat["state"]["payload"]["phase"], "changed");
}

/// An opening state is optional. A page whose first screen was rendered by the
/// server has nothing for AFUI to retain yet, and it must still become live —
/// otherwise "wait until you have the current state" would mean "wait
/// forever" for exactly the pages that already had one.
#[tokio::test]
async fn a_runtime_nobody_published_to_still_makes_the_page_live() {
    let session = UiSession::<()>::new("inspector", "live_state").unwrap();
    let (session, _runtime): (
        UiSession<()>,
        UiSessionRuntime<Action, Reply, SurfaceState<'static>>,
    ) = session.with_runtime().unwrap();
    let active = session
        .serve(Router::new(), UiUrlDelivery::new(UiExpiry::Never))
        .await
        .unwrap();
    let mut surface = connect(active.access_url()).await;
    hello(&mut surface, "surface-1").await;

    assert_eq!(
        receive(&mut surface).await["kind"],
        "ready",
        "a page with no retained state to receive must not be held short of live",
    );
}

#[tokio::test]
async fn managed_blobs_are_bounded_served_and_revoked_by_afui() {
    let session = UiSession::<()>::new("inspector", "blob_view").unwrap();
    let (session, runtime): (
        UiSession<()>,
        UiSessionRuntime<Action, Reply, SurfaceState<'static>>,
    ) = session
        .with_runtime_and_blobs(UiBlobPolicy::new(8, ["image/png"]))
        .unwrap();
    let blob = runtime.store_blob("image/png", b"tiny".to_vec()).unwrap();
    assert_eq!(blob.media_type(), "image/png");
    assert_eq!(blob.size_bytes(), 4);
    assert!(matches!(
        runtime.store_blob("text/html", b"<p>".to_vec()),
        Err(agent_first_ui::Error::SessionRuntimeBlobRejected { .. })
    ));
    assert!(matches!(
        runtime.store_blob("image/png", vec![0; 9]),
        Err(agent_first_ui::Error::SessionRuntimeBlobRejected { .. })
    ));

    let active = session
        .serve(Router::new(), UiUrlDelivery::new(UiExpiry::Never))
        .await
        .unwrap();
    let url = format!("{}{}", active.access_url(), blob.blob_url());
    let (status, response) = http_get(&url).await;
    assert_eq!(status, 200, "{response}");
    assert!(
        response
            .to_ascii_lowercase()
            .contains("content-type: image/png")
    );
    assert!(
        response
            .to_ascii_lowercase()
            .contains("cache-control: no-store")
    );
    assert!(response.ends_with("tiny"));

    assert!(runtime.release_blob(&blob));
    assert_eq!(http_get(&url).await.0, 404);
}

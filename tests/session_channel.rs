//! Full-duplex session I/O through the same credential and host as the page.

#![cfg(feature = "session")]
#![allow(clippy::unwrap_used)]

use agent_first_ui::{SESSION_CHANNEL_PATH, UiExpiry, UiSession, UiSessionChannel, UiUrlDelivery};
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;

#[derive(Debug, Deserialize, PartialEq, Eq)]
struct SurfaceCommand {
    operation: String,
}

#[derive(Debug, Serialize)]
struct SurfaceState<'a> {
    phase: &'a str,
}

/// One attempt at the session's channel, refusal included.
async fn try_connect(
    access_url: &str,
) -> Result<tokio_tungstenite::WebSocketStream<TcpStream>, tokio_tungstenite::tungstenite::Error> {
    let endpoint = format!(
        "{}{}",
        access_url.replacen("http://", "ws://", 1),
        SESSION_CHANNEL_PATH
    );
    let authority = access_url
        .strip_prefix("http://")
        .and_then(|rest| rest.split('/').next())
        .unwrap_or_default();
    let stream = TcpStream::connect(authority).await.unwrap();
    tokio_tungstenite::client_async(endpoint, stream)
        .await
        .map(|(socket, _response)| socket)
}

async fn connect(access_url: &str) -> tokio_tungstenite::WebSocketStream<TcpStream> {
    try_connect(access_url).await.unwrap()
}

#[tokio::test]
async fn provider_json_crosses_both_directions_without_an_afui_envelope() {
    let session = UiSession::<()>::new("inspector", "live_state").unwrap();
    let (session, channel): (
        UiSession<()>,
        UiSessionChannel<SurfaceCommand, SurfaceState<'static>>,
    ) = session.with_channel().unwrap();
    let active = session
        .serve(Router::new(), UiUrlDelivery::new(UiExpiry::Never))
        .await
        .unwrap();
    let mut surface = connect(active.access_url()).await;

    surface
        .send(Message::Text(
            r#"{"operation":"inspect"}"#.to_string().into(),
        ))
        .await
        .unwrap();
    assert_eq!(
        channel.recv().await.unwrap(),
        Some(SurfaceCommand {
            operation: "inspect".to_string()
        })
    );

    channel
        .send(&SurfaceState { phase: "ready" })
        .await
        .unwrap();
    let message = surface.next().await.unwrap().unwrap();
    assert_eq!(message.into_text().unwrap(), r#"{"phase":"ready"}"#);
}

#[tokio::test]
async fn only_one_surface_consumes_a_session_channel_at_a_time() {
    let session = UiSession::<()>::new("inspector", "live_state").unwrap();
    let (session, _channel) = session
        .with_channel::<serde_json::Value, serde_json::Value>()
        .unwrap();
    let active = session
        .serve(Router::new(), UiUrlDelivery::new(UiExpiry::Never))
        .await
        .unwrap();
    let _first = connect(active.access_url()).await;

    let error = try_connect(active.access_url()).await.unwrap_err();
    let status = match error {
        tokio_tungstenite::tungstenite::Error::Http(response) => response.status(),
        other => panic!("expected an HTTP rejection, got {other}"),
    };
    assert_eq!(status, axum::http::StatusCode::CONFLICT);
}

/// One surface at a time is a rule about *now*, not about the session: a page
/// that reloads, a laptop that slept, a proxy that dropped the connection all
/// end one surface and start another on the same session. Holding the slot
/// past the connection that owns it would refuse every one of them, and the
/// session would look alive while its interface could never come back.
#[tokio::test]
async fn the_next_surface_connects_once_the_previous_one_is_gone() {
    let session = UiSession::<()>::new("inspector", "live_state").unwrap();
    let (session, _channel) = session
        .with_channel::<serde_json::Value, serde_json::Value>()
        .unwrap();
    let active = session
        .serve(Router::new(), UiUrlDelivery::new(UiExpiry::Never))
        .await
        .unwrap();

    let first = connect(active.access_url()).await;
    drop(first);

    // The slot is released when the server notices the socket is gone, which
    // is a round trip away rather than immediate. Retrying is what a surface
    // does anyway; the assertion is that it stops being refused, not when.
    let mut refusals = 0;
    loop {
        match try_connect(active.access_url()).await {
            Ok(_second) => break,
            Err(tokio_tungstenite::tungstenite::Error::Http(response))
                if response.status() == axum::http::StatusCode::CONFLICT =>
            {
                refusals += 1;
                assert!(
                    refusals < 200,
                    "the surface slot was never released after the first connection closed"
                );
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            Err(other) => panic!("unexpected channel failure: {other}"),
        }
    }
}

//! Optional full-duplex I/O for the lifetime of one UI session.
//!
//! A channel transports Provider-defined JSON values between the surface and
//! the process that owns the session. It does not add another completion path:
//! [`crate::Outcome`] still describes the session's one terminal outcome, while
//! any number of non-terminal updates may cross this channel before then.

use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::sync::{Mutex, mpsc, watch};

use crate::{Error, Result};

/// Default number of messages held in either direction before senders apply
/// backpressure.
pub const DEFAULT_UI_SESSION_CHANNEL_CAPACITY: usize = 32;

/// Maximum encoded size of one channel message.
///
/// The queue is in memory and the route is reachable for the whole session, so
/// the transport owns one fixed bound instead of asking every Provider to
/// remember to add one. Providers move larger artifacts through their own
/// authenticated routes and use the channel to name or invalidate them.
pub const MAX_UI_SESSION_CHANNEL_MESSAGE_BYTES: usize = 1024 * 1024;

/// The controller side of a session's optional full-duplex channel.
///
/// `FromSurface` and `ToSurface` are Provider-owned JSON types. AFUI does not
/// wrap them in an event, assign request ids, retain history, replay messages,
/// or infer request/response pairs. The WebSocket connection already selects
/// the session; each frame is exactly one encoded Provider value.
pub struct UiSessionChannel<FromSurface, ToSurface> {
    from_surface: Mutex<mpsc::Receiver<Vec<u8>>>,
    to_surface: mpsc::Sender<Vec<u8>>,
    _types: PhantomData<fn(FromSurface) -> ToSurface>,
}

pub(crate) struct UiSessionChannelMount {
    pub(crate) endpoint: UiSessionChannelEndpoint,
    pub(crate) closer: UiSessionChannelCloser,
}

#[derive(Clone)]
pub(crate) struct UiSessionChannelEndpoint {
    from_surface: mpsc::Sender<Vec<u8>>,
    to_surface: Arc<Mutex<mpsc::Receiver<Vec<u8>>>>,
    connected: Arc<AtomicBool>,
    shutdown: watch::Receiver<bool>,
}

#[derive(Clone)]
pub(crate) struct UiSessionChannelCloser {
    shutdown: watch::Sender<bool>,
}

struct ConnectionGuard {
    connected: Arc<AtomicBool>,
}

impl<FromSurface, ToSurface> UiSessionChannel<FromSurface, ToSurface> {
    pub(crate) fn bounded(capacity: usize) -> Result<(Self, UiSessionChannelMount)> {
        if capacity == 0 {
            return Err(Error::InvalidSessionChannelCapacity);
        }
        let (from_surface_tx, from_surface_rx) = mpsc::channel(capacity);
        let (to_surface_tx, to_surface_rx) = mpsc::channel(capacity);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        Ok((
            Self {
                from_surface: Mutex::new(from_surface_rx),
                to_surface: to_surface_tx,
                _types: PhantomData,
            },
            UiSessionChannelMount {
                endpoint: UiSessionChannelEndpoint {
                    from_surface: from_surface_tx,
                    to_surface: Arc::new(Mutex::new(to_surface_rx)),
                    connected: Arc::new(AtomicBool::new(false)),
                    shutdown: shutdown_rx,
                },
                closer: UiSessionChannelCloser {
                    shutdown: shutdown_tx,
                },
            },
        ))
    }

    /// Send one Provider value to the surface, waiting for bounded queue space.
    ///
    /// Values may be queued before the surface connects. Once a frame has been
    /// attempted it is not replayed after a disconnect; persistence and replay
    /// belong to the Provider when its data requires them.
    ///
    /// The wait is real backpressure, not a formality: with nobody connected,
    /// the queue fills and this call waits until a surface drains it or the
    /// session ends. Nothing is dropped and there is no non-blocking send, so a
    /// Provider pushing periodic state on a timer stalls on its own timer
    /// rather than shedding stale values. Keep the latest value in
    /// Provider-owned state and send from there if the newest snapshot matters
    /// more than the backlog.
    pub async fn send(&self, value: &ToSurface) -> Result<()>
    where
        ToSurface: Serialize,
    {
        let encoded = serde_json::to_vec(value).map_err(Error::SessionChannelEncode)?;
        ensure_message_size(encoded.len())?;
        self.to_surface
            .send(encoded)
            .await
            .map_err(|_| Error::SessionChannelClosed)
    }

    /// Receive and decode the next value sent by the surface.
    ///
    /// `None` means the session endpoint has gone away. A malformed Provider
    /// payload is reported without assigning it any higher-level meaning; the
    /// next call may continue with the next frame.
    ///
    /// Call this for as long as the session is open, even by a Provider that
    /// only pushes. One connection carries both directions, so a surface-bound
    /// queue left full stops that connection reading *and* writing: outgoing
    /// values then wait behind incoming ones nobody is taking.
    pub async fn recv(&self) -> Result<Option<FromSurface>>
    where
        FromSurface: DeserializeOwned,
    {
        let Some(encoded) = self.from_surface.lock().await.recv().await else {
            return Ok(None);
        };
        serde_json::from_slice(&encoded)
            .map(Some)
            .map_err(Error::SessionChannelDecode)
    }
}

impl UiSessionChannelEndpoint {
    pub(crate) fn router(self) -> Router {
        Router::new()
            .route("/", get(upgrade_channel))
            .with_state(self)
    }

    /// Claim the one surface slot, or `None` while a connection holds it.
    ///
    /// The guard is created here rather than inside [`Self::serve`] because a
    /// `101` is not proof that the upgrade completed: when the upgrade future
    /// fails — a browser navigating away mid-handshake is enough — the
    /// callback that would have run `serve` is dropped without ever being
    /// called. Anything that released the slot only from inside `serve` would
    /// leave this session answering `409` for the rest of its life, with no
    /// way for the surface to reconnect and nothing to say why.
    fn reserve(&self) -> Option<ConnectionGuard> {
        self.connected
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| ConnectionGuard {
                connected: Arc::clone(&self.connected),
            })
    }

    async fn serve(self, socket: WebSocket, guard: ConnectionGuard) {
        let _guard = guard;
        let mut socket = socket;
        let mut to_surface = self.to_surface.lock().await;
        let mut shutdown = self.shutdown.clone();

        loop {
            tokio::select! {
                incoming = socket.recv() => {
                    let Some(Ok(message)) = incoming else {
                        return;
                    };
                    match message {
                        Message::Text(text) => {
                            if self.forward_from_surface(text.as_bytes().to_vec()).await.is_err() {
                                return;
                            }
                        }
                        Message::Binary(bytes) => {
                            if self.forward_from_surface(bytes.to_vec()).await.is_err() {
                                return;
                            }
                        }
                        Message::Ping(bytes) => {
                            if socket.send(Message::Pong(bytes)).await.is_err() {
                                return;
                            }
                        }
                        Message::Pong(_) => {}
                        Message::Close(_) => return,
                    }
                }
                outgoing = to_surface.recv() => {
                    let Some(encoded) = outgoing else {
                        let _ignored = socket.send(Message::Close(None)).await;
                        return;
                    };
                    let Ok(text) = String::from_utf8(encoded) else {
                        return;
                    };
                    if socket.send(Message::Text(text.into())).await.is_err() {
                        return;
                    }
                }
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        let _ignored = socket.send(Message::Close(None)).await;
                        return;
                    }
                }
            }
        }
    }

    async fn forward_from_surface(&self, encoded: Vec<u8>) -> Result<()> {
        ensure_message_size(encoded.len())?;
        self.from_surface
            .send(encoded)
            .await
            .map_err(|_| Error::SessionChannelClosed)
    }
}

impl UiSessionChannelCloser {
    pub(crate) fn close(&self) {
        let _ignored = self.shutdown.send(true);
    }
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.connected.store(false, Ordering::Release);
    }
}

async fn upgrade_channel(
    State(endpoint): State<UiSessionChannelEndpoint>,
    websocket: WebSocketUpgrade,
) -> Response {
    let Some(guard) = endpoint.reserve() else {
        return axum::http::StatusCode::CONFLICT.into_response();
    };
    websocket
        .max_message_size(MAX_UI_SESSION_CHANNEL_MESSAGE_BYTES)
        .max_frame_size(MAX_UI_SESSION_CHANNEL_MESSAGE_BYTES)
        // The guard travels with the callback, so an upgrade that never
        // happens frees the slot when the callback itself is dropped.
        .on_upgrade(move |socket| endpoint.serve(socket, guard))
}

fn ensure_message_size(size_bytes: usize) -> Result<()> {
    if size_bytes > MAX_UI_SESSION_CHANNEL_MESSAGE_BYTES {
        return Err(Error::SessionChannelMessageTooLarge {
            size_bytes,
            max_bytes: MAX_UI_SESSION_CHANNEL_MESSAGE_BYTES,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};

    use super::*;

    #[derive(Debug, Deserialize, PartialEq, Eq)]
    struct SurfaceInput {
        command: String,
    }

    #[derive(Serialize)]
    struct SurfaceUpdate<'a> {
        state: &'a str,
    }

    #[tokio::test]
    async fn typed_values_cross_without_a_transport_envelope() {
        let (channel, mount) =
            UiSessionChannel::<SurfaceInput, SurfaceUpdate<'_>>::bounded(2).unwrap();
        mount
            .endpoint
            .forward_from_surface(br#"{"command":"inspect"}"#.to_vec())
            .await
            .unwrap();
        assert_eq!(
            channel.recv().await.unwrap(),
            Some(SurfaceInput {
                command: "inspect".to_string()
            })
        );

        channel
            .send(&SurfaceUpdate { state: "ready" })
            .await
            .unwrap();
        let encoded = mount.endpoint.to_surface.lock().await.recv().await.unwrap();
        assert_eq!(encoded, br#"{"state":"ready"}"#);
    }

    /// The slot belongs to a guard, not to the code that answers the upgrade.
    /// A reservation nobody goes on to use has to release it, because that is
    /// exactly what a failed upgrade produces.
    #[test]
    fn a_reservation_nobody_uses_frees_the_surface_slot() {
        let (_channel, mount) = UiSessionChannel::<(), ()>::bounded(1).unwrap();
        let reserved = mount.endpoint.reserve().unwrap();
        assert!(mount.endpoint.reserve().is_none());
        drop(reserved);
        assert!(mount.endpoint.reserve().is_some());
    }

    #[test]
    fn zero_capacity_and_oversized_messages_are_refused() {
        assert!(matches!(
            UiSessionChannel::<(), ()>::bounded(0),
            Err(Error::InvalidSessionChannelCapacity)
        ));
        assert!(matches!(
            ensure_message_size(MAX_UI_SESSION_CHANNEL_MESSAGE_BYTES + 1),
            Err(Error::SessionChannelMessageTooLarge { .. })
        ));
    }
}

//! The AFUI-owned protocol for one live UI session.
//!
//! Providers declare typed state, actions, and replies. AFUI owns everything
//! that makes those values a reliable browser conversation: the versioned
//! envelope, request identity, latest-state coalescing, event sequence and
//! replay, reconnect, cancellation, bounds, and session shutdown.

use std::collections::{HashMap, VecDeque};
use std::marker::PhantomData;
use std::sync::{Arc, Mutex as StdMutex};

use axum::Router;
use axum::extract::State as AxumState;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use getrandom::fill;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::sync::{Mutex, broadcast, mpsc, watch};

use crate::reserved::SESSION_BLOB_PATH_PREFIX;
use crate::serving::UiBlobPolicy;
use crate::{Error, Result};

/// The protocol spoken by [`UiSessionRuntime`] and `afui.connect()`.
pub(crate) const UI_RUNTIME_PROTOCOL: &str = "afui-session-v1";

/// Default number of unacknowledged calls one runtime admits.
pub const DEFAULT_UI_RUNTIME_CALL_CAPACITY: usize = 32;

/// Default number of ordered events AFUI retains for reconnecting surfaces.
pub const DEFAULT_UI_RUNTIME_EVENT_CAPACITY: usize = 128;

/// Maximum encoded size of one runtime frame.
pub const MAX_UI_RUNTIME_MESSAGE_BYTES: usize = 1024 * 1024;

const RESPONSE_BROADCAST_CAPACITY: usize = 256;
const MAX_RETAINED_CALL_FRAMES: usize = 64;
const MAX_ID_BYTES: usize = 128;
const DEFAULT_RUNTIME_BLOB_CAPACITY: usize = 16;
const RUNTIME_BLOB_TOTAL_MULTIPLIER: u64 = 4;
const BLOB_ID_BYTES: usize = 16;

/// One AFUI-managed byte body a Provider can hand to its page.
///
/// The URL is session-relative, uncacheable, and valid only while this runtime
/// retains the body. Providers supply bytes and an admitted media type; AFUI
/// owns naming, bounds, serving headers, release, and session cleanup.
#[derive(Debug, Clone, Serialize)]
pub struct UiBlobRef {
    blob_url: String,
    media_type: String,
    size_bytes: usize,
}

impl UiBlobRef {
    #[must_use]
    pub fn blob_url(&self) -> &str {
        &self.blob_url
    }

    #[must_use]
    pub fn media_type(&self) -> &str {
        &self.media_type
    }

    #[must_use]
    pub fn size_bytes(&self) -> usize {
        self.size_bytes
    }
}

/// One caller-safe failure completing a surface call.
#[derive(Debug, Clone)]
pub struct UiCallError {
    code: String,
    message: String,
    hint: Option<String>,
    retryable: bool,
}

impl UiCallError {
    #[must_use]
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            hint: None,
            retryable: false,
        }
    }

    #[must_use]
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    #[must_use]
    pub fn retryable(mut self, retryable: bool) -> Self {
        self.retryable = retryable;
        self
    }

    #[must_use]
    pub fn code(&self) -> &str {
        &self.code
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    #[must_use]
    pub fn hint(&self) -> Option<&str> {
        self.hint.as_deref()
    }

    #[must_use]
    pub fn is_retryable(&self) -> bool {
        self.retryable
    }

    fn payload(&self, request_id: &str) -> Value {
        let mut value = json!({
            "request_id": request_id,
            "code": self.code,
            "message": self.message,
            "retryable": self.retryable,
        });
        if let Some(hint) = &self.hint {
            value["hint"] = Value::String(hint.clone());
        }
        value
    }
}

/// One typed action sent by a surface.
///
/// Transport identity deliberately is not exposed. A Provider answers through
/// this handle, so it cannot copy AFUI's `request_id` into a domain reply.
pub struct UiCall<Action, Reply> {
    action: Action,
    key: CallKey,
    shared: Arc<RuntimeShared>,
    cancelled: watch::Receiver<bool>,
    shutdown: watch::Receiver<bool>,
    finished: bool,
    _reply: PhantomData<fn() -> Reply>,
}

impl<Action, Reply> UiCall<Action, Reply> {
    #[must_use]
    pub fn action(&self) -> &Action {
        &self.action
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        *self.cancelled.borrow() || *self.shutdown.borrow()
    }

    pub async fn cancelled(&mut self) {
        if self.is_cancelled() {
            return;
        }
        tokio::select! {
            _changed = self.cancelled.changed() => {}
            _changed = self.shutdown.changed() => {}
        }
    }

    /// Retain bytes under this call's session and return their AFUI reference.
    pub fn store_blob(&self, media_type: impl Into<String>, bytes: Vec<u8>) -> Result<UiBlobRef> {
        self.shared.store_blob(media_type.into(), bytes)
    }
}

impl<Action, Reply> UiCall<Action, Reply>
where
    Reply: Serialize,
{
    /// Send a non-terminal update for this call.
    pub async fn progress(&self, reply: &Reply) -> Result<()> {
        let payload = serde_json::to_value(reply).map_err(Error::SessionRuntimeEncode)?;
        self.shared
            .send_call_frame(&self.key, "call_progress", payload, false)
            .await
    }

    /// Complete this call successfully.
    pub async fn finish(mut self, reply: &Reply) -> Result<()> {
        let payload = serde_json::to_value(reply).map_err(Error::SessionRuntimeEncode)?;
        self.shared
            .send_call_frame(&self.key, "call_result", payload, true)
            .await?;
        self.finished = true;
        Ok(())
    }

    /// Complete this call with a caller-safe error.
    pub async fn fail(mut self, failure: UiCallError) -> Result<()> {
        self.shared.fail_call(&self.key, failure).await?;
        self.finished = true;
        Ok(())
    }
}

impl<Action, Reply> Drop for UiCall<Action, Reply> {
    fn drop(&mut self) {
        if self.finished || self.shared.is_closed() {
            return;
        }
        let key = self.key.clone();
        let shared = Arc::clone(&self.shared);
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let _task = runtime.spawn(async move {
                let _ignored = shared
                    .fail_call(
                        &key,
                        UiCallError::new(
                            "ui_call_abandoned",
                            "the UI action ended without a result",
                        ),
                    )
                    .await;
            });
        }
    }
}

/// The Provider side of a session's UI runtime.
pub struct UiSessionRuntime<Action, Reply, SurfaceState> {
    calls: Mutex<mpsc::Receiver<IncomingCall>>,
    shared: Arc<RuntimeShared>,
    shutdown: watch::Receiver<bool>,
    closer: UiSessionRuntimeCloser,
    _types: PhantomData<fn(Action) -> (Reply, SurfaceState)>,
}

pub(crate) struct UiSessionRuntimeMount {
    pub(crate) endpoint: UiSessionRuntimeEndpoint,
    pub(crate) closer: UiSessionRuntimeCloser,
}

#[derive(Clone)]
pub(crate) struct UiSessionRuntimeEndpoint {
    shared: Arc<RuntimeShared>,
    state: watch::Receiver<Option<Arc<str>>>,
    shutdown: watch::Receiver<bool>,
}

#[derive(Clone)]
pub(crate) struct UiSessionRuntimeCloser {
    shutdown: watch::Sender<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CallKey {
    client_id: String,
    request_id: String,
}

struct IncomingCall {
    key: CallKey,
    payload: Value,
    cancelled: watch::Receiver<bool>,
}

struct CallRecord {
    frames: VecDeque<Arc<str>>,
    terminal: bool,
    cancel: watch::Sender<bool>,
}

#[derive(Clone)]
struct TargetFrame {
    client_id: String,
    encoded: Arc<str>,
}

#[derive(Clone)]
struct EventFrame {
    sequence: u64,
    encoded: Arc<str>,
}

struct EventHistory {
    frames: VecDeque<EventFrame>,
    last_sequence: u64,
}

struct EventReplay {
    frames: Vec<EventFrame>,
    reset: Option<(u64, u64)>,
}

struct StoredBlob {
    media_type: String,
    bytes: Vec<u8>,
}

#[derive(Default)]
struct StoredBlobs {
    values: HashMap<String, StoredBlob>,
    order: VecDeque<String>,
    total_bytes: u64,
}

struct RuntimeBlobStore {
    policy: UiBlobPolicy,
    blobs: StdMutex<StoredBlobs>,
    max_blobs: usize,
    max_total_bytes: u64,
}

struct RuntimeShared {
    calls: mpsc::Sender<IncomingCall>,
    state: watch::Sender<Option<Arc<str>>>,
    state_revision: StdMutex<u64>,
    responses: broadcast::Sender<TargetFrame>,
    call_records: Mutex<HashMap<CallKey, CallRecord>>,
    call_capacity: usize,
    events: broadcast::Sender<EventFrame>,
    event_history: StdMutex<EventHistory>,
    event_capacity: usize,
    blobs: Option<RuntimeBlobStore>,
    shutdown: watch::Receiver<bool>,
}

impl<Action, Reply, SurfaceState> UiSessionRuntime<Action, Reply, SurfaceState> {
    pub(crate) fn bounded(
        call_capacity: usize,
        event_capacity: usize,
    ) -> Result<(Self, UiSessionRuntimeMount)> {
        Self::bounded_with_blobs(call_capacity, event_capacity, None)
    }

    pub(crate) fn bounded_with_blob_policy(
        call_capacity: usize,
        event_capacity: usize,
        blob_policy: UiBlobPolicy,
    ) -> Result<(Self, UiSessionRuntimeMount)> {
        Self::bounded_with_blobs(call_capacity, event_capacity, Some(blob_policy))
    }

    fn bounded_with_blobs(
        call_capacity: usize,
        event_capacity: usize,
        blob_policy: Option<UiBlobPolicy>,
    ) -> Result<(Self, UiSessionRuntimeMount)> {
        if call_capacity == 0 || event_capacity == 0 {
            return Err(Error::InvalidSessionRuntimeCapacity);
        }
        let (call_tx, call_rx) = mpsc::channel(call_capacity);
        let (state_tx, state_rx) = watch::channel(None);
        let (response_tx, _response_rx) = broadcast::channel(RESPONSE_BROADCAST_CAPACITY);
        let (event_tx, _event_rx) = broadcast::channel(event_capacity);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let closer = UiSessionRuntimeCloser {
            shutdown: shutdown_tx,
        };
        let shared = Arc::new(RuntimeShared {
            calls: call_tx,
            state: state_tx,
            state_revision: StdMutex::new(0),
            responses: response_tx,
            call_records: Mutex::new(HashMap::new()),
            call_capacity,
            events: event_tx,
            event_history: StdMutex::new(EventHistory {
                frames: VecDeque::with_capacity(event_capacity),
                last_sequence: 0,
            }),
            event_capacity,
            blobs: blob_policy.map(|policy| RuntimeBlobStore {
                max_blobs: DEFAULT_RUNTIME_BLOB_CAPACITY,
                max_total_bytes: policy
                    .max_bytes()
                    .saturating_mul(RUNTIME_BLOB_TOTAL_MULTIPLIER),
                policy,
                blobs: StdMutex::new(StoredBlobs::default()),
            }),
            shutdown: shutdown_rx.clone(),
        });
        Ok((
            Self {
                calls: Mutex::new(call_rx),
                shared: Arc::clone(&shared),
                shutdown: shutdown_rx.clone(),
                closer: closer.clone(),
                _types: PhantomData,
            },
            UiSessionRuntimeMount {
                endpoint: UiSessionRuntimeEndpoint {
                    shared,
                    state: state_rx,
                    shutdown: shutdown_rx,
                },
                closer,
            },
        ))
    }

    /// Retain a bounded byte body under this session.
    pub fn store_blob(&self, media_type: impl Into<String>, bytes: Vec<u8>) -> Result<UiBlobRef> {
        self.shared.store_blob(media_type.into(), bytes)
    }

    /// Release one body before bounded eviction or session shutdown does.
    pub fn release_blob(&self, reference: &UiBlobRef) -> bool {
        self.shared.release_blob_url(reference.blob_url())
    }

    /// Replace the session's current state.
    ///
    /// This never waits for a surface. AFUI retains exactly the newest state
    /// and sends it on every initial connection and reconnect.
    pub fn publish_state(&self, state: &SurfaceState) -> Result<u64>
    where
        SurfaceState: Serialize,
    {
        self.shared.ensure_open()?;
        let payload = serde_json::to_value(state).map_err(Error::SessionRuntimeEncode)?;
        let mut current_revision = self
            .shared
            .state_revision
            .lock()
            .map_err(|_| Error::SessionRuntimeBusy)?;
        self.shared.ensure_open()?;
        let revision = current_revision.saturating_add(1);
        let encoded = encode(
            "state",
            json!({
                "revision": revision,
                "payload": payload,
            }),
        )?;
        self.shared.state.send_replace(Some(encoded));
        *current_revision = revision;
        Ok(revision)
    }

    /// Publish one ordered event, retained within the runtime's bounded replay
    /// window. Most live UI data should use [`Self::publish_state`] instead.
    pub fn publish_event<Event>(&self, event: &Event) -> Result<u64>
    where
        Event: Serialize,
    {
        self.shared.ensure_open()?;
        let payload = serde_json::to_value(event).map_err(Error::SessionRuntimeEncode)?;
        let mut history = self
            .shared
            .event_history
            .lock()
            .map_err(|_| Error::SessionRuntimeBusy)?;
        self.shared.ensure_open()?;
        let sequence = history.last_sequence.saturating_add(1);
        let encoded = encode(
            "event",
            json!({
                "sequence": sequence,
                "payload": payload,
            }),
        )?;
        let frame = EventFrame { sequence, encoded };
        if history.frames.len() == self.shared.event_capacity {
            history.frames.pop_front();
        }
        history.frames.push_back(frame.clone());
        history.last_sequence = sequence;
        let _no_surface_is_normal = self.shared.events.send(frame);
        drop(history);
        Ok(sequence)
    }

    /// Receive the next typed surface call.
    ///
    /// Invalid payloads are rejected by AFUI with a stable call error and are
    /// never handed to Provider code.
    pub async fn recv(&self) -> Result<Option<UiCall<Action, Reply>>>
    where
        Action: DeserializeOwned,
    {
        let mut calls = self.calls.lock().await;
        let mut shutdown = self.shutdown.clone();
        loop {
            if *shutdown.borrow() {
                return Ok(None);
            }
            tokio::select! {
                incoming = calls.recv() => {
                    let Some(incoming) = incoming else {
                        return Ok(None);
                    };
                    if *shutdown.borrow() {
                        return Ok(None);
                    }
                    if *incoming.cancelled.borrow() {
                        continue;
                    }
                    match serde_json::from_value(incoming.payload) {
                        Ok(action) => {
                            return Ok(Some(UiCall {
                                action,
                                key: incoming.key,
                                shared: Arc::clone(&self.shared),
                                cancelled: incoming.cancelled,
                                shutdown: self.shutdown.clone(),
                                finished: false,
                                _reply: PhantomData,
                            }));
                        }
                        Err(_error) => {
                            let failed = self.shared.fail_call(
                                &incoming.key,
                                UiCallError::new(
                                    "ui_action_invalid",
                                    "the UI action did not match the declared action type",
                                ),
                            ).await;
                            if let Err(error) = failed
                                && !matches!(error, Error::SessionRuntimeCallClosed)
                            {
                                return Err(error);
                            }
                        }
                    }
                }
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        return Ok(None);
                    }
                }
            }
        }
    }
}

impl<Action, Reply, SurfaceState> Drop for UiSessionRuntime<Action, Reply, SurfaceState> {
    fn drop(&mut self) {
        self.closer.close();
    }
}

impl RuntimeShared {
    fn is_closed(&self) -> bool {
        *self.shutdown.borrow()
    }

    fn ensure_open(&self) -> Result<()> {
        if self.is_closed() {
            Err(Error::SessionRuntimeClosed)
        } else {
            Ok(())
        }
    }

    fn store_blob(&self, media_type: String, bytes: Vec<u8>) -> Result<UiBlobRef> {
        self.ensure_open()?;
        let Some(blobs) = &self.blobs else {
            return Err(Error::SessionRuntimeBlobsDisabled);
        };
        blobs.store(media_type, bytes)
    }

    fn release_blob_url(&self, blob_url: &str) -> bool {
        self.blobs
            .as_ref()
            .is_some_and(|blobs| blobs.release_url(blob_url))
    }

    async fn send_call_frame(
        &self,
        key: &CallKey,
        kind: &'static str,
        payload: Value,
        terminal: bool,
    ) -> Result<()> {
        self.ensure_open()?;
        let encoded = encode(
            kind,
            json!({
                "request_id": key.request_id,
                "payload": payload,
            }),
        )?;
        let mut records = self.call_records.lock().await;
        let Some(record) = records.get_mut(key) else {
            return Err(Error::SessionRuntimeCallClosed);
        };
        if record.terminal {
            return Err(Error::SessionRuntimeCallClosed);
        }
        if record.frames.len() == MAX_RETAINED_CALL_FRAMES {
            record.frames.pop_front();
        }
        record.frames.push_back(Arc::clone(&encoded));
        record.terminal = terminal;
        let _no_surface_is_normal = self.responses.send(TargetFrame {
            client_id: key.client_id.clone(),
            encoded,
        });
        drop(records);
        Ok(())
    }

    async fn fail_call(&self, key: &CallKey, failure: UiCallError) -> Result<()> {
        let payload = failure.payload(&key.request_id);
        self.ensure_open()?;
        let encoded = encode("call_error", payload)?;
        let mut records = self.call_records.lock().await;
        let Some(record) = records.get_mut(key) else {
            return Err(Error::SessionRuntimeCallClosed);
        };
        if record.terminal {
            return Err(Error::SessionRuntimeCallClosed);
        }
        if record.frames.len() == MAX_RETAINED_CALL_FRAMES {
            record.frames.pop_front();
        }
        record.frames.push_back(Arc::clone(&encoded));
        record.terminal = true;
        let _no_surface_is_normal = self.responses.send(TargetFrame {
            client_id: key.client_id.clone(),
            encoded,
        });
        drop(records);
        Ok(())
    }

    async fn register_call(&self, key: CallKey, payload: Value) -> Result<Vec<Arc<str>>> {
        self.ensure_open()?;
        let mut records = self.call_records.lock().await;
        if let Some(record) = records.get(&key) {
            return Ok(record.frames.iter().cloned().collect());
        }
        if records.len() >= self.call_capacity {
            return Ok(vec![call_error_frame(
                &key.request_id,
                &UiCallError::new(
                    "ui_runtime_busy",
                    "the UI runtime has reached its unacknowledged call bound",
                )
                .retryable(true),
            )?]);
        }
        let (cancel_tx, cancel_rx) = watch::channel(false);
        records.insert(
            key.clone(),
            CallRecord {
                frames: VecDeque::new(),
                terminal: false,
                cancel: cancel_tx,
            },
        );
        drop(records);
        let rejected = match self.calls.try_send(IncomingCall {
            key: key.clone(),
            payload,
            cancelled: cancel_rx,
        }) {
            Ok(()) => return Ok(Vec::new()),
            Err(mpsc::error::TrySendError::Full(_incoming)) => UiCallError::new(
                "ui_runtime_busy",
                "the UI runtime has reached its call inbox bound",
            )
            .retryable(true),
            Err(mpsc::error::TrySendError::Closed(_incoming)) => {
                UiCallError::new("ui_runtime_closed", "the UI session runtime is closed")
            }
        };
        self.call_records.lock().await.remove(&key);
        Ok(vec![call_error_frame(&key.request_id, &rejected)?])
    }

    async fn cancel_call(&self, key: &CallKey) -> Result<()> {
        self.ensure_open()?;
        let encoded = call_error_frame(
            &key.request_id,
            &UiCallError::new("ui_call_cancelled", "the UI action was cancelled"),
        )?;
        let mut records = self.call_records.lock().await;
        let Some(record) = records.get_mut(key) else {
            return Ok(());
        };
        let _ignored = record.cancel.send(true);
        if record.terminal {
            return Ok(());
        }
        if record.frames.len() == MAX_RETAINED_CALL_FRAMES {
            record.frames.pop_front();
        }
        record.frames.push_back(Arc::clone(&encoded));
        record.terminal = true;
        let _no_surface_is_normal = self.responses.send(TargetFrame {
            client_id: key.client_id.clone(),
            encoded,
        });
        Ok(())
    }

    async fn acknowledge_call(&self, key: &CallKey) {
        let mut records = self.call_records.lock().await;
        if records.get(key).is_some_and(|record| record.terminal) {
            records.remove(key);
        }
    }
}

impl RuntimeBlobStore {
    fn store(&self, media_type: String, bytes: Vec<u8>) -> Result<UiBlobRef> {
        let size_bytes = bytes.len();
        if !self.policy.admits(&media_type)
            || size_bytes as u64 > self.policy.max_bytes()
            || size_bytes as u64 > self.max_total_bytes
        {
            return Err(Error::SessionRuntimeBlobRejected {
                media_type,
                size_bytes,
                max_bytes: self.policy.max_bytes(),
            });
        }
        let id = random_blob_id()?;
        let mut blobs = self.blobs.lock().map_err(|_| Error::SessionRuntimeBusy)?;
        while blobs.values.len() >= self.max_blobs
            || blobs.total_bytes.saturating_add(size_bytes as u64) > self.max_total_bytes
        {
            let Some(oldest) = blobs.order.pop_front() else {
                break;
            };
            if let Some(removed) = blobs.values.remove(&oldest) {
                blobs.total_bytes = blobs.total_bytes.saturating_sub(removed.bytes.len() as u64);
            }
        }
        // A collision is vanishingly unlikely, but replacement would corrupt
        // a reference already handed to the page. Treat it as entropy failure.
        if blobs.values.contains_key(&id) {
            return Err(Error::Random(std::io::Error::other(
                "generated a duplicate UI blob identifier",
            )));
        }
        blobs.total_bytes = blobs.total_bytes.saturating_add(size_bytes as u64);
        blobs.order.push_back(id.clone());
        blobs.values.insert(
            id.clone(),
            StoredBlob {
                media_type: media_type.clone(),
                bytes,
            },
        );
        Ok(UiBlobRef {
            blob_url: format!("{SESSION_BLOB_PATH_PREFIX}{id}"),
            media_type,
            size_bytes,
        })
    }

    fn response(&self, id: &str) -> Response {
        let Some((media_type, bytes)) = self.blobs.lock().ok().and_then(|blobs| {
            blobs
                .values
                .get(id)
                .map(|blob| (blob.media_type.clone(), blob.bytes.clone()))
        }) else {
            return StatusCode::NOT_FOUND.into_response();
        };
        self.policy.respond(&media_type, bytes)
    }

    fn release_url(&self, blob_url: &str) -> bool {
        let Some(id) = blob_url.strip_prefix(SESSION_BLOB_PATH_PREFIX) else {
            return false;
        };
        if id.is_empty() || id.contains('/') {
            return false;
        }
        let Ok(mut blobs) = self.blobs.lock() else {
            return false;
        };
        let Some(removed) = blobs.values.remove(id) else {
            return false;
        };
        blobs.total_bytes = blobs.total_bytes.saturating_sub(removed.bytes.len() as u64);
        blobs.order.retain(|candidate| candidate != id);
        true
    }
}

fn random_blob_id() -> Result<String> {
    let mut bytes = [0_u8; BLOB_ID_BYTES];
    fill(&mut bytes).map_err(|error| {
        Error::Random(std::io::Error::other(format!(
            "could not generate UI blob identifier: {error}"
        )))
    })?;
    Ok(format!("b{}", hex::encode(bytes)))
}

impl UiSessionRuntimeEndpoint {
    pub(crate) fn router(self) -> Router {
        Router::new()
            .route("/", get(upgrade_runtime))
            .with_state(self)
    }

    pub(crate) fn blob_response(&self, id: &str) -> Response {
        self.shared.blobs.as_ref().map_or_else(
            || StatusCode::NOT_FOUND.into_response(),
            |blobs| blobs.response(id),
        )
    }

    async fn serve(self, mut socket: WebSocket) {
        let Some(hello) = read_hello(&mut socket).await else {
            return;
        };
        if hello.protocol != UI_RUNTIME_PROTOCOL || !valid_id(&hello.client_id) {
            let _ignored = send_protocol_error(
                &mut socket,
                "ui_protocol_invalid",
                "the UI runtime hello is invalid or unsupported",
            )
            .await;
            return;
        }
        if *self.shutdown.borrow() {
            if let Ok(closed) = encode("closed", json!({ "reason": "session_ended" })) {
                let _ignored = send_encoded(&mut socket, &closed).await;
            }
            let _ignored = socket.send(Message::Close(None)).await;
            return;
        }

        let client_id = hello.client_id;
        let mut state = self.state.clone();
        let mut events = self.shared.events.subscribe();
        let mut responses = self.shared.responses.subscribe();
        let mut shutdown = self.shutdown.clone();
        let mut sent_event_sequence = hello.last_event_sequence;

        let initial_state = { state.borrow_and_update().clone() };
        if let Some(current) = initial_state
            && send_encoded(&mut socket, &current).await.is_err()
        {
            return;
        }

        let Ok(EventReplay {
            frames: replay,
            reset,
        }) = self.event_replay(sent_event_sequence)
        else {
            return;
        };
        if let Some((oldest_sequence, latest_sequence)) = reset {
            let Ok(frame) = encode(
                "event_reset",
                json!({
                    "oldest_sequence": oldest_sequence,
                    "latest_sequence": latest_sequence,
                }),
            ) else {
                return;
            };
            if send_encoded(&mut socket, &frame).await.is_err() {
                return;
            }
            sent_event_sequence = latest_sequence;
        } else {
            for frame in replay {
                if send_encoded(&mut socket, &frame.encoded).await.is_err() {
                    return;
                }
                sent_event_sequence = frame.sequence;
            }
        }

        if *shutdown.borrow() {
            if let Ok(closed) = encode("closed", json!({ "reason": "session_ended" })) {
                let _ignored = send_encoded(&mut socket, &closed).await;
            }
            let _ignored = socket.send(Message::Close(None)).await;
            return;
        }
        let Ok(ready) = encode("ready", json!({ "protocol": UI_RUNTIME_PROTOCOL })) else {
            return;
        };
        if send_encoded(&mut socket, &ready).await.is_err() {
            return;
        }

        loop {
            tokio::select! {
                incoming = socket.recv() => {
                    let Some(Ok(message)) = incoming else {
                        return;
                    };
                    match message {
                        Message::Text(text) => {
                            if self.handle_surface_frame(&client_id, text.as_bytes(), &mut socket).await.is_err() {
                                return;
                            }
                        }
                        Message::Binary(_) => {
                            let _ignored = send_protocol_error(
                                &mut socket,
                                "ui_protocol_invalid",
                                "the UI runtime accepts JSON text frames only",
                            ).await;
                            return;
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
                changed = state.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    let current = { state.borrow_and_update().clone() };
                    if let Some(current) = current
                        && send_encoded(&mut socket, &current).await.is_err()
                    {
                        return;
                    }
                }
                event = events.recv() => {
                    match event {
                        Ok(frame) if frame.sequence > sent_event_sequence => {
                            if send_encoded(&mut socket, &frame.encoded).await.is_err() {
                                return;
                            }
                            sent_event_sequence = frame.sequence;
                        }
                        Ok(_) => {}
                        Err(broadcast::error::RecvError::Lagged(_)) => return,
                        Err(broadcast::error::RecvError::Closed) => return,
                    }
                }
                response = responses.recv() => {
                    match response {
                        Ok(frame) if frame.client_id == client_id => {
                            if send_encoded(&mut socket, &frame.encoded).await.is_err() {
                                return;
                            }
                        }
                        Ok(_) => {}
                        Err(broadcast::error::RecvError::Lagged(_)) => return,
                        Err(broadcast::error::RecvError::Closed) => return,
                    }
                }
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        if let Ok(closed) = encode("closed", json!({ "reason": "session_ended" })) {
                            let _ignored = send_encoded(&mut socket, &closed).await;
                        }
                        let _ignored = socket.send(Message::Close(None)).await;
                        return;
                    }
                }
            }
        }
    }

    fn event_replay(&self, after: u64) -> Result<EventReplay> {
        let history = self
            .shared
            .event_history
            .lock()
            .map_err(|_| Error::SessionRuntimeBusy)?;
        let Some(oldest) = history.frames.front().map(|frame| frame.sequence) else {
            return Ok(EventReplay {
                frames: Vec::new(),
                reset: (after > 0).then_some((0, 0)),
            });
        };
        let latest = history.frames.back().map_or(oldest, |frame| frame.sequence);
        if after > latest || after.saturating_add(1) < oldest {
            return Ok(EventReplay {
                frames: Vec::new(),
                reset: Some((oldest, latest)),
            });
        }
        Ok(EventReplay {
            frames: history
                .frames
                .iter()
                .filter(|frame| frame.sequence > after)
                .cloned()
                .collect(),
            reset: None,
        })
    }

    async fn handle_surface_frame(
        &self,
        client_id: &str,
        encoded: &[u8],
        socket: &mut WebSocket,
    ) -> Result<()> {
        ensure_size(encoded.len())?;
        let frame: SurfaceFrame =
            serde_json::from_slice(encoded).map_err(Error::SessionRuntimeDecode)?;
        match frame {
            SurfaceFrame::Call { call } => {
                if !valid_id(&call.request_id) {
                    send_protocol_error(
                        socket,
                        "ui_request_id_invalid",
                        "the UI runtime request id is invalid",
                    )
                    .await?;
                    return Ok(());
                }
                let key = CallKey {
                    client_id: client_id.to_owned(),
                    request_id: call.request_id,
                };
                for replay in self.shared.register_call(key, call.payload).await? {
                    send_encoded(socket, &replay).await?;
                }
            }
            SurfaceFrame::Cancel { cancel } => {
                if valid_id(&cancel.request_id) {
                    self.shared
                        .cancel_call(&CallKey {
                            client_id: client_id.to_owned(),
                            request_id: cancel.request_id,
                        })
                        .await?;
                }
            }
            SurfaceFrame::Ack { ack } => {
                if valid_id(&ack.request_id) {
                    self.shared
                        .acknowledge_call(&CallKey {
                            client_id: client_id.to_owned(),
                            request_id: ack.request_id,
                        })
                        .await;
                }
            }
            SurfaceFrame::ReleaseBlob { release_blob } => {
                self.shared.release_blob_url(&release_blob.blob_url);
            }
        }
        Ok(())
    }
}

impl UiSessionRuntimeCloser {
    pub(crate) fn close(&self) {
        let _ignored = self.shutdown.send(true);
    }
}

#[derive(serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum FirstSurfaceFrame {
    Hello { hello: HelloFrame },
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct HelloFrame {
    protocol: String,
    client_id: String,
    #[serde(default)]
    last_event_sequence: u64,
}

#[derive(serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum SurfaceFrame {
    Call { call: CallFrame },
    Cancel { cancel: RequestFrame },
    Ack { ack: RequestFrame },
    ReleaseBlob { release_blob: BlobReferenceFrame },
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CallFrame {
    request_id: String,
    payload: Value,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestFrame {
    request_id: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct BlobReferenceFrame {
    blob_url: String,
}

async fn read_hello(socket: &mut WebSocket) -> Option<HelloFrame> {
    loop {
        let message = socket.recv().await?.ok()?;
        match message {
            Message::Text(text) => {
                ensure_size(text.len()).ok()?;
                let FirstSurfaceFrame::Hello { hello } =
                    serde_json::from_slice(text.as_bytes()).ok()?;
                return Some(hello);
            }
            Message::Ping(bytes) => {
                socket.send(Message::Pong(bytes)).await.ok()?;
            }
            Message::Pong(_) => {}
            Message::Binary(_) | Message::Close(_) => return None,
        }
    }
}

async fn upgrade_runtime(
    AxumState(endpoint): AxumState<UiSessionRuntimeEndpoint>,
    websocket: WebSocketUpgrade,
) -> Response {
    websocket
        .max_message_size(MAX_UI_RUNTIME_MESSAGE_BYTES)
        .max_frame_size(MAX_UI_RUNTIME_MESSAGE_BYTES)
        .on_upgrade(move |socket| endpoint.serve(socket))
        .into_response()
}

fn encode(kind: &'static str, payload: Value) -> Result<Arc<str>> {
    let mut envelope = serde_json::Map::with_capacity(2);
    envelope.insert("kind".to_string(), Value::String(kind.to_string()));
    envelope.insert(kind.to_string(), payload);
    let encoded =
        serde_json::to_string(&Value::Object(envelope)).map_err(Error::SessionRuntimeEncode)?;
    ensure_size(encoded.len())?;
    Ok(Arc::from(encoded))
}

fn call_error_frame(request_id: &str, failure: &UiCallError) -> Result<Arc<str>> {
    encode("call_error", failure.payload(request_id))
}

fn ensure_size(size_bytes: usize) -> Result<()> {
    if size_bytes > MAX_UI_RUNTIME_MESSAGE_BYTES {
        return Err(Error::SessionRuntimeMessageTooLarge {
            size_bytes,
            max_bytes: MAX_UI_RUNTIME_MESSAGE_BYTES,
        });
    }
    Ok(())
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_ID_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

async fn send_encoded(socket: &mut WebSocket, encoded: &str) -> Result<()> {
    socket
        .send(Message::Text(encoded.to_owned().into()))
        .await
        .map_err(|_| Error::SessionRuntimeClosed)
}

async fn send_protocol_error(
    socket: &mut WebSocket,
    code: &'static str,
    message: &'static str,
) -> Result<()> {
    let encoded = encode(
        "protocol_error",
        json!({
            "code": code,
            "message": message,
            "retryable": false,
        }),
    )?;
    send_encoded(socket, &encoded).await
}

#[cfg(test)]
mod tests {
    use std::sync::Barrier;

    use serde::ser::SerializeStruct;
    use serde::{Deserialize, Serialize, Serializer};

    use super::*;

    #[derive(Debug, Deserialize, PartialEq, Eq)]
    struct Action {
        command: String,
    }

    #[derive(Serialize)]
    struct Reply<'a> {
        answer: &'a str,
    }

    #[derive(Serialize)]
    struct SurfaceState<'a> {
        status: &'a str,
    }

    struct DelayedState {
        status: &'static str,
        entered: Option<Arc<Barrier>>,
        release: Option<Arc<Barrier>>,
    }

    impl Serialize for DelayedState {
        fn serialize<SerializerType>(
            &self,
            serializer: SerializerType,
        ) -> std::result::Result<SerializerType::Ok, SerializerType::Error>
        where
            SerializerType: Serializer,
        {
            if let Some(entered) = &self.entered {
                entered.wait();
            }
            if let Some(release) = &self.release {
                release.wait();
            }
            let mut state = serializer.serialize_struct("DelayedState", 1)?;
            state.serialize_field("status", self.status)?;
            state.end()
        }
    }

    #[test]
    fn envelopes_are_explicit_and_provider_payloads_remain_nested() {
        let encoded = encode(
            "state",
            json!({"revision": 2, "payload": {"status": "ready"}}),
        )
        .unwrap();
        assert_eq!(
            encoded.as_ref(),
            r#"{"kind":"state","state":{"payload":{"status":"ready"},"revision":2}}"#
        );
    }

    #[tokio::test]
    async fn latest_state_replaces_without_waiting_for_a_surface() {
        let (runtime, mount) =
            UiSessionRuntime::<Action, Reply<'static>, SurfaceState<'static>>::bounded(2, 2)
                .unwrap();
        assert_eq!(
            runtime
                .publish_state(&SurfaceState { status: "first" })
                .unwrap(),
            1
        );
        assert_eq!(
            runtime
                .publish_state(&SurfaceState { status: "latest" })
                .unwrap(),
            2
        );
        let encoded = mount.endpoint.state.borrow().clone().unwrap();
        let value: Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(value["state"]["revision"], 2);
        assert_eq!(value["state"]["payload"]["status"], "latest");
    }

    #[test]
    fn a_failed_state_publish_does_not_consume_a_revision() {
        let (runtime, mount) =
            UiSessionRuntime::<Action, Reply<'static>, Value>::bounded(2, 2).unwrap();
        let oversized = json!({"body": "x".repeat(MAX_UI_RUNTIME_MESSAGE_BYTES)});
        assert!(matches!(
            runtime.publish_state(&oversized),
            Err(Error::SessionRuntimeMessageTooLarge { .. })
        ));
        assert_eq!(
            runtime.publish_state(&json!({"status": "ready"})).unwrap(),
            1
        );
        let encoded = mount.endpoint.state.borrow().clone().unwrap();
        let state: Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(state["state"]["revision"], 1);
    }

    #[test]
    fn concurrent_state_publications_cannot_write_an_older_revision_last() {
        let (runtime, mount) =
            UiSessionRuntime::<Action, Reply<'static>, DelayedState>::bounded(2, 2).unwrap();
        let runtime = Arc::new(runtime);
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let slow_runtime = Arc::clone(&runtime);
        let slow_entered = Arc::clone(&entered);
        let slow_release = Arc::clone(&release);
        let slow = std::thread::spawn(move || {
            slow_runtime.publish_state(&DelayedState {
                status: "slow",
                entered: Some(slow_entered),
                release: Some(slow_release),
            })
        });

        entered.wait();
        assert_eq!(
            runtime
                .publish_state(&DelayedState {
                    status: "fast",
                    entered: None,
                    release: None,
                })
                .unwrap(),
            1
        );
        release.wait();
        assert_eq!(slow.join().unwrap().unwrap(), 2);

        let encoded = mount.endpoint.state.borrow().clone().unwrap();
        let state: Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(state["state"]["revision"], 2);
        assert_eq!(state["state"]["payload"]["status"], "slow");
    }

    #[tokio::test]
    async fn transport_identity_never_enters_the_typed_action_or_reply() {
        let (runtime, mount) =
            UiSessionRuntime::<Action, Reply<'static>, SurfaceState<'static>>::bounded(2, 2)
                .unwrap();
        let key = CallKey {
            client_id: "client-1".to_string(),
            request_id: "r1".to_string(),
        };
        mount
            .endpoint
            .shared
            .register_call(key.clone(), json!({"command": "inspect"}))
            .await
            .unwrap();
        let call = runtime.recv().await.unwrap().unwrap();
        assert_eq!(
            call.action(),
            &Action {
                command: "inspect".to_string()
            }
        );
        call.finish(&Reply { answer: "done" }).await.unwrap();
        let records = mount.endpoint.shared.call_records.lock().await;
        let frame = records[&key].frames.back().unwrap();
        let value: Value = serde_json::from_str(frame).unwrap();
        assert_eq!(value["call_result"]["request_id"], "r1");
        assert_eq!(value["call_result"]["payload"]["answer"], "done");
        assert!(value["call_result"]["payload"].get("request_id").is_none());
    }

    #[tokio::test]
    async fn ordered_events_replay_or_reset_inside_a_fixed_bound() {
        let (runtime, mount) =
            UiSessionRuntime::<Action, Reply<'static>, SurfaceState<'static>>::bounded(2, 2)
                .unwrap();
        runtime.publish_event(&json!({"name": "one"})).unwrap();
        runtime.publish_event(&json!({"name": "two"})).unwrap();
        runtime.publish_event(&json!({"name": "three"})).unwrap();

        let EventReplay {
            frames: replay,
            reset,
        } = mount.endpoint.event_replay(1).unwrap();
        assert!(reset.is_none());
        assert_eq!(
            replay
                .iter()
                .map(|frame| frame.sequence)
                .collect::<Vec<_>>(),
            vec![2, 3]
        );

        let EventReplay {
            frames: replay,
            reset,
        } = mount.endpoint.event_replay(0).unwrap();
        assert!(replay.is_empty());
        assert_eq!(reset, Some((2, 3)));
    }

    #[tokio::test]
    async fn failed_events_do_not_consume_or_reorder_sequence_numbers() {
        let (runtime, mount) =
            UiSessionRuntime::<Action, Reply<'static>, SurfaceState<'static>>::bounded(2, 2)
                .unwrap();
        let oversized = "x".repeat(MAX_UI_RUNTIME_MESSAGE_BYTES);
        assert!(matches!(
            runtime.publish_event(&json!({"body": oversized})),
            Err(Error::SessionRuntimeMessageTooLarge { .. })
        ));
        assert_eq!(runtime.publish_event(&json!({"name": "first"})).unwrap(), 1);
        let EventReplay {
            frames: replay,
            reset,
        } = mount.endpoint.event_replay(0).unwrap();
        assert!(reset.is_none());
        assert_eq!(
            replay
                .iter()
                .map(|frame| frame.sequence)
                .collect::<Vec<_>>(),
            vec![1]
        );
    }

    #[test]
    fn an_old_cursor_resets_against_a_new_empty_event_window() {
        let (_runtime, mount) =
            UiSessionRuntime::<Action, Reply<'static>, SurfaceState<'static>>::bounded(2, 2)
                .unwrap();
        let replay = mount.endpoint.event_replay(9).unwrap();
        assert!(replay.frames.is_empty());
        assert_eq!(replay.reset, Some((0, 0)));
    }

    #[test]
    fn dropping_the_provider_runtime_closes_its_surface_endpoint() {
        let (runtime, mount) =
            UiSessionRuntime::<Action, Reply<'static>, SurfaceState<'static>>::bounded(2, 2)
                .unwrap();
        assert!(!*mount.endpoint.shutdown.borrow());
        drop(runtime);
        assert!(*mount.endpoint.shutdown.borrow());
    }

    #[tokio::test]
    async fn call_capacity_bounds_every_unacknowledged_call() {
        let (runtime, mount) =
            UiSessionRuntime::<Action, Reply<'static>, SurfaceState<'static>>::bounded(1, 2)
                .unwrap();
        let first = CallKey {
            client_id: "client-1".to_string(),
            request_id: "r1".to_string(),
        };
        let second = CallKey {
            client_id: "client-1".to_string(),
            request_id: "r2".to_string(),
        };
        mount
            .endpoint
            .shared
            .register_call(first.clone(), json!({"command": "first"}))
            .await
            .unwrap();
        runtime
            .recv()
            .await
            .unwrap()
            .unwrap()
            .finish(&Reply { answer: "done" })
            .await
            .unwrap();

        let busy = mount
            .endpoint
            .shared
            .register_call(second.clone(), json!({"command": "second"}))
            .await
            .unwrap();
        assert_eq!(busy.len(), 1);
        let busy: Value = serde_json::from_str(&busy[0]).unwrap();
        assert_eq!(busy["call_error"]["code"], "ui_runtime_busy");
        assert_eq!(busy["call_error"]["retryable"], true);

        mount.endpoint.shared.acknowledge_call(&first).await;
        assert!(
            mount
                .endpoint
                .shared
                .register_call(second, json!({"command": "second"}))
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            runtime.recv().await.unwrap().unwrap().action(),
            &Action {
                command: "second".to_string()
            }
        );
    }

    #[tokio::test]
    async fn cancellation_is_terminal_until_the_surface_acknowledges_it() {
        let (runtime, mount) =
            UiSessionRuntime::<Action, Reply<'static>, SurfaceState<'static>>::bounded(2, 2)
                .unwrap();
        let first = CallKey {
            client_id: "client-1".to_string(),
            request_id: "r1".to_string(),
        };
        mount
            .endpoint
            .shared
            .register_call(first.clone(), json!({"command": "first"}))
            .await
            .unwrap();
        let call = runtime.recv().await.unwrap().unwrap();
        mount.endpoint.shared.cancel_call(&first).await.unwrap();
        assert!(call.is_cancelled());
        let records = mount.endpoint.shared.call_records.lock().await;
        let terminal: Value = serde_json::from_str(
            records[&first]
                .frames
                .back()
                .expect("cancellation must retain its terminal frame"),
        )
        .unwrap();
        assert_eq!(terminal["call_error"]["code"], "ui_call_cancelled");
        drop(records);

        mount.endpoint.shared.acknowledge_call(&first).await;
        assert!(
            mount
                .endpoint
                .shared
                .call_records
                .lock()
                .await
                .get(&first)
                .is_none()
        );

        let cancelled_while_queued = CallKey {
            client_id: "client-1".to_string(),
            request_id: "r2".to_string(),
        };
        mount
            .endpoint
            .shared
            .register_call(
                cancelled_while_queued.clone(),
                json!({"command": "cancelled"}),
            )
            .await
            .unwrap();
        mount
            .endpoint
            .shared
            .cancel_call(&cancelled_while_queued)
            .await
            .unwrap();
        mount
            .endpoint
            .shared
            .acknowledge_call(&cancelled_while_queued)
            .await;
        let next = CallKey {
            client_id: "client-1".to_string(),
            request_id: "r3".to_string(),
        };
        mount
            .endpoint
            .shared
            .register_call(next, json!({"command": "next"}))
            .await
            .unwrap();
        let mut next_call = runtime.recv().await.unwrap().unwrap();
        assert_eq!(
            next_call.action(),
            &Action {
                command: "next".to_string()
            },
            "a call cancelled before dispatch must never reach Provider code"
        );
        mount.closer.close();
        next_call.cancelled().await;
        assert!(next_call.is_cancelled());
    }
}

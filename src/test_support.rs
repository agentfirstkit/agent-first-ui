//! What a Provider's own tests need from AFUI in order to test *through* it.
//!
//! A Provider that tests its UI end to end has to arrange the world AFUI reads
//! from: a frontend on disk, trusted, under a config directory that is not the
//! developer's own; a browser that does not open; an attention policy; a
//! registry it can wait on; and a client for AFUI's real live runtime. Each of
//! those was otherwise reimplemented per Provider, including the WebSocket
//! framing and AFUI envelope a test should never need to know.
//!
//! A reimplemented fingerprint has a specific failure: it agrees with AFUI
//! today, so the test passes, and it goes on agreeing right up until AFUI's
//! own algorithm changes, at which point four consumer test suites fail for a
//! reason none of them is about. Keeping AFUI honest about *not* drifting is
//! AFUI's own job, pinned by its own tests; a consumer's job is to prove its
//! UI works, and it should be able to do that without knowing how a
//! fingerprint or AFUI request identity is computed.
//!
//! Behind `test-support`, which is not in the default set and never reaches a
//! release binary.

use std::collections::BTreeMap;
use std::fmt;
use std::net::TcpStream as BlockingTcpStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::net::TcpStream as AsyncTcpStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::WebSocket;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

use crate::frontend::frontend_fingerprint;
use crate::reserved::SESSION_RUNTIME_PATH;
use crate::runtime::UI_RUNTIME_PROTOCOL;
use crate::{Error, Result, UiSessionEntry};

/// A failure while a Provider test drives AFUI's real browser protocol.
#[derive(Debug)]
pub struct RuntimeClientError {
    message: String,
    call_failure: Option<Value>,
}

impl RuntimeClientError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            call_failure: None,
        }
    }

    fn with_call_failure(mut self, failure: Value) -> Self {
        self.call_failure = Some(failure);
        self
    }

    fn from_call_failure(failure: &Value) -> Self {
        let mut caller_safe = failure.clone();
        if let Some(object) = caller_safe.as_object_mut() {
            object.remove("request_id");
        }
        Self::new(format!("AFUI test call failed: {caller_safe}")).with_call_failure(caller_safe)
    }

    /// The caller-safe AFUI call failure, when the transport itself succeeded.
    #[must_use]
    pub fn call_failure(&self) -> Option<&Value> {
        self.call_failure.as_ref()
    }

    /// The caller-safe failure code, when this was an AFUI call failure.
    #[must_use]
    pub fn code(&self) -> Option<&str> {
        self.call_failure
            .as_ref()
            .and_then(|failure| failure.get("code"))
            .and_then(Value::as_str)
    }
}

impl fmt::Display for RuntimeClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for RuntimeClientError {}

/// AFUI's real session runtime, packaged for a Provider's integration tests.
///
/// The test supplies only its domain payloads. This client owns the reserved
/// endpoint, protocol hello, request ids, acknowledgements, WebSocket framing,
/// and decoding the retained state. It deliberately lives behind
/// `test-support`; production pages use `afui.connect()`.
pub struct RuntimeClient {
    socket: WebSocketStream<AsyncTcpStream>,
    next_request_number: u64,
}

impl RuntimeClient {
    /// Connect to one issued AFUI access URL and start a fresh browser client.
    pub async fn connect(
        access_url: &str,
        client_id: &str,
    ) -> std::result::Result<Self, RuntimeClientError> {
        let (authority, endpoint) = runtime_endpoint(access_url)?;
        let mut request = endpoint.into_client_request().map_err(|error| {
            RuntimeClientError::new(format!("AFUI runtime URL is invalid: {error}"))
        })?;
        request.headers_mut().insert(
            tokio_tungstenite::tungstenite::http::header::ORIGIN,
            format!("http://{authority}").parse().map_err(|error| {
                RuntimeClientError::new(format!("AFUI runtime origin is invalid: {error}"))
            })?,
        );
        let stream = AsyncTcpStream::connect(&authority).await.map_err(|error| {
            RuntimeClientError::new(format!("could not connect to AFUI runtime: {error}"))
        })?;
        let (socket, _response) = tokio_tungstenite::client_async(request, stream)
            .await
            .map_err(|error| {
                RuntimeClientError::new(format!("AFUI runtime upgrade failed: {error}"))
            })?;
        let mut client = Self {
            socket,
            next_request_number: 1,
        };
        client
            .send(json!({
                "kind": "hello",
                "hello": {
                    "protocol": UI_RUNTIME_PROTOCOL,
                    "client_id": client_id,
                    "last_event_sequence": 0,
                }
            }))
            .await?;
        Ok(client)
    }

    /// Receive the retained state sent before the runtime becomes ready.
    ///
    /// `None` means the Provider had not published state when this connection
    /// opened. A retained-state UI normally treats that as a failed fixture.
    pub async fn opening_state<SurfaceState>(
        &mut self,
    ) -> std::result::Result<Option<SurfaceState>, RuntimeClientError>
    where
        SurfaceState: DeserializeOwned,
    {
        let mut state = None;
        loop {
            let envelope = self.receive().await?;
            match envelope.get("kind").and_then(Value::as_str) {
                Some("state") => {
                    let payload = envelope
                        .get("state")
                        .and_then(|body| body.get("payload"))
                        .cloned()
                        .ok_or_else(|| {
                            RuntimeClientError::new(format!(
                                "AFUI state envelope has no payload: {envelope}"
                            ))
                        })?;
                    state = Some(serde_json::from_value(payload).map_err(|error| {
                        RuntimeClientError::new(format!(
                            "AFUI retained state has the wrong type: {error}"
                        ))
                    })?);
                }
                Some("ready") => return Ok(state),
                Some("protocol_error" | "closed") => {
                    return Err(RuntimeClientError::new(format!(
                        "AFUI runtime refused the test client: {envelope}"
                    )));
                }
                _ => {}
            }
        }
    }

    /// Send one typed domain action and wait for its typed reply.
    ///
    /// Progress frames and unrelated live state are consumed while the call is
    /// active. AFUI supplies and acknowledges the request identity.
    pub async fn call<Action, Reply>(
        &mut self,
        action: &Action,
    ) -> std::result::Result<Reply, RuntimeClientError>
    where
        Action: Serialize,
        Reply: DeserializeOwned,
    {
        self.call_with_progress::<Action, Reply, Value, _>(action, |_progress| {})
            .await
    }

    /// Send one typed action, observing typed progress before its typed reply.
    pub async fn call_with_progress<Action, Reply, Progress, Observe>(
        &mut self,
        action: &Action,
        mut observe: Observe,
    ) -> std::result::Result<Reply, RuntimeClientError>
    where
        Action: Serialize,
        Reply: DeserializeOwned,
        Progress: DeserializeOwned,
        Observe: FnMut(Progress),
    {
        let request_id = format!("afui-test-{}", self.next_request_number);
        self.next_request_number = self.next_request_number.saturating_add(1);
        let payload = serde_json::to_value(action).map_err(|error| {
            RuntimeClientError::new(format!("AFUI test action could not serialize: {error}"))
        })?;
        self.send(json!({
            "kind": "call",
            "call": {
                "request_id": request_id,
                "payload": payload,
            }
        }))
        .await?;

        loop {
            let envelope = self.receive().await?;
            let Some(kind) = envelope.get("kind").and_then(Value::as_str) else {
                continue;
            };
            if matches!(kind, "protocol_error" | "closed") {
                return Err(RuntimeClientError::new(format!(
                    "AFUI runtime ended the test call: {envelope}"
                )));
            }
            let Some(body) = envelope.get(kind) else {
                continue;
            };
            if body.get("request_id").and_then(Value::as_str) != Some(request_id.as_str()) {
                continue;
            }
            match kind {
                "call_progress" => {
                    let payload = body.get("payload").cloned().ok_or_else(|| {
                        RuntimeClientError::new(format!(
                            "AFUI call progress has no payload: {envelope}"
                        ))
                    })?;
                    observe(serde_json::from_value(payload).map_err(|error| {
                        RuntimeClientError::new(format!(
                            "AFUI call progress has the wrong type: {error}"
                        ))
                    })?);
                }
                "call_result" => {
                    let payload = body.get("payload").cloned().ok_or_else(|| {
                        RuntimeClientError::new(format!(
                            "AFUI call result has no payload: {envelope}"
                        ))
                    })?;
                    self.acknowledge(&request_id).await?;
                    return serde_json::from_value(payload).map_err(|error| {
                        RuntimeClientError::new(format!(
                            "AFUI call reply has the wrong type: {error}"
                        ))
                    });
                }
                "call_error" => {
                    self.acknowledge(&request_id).await?;
                    return Err(RuntimeClientError::from_call_failure(body));
                }
                _ => {}
            }
        }
    }

    /// Close the test connection cleanly.
    pub async fn close(&mut self) -> std::result::Result<(), RuntimeClientError> {
        self.socket.close(None).await.map_err(|error| {
            RuntimeClientError::new(format!("could not close AFUI test runtime: {error}"))
        })
    }

    async fn acknowledge(
        &mut self,
        request_id: &str,
    ) -> std::result::Result<(), RuntimeClientError> {
        self.send(json!({
            "kind": "ack",
            "ack": { "request_id": request_id },
        }))
        .await
    }

    async fn send(&mut self, value: Value) -> std::result::Result<(), RuntimeClientError> {
        self.socket
            .send(Message::Text(value.to_string().into()))
            .await
            .map_err(|error| {
                RuntimeClientError::new(format!("could not send AFUI test frame: {error}"))
            })
    }

    async fn receive(&mut self) -> std::result::Result<Value, RuntimeClientError> {
        loop {
            let message = self.socket.next().await.ok_or_else(|| {
                RuntimeClientError::new("AFUI test runtime closed without a close frame")
            })?;
            match message.map_err(|error| {
                RuntimeClientError::new(format!("could not read AFUI test frame: {error}"))
            })? {
                Message::Text(text) => {
                    return serde_json::from_str(&text).map_err(|error| {
                        RuntimeClientError::new(format!(
                            "AFUI test runtime sent invalid JSON: {error}"
                        ))
                    });
                }
                Message::Ping(payload) => {
                    self.socket
                        .send(Message::Pong(payload))
                        .await
                        .map_err(|error| {
                            RuntimeClientError::new(format!(
                                "could not answer AFUI runtime ping: {error}"
                            ))
                        })?;
                }
                Message::Close(frame) => {
                    return Err(RuntimeClientError::new(format!(
                        "AFUI test runtime closed before the expected value: {frame:?}"
                    )));
                }
                Message::Binary(_) => {
                    return Err(RuntimeClientError::new(
                        "AFUI test runtime sent an unexpected binary frame",
                    ));
                }
                Message::Pong(_) | Message::Frame(_) => {}
            }
        }
    }
}

/// The blocking form of [`RuntimeClient`] for process-oriented tests.
///
/// A synchronous CLI test often has to read and write child pipes between an
/// action's progress and result. This client keeps those tests synchronous
/// without handing them AFUI's endpoint, envelope, identity, or WebSocket.
pub struct BlockingRuntimeClient {
    socket: WebSocket<BlockingTcpStream>,
    next_request_number: u64,
}

impl BlockingRuntimeClient {
    /// Connect to one issued AFUI access URL and start a fresh browser client.
    pub fn connect(
        access_url: &str,
        client_id: &str,
    ) -> std::result::Result<Self, RuntimeClientError> {
        let (authority, endpoint) = runtime_endpoint(access_url)?;
        let mut request = endpoint.into_client_request().map_err(|error| {
            RuntimeClientError::new(format!("AFUI runtime URL is invalid: {error}"))
        })?;
        request.headers_mut().insert(
            tokio_tungstenite::tungstenite::http::header::ORIGIN,
            format!("http://{authority}").parse().map_err(|error| {
                RuntimeClientError::new(format!("AFUI runtime origin is invalid: {error}"))
            })?,
        );
        let stream = BlockingTcpStream::connect(&authority).map_err(|error| {
            RuntimeClientError::new(format!("could not connect to AFUI runtime: {error}"))
        })?;
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .map_err(|error| {
                RuntimeClientError::new(format!("could not bound AFUI runtime reads: {error}"))
            })?;
        let (socket, _response) =
            tokio_tungstenite::tungstenite::client(request, stream).map_err(|error| {
                RuntimeClientError::new(format!("AFUI runtime upgrade failed: {error}"))
            })?;
        let mut client = Self {
            socket,
            next_request_number: 1,
        };
        client.send(json!({
            "kind": "hello",
            "hello": {
                "protocol": UI_RUNTIME_PROTOCOL,
                "client_id": client_id,
                "last_event_sequence": 0,
            }
        }))?;
        Ok(client)
    }

    /// Receive the retained state sent before the runtime becomes ready.
    pub fn opening_state<SurfaceState>(
        &mut self,
    ) -> std::result::Result<Option<SurfaceState>, RuntimeClientError>
    where
        SurfaceState: DeserializeOwned,
    {
        let mut state = None;
        loop {
            let envelope = self.receive()?;
            match envelope.get("kind").and_then(Value::as_str) {
                Some("state") => {
                    let payload = envelope
                        .get("state")
                        .and_then(|body| body.get("payload"))
                        .cloned()
                        .ok_or_else(|| {
                            RuntimeClientError::new(format!(
                                "AFUI state envelope has no payload: {envelope}"
                            ))
                        })?;
                    state = Some(serde_json::from_value(payload).map_err(|error| {
                        RuntimeClientError::new(format!(
                            "AFUI retained state has the wrong type: {error}"
                        ))
                    })?);
                }
                Some("ready") => return Ok(state),
                Some("protocol_error" | "closed") => {
                    return Err(RuntimeClientError::new(format!(
                        "AFUI runtime refused the test client: {envelope}"
                    )));
                }
                _ => {}
            }
        }
    }

    /// Send one typed action and wait for its typed reply.
    pub fn call<Action, Reply>(
        &mut self,
        action: &Action,
    ) -> std::result::Result<Reply, RuntimeClientError>
    where
        Action: Serialize,
        Reply: DeserializeOwned,
    {
        self.call_with_progress::<Action, Reply, Value, _>(action, |_progress| {})
    }

    /// Send one typed action, observing typed progress before its typed reply.
    ///
    /// The callback may synchronously drive other process pipes. AFUI still
    /// owns the call identity and acknowledgement around every callback.
    pub fn call_with_progress<Action, Reply, Progress, Observe>(
        &mut self,
        action: &Action,
        mut observe: Observe,
    ) -> std::result::Result<Reply, RuntimeClientError>
    where
        Action: Serialize,
        Reply: DeserializeOwned,
        Progress: DeserializeOwned,
        Observe: FnMut(Progress),
    {
        let request_id = format!("afui-test-{}", self.next_request_number);
        self.next_request_number = self.next_request_number.saturating_add(1);
        let payload = serde_json::to_value(action).map_err(|error| {
            RuntimeClientError::new(format!("AFUI test action could not serialize: {error}"))
        })?;
        self.send(json!({
            "kind": "call",
            "call": {
                "request_id": request_id,
                "payload": payload,
            }
        }))?;

        loop {
            let envelope = self.receive()?;
            let Some(kind) = envelope.get("kind").and_then(Value::as_str) else {
                continue;
            };
            if matches!(kind, "protocol_error" | "closed") {
                return Err(RuntimeClientError::new(format!(
                    "AFUI runtime ended the test call: {envelope}"
                )));
            }
            let Some(body) = envelope.get(kind) else {
                continue;
            };
            if body.get("request_id").and_then(Value::as_str) != Some(request_id.as_str()) {
                continue;
            }
            match kind {
                "call_progress" => {
                    let payload = body.get("payload").cloned().ok_or_else(|| {
                        RuntimeClientError::new(format!(
                            "AFUI call progress has no payload: {envelope}"
                        ))
                    })?;
                    observe(serde_json::from_value(payload).map_err(|error| {
                        RuntimeClientError::new(format!(
                            "AFUI call progress has the wrong type: {error}"
                        ))
                    })?);
                }
                "call_result" => {
                    let payload = body.get("payload").cloned().ok_or_else(|| {
                        RuntimeClientError::new(format!(
                            "AFUI call result has no payload: {envelope}"
                        ))
                    })?;
                    self.acknowledge(&request_id)?;
                    return serde_json::from_value(payload).map_err(|error| {
                        RuntimeClientError::new(format!(
                            "AFUI call reply has the wrong type: {error}"
                        ))
                    });
                }
                "call_error" => {
                    self.acknowledge(&request_id)?;
                    return Err(RuntimeClientError::from_call_failure(body));
                }
                _ => {}
            }
        }
    }

    /// Close the test connection cleanly.
    pub fn close(&mut self) -> std::result::Result<(), RuntimeClientError> {
        self.socket.close(None).map_err(|error| {
            RuntimeClientError::new(format!("could not close AFUI test runtime: {error}"))
        })
    }

    fn acknowledge(&mut self, request_id: &str) -> std::result::Result<(), RuntimeClientError> {
        self.send(json!({
            "kind": "ack",
            "ack": { "request_id": request_id },
        }))
    }

    fn send(&mut self, value: Value) -> std::result::Result<(), RuntimeClientError> {
        self.socket
            .send(Message::Text(value.to_string().into()))
            .map_err(|error| {
                RuntimeClientError::new(format!("could not send AFUI test frame: {error}"))
            })
    }

    fn receive(&mut self) -> std::result::Result<Value, RuntimeClientError> {
        loop {
            match self.socket.read().map_err(|error| {
                RuntimeClientError::new(format!("could not read AFUI test frame: {error}"))
            })? {
                Message::Text(text) => {
                    return serde_json::from_str(&text).map_err(|error| {
                        RuntimeClientError::new(format!(
                            "AFUI test runtime sent invalid JSON: {error}"
                        ))
                    });
                }
                Message::Ping(payload) => {
                    self.socket.send(Message::Pong(payload)).map_err(|error| {
                        RuntimeClientError::new(format!(
                            "could not answer AFUI runtime ping: {error}"
                        ))
                    })?;
                }
                Message::Close(frame) => {
                    return Err(RuntimeClientError::new(format!(
                        "AFUI test runtime closed before the expected value: {frame:?}"
                    )));
                }
                Message::Binary(_) => {
                    return Err(RuntimeClientError::new(
                        "AFUI test runtime sent an unexpected binary frame",
                    ));
                }
                Message::Pong(_) | Message::Frame(_) => {}
            }
        }
    }
}

fn runtime_endpoint(access_url: &str) -> std::result::Result<(String, String), RuntimeClientError> {
    let rest = access_url.strip_prefix("http://").ok_or_else(|| {
        RuntimeClientError::new(format!(
            "AFUI test runtime requires an http access URL, got {access_url}"
        ))
    })?;
    let authority = rest
        .split('/')
        .next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            RuntimeClientError::new(format!("AFUI access URL has no authority: {access_url}"))
        })?
        .to_owned();
    let endpoint = format!(
        "{}/{}",
        access_url
            .replacen("http://", "ws://", 1)
            .trim_end_matches('/'),
        SESSION_RUNTIME_PATH
    );
    Ok((authority, endpoint))
}

/// A frontend on disk, as `afui frontend init` would leave it.
///
/// The files are the caller's — this is not a template gallery. What it owns
/// is where they go and what `frontend.json` has to say, both of which are
/// AFUI's rules rather than the Provider's.
///
/// ```no_run
/// # use agent_first_ui::test_support::InstalledFrontend;
/// # fn example(workspace: &std::path::Path) -> agent_first_ui::Result<()> {
/// InstalledFrontend::new(workspace, "reviewer", "console", "1")
///     .with_frontend_id("custom")
///     .with_file("templates/page.html.j2", "<main><!-- marker --></main>")
///     .install()?
///     .trust()?;
/// # Ok(())
/// # }
/// ```
pub struct InstalledFrontend {
    workspace_root: PathBuf,
    provider_id: String,
    ui_kind: String,
    ui_api_version: String,
    frontend_id: String,
    files: BTreeMap<String, Vec<u8>>,
}

/// A frontend that is on disk, and knows where it went.
pub struct FrontendOnDisk {
    root: PathBuf,
    provider_id: String,
    ui_kind: String,
}

impl InstalledFrontend {
    /// A frontend for one `provider_id` + `ui_kind`, in this workspace.
    pub fn new(
        workspace_root: impl Into<PathBuf>,
        provider_id: impl Into<String>,
        ui_kind: impl Into<String>,
        ui_api_version: impl Into<String>,
    ) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            provider_id: provider_id.into(),
            ui_kind: ui_kind.into(),
            ui_api_version: ui_api_version.into(),
            frontend_id: "test_frontend".to_owned(),
            files: BTreeMap::new(),
        }
    }

    /// The id this frontend reports as, which is what a Provider's readiness
    /// output names when an override is actually serving.
    #[must_use]
    pub fn with_frontend_id(mut self, frontend_id: impl Into<String>) -> Self {
        self.frontend_id = frontend_id.into();
        self
    }

    /// One file, at a path relative to the frontend's own root.
    #[must_use]
    pub fn with_file(mut self, path: impl Into<String>, contents: impl Into<Vec<u8>>) -> Self {
        self.files.insert(path.into(), contents.into());
        self
    }

    /// Write it, `frontend.json` included.
    pub fn install(self) -> Result<FrontendOnDisk> {
        let root = crate::frontend_root(
            crate::FrontendScope::Workspace,
            &self.workspace_root,
            &self.provider_id,
            &self.ui_kind,
        )?;
        std::fs::create_dir_all(&root).map_err(Error::Io)?;
        let manifest = serde_json::json!({
            "frontend_id": self.frontend_id,
            "ui_api_version": self.ui_api_version,
        });
        std::fs::write(
            root.join("frontend.json"),
            serde_json::to_vec_pretty(&manifest).map_err(|error| Error::FrontendManifest {
                message: error.to_string(),
            })?,
        )
        .map_err(Error::Io)?;
        for (path, contents) in &self.files {
            let file = root.join(path);
            if let Some(parent) = file.parent() {
                std::fs::create_dir_all(parent).map_err(Error::Io)?;
            }
            std::fs::write(file, contents).map_err(Error::Io)?;
        }
        Ok(FrontendOnDisk {
            root,
            provider_id: self.provider_id,
            ui_kind: self.ui_kind,
        })
    }
}

impl FrontendOnDisk {
    /// A frontend already on disk, for a test that wrote the files itself.
    ///
    /// The trust store is keyed by `provider_id` and `ui_kind`, so both are
    /// needed even though the root already contains them: the key is AFUI's
    /// format, not a path.
    pub fn at(
        root: impl Into<PathBuf>,
        provider_id: impl Into<String>,
        ui_kind: impl Into<String>,
    ) -> Self {
        Self {
            root: root.into(),
            provider_id: provider_id.into(),
            ui_kind: ui_kind.into(),
        }
    }

    /// Where the files went, for a test that wants to edit one afterwards.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Trust exactly these bytes, as `afui frontend enable` would, in the
    /// config directory this test is using.
    ///
    /// The fingerprint is AFUI's own, computed by the code AFUI itself trusts
    /// against — so a test proves its frontend serves, rather than proving
    /// that its copy of a hash function still matches.
    ///
    /// Naming the directory rather than reading `AFUI_CONFIG_DIR`: a test
    /// binary runs its tests on several threads, so pointing a process-wide
    /// variable at one test's tree points it at every concurrent test's tree
    /// too. A consumer doing exactly that — set, trust, restore, with a
    /// comment asserting the suite was single-threaded — had two frontend
    /// tests writing each other's trust store, which reads as "my override
    /// did not serve" in whichever one lost.
    pub fn trust_in(&self, config_dir: impl AsRef<Path>) -> Result<&Self> {
        self.write_trust(config_dir.as_ref().join("trust.json"))
    }

    /// The same, in whatever `AFUI_CONFIG_DIR` names — for a test that has
    /// only one config directory and never runs beside another.
    pub fn trust(&self) -> Result<&Self> {
        let trust_path = crate::global_afui_root()?.join("trust.json");
        self.write_trust(trust_path)
    }

    fn write_trust(&self, trust_path: PathBuf) -> Result<&Self> {
        if let Some(parent) = trust_path.parent() {
            std::fs::create_dir_all(parent).map_err(Error::Io)?;
        }
        let mut store: serde_json::Value = match std::fs::read(&trust_path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|_| serde_json::json!({})),
            Err(_) => serde_json::json!({}),
        };
        let key = format!("workspace:{}:{}", self.provider_id, self.ui_kind);
        let entry = serde_json::json!({ "fingerprint": frontend_fingerprint(&self.root)? });
        if let Some(object) = store.as_object_mut() {
            let frontends = object
                .entry("frontends")
                .or_insert_with(|| serde_json::json!({}));
            if let Some(frontends) = frontends.as_object_mut() {
                frontends.insert(key, entry);
            }
        }
        std::fs::write(
            &trust_path,
            serde_json::to_vec_pretty(&store).map_err(|error| Error::FrontendManifest {
                message: error.to_string(),
            })?,
        )
        .map_err(Error::Io)?;
        Ok(self)
    }

    /// Revoke trust by editing a file, which is what a person does by accident
    /// and what the whole trust model is about noticing.
    pub fn edit(&self, path: &str, contents: impl AsRef<[u8]>) -> Result<()> {
        std::fs::write(self.root.join(path), contents).map_err(Error::Io)
    }
}

/// Writes the idle and grace periods you pass as the global attention policy,
/// where AFUI reads it.
///
/// Reads `AFUI_CONFIG_DIR`. A test that wants a Link to lapse in a second
/// writes it here rather than waiting fifteen minutes for the default.
pub fn write_attention_config(idle_timeout: Duration, grace_period: Duration) -> Result<PathBuf> {
    let root = crate::global_afui_root()?;
    std::fs::create_dir_all(&root).map_err(Error::Io)?;
    let path = root.join("config.json");
    let config = serde_json::json!({
        "attention": {
            "idle_timeout_s": idle_timeout.as_secs(),
            "grace_period_s": grace_period.as_secs(),
        }
    });
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&config).map_err(|error| Error::FrontendManifest {
            message: error.to_string(),
        })?,
    )
    .map_err(Error::Io)?;
    Ok(path)
}

/// A browser that records the session URL it was told to open and does not
/// open it.
///
/// Any test of window delivery needs one, and the script is easy to get
/// subtly wrong per platform. Returns the path to point `AFUI_BROWSER_BINARY`
/// at; the arguments land in `record_at`, one line per launch.
///
/// AFUI launches a window on a private page that moves on to the session URL,
/// so the credential is never in the browser's arguments. The stub reads that
/// page and records `--app=<the session URL>` in its place, which is what a
/// test asserting on the URL means.
///
/// On Unix this is a shell script; on Windows a batch file that runs a
/// PowerShell script beside it. A test asserting on the recorded URL does not
/// have to know which.
pub fn stub_browser(directory: &Path, record_at: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(directory).map_err(Error::Io)?;
    let record = record_at.display().to_string();
    #[cfg(windows)]
    let (name, script) = {
        let resolver = directory.join("stub-browser.ps1");
        std::fs::write(
            &resolver,
            format!(
                "$out = foreach ($arg in $args) {{\r\n\
                 if ($arg -like '--app=file:*') {{\r\n\
                 $page = [Uri]::new($arg.Substring(6)).LocalPath\r\n\
                 $match = [regex]::Match((Get-Content -Raw $page), 'location\\.replace\\(\"(.*)\"\\);')\r\n\
                 '--app=' + ($match.Groups[1].Value -replace '\\\\u003c', '<')\r\n\
                 }} else {{ $arg }}\r\n\
                 }}\r\n\
                 Add-Content -LiteralPath '{record}' -Value ($out -join ' ')\r\n"
            ),
        )
        .map_err(Error::Io)?;
        (
            "stub-browser.bat",
            "@powershell -NoProfile -ExecutionPolicy Bypass -File \"%~dp0stub-browser.ps1\" %*\r\n"
                .to_owned(),
        )
    };
    #[cfg(not(windows))]
    let (name, script) = (
        "stub-browser.sh",
        format!(
            "#!/bin/sh\n\
             line=''\n\
             for arg in \"$@\"; do\n\
             case \"$arg\" in\n\
             --app=file://*) arg=\"--app=$(sed -n 's/.*location\\.replace(\"\\(.*\\)\");.*/\\1/p' \"${{arg#--app=file://}}\" | sed 's/\\\\u003c/</g')\" ;;\n\
             esac\n\
             line=\"$line${{line:+ }}$arg\"\n\
             done\n\
             printf '%s\\n' \"$line\" >> '{record}'\n"
        ),
    );
    let path = directory.join(name);
    std::fs::write(&path, script).map_err(Error::Io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .map_err(Error::Io)?;
    }
    Ok(path)
}

/// Wait for a session another process announced to appear in the registry.
///
/// For a test that drives a Provider as a child process, where nothing on this
/// side knows how far along that process is. A test that holds the session
/// itself does not need this: announcing happens inside `attach`, before the
/// active session is handed back. `None` when nothing matched within
/// `timeout`.
///
/// Blocks the calling thread. From an async test that is `spawn_blocking`, not
/// a direct call — a single-threaded runtime blocked here cannot also run the
/// task that would satisfy the wait.
///
/// Reads `AFUI_CONFIG_DIR`, and honours `AFUI_NO_REGISTRY` the same way
/// everything else does — which is why this returns `None` rather than
/// asserting: a test that opted out of the registry should see nothing, not a
/// panic.
#[must_use]
pub fn wait_for_session(
    provider_id: &str,
    ui_kind: &str,
    timeout: Duration,
) -> Option<UiSessionEntry> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(sessions) = crate::list_sessions()
            && let Some(found) = sessions
                .into_iter()
                .find(|entry| entry.provider_id == provider_id && entry.ui_kind == ui_kind)
        {
            return Some(found);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Wait for the registry to hold no session for this Provider.
///
/// The direction that is genuinely late even in-process: a session is
/// withdrawn when its handle drops, which a test observes some moments later.
/// Blocks the calling thread, on the same terms as [`wait_for_session`].
#[must_use]
pub fn wait_for_no_session(provider_id: &str, ui_kind: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        let gone = crate::list_sessions().is_ok_and(|sessions| {
            !sessions
                .iter()
                .any(|entry| entry.provider_id == provider_id && entry.ui_kind == ui_kind)
        });
        if gone {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

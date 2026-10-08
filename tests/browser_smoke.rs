//! One optional release check against a real Chrome or Chromium browser.
//!
//! Run through scripts/test.sh browser. Ordinary unit suites compile this
//! fixture but never launch a browser. Every server, profile and registry here
//! belongs to this invocation; no external page or installed frontend is used.

#![cfg(feature = "session")]
#![allow(clippy::expect_used)]

use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use agent_first_ui::{SessionServer, UiAttentionPolicy, UiExpiry, UiSession, UiUrlDelivery};
use axum::Router;
use axum::http::header;
use axum::response::Html;
use axum::routing::get;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio::process::Command;
use tokio::time::{Instant, sleep, timeout};
use tokio_tungstenite::{WebSocketStream, client_async, tungstenite::Message};

const STEP_TIMEOUT: Duration = Duration::from_secs(10);
/// Starting Chrome is setup, not behaviour under test. A cold start on a fresh
/// CI runner, right after a release build, has taken longer than a page step
/// is allowed; this budget only has to catch a browser that never comes up.
const BROWSER_START_TIMEOUT: Duration = Duration::from_secs(60);

struct FixtureTask(tokio::task::JoinHandle<()>);

impl Drop for FixtureTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Only the CDP methods this one browser check uses, over existing test deps.
struct Cdp {
    socket: WebSocketStream<TcpStream>,
    next_id: u64,
}

impl Cdp {
    async fn command(&mut self, method: &str, params: Value, session: Option<&str>) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        let mut request = json!({ "id": id, "method": method, "params": params });
        if let Some(session) = session {
            request["sessionId"] = json!(session);
        }
        self.socket
            .send(Message::Text(request.to_string().into()))
            .await
            .expect("send CDP request");
        timeout(STEP_TIMEOUT, async {
            loop {
                let message = self
                    .socket
                    .next()
                    .await
                    .expect("CDP remains open")
                    .expect("CDP frame");
                let Message::Text(text) = message else {
                    continue;
                };
                let response: Value = serde_json::from_str(&text).expect("CDP JSON");
                if response["id"] == id {
                    assert!(response.get("error").is_none(), "{method}: {response}");
                    return response["result"].clone();
                }
            }
        })
        .await
        .expect("CDP responds within the fixture budget")
    }

    async fn evaluate(&mut self, session: &str, expression: &str) -> Value {
        let params = json!({
            "expression": expression,
            "returnByValue": true,
            "awaitPromise": true,
        });
        let result = self
            .command("Runtime.evaluate", params, Some(session))
            .await;
        assert!(result.get("exceptionDetails").is_none(), "{result}");
        result["result"]["value"].clone()
    }

    async fn wait_for(&mut self, session: &str, expression: &str, description: &str) {
        let deadline = Instant::now() + STEP_TIMEOUT;
        loop {
            if self.evaluate(session, expression).await == true {
                return;
            }
            assert!(Instant::now() < deadline, "browser did not {description}");
            sleep(Duration::from_millis(100)).await;
        }
    }
}

#[tokio::test]
#[ignore = "a real browser is selected and isolated by scripts/test.sh browser"]
async fn headless_browser_smoke() {
    let browser_binary =
        std::env::var("AFUI_TEST_BROWSER_BINARY").expect("fixed script selects Chrome");
    let config_root =
        std::env::var(agent_first_ui::CONFIG_DIR_ENV).expect("fixed script isolates config");
    let profile = tempfile::tempdir().expect("browser profile");
    // Chrome says why it will not start on stderr and nowhere else. Keep it in
    // a file beside the profile so a start failure carries its own cause.
    let stderr_path = profile.path().join("chrome-stderr.log");
    let stderr_file = std::fs::File::create(&stderr_path).expect("Chrome stderr file");
    let chrome_stderr = || {
        let text = std::fs::read_to_string(&stderr_path).unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        lines[lines.len().saturating_sub(40)..].join("\n")
    };
    let mut browser = Command::new(browser_binary)
        .args([
            "--headless=new",
            "--disable-gpu",
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-background-networking",
            "--disable-component-update",
            "--disable-sync",
            "--remote-debugging-address=127.0.0.1",
            "--remote-debugging-port=0",
            "--window-size=360,640",
        ])
        .arg(format!("--user-data-dir={}", profile.path().display()))
        .arg("about:blank")
        .stdout(Stdio::null())
        .stderr(stderr_file)
        .kill_on_drop(true)
        .spawn()
        .expect("start fixture Chrome");
    let deadline = Instant::now() + BROWSER_START_TIMEOUT;
    let control = loop {
        if let Ok(control) = std::fs::read_to_string(profile.path().join("DevToolsActivePort")) {
            break control;
        }
        if let Some(status) = browser.try_wait().expect("Chrome status") {
            panic!(
                "Chrome exited before CDP was ready ({status}); its stderr:\n{}",
                chrome_stderr()
            );
        }
        assert!(
            Instant::now() < deadline,
            "Chrome did not start CDP within {BROWSER_START_TIMEOUT:?}; its stderr:\n{}",
            chrome_stderr()
        );
        sleep(Duration::from_millis(50)).await;
    };
    let mut lines = control.lines();
    let port: u16 = lines
        .next()
        .expect("CDP port")
        .parse()
        .expect("numeric CDP port");
    let browser_path = lines.next().expect("CDP browser path");
    let stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("CDP TCP");
    let (socket, _) = client_async(format!("ws://127.0.0.1:{port}{browser_path}"), stream)
        .await
        .expect("CDP WebSocket");
    let mut cdp = Cdp { socket, next_id: 0 };
    let target = cdp
        .command("Target.createTarget", json!({ "url": "about:blank" }), None)
        .await;
    let target_id = target["targetId"]
        .as_str()
        .expect("shell target")
        .to_owned();
    let attached = cdp
        .command(
            "Target.attachToTarget",
            json!({ "targetId": target_id, "flatten": true }),
            None,
        )
        .await;
    let session = attached["sessionId"]
        .as_str()
        .expect("CDP session")
        .to_owned();
    cdp.command("Page.enable", json!({}), Some(&session)).await;
    cdp.command("Runtime.enable", json!({}), Some(&session))
        .await;
    // Observe the fixture resource without reaching into a sandboxed frame or
    // depending on whether Chrome puts that frame in a separate process.
    cdp.command(
        "Page.addScriptToEvaluateOnNewDocument",
        json!({ "source": r#"
        addEventListener('message', event => {
            const frame = document.querySelector('.card iframe');
            if (frame && event.source === frame.contentWindow &&
                event.data === 'fixture-resource-loaded') {
                document.body.dataset.browserSmokeReady = 'true';
            }
        });
    "# }),
        Some(&session),
    )
    .await;
    cdp.command(
        "Emulation.setDeviceMetricsOverride",
        json!({
            "width": 360, "height": 640, "deviceScaleFactor": 1, "mobile": true,
        }),
        Some(&session),
    )
    .await;
    cdp.command(
        "Target.activateTarget",
        json!({ "targetId": target_id }),
        None,
    )
    .await;

    let provider = UiSession::<()>::new("inspector", "browser_check")
        .expect("neutral provider")
        .with_subject("sample")
        .serve(
            Router::new()
                .route("/", get(|| async {
                    Html("<!doctype html><meta charset=utf-8><title>Inspector</title><body><h1>Inspector</h1><script src=fixture.js></script>")
                }))
                .route("/fixture.js", get(|| async {
                    ([(header::CONTENT_TYPE, "application/javascript")], "parent.postMessage('fixture-resource-loaded', '*');")
                })),
            UiUrlDelivery::new(UiExpiry::Never),
        )
        .await
        .expect("serve neutral provider");
    let blocked_requests = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&blocked_requests);
    let forbidden = Router::new().route(
        "/blocked.js",
        get(move || {
            let count = Arc::clone(&count);
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                (
                    [(header::CONTENT_TYPE, "application/javascript")],
                    "globalThis.forbiddenResourceRan = true;",
                )
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("forbidden resource listener");
    let forbidden_url = format!(
        "http://{}/blocked.js",
        listener.local_addr().expect("forbidden resource address")
    );
    let _forbidden_task = FixtureTask(tokio::spawn(async move {
        axum::serve(listener, forbidden)
            .await
            .expect("serve forbidden resource fixture");
    }));
    // Three warning seconds span a stock two-second refresh without changing
    // the page's timers. Only server wall time decides when attention expires.
    let served = SessionServer::new("127.0.0.1:0".parse().expect("loopback"))
        .with_workspace_root(config_root)
        .with_attention_policy(
            UiAttentionPolicy::default()
                .with_idle_timeout(Duration::from_secs(1))
                .with_grace_period(Duration::from_secs(3)),
        )
        .bind()
        .await
        .expect("serve shell");
    let access_url = served.access_url().to_owned();
    let _shell_task = FixtureTask(tokio::spawn(async move {
        served
            .run(std::future::pending::<()>())
            .await
            .expect("shell server");
    }));
    cdp.command(
        "Page.navigate",
        json!({ "url": access_url }),
        Some(&session),
    )
    .await;
    cdp.wait_for(&session,
        "document.querySelector('.card .provider')?.textContent === 'inspector' && !!document.querySelector('.card iframe') && document.body.dataset.browserSmokeReady === 'true'",
        "mount the provider iframe and execute its own resource").await;
    let viewport = cdp
        .evaluate(
            &session,
            r#"(() => {
        const frame = document.querySelector('.card iframe');
        const box = frame.getBoundingClientRect();
        return {width: innerWidth, scrollWidth: document.documentElement.scrollWidth,
            frameWidth: box.width, sandbox: frame.getAttribute('sandbox')};
    })()"#,
        )
        .await;
    assert_eq!(viewport["width"], 360, "{viewport}");
    assert!(
        viewport["scrollWidth"].as_f64().expect("scroll width") <= 361.0,
        "{viewport}"
    );
    assert!(
        viewport["frameWidth"].as_f64().expect("frame width") > 0.0,
        "{viewport}"
    );
    assert_eq!(viewport["sandbox"], "allow-scripts allow-forms");
    let frame_url = cdp
        .evaluate(&session, "document.querySelector('.card iframe').src")
        .await;
    let old_frame_url = frame_url.as_str().expect("mounted frame URL").to_owned();

    let blocked = cdp
        .evaluate(
            &session,
            &format!(
                r#"new Promise(resolve => {{
        const source = {};
        document.addEventListener('securitypolicyviolation', event => {{
            if (event.blockedURI === source || event.blockedURI === new URL(source).origin) {{
                resolve(event.effectiveDirective);
            }}
        }});
        const script = document.createElement('script');
        script.src = source;
        document.head.append(script);
    }})"#,
                serde_json::to_string(&forbidden_url).expect("fixture URL JSON")
            ),
        )
        .await;
    assert!(
        blocked
            .as_str()
            .expect("CSP violation")
            .starts_with("script-src"),
        "{blocked}"
    );
    assert_eq!(
        blocked_requests.load(Ordering::SeqCst),
        0,
        "CSP refuses the request before it reaches another origin"
    );
    assert_eq!(
        cdp.evaluate(&session, "typeof globalThis.forbiddenResourceRan")
            .await,
        "undefined"
    );

    // Switching to another actual target makes this page hidden. Its polling
    // remains stock, while its visibility check stops attention heartbeats.
    let away = cdp
        .command("Target.createTarget", json!({ "url": "about:blank" }), None)
        .await;
    cdp.command(
        "Target.activateTarget",
        json!({ "targetId": away["targetId"] }),
        None,
    )
    .await;
    cdp.wait_for(
        &session,
        "document.visibilityState === 'hidden'",
        "leave the shell tab",
    )
    .await;
    cdp.wait_for(
        &session,
        r#"(() => {
        const notice = document.querySelector('#page-notice');
        return notice?.dataset.state === 'expiring' && !notice.hidden &&
            !!notice.textContent.trim() && !!document.querySelector('.card iframe');
    })()"#,
        "show its idle warning while the frame is still mounted",
    )
    .await;
    cdp.wait_for(&session,
        "document.querySelector('#page-notice')?.dataset.state === 'dead' && document.querySelector('#state')?.textContent === 'expired' && document.querySelectorAll('iframe').length === 0",
        "remove every iframe after expiry").await;
    cdp.command(
        "Target.activateTarget",
        json!({ "targetId": target_id }),
        None,
    )
    .await;
    cdp.wait_for(&session, "document.visibilityState === 'visible' && document.querySelectorAll('iframe').length === 0", "keep the expired view empty on return").await;
    assert_eq!(
        cdp.evaluate(
            &session,
            &format!(
                "fetch({}).then(response => response.status)",
                serde_json::to_string(&old_frame_url).expect("frame URL JSON")
            )
        )
        .await,
        404
    );
    assert!(
        agent_first_ui::list_sessions()
            .expect("registry remains live")
            .iter()
            .any(|entry| entry.session_id == provider.metadata().session_id.as_str())
    );

    cdp.command("Browser.close", json!({}), None).await;
    let status = timeout(STEP_TIMEOUT, browser.wait())
        .await
        .expect("Chrome closes")
        .expect("Chrome exit status");
    assert!(status.success(), "fixture Chrome exits cleanly: {status}");
}

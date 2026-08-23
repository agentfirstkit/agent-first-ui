//! What `UiSurface` promises beyond composing the pieces by hand.
//!
//! Two things, and they are the two that a hand-composed panel got wrong: a
//! submission completes the session with what the Provider's own parse made of
//! the fields, and only the first one counts — the second person to press a
//! button is told their answer arrived after the decision, rather than having
//! it silently discarded.

#![cfg(feature = "session")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use agent_first_ui::{
    CONFIG_DIR_ENV, Outcome, UiDeliveryMode, UiDeliveryPlan, UiFrontend, UiPagePolicy,
    UiPageScript, UiSurface,
};

/// `AFUI_CONFIG_DIR` is process-wide, so this binary sets it once to one root
/// rather than each test racing to point it somewhere of its own.
///
/// One root is enough because the tests below use different `provider_id`s:
/// what lives under it is a registry keyed by session, and two sessions that
/// were never the same session cannot collide.
static CONFIG_ROOT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

fn config_root() -> &'static PathBuf {
    CONFIG_ROOT.get_or_init(|| {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let root = std::env::temp_dir().join(format!("afui-surface-{stamp}"));
        std::fs::create_dir_all(&root).unwrap();
        // SAFETY: set once, to the same value, before any test reads it — no
        // test ever observes it changing.
        unsafe { std::env::set_var(CONFIG_DIR_ENV, &root) };
        root
    })
}

const PAGE: &str = "<main id=\"status\">a page</main>";

async fn post(access_url: &str, route: &str, body: &str) -> (u16, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let rest = access_url.strip_prefix("http://").unwrap();
    let (authority, path) = rest.split_once('/').unwrap();
    let request = format!(
        "POST /{path}{route} HTTP/1.1\r\nHost: {authority}\r\nContent-Type: \
         application/x-www-form-urlencoded\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let mut stream = tokio::net::TcpStream::connect(authority).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.unwrap();
    let text = String::from_utf8_lossy(&raw).into_owned();
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    (status, text)
}

#[tokio::test]
async fn a_submission_completes_the_session_with_what_the_provider_made_of_the_fields() {
    let _root = config_root();

    let frontend = UiFrontend::builtin("reviewer", "console", "1");
    let active = UiSurface::<String>::new("reviewer", "console")
        .unwrap()
        .with_subject("case-1")
        .with_page_policy(UiPagePolicy::new(UiPageScript::None).allow_form_submission())
        // AFUI never looks inside the map; this is the Provider's own parse,
        // and the value it produces is the outcome.
        .on("approve", |fields| {
            format!(
                "approved:{}",
                fields.get("note").cloned().unwrap_or_default()
            )
        })
        .with_answer_page(|value, recorded| format!("<p>{value} recorded={recorded}</p>"))
        .start(
            UiDeliveryPlan::resolve(Some(UiDeliveryMode::Session)).unwrap(),
            PAGE,
            b"body{}".to_vec(),
            &frontend,
        )
        .await
        .unwrap();

    let url = agent_first_ui::find_session(&active.metadata().session_id.to_string())
        .unwrap()
        .expect("the surface is announced")
        .access_url_secret;

    let (status, body) = post(&url, "approve", "note=looks+fine").await;
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("approved:looks fine"), "{body}");
    assert!(body.contains("recorded=true"), "{body}");

    // A second answer arrives after the decision. It is told so rather than
    // quietly recorded, which is the difference between an honest page and one
    // that lets somebody believe they changed the outcome.
    let (status, second) = post(&url, "approve", "note=too+late").await;
    assert_eq!(status, 200, "{second}");
    assert!(second.contains("recorded=false"), "{second}");

    assert_eq!(
        active.wait().await.unwrap(),
        Outcome::Completed("approved:looks fine".to_owned()),
        "the first answer is the one the session ends with"
    );
}

/// A watch panel has nothing to submit, and the delivery facts are the whole
/// of what there is to report about it.
#[tokio::test]
async fn a_surface_with_no_submission_still_reports_its_delivery() {
    let _root = config_root();

    let frontend = UiFrontend::builtin("inspector", "table_inspect", "1");
    let active = UiSurface::<()>::new("inspector", "table_inspect")
        .unwrap()
        .with_page_policy(UiPagePolicy::new(UiPageScript::None))
        .start(
            UiDeliveryPlan::resolve(Some(UiDeliveryMode::Session)).unwrap(),
            PAGE,
            Vec::new(),
            &frontend,
        )
        .await
        .unwrap();

    let facts = active.facts();
    assert_eq!(facts.mode, UiDeliveryMode::Session);
    assert_eq!(facts.session_id, active.metadata().session_id.to_string());
    assert!(facts.link_url_secret.is_none());

    // Ending it from outside is the same ending a closed window is.
    let url = agent_first_ui::find_session(&facts.session_id)
        .unwrap()
        .expect("the surface is announced")
        .access_url_secret;
    let (status, body) = post(&url, agent_first_ui::SESSION_END_PATH, "").await;
    assert_eq!(status, 204, "{body}");
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), active.wait())
            .await
            .expect("the end route wakes the waiter")
            .unwrap(),
        Outcome::Closed
    );
}

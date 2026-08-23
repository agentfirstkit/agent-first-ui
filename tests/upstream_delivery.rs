//! The same AFUI delivery executor applied to a page another listener serves.

#![cfg(feature = "session")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::OnceLock;
use std::time::Duration;

use agent_first_ui::{CONFIG_DIR_ENV, NO_REGISTRY_ENV, UiDeliveryMode, UiDeliveryPlan, UiUpstream};
use axum::Router;

fn config_root() -> &'static tempfile::TempDir {
    static ROOT: OnceLock<tempfile::TempDir> = OnceLock::new();
    ROOT.get_or_init(|| {
        let root = tempfile::tempdir().expect("temporary AFUI config root");
        std::fs::write(
            root.path().join("config.json"),
            br#"{"attention":{"idle_timeout_s":7,"grace_period_s":3}}"#,
        )
        .expect("write AFUI attention config");
        // SAFETY: initialized once before any test in this binary creates a
        // delivery; every test enters through this helper.
        unsafe {
            std::env::set_var(CONFIG_DIR_ENV, root.path());
            std::env::set_var(NO_REGISTRY_ENV, "1");
        }
        root
    })
}

async fn upstream_url(secret: &'static str) -> String {
    let router = Router::new().fallback(|| async { "upstream" });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ignored = axum::serve(listener, router).await;
    });
    format!("http://{address}/panel?handoff_secret={secret}")
}

#[tokio::test]
async fn upstream_link_returns_only_the_afui_shell_url_and_uses_global_attention() {
    config_root();
    let source_secret = "source-must-stay-behind-the-proxy";
    let source_url = upstream_url(source_secret).await;
    let active = UiDeliveryPlan::resolve_allowing_link(Some(UiDeliveryMode::Link))
        .unwrap()
        .start_upstream(
            UiUpstream::new("inspector", "takeover", &source_url)
                .unwrap()
                .with_subject("challenge"),
        )
        .await
        .unwrap();

    assert_eq!(active.mode(), UiDeliveryMode::Link);
    assert_eq!(active.metadata().provider_id, "inspector");
    let link_url = active.link_url_secret().expect("AFUI Link URL");
    assert!(link_url.starts_with("http://"), "{link_url}");
    assert!(!link_url.contains(source_secret), "{link_url}");
    let policy = active.attention_policy().expect("attention policy");
    assert_eq!(policy.idle_timeout(), Some(Duration::from_secs(7)));
    assert_eq!(policy.grace_period(), Duration::from_secs(3));
}

#[tokio::test]
async fn upstream_session_has_no_afui_delivery_deadline() {
    config_root();
    let source_url = upstream_url("session-source").await;
    let active = UiDeliveryPlan::resolve(Some(UiDeliveryMode::Session))
        .unwrap()
        .start_upstream(UiUpstream::new("inspector", "takeover", source_url).unwrap())
        .await
        .unwrap();

    assert!(active.link_url_secret().is_none());
    assert!(active.attention_policy().is_none());
    assert!(
        tokio::time::timeout(Duration::from_millis(50), active.wait())
            .await
            .is_err(),
        "registered-only delivery must be bounded by its Provider, not an AFUI clock"
    );
}

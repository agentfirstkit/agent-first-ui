//! The three deliveries collapsed into one call (`UiDeliveryPlan::deliver`),
//! exercised end to end for the two modes that do not need a browser.
//!
//! `link` uses the same AFUI-owned shell, proxy isolation, and attention clock
//! as `afui session serve`, but it fixes that shell to the one session this
//! delivery created. `session` delivery is the other half of the same call: it
//! registers and binds loopback exactly as `window` does, and simply never
//! opens a browser or advertises an address.
//!
//! `window`'s priority over the environment is covered in
//! `delivery_priority.rs`, kept in its own test binary because it sets
//! process-global environment variables this file must not share a process
//! with.

#![cfg(feature = "session")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::OnceLock;
use std::time::Duration;

use agent_first_ui::{
    CONFIG_DIR_ENV, NO_REGISTRY_ENV, Outcome, UiDeliveryMode, UiDeliveryPlan, UiSession,
};
use axum::Router;

fn config_root() -> &'static tempfile::TempDir {
    static ROOT: OnceLock<tempfile::TempDir> = OnceLock::new();
    ROOT.get_or_init(|| {
        let root = tempfile::tempdir().expect("temporary AFUI config root");
        std::fs::write(
            root.path().join("config.json"),
            br#"{"attention":{"idle_timeout_s":1,"grace_period_s":1}}"#,
        )
        .expect("write AFUI attention config");
        // SAFETY: initialized once before any test in this binary creates an
        // AFUI delivery; every test enters through this helper.
        unsafe {
            std::env::set_var(CONFIG_DIR_ENV, root.path());
            std::env::set_var(NO_REGISTRY_ENV, "1");
        }
        root
    })
}

#[tokio::test]
async fn link_delivery_completes_a_session_with_no_cli_or_serve_feature_involved() {
    config_root();
    let session = UiSession::<u8>::new("console", "terminal").unwrap();
    let completion = session.completion();

    let plan = UiDeliveryPlan::resolve_allowing_link(Some(UiDeliveryMode::Link)).unwrap();

    // `deliver` does everything `link` needs on its own: find a LAN-reachable
    // address, bind it, serve the router, and wait. None of that is behind
    // `serve`, and nothing here calls into `bin/afui.rs`.
    let delivering = tokio::spawn(plan.deliver(session, Router::new()));
    // Give `serve` a moment to bind before finishing the session, so this
    // exercises the same "bind, then wait for a person" path a real answer
    // would arrive through, not just a future that resolves instantly.
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(completion.complete(7).await);

    let outcome = delivering.await.unwrap().unwrap();
    assert_eq!(outcome, Outcome::Completed(7));
}

/// The refusal arrives from `resolve`, not from `deliver`: a Provider that
/// announces its resolved mode before delivering — which is when the mode is
/// interesting — must never get the chance to announce one that cannot run.
#[test]
fn link_delivery_is_refused_at_resolve_with_the_modes_that_are_on_offer() {
    config_root();
    let error = UiDeliveryPlan::resolve(Some(UiDeliveryMode::Link)).unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains("`link`"),
        "the refusal must say which delivery was asked for: {message}"
    );
    assert!(
        message.contains("`window`") && message.contains("`session`"),
        "the refusal must list what this Provider does offer: {message}"
    );
}

#[tokio::test]
async fn unattended_link_uses_the_global_attention_policy_and_expires_the_delivery() {
    config_root();
    let session = UiSession::<()>::new("console", "terminal").unwrap();
    let active = UiDeliveryPlan::resolve_allowing_link(Some(UiDeliveryMode::Link))
        .unwrap()
        .start(session, Router::new())
        .await
        .unwrap();
    let policy = active.attention_policy().expect("link attention policy");
    assert_eq!(policy.idle_timeout(), Some(Duration::from_secs(1)));
    assert_eq!(policy.grace_period(), Duration::from_secs(1));

    // The same three facts a readiness event carries, read once rather than
    // reassembled from the accessors above.
    let facts = active.facts();
    assert_eq!(facts.mode, UiDeliveryMode::Link);
    assert_eq!(facts.idle_timeout_s, Some(1));
    assert_eq!(facts.grace_period_s, Some(1));
    assert_eq!(
        facts.link_url_secret.as_deref(),
        active.link_url_secret(),
        "the URL a person is handed is the one the facts report"
    );

    let outcome = tokio::time::timeout(Duration::from_secs(4), active.wait())
        .await
        .expect("attention deadline")
        .unwrap();
    assert_eq!(outcome, Outcome::Expired);
}

#[tokio::test]
async fn session_delivery_registers_and_waits_without_opening_a_window_or_a_link() {
    config_root();
    let session = UiSession::<u8>::new("console", "terminal").unwrap();
    let completion = session.completion();

    let plan = UiDeliveryPlan::resolve(Some(UiDeliveryMode::Session)).unwrap();
    let delivering = tokio::spawn(plan.deliver(session, Router::new()));
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(completion.complete(3).await);

    let outcome = delivering.await.unwrap().unwrap();
    assert_eq!(outcome, Outcome::Completed(3));
}

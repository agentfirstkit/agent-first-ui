//! A live session announces itself, so another process can find it.
//!
//! In its own test binary because it sets `AFUI_CONFIG_DIR`, which is process
//! global — sharing a binary with other tests would let them race over it.

// Every session affordance this drives is behind `session`, so a guard-tier
// build has nothing here to compile.
#![cfg(feature = "session")]

use agent_first_ui::{UiExpiry, UiHost, UiSession, list_sessions};
use axum::Router;

#[tokio::test]
async fn a_session_is_announced_while_it_runs_and_withdrawn_when_it_ends() {
    let root = tempfile::tempdir().unwrap();
    // SAFETY: this binary contains exactly one test, so nothing races here.
    unsafe { std::env::set_var("AFUI_CONFIG_DIR", root.path()) };

    assert!(list_sessions().unwrap().is_empty());

    let host = UiHost::bind_local().await.unwrap();
    let orders = host
        .attach(
            UiSession::<()>::new("inspector", "table_inspect")
                .unwrap()
                .with_subject("public.orders"),
            Router::new(),
            UiExpiry::Never,
        )
        .unwrap();
    let review = host
        .attach(
            UiSession::<()>::new("reviewer", "case_review")
                .unwrap()
                .with_subject("case-42"),
            Router::new(),
            UiExpiry::Never,
        )
        .unwrap();

    let listed = list_sessions().unwrap();
    assert_eq!(listed.len(), 2, "{listed:#?}");
    // Announcing is automatic. A Provider that had to opt in would be a
    // Provider that could forget, and `afui session list` would quietly
    // under-report rather than fail.
    let review_entry = listed
        .iter()
        .find(|entry| entry.provider_id == "reviewer")
        .unwrap();
    assert_eq!(review_entry.ui_kind, "case_review");
    assert_eq!(review_entry.subject.as_deref(), Some("case-42"));
    assert_eq!(
        review_entry.access_url_secret,
        review.access_url(),
        "the announced URL must be the one that actually serves the session"
    );
    assert_eq!(review_entry.owner_pid, std::process::id());

    // Two sessions of different kinds, one host, one port.
    let ports: Vec<_> = listed
        .iter()
        .map(|entry| {
            entry
                .access_url_secret
                .rsplit_once(':')
                .and_then(|(_, tail)| tail.split('/').next().map(str::to_owned))
        })
        .collect();
    assert_eq!(
        ports[0], ports[1],
        "sessions did not share a port: {ports:?}"
    );

    drop(orders);
    let remaining = list_sessions().unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].provider_id, "reviewer");

    drop(review);
    assert!(list_sessions().unwrap().is_empty());
}

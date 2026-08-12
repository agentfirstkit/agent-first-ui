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

    // The order is the order things happened, and it has to survive a name
    // that sorts the other way. `inspector` precedes `reviewer` in the
    // alphabet and was announced first, so a third session whose name sorts
    // before both of them still belongs last — it is the one the person asked
    // for while the other two were already open.
    //
    // This is the whole reason the entry carries a timestamp: a person who
    // opens a terminal, works in it, and asks for a second interface expects
    // that one below the terminal, not filed wherever its Provider's name falls
    // in an alphabet they never think about.
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let latecomer = host
        .attach(
            UiSession::<()>::new("aardvark", "panel").unwrap(),
            Router::new(),
            UiExpiry::Never,
        )
        .unwrap();
    let ordered: Vec<&str> = list_sessions()
        .unwrap()
        .iter()
        .map(|entry| entry.provider_id.as_str())
        .map(|name| Box::leak(name.to_owned().into_boxed_str()) as &str)
        .collect();
    assert_eq!(
        ordered,
        vec!["inspector", "reviewer", "aardvark"],
        "sessions must be listed in the order they were announced"
    );

    // And that order does not depend on which build of this crate wrote the
    // entry. Nothing about the time is in the file — it is the file's own
    // modification time — so a Provider pinned to an older version announces
    // an entry that sorts exactly like everyone else's rather than sorting
    // first forever.
    let written = std::fs::read_to_string(
        agent_first_ui::sessions_directory()
            .unwrap()
            .join(format!("{}.json", listed[0].session_id)),
    )
    .unwrap();
    assert!(
        !written.contains("announced_at"),
        "the announcement time must not be a stored field: {written}"
    );
    assert!(
        list_sessions()
            .unwrap()
            .iter()
            .all(|entry| entry.announced_at_epoch_ms > 0),
        "every entry still reports when it was announced"
    );
    drop(latecomer);

    drop(orders);
    let remaining = list_sessions().unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].provider_id, "reviewer");

    drop(review);
    assert!(list_sessions().unwrap().is_empty());
}

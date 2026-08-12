//! Whether a person is being waited on, as a fact the list can report.
//!
//! `afui session list` used to be unable to tell a stopped agent from an open
//! monitor (design notes §1.4): both looked like an entry with a
//! `provider_id` and a `ui_kind`. `waits_for_decision` is the two-source OR
//! this drives — `T != ()` automatically, `UiSession::awaiting_person` for
//! the `T = ()` session that is still, unmistakably, waited on — and
//! `announced_at_epoch_ms` is the ordering that makes "what has waited
//! longest" answerable once several entries share that fact.
//!
//! In its own test binary because it sets `AFUI_CONFIG_DIR`, which is process
//! global — sharing a binary with other tests would let them race over it.

// Every session affordance this drives is behind `session`, so a guard-tier
// build has nothing here to compile.
#![cfg(feature = "session")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use agent_first_ui::{UiExpiry, UiHost, UiSession, list_sessions};
use axum::Router;

#[tokio::test]
async fn waits_for_decision_is_the_or_of_its_two_sources_and_entries_still_sort_by_announcement() {
    let root = tempfile::tempdir().unwrap();
    // SAFETY: this binary contains exactly one test, so nothing races here.
    unsafe { std::env::set_var("AFUI_CONFIG_DIR", root.path()) };

    let host = UiHost::bind_local().await.unwrap();

    // Automatic: a session whose completion type is not `()` hands a value
    // back, which is by definition waiting on whoever supplies it.
    let decides = host
        .attach(
            UiSession::<String>::new("reviewer", "case_review").unwrap(),
            Router::new(),
            UiExpiry::Never,
        )
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(5)).await;

    // Explicit: `T = ()`, nothing is ever handed back, but a person is
    // unmistakably being waited on — the shape design notes §9 found: several
    // unrelated items settled one at a time, each written to disk as it is
    // decided, with the window closing the only thing that ends it.
    let waits_but_watches = host
        .attach(
            UiSession::<()>::new("reviewer", "queue_settle")
                .unwrap()
                .awaiting_person(),
            Router::new(),
            UiExpiry::Never,
        )
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(5)).await;

    // Neither source: `T = ()` and nobody declared it. A monitor, not a
    // stopped agent.
    let just_watched = host
        .attach(
            UiSession::<()>::new("console", "terminal").unwrap(),
            Router::new(),
            UiExpiry::Never,
        )
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(5)).await;

    // A second entry of the exact same kind as the first, so nothing but
    // `announced_at_epoch_ms` can tell them apart: two same-`ui_kind`
    // sessions that both wait for a decision must still sort in the order
    // they were announced.
    let second_review = host
        .attach(
            UiSession::<String>::new("reviewer", "case_review").unwrap(),
            Router::new(),
            UiExpiry::Never,
        )
        .unwrap();

    let listed = list_sessions().unwrap();
    assert_eq!(listed.len(), 4, "{listed:#?}");

    let by_id = |id: &agent_first_ui::UiSessionId| {
        listed
            .iter()
            .find(|entry| entry.session_id == id.as_str())
            .unwrap()
    };
    assert!(by_id(&decides.metadata().session_id).waits_for_decision);
    assert!(by_id(&waits_but_watches.metadata().session_id).waits_for_decision);
    assert!(!by_id(&just_watched.metadata().session_id).waits_for_decision);
    assert!(by_id(&second_review.metadata().session_id).waits_for_decision);

    // The order things were announced in, which `list_sessions` already
    // returns sorted by and `announced_at_epoch_ms` exposes on each entry.
    let ordered: Vec<&str> = listed
        .iter()
        .map(|entry| entry.session_id.as_str())
        .collect();
    assert_eq!(
        ordered,
        vec![
            decides.metadata().session_id.as_str(),
            waits_but_watches.metadata().session_id.as_str(),
            just_watched.metadata().session_id.as_str(),
            second_review.metadata().session_id.as_str(),
        ],
        "entries must sort by when they were announced, not by kind or provider name"
    );
    assert!(
        listed.iter().all(|entry| entry.announced_at_epoch_ms > 0),
        "every entry reports when it was announced"
    );
    let timestamps: Vec<u64> = listed
        .iter()
        .map(|entry| entry.announced_at_epoch_ms)
        .collect();
    let mut sorted_timestamps = timestamps.clone();
    sorted_timestamps.sort_unstable();
    assert_eq!(
        timestamps, sorted_timestamps,
        "the listing order and the announcement-time order must agree: {timestamps:?}"
    );

    drop(decides);
    drop(waits_but_watches);
    drop(just_watched);
    drop(second_review);
}

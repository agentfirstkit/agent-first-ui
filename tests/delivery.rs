//! The three deliveries collapsed into one call (`UiDeliveryPlan::deliver`),
//! exercised end to end for the two modes that do not need a browser.
//!
//! `link` mattering here, under `session` and not `serve`, is the whole point
//! of it existing as a library feature rather than a CLI verb: a Provider
//! that never links against `agent-first-ui/serve` — never builds the `afui`
//! binary's reverse proxy, never runs `afui session serve` — still gets a URL
//! it can hand to a person on another device. `session` delivery is the other
//! half of the same call: it registers and binds loopback exactly as
//! `window` does, and simply never opens a browser or advertises an address.
//!
//! `window`'s priority over the environment is covered in
//! `delivery_priority.rs`, kept in its own test binary because it sets
//! process-global environment variables this file must not share a process
//! with.

#![cfg(feature = "session")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use agent_first_ui::{Outcome, UiDeliveryMode, UiDeliveryPlan, UiExpiry, UiSession};
use axum::Router;

#[tokio::test]
async fn link_delivery_completes_a_session_with_no_cli_or_serve_feature_involved() {
    let session = UiSession::<u8>::new("console", "terminal").unwrap();
    let completion = session.completion();

    let plan = UiDeliveryPlan::resolve(Some(UiDeliveryMode::Link))
        .unwrap()
        .with_link_expiry(UiExpiry::After(Duration::from_secs(60)));

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

#[tokio::test]
async fn link_delivery_is_refused_with_a_clear_reason_when_never_declared() {
    let session = UiSession::<()>::new("console", "terminal").unwrap();
    // No `with_link_expiry` call: this Provider never opted into `link`.
    let plan = UiDeliveryPlan::resolve(Some(UiDeliveryMode::Link)).unwrap();
    let error = plan.deliver(session, Router::new()).await.unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains("link"),
        "the refusal must say which delivery is missing: {message}"
    );
    assert!(
        message.contains("with_link_expiry"),
        "the refusal must say how a Provider opts in: {message}"
    );
}

#[tokio::test]
async fn session_delivery_registers_and_waits_without_opening_a_window_or_a_link() {
    let session = UiSession::<u8>::new("console", "terminal").unwrap();
    let completion = session.completion();

    let plan = UiDeliveryPlan::resolve(Some(UiDeliveryMode::Session)).unwrap();
    let delivering = tokio::spawn(plan.deliver(session, Router::new()));
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(completion.complete(3).await);

    let outcome = delivering.await.unwrap().unwrap();
    assert_eq!(outcome, Outcome::Completed(3));
}

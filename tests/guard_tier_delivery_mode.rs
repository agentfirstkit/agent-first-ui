//! The three delivery words are reachable in a build with no session at all.
//!
//! Deliberately ungated. A Provider whose human UI is an optional feature
//! still parses a `--mode` flag and still reports which delivery a run used in
//! the build where that UI is compiled out; if the word list needed the
//! executor, that Provider would have to keep a second enum of its own, and
//! the two would drift the first time a word changed. This file fails to
//! compile — under `--no-default-features` — the moment the words move back
//! behind a feature.
//!
//! Nothing here reads `AFUI_DELIVERY`: that variable is process-global and
//! belongs to `tests/delivery_priority.rs`, which owns it. What this pins is
//! the tier, not the priority.

use agent_first_ui::{DELIVERY_ENV, UiDeliveryMode};

#[test]
fn the_word_list_parses_and_spells_itself_without_a_session() {
    for mode in [
        UiDeliveryMode::Window,
        UiDeliveryMode::Link,
        UiDeliveryMode::Session,
    ] {
        assert_eq!(UiDeliveryMode::parse(mode.as_str()).unwrap(), mode);
        assert!(!mode.description().is_empty());
    }
    assert!(UiDeliveryMode::parse("lan").is_err());
}

#[test]
fn an_explicit_mode_resolves_without_a_session() {
    assert_eq!(
        UiDeliveryMode::resolve(Some(UiDeliveryMode::Session)).unwrap(),
        UiDeliveryMode::Session
    );
}

/// The variable's name is part of the same word list: a Provider that
/// documents it, or a test harness that sets it, needs the constant rather
/// than a string literal of its own.
#[test]
fn the_environment_variable_name_is_a_constant_not_a_literal() {
    assert_eq!(DELIVERY_ENV, "AFUI_DELIVERY");
}

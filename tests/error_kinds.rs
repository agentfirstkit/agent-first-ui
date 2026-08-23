//! The classification codes Providers build their own error codes from.
//!
//! `UiErrorKind` is closed, so the compiler already stops a Provider from
//! missing a classification. What the compiler cannot check is the other two
//! halves of the promise: that each one is spelled the way `docs/providers.md`
//! says, and that each one is actually *reachable* — a documented code no
//! error ever produces is a row Providers write an arm for and never hit.
//!
//! Deliberately ungated, so the guard tier — where the frontend and page
//! errors a template loader actually sees are raised — is covered too.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeSet;

use agent_first_ui::{Error, SAFE_MODE_HINT, UiErrorKind};

/// Every word `UiErrorKind` may be spelled as. Sorted, so a new entry is a
/// visible line in a diff rather than an append nobody reads.
const KINDS: &[&str] = &[
    "config_unreadable",
    "delivery_mode_invalid",
    "delivery_mode_not_offered",
    "frontend_incompatible",
    "frontend_unreadable",
    "frontend_unsafe",
    "invalid_argument",
    "io",
    "link_address_unavailable",
    "page_incomplete",
    "page_render",
    "runtime_blob",
    "runtime_busy",
    "runtime_closed",
    "runtime_message_too_large",
    "runtime_misconfigured",
    "runtime_payload",
    "upstream_not_proxyable",
    "window_unavailable",
    "window_wait_failed",
];

/// One error per classification, so every code in the list above is reachable
/// and every reachable code is in the list.
fn one_of_each() -> Vec<Error> {
    vec![
        Error::InvalidTtl,
        Error::InvalidDeliveryMode {
            value: "lan".to_owned(),
        },
        Error::DeliveryModeNotOffered {
            requested: agent_first_ui::UiDeliveryMode::Link,
        },
        Error::UpstreamDeliveryRequiresHttp { mode: "link" },
        Error::LinkAddressUnavailable(std::io::Error::other("no address")),
        Error::FrontendManifest {
            message: "bad json".to_owned(),
        },
        Error::FrontendIncompatible {
            provider_id: "reviewer".to_owned(),
            ui_kind: "console".to_owned(),
            frontend_id: "custom".to_owned(),
            declared: "2".to_owned(),
            implemented: "1".to_owned(),
        },
        Error::FrontendScript {
            label: "page.html.j2".to_owned(),
            violation: "a <script> element",
        },
        Error::PageRender {
            frontend_id: Some("custom".to_owned()),
            template: "page.html.j2".to_owned(),
            detail: "undefined value".to_owned(),
        },
        Error::PageIncomplete {
            frontend_id: Some("custom".to_owned()),
            what: "no element with id `status`".to_owned(),
        },
        Error::SessionRuntimeBlobsDisabled,
        Error::SessionRuntimeBusy,
        Error::SessionRuntimeClosed,
        Error::SessionRuntimeMessageTooLarge {
            size_bytes: 2,
            max_bytes: 1,
        },
        Error::InvalidSessionRuntimeCapacity,
        Error::SessionRuntimeDecode(serde_json::from_str::<serde_json::Value>("{").unwrap_err()),
        Error::ConfigDirectoryUnavailable,
        Error::WindowBinaryNotFound,
        Error::WindowWait(std::io::Error::other("wait")),
        Error::Io(std::io::Error::other("disk")),
    ]
}

#[test]
fn every_classification_is_snake_case_and_carries_no_domain_word() {
    for kind in UiErrorKind::ALL.iter().map(|kind| kind.as_str()) {
        assert!(
            kind.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
            "{kind} is not snake_case"
        );
        // A code that named a consumer's domain would be a code every other
        // consumer has to translate away.
        for domain in ["review", "panel", "payment", "terminal", "mail", "sql"] {
            assert!(!kind.contains(domain), "{kind} names a domain: {domain}");
        }
    }
    let unique: BTreeSet<_> = KINDS.iter().collect();
    assert_eq!(unique.len(), KINDS.len(), "duplicate classification");
    let mut sorted = KINDS.to_vec();
    sorted.sort_unstable();
    assert_eq!(sorted, KINDS, "keep the list sorted");
}

/// The written-down set and the closed enum are the same set, in the same
/// order. Renaming a classification, or adding one without documenting it,
/// stops here rather than in a consumer's output.
#[test]
fn the_documented_set_is_exactly_what_the_closed_axis_spells() {
    let spelled: Vec<&str> = UiErrorKind::ALL
        .iter()
        .map(|kind| kind.as_str())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    assert_eq!(
        spelled, KINDS,
        "the documented set and `UiErrorKind::ALL` differ"
    );
    assert_eq!(
        UiErrorKind::ALL.len(),
        KINDS.len(),
        "two classifications spell the same word"
    );
}

/// Every classification is one some error actually raises. A row nobody can
/// reach is a row a Provider writes an arm for and never runs.
#[test]
fn every_classification_is_reachable_from_a_real_error() {
    let reached: BTreeSet<UiErrorKind> = one_of_each().iter().map(Error::kind).collect();
    let missing: Vec<&str> = UiErrorKind::ALL
        .iter()
        .filter(|kind| !reached.contains(kind))
        .map(|kind| kind.as_str())
        .collect();
    assert!(missing.is_empty(), "no error produces {missing:?}");
}

/// The whole reason the hint left `Display`: a caller with a hint field of its
/// own was printing the same sentence twice, and the only way out was a second
/// conversion path that stripped it back off.
#[test]
fn a_recovery_action_appears_beside_the_message_and_never_inside_it() {
    let error = Error::PageIncomplete {
        frontend_id: Some("custom".to_owned()),
        what: "no element with id `status`".to_owned(),
    };
    assert_eq!(error.hint(), Some(SAFE_MODE_HINT));
    assert!(
        !error.to_string().contains(SAFE_MODE_HINT),
        "the hint must not also be in the message: {error}"
    );
}

/// Safe mode turns off an override. A built-in page that fails to render is
/// the Provider's own bug, and suggesting safe mode for it would send a person
/// to disable something they never installed.
#[test]
fn only_a_failure_an_override_could_have_caused_suggests_turning_overrides_off() {
    let from_override = Error::PageRender {
        frontend_id: Some("custom".to_owned()),
        template: "page.html.j2".to_owned(),
        detail: "undefined value".to_owned(),
    };
    let from_builtin = Error::PageRender {
        frontend_id: None,
        template: "page.html.j2".to_owned(),
        detail: "undefined value".to_owned(),
    };
    assert_eq!(from_override.hint(), Some(SAFE_MODE_HINT));
    assert_eq!(from_builtin.hint(), None);
    // Same classification either way: what failed is the same thing, and a
    // Provider's own error code should not fork on whose fault it was.
    assert_eq!(from_override.kind(), from_builtin.kind());
    // And the classification is what a Provider prefixes, so it has to render
    // as its own word and nothing else.
    assert_eq!(
        format!("myprovider_{}", from_override.kind()),
        "myprovider_page_render"
    );
}

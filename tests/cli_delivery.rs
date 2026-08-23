//! The command-line half of a delivery, held to the two rules Providers kept
//! getting wrong when each of them owned a copy.
//!
//! Both failures below were real, in shipped code, found by reading six
//! implementations side by side: a flag whose values disagreed with the plan
//! behind it, and a readiness event that masked the one URL it existed to
//! hand over.

#![cfg(feature = "session")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use agent_first_ui::cli::{
    LINK_URL_FIELD, UiDeliveryOffer, ready_event, ready_event_revealing_link,
};
use agent_first_ui::{UiDeliveryFacts, UiDeliveryMode};
use serde_json::json;

fn facts(mode: UiDeliveryMode, link: Option<&str>) -> UiDeliveryFacts {
    UiDeliveryFacts {
        mode,
        session_id: "0123456789abcdef".to_owned(),
        link_url_secret: link.map(str::to_owned),
        idle_timeout_s: link.map(|_| 900),
        grace_period_s: link.map(|_| 300),
    }
}

/// A command cannot offer a word it will not honour, because the value that
/// declares the flag is the value that builds the plan.
#[test]
fn the_flag_and_the_plan_cannot_disagree() {
    for offer in [UiDeliveryOffer::Local, UiDeliveryOffer::WithLink] {
        for word in offer.words() {
            let mode = UiDeliveryMode::parse(word).expect("an offered word is a delivery word");
            assert!(
                offer.resolve(Some(mode)).is_ok(),
                "{offer:?} offers {word} and refuses it"
            );
        }
    }

    // And the reverse: a word left out of the flag is refused rather than
    // quietly honoured, so the enum in `--help` is the whole truth.
    assert!(!UiDeliveryOffer::Local.words().contains(&"link"));
    assert!(
        UiDeliveryOffer::Local
            .resolve(Some(UiDeliveryMode::Link))
            .is_err()
    );
}

/// The flag never carries a default, whatever the offer.
///
/// A default is indistinguishable from a person typing `--mode window`, which
/// is how an unattended machine ends up trying to open one.
#[test]
fn the_flag_leaves_absent_meaning_absent() {
    for offer in [UiDeliveryOffer::Local, UiDeliveryOffer::WithLink] {
        // The built spec, not the builder call: what matters is that nothing
        // downstream ever sees a value where the person supplied none.
        let rendered = format!("{:?}", offer.arg("--mode"));
        assert!(
            rendered.contains("default: None"),
            "{offer:?} gave the delivery flag a default: {rendered}"
        );
    }
}

/// The words in the flag are AFUI's, and the help text says what each means
/// without a Provider restating it.
#[test]
fn the_flag_describes_the_words_it_offers_and_no_others() {
    let rendered = format!("{:?}", UiDeliveryOffer::Local.arg("--mode"));
    assert!(rendered.contains("window"), "{rendered}");
    assert!(rendered.contains("session"), "{rendered}");
    assert!(!rendered.contains("link URL"), "{rendered}");

    let rendered = format!("{:?}", UiDeliveryOffer::WithLink.arg("--panel-mode"));
    assert!(rendered.contains("--panel-mode"), "{rendered}");
    assert!(rendered.contains("link"), "{rendered}");
    // The environment fallback is part of what the flag means.
    assert!(
        rendered.contains(agent_first_ui::DELIVERY_ENV),
        "{rendered}"
    );
}

/// The one event whose job is to hand over a URL hands over a URL — through
/// the Provider's ordinary emitter, with its ordinary redaction.
///
/// AFDATA masks `_secret`-suffixed fields, which is right everywhere else. A
/// Provider that merged the facts and emitted them published
/// `link_url_secret: "***"` beside a sentence saying the page was reachable
/// at it. So the URL is published under a name that is not a secret's name,
/// and no caller has to remember to disable anything.
#[test]
fn a_link_readiness_event_keeps_the_url_a_person_must_open() {
    let facts = facts(UiDeliveryMode::Link, Some("http://192.168.1.9:8080/abc/"));
    let event = ready_event_revealing_link(
        &facts,
        json!({"phase": "ui_ready", "subject": "the thing", "api_token_secret": "hunter2"}),
    );

    assert_eq!(event[LINK_URL_FIELD], "http://192.168.1.9:8080/abc/");
    // The suffixed name never reaches the wire, so nothing downstream can
    // mask the one value a person needs.
    assert!(event.get("link_url_secret").is_none(), "{event}");
    assert_eq!(event["subject"], "the thing");
    assert_eq!(event["mode"], "link");
    assert_eq!(event["session_id"], "0123456789abcdef");
    assert_eq!(event["idle_timeout_s"], 900);

    // And it survives the redaction a Provider's emitter applies, which is
    // the property the previous shape did not have.
    let emitted = agent_first_data::Redactor::new().value(&event);
    assert_eq!(emitted[LINK_URL_FIELD], "http://192.168.1.9:8080/abc/");
    assert_ne!(emitted["api_token_secret"], "hunter2");
}

/// A delivery with no URL to reveal reveals nothing, and says so by leaving
/// the field out rather than by publishing an empty one.
#[test]
fn a_window_readiness_event_has_no_url_at_all() {
    let event = ready_event_revealing_link(&facts(UiDeliveryMode::Window, None), json!({}));
    assert!(event.get(LINK_URL_FIELD).is_none(), "{event}");
    assert!(event.get("link_url_secret").is_none(), "{event}");
    assert!(event.get("idle_timeout_s").is_none(), "{event}");
    assert_eq!(event["mode"], "window");
}

/// The merge keeps the Provider's own fields and adds the facts beside them.
#[test]
fn the_facts_join_the_providers_own_event_rather_than_replacing_it() {
    let event = ready_event(
        &facts(UiDeliveryMode::Session, None),
        json!({"phase": "ui_ready", "row_count": 12}),
    );
    assert_eq!(event["row_count"], 12);
    assert_eq!(event["phase"], "ui_ready");
    assert_eq!(event["mode"], "session");
}

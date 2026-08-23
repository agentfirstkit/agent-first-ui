//! The command-line half of handing a UI to a person.
//!
//! Everything here was written once per Provider before it was written here.
//! The words are AFUI's, the environment variable is AFUI's, the readiness
//! facts are AFUI's — but declaring the flag, reading it back, and getting the
//! facts into an event were each a small pile of Provider code, and six piles
//! of the same small code disagree the way six piles do: one spelled a typo as
//! `window`, one published a URL nobody could read.
//!
//! What stays the Provider's is what it actually knows: which deliveries this
//! particular command offers, what the flag is called when `--mode` is already
//! taken, and everything in its own readiness event that is not a delivery
//! fact.

use agent_first_data::{ArgSpec, CliValue, ResolvedInvocation};
#[cfg(feature = "session")]
use serde_json::{Map, Value};

use crate::Result;
use crate::delivery::UiDeliveryMode;
#[cfg(feature = "session")]
use crate::delivery::{UiDeliveryFacts, UiDeliveryPlan};

/// The field a link delivery's URL is published under.
///
/// Deliberately not `_secret`-suffixed, and that is the whole design. The
/// facts struct calls it [`UiDeliveryFacts::link_url_secret`] because code
/// holding that value is holding a capability — but the one event whose job
/// is to hand the URL to a person must not have it masked, and AFDATA masks
/// by suffix. A field that must always be shown should not wear the mark that
/// means "never show this".
///
/// The alternative — publish it suffixed, redact the event, then restore this
/// one field — was tried first, in three Providers and then here. It works
/// only if every caller emits through a non-redacting path, which is a
/// contract the caller cannot be stopped from breaking: the first migration
/// onto it broke exactly that way and published `***` again. This shape has
/// no such contract.
pub const LINK_URL_FIELD: &str = "link_url";

/// Which deliveries one command offers.
///
/// Not which words exist — that is [`UiDeliveryMode`], and it does not vary.
/// This is the Provider's own answer to "may this page be reachable from the
/// network at a URL that is itself the credential?", and it differs per
/// command rather than per Provider: the same binary can have a panel that
/// only shows something and a panel that authorizes something.
///
/// One value builds both the flag and the plan ([`Self::arg`],
/// [`Self::resolve`]) so the two cannot drift. Declaring `window|session` in
/// the parser while resolving with link enabled is a page that refuses at run
/// time a word its own `--help` offered; declaring the reverse is a word the
/// parser accepts and the runtime cannot honour. Both were possible before,
/// and one Provider carried a comment in the flag pointing at the file where
/// the other half lived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiDeliveryOffer {
    /// A window here, or a registered session. Nothing binds a network
    /// address, so nothing is reachable from off this machine without a
    /// person taking a further, explicit step.
    Local,
    /// The two above plus AFUI's own LAN link page.
    ///
    /// Choosing this is the opt-in to that exposure. What leaks if the URL
    /// does is whatever the page shows and whatever it can be told to do.
    WithLink,
}

impl UiDeliveryOffer {
    /// The words this offer accepts, in the order a person reads them.
    #[must_use]
    pub fn words(self) -> &'static [&'static str] {
        match self {
            Self::Local => &["window", "session"],
            Self::WithLink => &["window", "link", "session"],
        }
    }

    /// The flag, with the values this command actually honours.
    ///
    /// No default value, deliberately, and this is the reason the helper
    /// exists rather than a documented convention: a default here is
    /// indistinguishable from a person having typed `--mode window`, so
    /// [`crate::DELIVERY_ENV`] would never be consulted and an unattended run
    /// would always try to open a window. Every Provider wrote that comment;
    /// now the code enforces it.
    ///
    /// `flag` is a name rather than a constant because `--mode` is a popular
    /// word: Providers already use it for network exposure, for a REPL
    /// frontend, and for a psql wire mode. The `about` text is AFUI's summary
    /// of what each word means; a Provider with something domain-specific to
    /// add can call `.about(...)` on the result.
    #[must_use]
    pub fn arg(self, flag: &str) -> ArgSpec {
        ArgSpec::option_enum(flag, self.words().iter().copied())
            .value_name("MODE")
            .about(self.about())
    }

    fn about(self) -> String {
        let mut about = String::from("How this reaches the person:");
        for (index, mode) in self.modes().enumerate() {
            if index > 0 {
                about.push(';');
            }
            about.push_str(&format!(" {} {}", mode.as_str(), mode.description()));
        }
        about.push_str(&format!(
            ". Falls back to {}, then window",
            crate::DELIVERY_ENV
        ));
        about
    }

    fn modes(self) -> impl Iterator<Item = UiDeliveryMode> {
        self.words()
            .iter()
            .filter_map(|word| UiDeliveryMode::parse(word).ok())
    }

    /// The plan this offer resolves to, from the flag a person typed.
    ///
    /// The same value that produced the flag produces the plan, so a Provider
    /// cannot offer a word it will not honour.
    ///
    /// Behind `session` because a plan needs the executor; the flag above does
    /// not, and a Provider that declares its CLI in a build with the UI
    /// compiled out still needs to declare this flag.
    #[cfg(feature = "session")]
    pub fn resolve(self, explicit: Option<UiDeliveryMode>) -> Result<UiDeliveryPlan> {
        match self {
            Self::Local => UiDeliveryPlan::resolve(explicit),
            Self::WithLink => UiDeliveryPlan::resolve_allowing_link(explicit),
        }
    }
}

/// The delivery word a person typed, or `None` when they typed nothing.
///
/// Absent has to stay absent all the way to [`UiDeliveryOffer::resolve`]:
/// substituting a default here is the same mistake as putting one on the flag.
///
/// A value the parser already constrained to this command's enum cannot fail
/// to parse — but a Provider reaching this from somewhere other than a
/// resolved invocation can hand it anything, so the error is returned rather
/// than assumed away. The one Provider that assumed it away wrote
/// `.unwrap_or(Window)`, which is precisely the silent typo this refuses.
pub fn delivery_of(invocation: &ResolvedInvocation, flag: &str) -> Result<Option<UiDeliveryMode>> {
    let Some(value) = invocation.optional(flag).and_then(CliValue::as_str) else {
        return Ok(None);
    };
    UiDeliveryMode::parse(value).map(Some)
}

/// One readiness event: the Provider's own fields, the delivery facts, and the
/// link URL under a name that survives being emitted.
///
/// Emit it the ordinary way, through whatever redaction the Provider already
/// applies. Nothing here needs redaction turned off, and nothing else in the
/// event is exposed: a Provider's own secret-suffixed fields are masked by the
/// same emitter as always.
///
/// A delivery with no URL — every `window` and `session` one — publishes no
/// URL field at all rather than an empty one.
#[cfg(feature = "session")]
#[must_use]
pub fn ready_event_revealing_link(facts: &UiDeliveryFacts, fields: Value) -> Value {
    let mut event = ready_event(facts, fields);
    let Some(object) = event.as_object_mut() else {
        return event;
    };
    // The suffixed name never reaches the wire: it would be masked, and the
    // masked value is the bug this exists to prevent.
    if let Some(link_url) = object.remove("link_url_secret") {
        object.insert(LINK_URL_FIELD.to_string(), link_url);
    }
    event
}

/// The Provider's own fields with the delivery facts merged in, unredacted.
///
/// For a Provider that emits through its own redacting path and has no link
/// URL to hand over — a delivery that offers only [`UiDeliveryOffer::Local`]
/// never produces one.
#[cfg(feature = "session")]
#[must_use]
pub fn ready_event(facts: &UiDeliveryFacts, fields: Value) -> Value {
    let mut object = match fields {
        Value::Object(object) => object,
        other => {
            let mut object = Map::new();
            object.insert("event".to_string(), other);
            object
        }
    };
    if let Ok(Value::Object(delivery)) = serde_json::to_value(facts) {
        for (key, value) in delivery {
            object.insert(key, value);
        }
    }
    Value::Object(object)
}

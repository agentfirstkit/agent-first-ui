use serde::{Serialize, Serializer};

use crate::{Error, Result};

/// The default delivery mode when a Provider's own `--mode` flag was not
/// given. See [`UiDeliveryMode::resolve`] for the priority this participates
/// in.
pub const DELIVERY_ENV: &str = "AFUI_DELIVERY";

/// One of the three ways a session reaches a person.
///
/// A closed set on purpose: three words a Provider's own `--mode` flag can
/// reuse verbatim, and the same three words [`DELIVERY_ENV`] accepts. A
/// Provider that needs a second axis — which network a listener binds,
/// which of several tools to run — needs a second flag, not a fourth
/// variant here; reusing this one to mean two things at once is how a runner
/// ends up needing a compatibility matrix instead of a name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiDeliveryMode {
    /// A local, isolated browser window. Never expires: the window is the
    /// bound.
    Window,
    /// An AFUI-owned page another device can open directly. The Provider must
    /// opt into this network exposure by building its plan with
    /// [`crate::UiDeliveryPlan::resolve_allowing_link`], but never chooses its
    /// lifetime: AFUI applies the same attention policy and warning grace used
    /// by `afui session serve`.
    Link,
    /// Registered only, with no window opened and no address advertised.
    /// Reached later through `afui session open` or `afui session serve`,
    /// whose own attention policy bounds the view rather than the credential
    /// itself expiring.
    Session,
}

impl UiDeliveryMode {
    /// Parse one of the three delivery words.
    ///
    /// Anything else is an error naming all three, rather than a `None` that
    /// looks the same as "not specified" — a caller resolving `--mode`
    /// against this needs to tell "the person typed something wrong" from
    /// "the person typed nothing" apart.
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "window" => Ok(Self::Window),
            "link" => Ok(Self::Link),
            "session" => Ok(Self::Session),
            other => Err(Error::InvalidDeliveryMode {
                value: other.to_owned(),
            }),
        }
    }

    /// The word this mode is spelled with, which is the same word
    /// [`Self::parse`] reads.
    ///
    /// A Provider that reports which delivery a run used — in an event, a log
    /// line, a status field — says it with this rather than a word of its own,
    /// so the name a person types and the name they read back are one name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Window => "window",
            Self::Link => "link",
            Self::Session => "session",
        }
    }

    /// What actually happened, in one sentence a person can read.
    ///
    /// Purely factual, and deliberately not an instruction: whether the
    /// sentence belongs in a ready event, a log line, or nowhere is the
    /// Provider's call, and so is any domain wording around it. It exists
    /// because the alternative — every Provider writing its own three-armed
    /// match over the same three facts — is how "the window is open" and "a
    /// window was opened" end up meaning the same thing in two tools.
    #[must_use]
    pub fn description(self) -> &'static str {
        match self {
            Self::Window => "a browser window is open on this machine",
            Self::Link => "the page is reachable from this machine's network at the link URL",
            Self::Session => {
                "the session is registered only; open it with `afui session open` or reach it \
                 through `afui session serve`"
            }
        }
    }

    /// The two deliveries every Provider offers without opting into any
    /// network exposure — the set [`crate::UiDeliveryPlan::resolve`] resolves
    /// within. Named here so a refusal can list what *is* on offer using the
    /// same words a person types back.
    pub(crate) const LOCAL_ONLY: [Self; 2] = [Self::Window, Self::Session];

    /// [`DELIVERY_ENV`], read once: unset is `None`; set to one of the three
    /// words is `Some`; set to anything else — including an empty string — is
    /// an error.
    ///
    /// Deliberately not folded into a plain `Option<Self>` that also treats a
    /// bad value as absent. The reason this variable exists at all is to stop
    /// an unattended machine from popping a window nobody is there to see;
    /// silently defaulting a typo to `window` is exactly the failure this is
    /// supposed to prevent.
    pub fn from_env() -> Result<Option<Self>> {
        match std::env::var_os(DELIVERY_ENV) {
            None => Ok(None),
            Some(value) => Self::parse(&value.to_string_lossy()).map(Some),
        }
    }

    /// Which of the three deliveries a call uses, from the one flag and the
    /// one variable.
    ///
    /// Priority is fixed and the same for every Provider:
    /// `explicit > `[`DELIVERY_ENV`]` > window`.
    ///
    /// `explicit` is a person's own `--mode` flag, already parsed with
    /// [`Self::parse`] — pass `None` when the flag was not given. **Do not**
    /// give that flag a default value of its own: a default there is
    /// indistinguishable from a person having typed `--mode window`, so
    /// [`DELIVERY_ENV`] would never be consulted and an unattended run would
    /// always pop a window.
    ///
    /// [`crate::UiDeliveryPlan::resolve`] is this call plus the executor, and
    /// is what Providers normally want whether AFUI serves their router or
    /// they name an existing page with [`crate::UiUpstream`]. This lower-level
    /// call remains useful when the resolved word is needed before either
    /// source is ready — or in a build where the executor is compiled out
    /// altogether and the word is still on the command line.
    pub fn resolve(explicit: Option<Self>) -> Result<Self> {
        match explicit {
            Some(mode) => Ok(mode),
            None => Ok(Self::from_env()?.unwrap_or(Self::Window)),
        }
    }
}

/// Serialized as the word, so a Provider that puts a resolved mode straight
/// into an event gets the same spelling a person can type back — rather than
/// a variant name that only matches by accident.
impl Serialize for UiDeliveryMode {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::ENV_LOCK as TEST_LOCK;

    fn set_env(value: &str) {
        // SAFETY: serialised by `TEST_LOCK`.
        unsafe { std::env::set_var(DELIVERY_ENV, value) };
    }

    fn clear_env() {
        // SAFETY: serialised by `TEST_LOCK`.
        unsafe { std::env::remove_var(DELIVERY_ENV) };
    }

    #[test]
    fn parse_accepts_only_the_three_words() {
        assert_eq!(
            UiDeliveryMode::parse("window").unwrap(),
            UiDeliveryMode::Window
        );
        assert_eq!(UiDeliveryMode::parse("link").unwrap(), UiDeliveryMode::Link);
        assert_eq!(
            UiDeliveryMode::parse("session").unwrap(),
            UiDeliveryMode::Session
        );
        let error = UiDeliveryMode::parse("lan").unwrap_err().to_string();
        assert!(error.contains("window"), "{error}");
        assert!(error.contains("link"), "{error}");
        assert!(error.contains("session"), "{error}");
    }

    /// The word a Provider reports is the word a person can type back.
    #[test]
    fn every_mode_spells_itself_as_the_word_that_parses_to_it() {
        for mode in [
            UiDeliveryMode::Window,
            UiDeliveryMode::Link,
            UiDeliveryMode::Session,
        ] {
            assert_eq!(UiDeliveryMode::parse(mode.as_str()).unwrap(), mode);
        }
    }

    /// The one-line description is a fact, not a word a person types back:
    /// each mode has its own, and none of them is the bare mode word.
    #[test]
    fn every_mode_describes_itself_distinctly() {
        let mut seen = std::collections::HashSet::new();
        for mode in [
            UiDeliveryMode::Window,
            UiDeliveryMode::Link,
            UiDeliveryMode::Session,
        ] {
            let description = mode.description();
            assert!(!description.is_empty());
            assert!(seen.insert(description), "{description} repeated");
        }
    }

    /// A mode in an event is the same word a person types, not a variant name.
    #[test]
    fn a_mode_serializes_as_the_word_it_parses_from() {
        assert_eq!(
            serde_json::to_string(&UiDeliveryMode::Session).unwrap(),
            "\"session\""
        );
    }

    #[test]
    fn an_unset_env_var_resolves_to_none_not_an_error() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        clear_env();
        assert_eq!(UiDeliveryMode::from_env().unwrap(), None);
    }

    #[test]
    fn an_unrecognised_env_value_is_rejected_rather_than_silently_defaulted() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        set_env("lan");
        let error = UiDeliveryMode::from_env().unwrap_err().to_string();
        clear_env();
        // The one failure mode this variable exists to prevent: a typo must
        // not read the same as "unset", which resolves to `window`.
        assert!(error.contains("window"), "{error}");
        assert!(error.contains("link"), "{error}");
        assert!(error.contains("session"), "{error}");
    }

    #[test]
    fn an_empty_env_value_is_rejected_rather_than_treated_as_unset() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        set_env("");
        assert!(UiDeliveryMode::from_env().is_err());
        clear_env();
    }

    #[test]
    fn resolve_prefers_the_explicit_mode_over_the_environment() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        set_env("session");
        let mode = UiDeliveryMode::resolve(Some(UiDeliveryMode::Window)).unwrap();
        clear_env();
        assert_eq!(mode, UiDeliveryMode::Window);
    }

    #[test]
    fn resolve_falls_back_to_the_environment_when_nothing_explicit_was_given() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        set_env("session");
        let mode = UiDeliveryMode::resolve(None).unwrap();
        clear_env();
        assert_eq!(mode, UiDeliveryMode::Session);
    }

    #[test]
    fn resolve_defaults_to_window_when_nothing_was_said_at_all() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        clear_env();
        let mode = UiDeliveryMode::resolve(None).unwrap();
        assert_eq!(mode, UiDeliveryMode::Window);
    }

    #[test]
    fn resolve_propagates_an_unrecognised_environment_value() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        set_env("lan");
        let error = UiDeliveryMode::resolve(None).unwrap_err().to_string();
        clear_env();
        assert!(error.contains("window"), "{error}");
        assert!(error.contains("link"), "{error}");
        assert!(error.contains("session"), "{error}");
    }
}

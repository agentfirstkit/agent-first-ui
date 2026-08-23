//! The half of a decide page a frontend cannot write.
//!
//! A template declares what a control is *for* — `data-<provider>-decision="approve"`
//! — and the Provider decides what that does. Keeping the two apart is what
//! lets a person replace the entire page and still not be able to make a
//! button labelled "approve" do something else: the mapping from declaration
//! to route lives in a script served under a per-session nonce that no
//! frontend file can forge.
//!
//! What AFUI supplies here is the *building material* for that script, not the
//! script's authority. The rules below are the ones every decide page in this
//! kit needs and every one of them was writing again: match the declaration
//! exactly, bind an unrecognised declaration to nothing, submit at most once,
//! disable every control the moment one is pressed, and travel as a form
//! navigation rather than a fetch. The binding still happens inside the
//! Provider's own trusted runtime — AFUI only generates the source it splices
//! in, and never learns what any declaration means.

use std::collections::BTreeMap;

use crate::{Error, Result};

/// The `data-*` key the generated runtime writes on `<body>` while a decision
/// is in flight, carrying the declaration that was pressed.
///
/// Fixed rather than configurable: it exists so a page's own stylesheet, and a
/// test driving a real browser, can see that something is happening. A name
/// per Provider would be one more thing for each of them to invent and for a
/// replacement frontend to get wrong.
pub const DECISION_PENDING_DATASET_KEY: &str = "decisionPending";

/// One declaration a page may carry, and what the Provider does about it.
#[derive(Debug, Clone)]
struct Decision {
    route: String,
    pending_text: Option<String>,
}

/// A Provider's decide-page runtime, generated rather than written again.
///
/// ```no_run
/// use agent_first_ui::UiDecisionRuntime;
///
/// # fn example() -> agent_first_ui::Result<()> {
/// let runtime = UiDecisionRuntime::new("data-reviewer-decision")?
///     .with_status_attribute("data-reviewer-decision-status")?
///     .with_decision("approve", "approve")?
///     .with_pending_text("approve", "Approving…")?
///     .with_decision("refuse", "refuse")?
///     .with_pending_text("refuse", "Refusing…")?;
/// let source = runtime.source();
/// # let _ = source;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct UiDecisionRuntime {
    attribute: String,
    status_attribute: Option<String>,
    decisions: BTreeMap<String, Decision>,
}

impl UiDecisionRuntime {
    /// Start a runtime bound to one declaration attribute.
    ///
    /// The name reaches a CSS selector and an attribute lookup in the
    /// generated source, so it is checked here rather than trusted: an
    /// attribute name that could carry a quote or a bracket could carry a
    /// selector, and a selector is one step from a script.
    pub fn new(attribute: impl Into<String>) -> Result<Self> {
        let attribute = attribute.into();
        validate_attribute_name("decision attribute", &attribute)?;
        Ok(Self {
            attribute,
            status_attribute: None,
            decisions: BTreeMap::new(),
        })
    }

    /// Where the runtime writes what is happening, for a page that shows it.
    ///
    /// Optional: a page with nowhere to put a status line still disables its
    /// controls and still submits exactly once.
    pub fn with_status_attribute(mut self, attribute: impl Into<String>) -> Result<Self> {
        let attribute = attribute.into();
        validate_attribute_name("decision status attribute", &attribute)?;
        self.status_attribute = Some(attribute);
        Ok(self)
    }

    /// One declaration and the Provider route it posts to.
    ///
    /// `route` is relative to the page, which is what keeps the answer inside
    /// the session's own credential and under `form-action 'self'`.
    pub fn with_decision(
        mut self,
        declared: impl Into<String>,
        route: impl Into<String>,
    ) -> Result<Self> {
        let declared = declared.into();
        let route = route.into();
        validate_declaration(&declared)?;
        validate_route(&route)?;
        self.decisions.insert(
            declared,
            Decision {
                route,
                pending_text: None,
            },
        );
        Ok(self)
    }

    /// What the status line says while this decision is in flight.
    ///
    /// Supplied by the Provider, in the Provider's own words. AFUI carries no
    /// default here and never will: "Approving…" and "Sending payment…" are
    /// the same moment in two domains, and a library that guessed between them
    /// would be writing someone else's product copy.
    pub fn with_pending_text(mut self, declared: &str, text: impl Into<String>) -> Result<Self> {
        let decision =
            self.decisions
                .get_mut(declared)
                .ok_or_else(|| Error::InvalidIdentifier {
                    field: "decision",
                    value: declared.to_owned(),
                })?;
        decision.pending_text = Some(text.into());
        Ok(self)
    }

    /// The attribute a template declares its controls with.
    #[must_use]
    pub fn attribute(&self) -> &str {
        &self.attribute
    }

    /// Every declaration this runtime binds, in a stable order.
    ///
    /// This is what a page's completeness check reads, so the list a runtime
    /// binds and the list a page must carry are one list — see
    /// [`crate::UiPageBuilder::requires_decisions`].
    pub fn declarations(&self) -> impl Iterator<Item = &str> {
        self.decisions.keys().map(String::as_str)
    }

    /// The runtime's JavaScript source.
    ///
    /// Returned as source rather than as a `<script>` element: the Provider
    /// owns the tag, because only the Provider knows this session's nonce, and
    /// the nonce is the whole reason a frontend cannot supply this script.
    #[must_use]
    pub fn source(&self) -> String {
        let routes: BTreeMap<&str, &str> = self
            .decisions
            .iter()
            .map(|(declared, decision)| (declared.as_str(), decision.route.as_str()))
            .collect();
        let pending: BTreeMap<&str, &str> = self
            .decisions
            .iter()
            .filter_map(|(declared, decision)| {
                decision
                    .pending_text
                    .as_deref()
                    .map(|text| (declared.as_str(), text))
            })
            .collect();
        RUNTIME_SOURCE
            .replace("__AFUI_ATTRIBUTE__", &json_for_script(&self.attribute))
            .replace(
                "__AFUI_STATUS_ATTRIBUTE__",
                &json_for_script(&self.status_attribute),
            )
            .replace("__AFUI_ROUTES__", &json_for_script(&routes))
            .replace("__AFUI_PENDING__", &json_for_script(&pending))
            .replace(
                "__AFUI_PENDING_KEY__",
                &json_for_script(&DECISION_PENDING_DATASET_KEY),
            )
    }
}

/// The generated runtime, with the Provider's own values spliced in as JSON.
///
/// ES5-shaped on purpose: this runs in whatever browser a person actually has,
/// and a decide page is the last place to discover a syntax the runtime does
/// not parse.
const RUNTIME_SOURCE: &str = r#"(function () {
  var ATTRIBUTE = __AFUI_ATTRIBUTE__;
  var STATUS_ATTRIBUTE = __AFUI_STATUS_ATTRIBUTE__;
  var ROUTES = __AFUI_ROUTES__;
  var PENDING = __AFUI_PENDING__;
  var PENDING_KEY = __AFUI_PENDING_KEY__;
  var controls = document.querySelectorAll("[" + ATTRIBUTE + "]");
  var status = STATUS_ATTRIBUTE ? document.querySelector("[" + STATUS_ATTRIBUTE + "]") : null;
  var submitted = false;
  var index = 0;
  for (index = 0; index < controls.length; index += 1) {
    bind(controls[index]);
  }

  function bind(control) {
    var declared = control.getAttribute(ATTRIBUTE);
    /* Exact match, and nothing else. A prefix match would accept
     * `approve-all`, which maps to no route — so the control would look bound
     * and submit nothing. An unrecognised declaration binds to nothing at all:
     * nothing on this page answers by default, and nothing answers by
     * omission. */
    if (!Object.prototype.hasOwnProperty.call(ROUTES, declared)) {
      return;
    }
    var action = ROUTES[declared];
    control.addEventListener("click", function (event) {
      event.preventDefault();
      /* One answer per page. The latch is checked before anything is disabled,
       * so a second press during the navigation cannot start a second one. */
      if (submitted) return;
      submitted = true;
      if (document.body) document.body.dataset[PENDING_KEY] = declared;
      for (var i = 0; i < controls.length; i += 1) {
        controls[i].setAttribute("aria-disabled", "true");
        if ("disabled" in controls[i]) controls[i].disabled = true;
      }
      if (status && Object.prototype.hasOwnProperty.call(PENDING, declared)) {
        status.textContent = PENDING[declared];
      }
      /* A form rather than fetch: the answer is a navigation, so the page a
       * person ends on is the Provider's own reply, and the policy that governs
       * it is `form-action 'self'` rather than an opening in `connect-src`. */
      var form = document.createElement("form");
      form.method = "post";
      form.action = action;
      document.body.appendChild(form);
      window.requestAnimationFrame(function () { form.submit(); });
    });
  }
})();
"#;

/// A JSON literal safe to embed inside a `<script>` element.
///
/// `</script>` inside a string literal ends the element as far as the HTML
/// parser is concerned, whatever JavaScript thinks — so `<` never survives as
/// itself. The line and paragraph separators go the same way: they are legal
/// JSON and were illegal in a JavaScript string literal for most of the
/// language's life.
fn json_for_script(value: &impl serde::Serialize) -> String {
    let json = serde_json::to_string(value).unwrap_or_else(|_| "null".to_owned());
    json.replace('<', "\\u003c")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

/// An HTML attribute name a generated selector can carry safely.
fn validate_attribute_name(field: &'static str, value: &str) -> Result<()> {
    let acceptable = !value.is_empty()
        && value.starts_with(|c: char| c.is_ascii_lowercase())
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !value.ends_with('-');
    if acceptable {
        Ok(())
    } else {
        Err(Error::InvalidIdentifier {
            field,
            value: value.to_owned(),
        })
    }
}

/// A declaration value, which a template writes by hand and this compares
/// exactly.
fn validate_declaration(value: &str) -> Result<()> {
    let acceptable = !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if acceptable {
        Ok(())
    } else {
        Err(Error::InvalidIdentifier {
            field: "decision",
            value: value.to_owned(),
        })
    }
}

/// A route relative to the page. Absolute or scheme-bearing targets are
/// refused: the answer has to stay inside the session's own credential, which
/// is also the only thing `form-action 'self'` permits.
fn validate_route(value: &str) -> Result<()> {
    let acceptable = !value.is_empty()
        && !value.starts_with('/')
        && !value.contains(':')
        && !value.contains("//")
        && !value.contains(|c: char| c.is_ascii_whitespace() || c == '"' || c == '\'' || c == '<');
    if acceptable {
        Ok(())
    } else {
        Err(Error::InvalidIdentifier {
            field: "decision route",
            value: value.to_owned(),
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn runtime() -> UiDecisionRuntime {
        UiDecisionRuntime::new("data-reviewer-decision")
            .unwrap()
            .with_status_attribute("data-reviewer-decision-status")
            .unwrap()
            .with_decision("approve", "approve")
            .unwrap()
            .with_pending_text("approve", "Approving…")
            .unwrap()
            .with_decision("refuse", "refuse")
            .unwrap()
    }

    #[test]
    fn the_generated_source_carries_the_providers_own_attribute_routes_and_words() {
        let source = runtime().source();
        assert!(source.contains("\"data-reviewer-decision\""), "{source}");
        assert!(
            source.contains("\"data-reviewer-decision-status\""),
            "{source}"
        );
        assert!(source.contains("\"approve\":\"approve\""), "{source}");
        assert!(source.contains("Approving"), "{source}");
        // No placeholder survives into the served source.
        assert!(!source.contains("__AFUI_"), "{source}");
    }

    /// AFUI supplies no domain words. A decision with no text the Provider
    /// gave it leaves the status line alone rather than inventing English.
    #[test]
    fn a_decision_with_no_provider_text_contributes_none() {
        let source = runtime().source();
        assert!(source.contains("\"approve\":\"Approving"), "{source}");
        assert!(!source.contains("\"refuse\":\"Refus"), "{source}");
    }

    /// The one way a JSON literal escapes a `<script>` element.
    #[test]
    fn a_value_containing_a_closing_script_tag_cannot_end_the_element() {
        let source = UiDecisionRuntime::new("data-reviewer-decision")
            .unwrap()
            .with_decision("approve", "approve")
            .unwrap()
            .with_pending_text("approve", "</script><script>alert(1)</script>")
            .unwrap()
            .source();
        assert!(!source.contains("</script>"), "{source}");
        assert!(source.contains("\\u003c/script"), "{source}");
    }

    #[test]
    fn an_attribute_name_that_could_carry_a_selector_is_refused() {
        for name in [
            "data-x\"]",
            "Data-X",
            "data x",
            "",
            "data-",
            "data-x[y]",
            "1-data",
        ] {
            assert!(UiDecisionRuntime::new(name).is_err(), "{name} was accepted");
        }
        assert!(UiDecisionRuntime::new("data-reviewer-decision").is_ok());
    }

    /// The answer stays inside the session's own credential, which is also all
    /// `form-action 'self'` permits.
    #[test]
    fn a_route_that_would_leave_this_session_is_refused() {
        for route in [
            "/approve",
            "https://elsewhere.example/approve",
            "//elsewhere.example/approve",
            "javascript:alert(1)",
            "",
            "app rove",
            "app\"rove",
        ] {
            let runtime = UiDecisionRuntime::new("data-reviewer-decision")
                .unwrap()
                .with_decision("approve", route);
            assert!(runtime.is_err(), "{route} was accepted");
        }
    }

    #[test]
    fn text_for_a_decision_that_was_never_declared_is_refused() {
        let error = UiDecisionRuntime::new("data-reviewer-decision")
            .unwrap()
            .with_decision("approve", "approve")
            .unwrap()
            .with_pending_text("approv", "typo")
            .unwrap_err();
        assert!(matches!(error, Error::InvalidIdentifier { .. }), "{error}");
    }

    #[test]
    fn the_declarations_a_runtime_binds_are_readable_as_a_page_contract() {
        let runtime = runtime();
        let declarations: Vec<_> = runtime.declarations().collect();
        assert_eq!(declarations, ["approve", "refuse"]);
    }
}

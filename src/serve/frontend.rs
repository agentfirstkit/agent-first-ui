//! Where this listener's own page comes from, and what a person may replace.
//!
//! Every other page in this kit is a Provider's, and AFUI holds it to a
//! contract: content-addressed trust, no scripts from a frontend, an error
//! rather than a quiet fallback. This one is AFUI's, and it is held to exactly
//! the same contract by exactly the same machinery — a page that lists what is
//! running is still a page, and a person who wants theirs to look different
//! should not have to fork a crate to get it.
//!
//! # What that does and does not protect
//!
//! Said plainly, because this page is not like the others: its URL carries the
//! credential for every session on this machine.
//!
//! A frontend cannot supply behaviour. AFUI refuses a file whose name says it is
//! a script and [`reject_frontend_script`] refuses one hidden inside a template,
//! so an override is markup and CSS — it cannot read this page's URL, because
//! reading anything requires script. What is left is what markup alone can do,
//! and the page's own Content-Security-Policy is what bounds that: no image, no
//! font, no frame and no connection to anywhere but this listener. An override
//! that tries to phone home has nowhere to phone.
//!
//! On top of that sits the same trust gate every Provider frontend gets:
//! workspace files are inert until somebody enables that exact fingerprint, any
//! later edit revokes it, and `AFUI_SAFE_MODE=1` ignores overrides entirely.
//!
//! What none of that covers is a person who enables a fingerprint without
//! reading it. That is the same bargain as every other frontend in this kit,
//! and it is why enabling is a deliberate act rather than a directory being
//! present.

use minijinja::{AutoEscape, Environment, UndefinedBehavior};
use serde::Serialize;

use crate::frontend::{SAFE_MODE_HINT, UiFrontend};
use crate::frontend_script::{reject_frontend_script, reject_rendered_script};
use crate::{Error, Result, UiAppIcon};

/// How AFUI's own page is identified in `.afui/frontends/<provider>/<kind>/`.
pub const PROVIDER_ID: &str = "afui";
pub const UI_KIND: &str = "sessions";

/// The contract a replacement page is written against: the document it renders
/// against, the trusted-runtime marker, the element ids and card classes the
/// runtime binds to, and the `data-act` attribute it reads.
///
/// `data-size` is not in that list, deliberately. It is written *by* the
/// runtime onto a card rather than read from the page: the three sizes are one
/// control's states, not a set of elements a page lays out, and their words are
/// the runtime's the same way `live` and `not answering` are. What a
/// replacement gets is the attribute to style — which the stylesheet check
/// below keeps honest.
const UI_API_VERSION: &str = "1";

/// Where AFUI splices in its own runtime.
const TRUSTED_RUNTIME_MARKER: &str = "<!-- afui:trusted-runtime -->";
const TRUSTED_RUNTIME: &str = "<script defer src=\"shell.js\"></script>";

const ENTRY_TEMPLATE: &str = "page.html.j2";
const BUILTIN_PAGE: &str = include_str!("assets/page.html.j2");
const BUILTIN_STYLE: &str = include_str!("assets/shell.css");
const BUILTIN_APP_ICON: &str = include_str!("assets/app-icon.svg");

/// Every element the runtime looks up by id.
#[derive(Serialize)]
struct ElementIds {
    count: &'static str,
    state: &'static str,
    state_line: &'static str,
    slot: &'static str,
    empty: &'static str,
    notice: &'static str,
    notice_text: &'static str,
    notice_renew: &'static str,
    card: &'static str,
}

const ELEMENT_IDS: ElementIds = ElementIds {
    count: "count",
    state: "state",
    state_line: "state-line",
    slot: "ui-slot",
    empty: "ui-empty",
    notice: "page-notice",
    notice_text: "page-notice-text",
    notice_renew: "page-renew",
    card: "card",
};

/// Every class the runtime looks up inside one card.
///
/// Classes rather than ids because there are many cards; the runtime finds them
/// within the card it is holding, so a template may nest them however it likes.
#[derive(Serialize)]
struct CardClasses {
    root: &'static str,
    provider: &'static str,
    kind: &'static str,
    subject: &'static str,
    notice: &'static str,
    frame: &'static str,
    open: &'static str,
    /// Additive in UI API v1: older frontends may omit it, in which case the
    /// runtime uses the required card notice for action feedback.
    action_status: &'static str,
    /// Where the interface inside the frame gets to say how it is going. A
    /// template that omits it is a card whose head says everything but that,
    /// which is why it is not required: the interface keeps its own head then.
    page_status: &'static str,
}

const CARD_CLASSES: CardClasses = CardClasses {
    root: "card",
    provider: "provider",
    kind: "kind",
    subject: "subject",
    notice: "card-notice",
    frame: "frame",
    open: "act-open",
    action_status: "card-action-status",
    page_status: "card-page-status",
};

/// One control, declared rather than wired.
///
/// `name` is the whole of the semantics: the runtime reads `data-act` and
/// decides what pressing it does. A template may relabel, reorder or drop these;
/// what it cannot do is make a button labelled "Fold" remove a card.
#[derive(Serialize)]
struct Action {
    name: &'static str,
    /// What this control is called. On one drawn as a glyph it is the whole of
    /// the accessible name, so it says what pressing does rather than naming a
    /// picture.
    label: &'static str,
    /// The character a head with room for one more control and not one more
    /// word draws it as, or empty for a control that keeps its word. Presentation
    /// only: it is the `name` above that decides what pressing one does.
    glyph: &'static str,
    /// Starts hidden because it only means something in one state — a warning
    /// running, a view that has lapsed, or a page that has said it is somewhere
    /// to come back from.
    hidden: bool,
    /// Where in the head this control belongs: `navigation` is the one slot
    /// ahead of the title, `action` is the row after it, `menu` is behind the
    /// overflow control. Layout guidance only. The action name remains the
    /// complete semantics.
    slot: &'static str,
}

const ACTIONS: [Action; 7] = [
    // The one control that belongs to the interface inside the frame rather
    // than to the card around it. Hidden until that interface says it is
    // somewhere with a way back, and pressing it says so to that interface and
    // to nothing else — this page never moves anybody's history but its own.
    Action {
        name: "back",
        label: "Back",
        glyph: "←",
        hidden: true,
        slot: "navigation",
    },
    // A glyph rather than the word it used to carry: the card's own height is
    // where a person reads its size off, so the word is worth more as this
    // control's name — which the runtime keeps current — than as its face.
    Action {
        name: "size",
        label: "Card size",
        glyph: "↕",
        hidden: false,
        slot: "action",
    },
    // Answers to a warning, and rare. A word, because a person who has never
    // seen this control before is being asked to decide something about it.
    Action {
        name: "renew",
        label: "Keep open",
        glyph: "",
        hidden: true,
        slot: "action",
    },
    Action {
        name: "reopen",
        label: "Reopen",
        glyph: "",
        hidden: true,
        slot: "action",
    },
    Action {
        name: "fold",
        label: "Fold",
        glyph: "",
        hidden: false,
        slot: "menu",
    },
    Action {
        name: "drop",
        label: "Hide",
        glyph: "",
        hidden: false,
        slot: "menu",
    },
    // Ends the session itself, not merely this view of it — the same
    // reserved route `afui session close` and a closed window both reach.
    // Visible by default, the way `fold`/`drop`/`size` are: a session starts
    // out live. The runtime hides it once a card's own state is `dead`, the
    // one case with no live proxy path left to send the request through.
    Action {
        name: "end",
        label: "End session",
        glyph: "",
        hidden: false,
        slot: "menu",
    },
];

#[derive(Serialize)]
struct ShellDocument {
    ui_api_version: &'static str,
    title: &'static str,
    heading: &'static str,
    slot_heading: &'static str,
    empty_label: &'static str,
    open_label: &'static str,
    more_label: &'static str,
    renew_label: &'static str,
    elements: ElementIds,
    card: CardClasses,
    actions: [Action; 7],
    action_attribute: &'static str,
}

fn shell_document() -> ShellDocument {
    ShellDocument {
        ui_api_version: UI_API_VERSION,
        title: "Agent-First UI",
        heading: "Agent-First UI",
        slot_heading: "Open interfaces",
        empty_label: "Nothing is open.",
        open_label: "Open in a new tab",
        more_label: "More actions",
        renew_label: "Keep open",
        elements: ELEMENT_IDS,
        card: CARD_CLASSES,
        actions: ACTIONS,
        action_attribute: "data-act",
    }
}

/// The page and stylesheet this listener will actually serve.
///
/// Resolved and rendered before a port is bound, so a frontend AFUI cannot use
/// costs a person an error rather than a listener serving a page that does
/// nothing.
pub(super) struct ShellUi {
    page: String,
    stylesheet: Vec<u8>,
    app_icon: UiAppIcon,
    frontend: UiFrontend,
}

impl ShellUi {
    pub(super) fn resolve(workspace_root: &std::path::Path) -> Result<Self> {
        let frontend = UiFrontend::resolve(workspace_root, PROVIDER_ID, UI_KIND, UI_API_VERSION)?;
        let stylesheet = frontend
            .file("style.css")?
            .unwrap_or_else(|| BUILTIN_STYLE.as_bytes().to_vec());
        let app_icon = frontend.app_icon(BUILTIN_APP_ICON)?;
        let page = render(&frontend)?;
        Ok(Self {
            page,
            stylesheet,
            app_icon,
            frontend,
        })
    }

    pub(super) fn page(&self) -> &str {
        &self.page
    }

    pub(super) fn stylesheet(&self) -> &[u8] {
        &self.stylesheet
    }

    pub(super) fn app_icon(&self) -> &UiAppIcon {
        &self.app_icon
    }

    pub(super) fn frontend_id(&self) -> Option<&str> {
        self.frontend.frontend_id()
    }

    /// One file out of a replacement's `assets/` tree.
    ///
    /// The same answer a Provider's assets get, from the same code: the type is
    /// named from a fixed list or the file is refused, rather than being handed
    /// over as bytes a browser will not use. Routed by hand rather than by
    /// nesting the frontend's own router, because this page dispatches on a path
    /// it has already taken a credential off the front of.
    pub(super) fn asset(&self, relative: &str) -> axum::response::Response {
        self.frontend.asset_response(relative)
    }
}

fn render(frontend: &UiFrontend) -> Result<String> {
    let source = match frontend.text(&format!("templates/{ENTRY_TEMPLATE}"))? {
        Some(text) => {
            reject_frontend_script(ENTRY_TEMPLATE, &text)?;
            text
        }
        None => BUILTIN_PAGE.to_owned(),
    };
    render_source(&source, frontend.frontend_id())
}

/// The checks, over whatever source was resolved.
///
/// Separated from resolution so that what a replacement must satisfy can be
/// tested against a replacement, rather than only against the one page that
/// happens to satisfy it.
fn render_source(source: &str, frontend_id: Option<&str>) -> Result<String> {
    let source = source.to_owned();
    let mut environment = Environment::new();
    environment.set_undefined_behavior(UndefinedBehavior::SemiStrict);
    environment.set_auto_escape_callback(|_| AutoEscape::Html);
    environment.remove_filter("safe");
    environment
        .add_template_owned(ENTRY_TEMPLATE.to_owned(), source)
        .map_err(|error| broken(frontend_id, &error.to_string()))?;
    let template = environment
        .get_template(ENTRY_TEMPLATE)
        .map_err(|error| broken(frontend_id, &error.to_string()))?;
    let page = template
        .render(minijinja::context! {
            document => minijinja::Value::from_serialize(shell_document()),
        })
        .map_err(|error| broken(frontend_id, &error.to_string()))?;

    reject_rendered_script("rendered page", &page)?;
    if !page.contains(TRUSTED_RUNTIME_MARKER) {
        return Err(broken(
            frontend_id,
            &format!(
                "the rendered page does not contain `{TRUSTED_RUNTIME_MARKER}`, so AFUI has \
                 nowhere to load its trusted runtime"
            ),
        ));
    }
    for id in [
        ELEMENT_IDS.count,
        ELEMENT_IDS.state,
        ELEMENT_IDS.state_line,
        ELEMENT_IDS.slot,
        ELEMENT_IDS.empty,
        ELEMENT_IDS.notice,
        ELEMENT_IDS.notice_text,
        ELEMENT_IDS.notice_renew,
        ELEMENT_IDS.card,
    ] {
        if !page.contains(&format!("id=\"{id}\"")) && !page.contains(&format!("id='{id}'")) {
            return Err(broken(
                frontend_id,
                &format!(
                    "the rendered page has no element with id `{id}`, which the runtime binds to; \
                     the page would open and do nothing"
                ),
            ));
        }
    }
    // The card is a template rather than an element, so its parts are checked by
    // the classes the runtime looks them up by.
    for class in [
        CARD_CLASSES.root,
        CARD_CLASSES.provider,
        CARD_CLASSES.kind,
        CARD_CLASSES.subject,
        CARD_CLASSES.notice,
        CARD_CLASSES.frame,
        CARD_CLASSES.open,
    ] {
        if !page.contains(class) {
            return Err(broken(
                frontend_id,
                &format!(
                    "the card template has nothing with the class `{class}`, which the runtime \
                     fills in; every card would be blank"
                ),
            ));
        }
    }
    Ok(page.replace(TRUSTED_RUNTIME_MARKER, TRUSTED_RUNTIME))
}

fn broken(frontend_id: Option<&str>, what: &str) -> Error {
    Error::FrontendUnreadable {
        provider_id: PROVIDER_ID.to_owned(),
        ui_kind: UI_KIND.to_owned(),
        message: match frontend_id {
            Some(frontend_id) => {
                format!("the frontend `{frontend_id}` is unusable: {what}; {SAFE_MODE_HINT}")
            }
            None => format!("AFUI's own page is unusable: {what}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// AFUI's own page has to satisfy the contract it holds a replacement to,
    /// or the contract describes something that has never been true.
    #[test]
    fn the_built_in_page_meets_every_rule_a_frontend_is_held_to() {
        let ui = ShellUi::resolve(std::path::Path::new("/nonexistent")).unwrap();
        assert!(ui.frontend_id().is_none());
        assert!(!ui.page().contains(TRUSTED_RUNTIME_MARKER));
        assert!(ui.page().contains(TRUSTED_RUNTIME));
        assert!(!ui.stylesheet().is_empty());
        for action in ACTIONS {
            assert!(
                ui.page().contains(&format!("data-act=\"{}\"", action.name)),
                "the page never offers `{}`",
                action.name
            );
        }
        // The template itself may not carry a script, and AFUI's own page is the
        // reference implementation of that rule.
        reject_frontend_script("page.html.j2", BUILTIN_PAGE).unwrap();
    }

    /// The listener page is itself a human interface. It must follow the
    /// person's system theme, keep warnings attached to the card they describe,
    /// and distinguish hiding a card from ending the session behind it.
    #[test]
    fn the_built_in_shell_has_human_theme_status_and_action_semantics() {
        let ui = ShellUi::resolve(std::path::Path::new("/nonexistent")).unwrap();
        let style = String::from_utf8_lossy(ui.stylesheet());
        // Both themes come from the baseline this page links first, which is
        // the same one the sessions it frames are served. A card and the
        // interface inside it are the one place where two palettes would be
        // visible side by side.
        assert!(
            ui.page().contains("href=\"__afui/base.css\""),
            "{}",
            ui.page()
        );
        assert!(
            crate::page_style::BASE_STYLE.contains("color-scheme: light dark"),
            "the baseline carries the theme"
        );
        assert!(style.contains(".card-notice[data-state=\"dead\"]"));
        assert!(style.contains("@media (pointer: coarse)"));
        assert!(ui.page().contains("card-action-status"), "{}", ui.page());
        // The head takes the one thing it cannot know, so an interface that
        // trusts it with all three stops drawing a second head underneath.
        assert!(ui.page().contains("card-page-status"), "{}", ui.page());
        // Removing a card and ending the session behind it stay words, behind
        // the overflow control: they are the two nobody should reach for by
        // recognising a picture.
        assert!(ui.page().contains(">Hide</button>"), "{}", ui.page());
        assert!(ui.page().contains(">End session</button>"), "{}", ui.page());
        // A control drawn as a glyph still has to say what it is. The name is
        // the label, on the control itself, with the picture hidden from
        // anything reading the page out — a bar of unnamed glyphs is a bar
        // nobody can use without pressing things to find out.
        for named in [
            "aria-label=\"More actions\"",
            "aria-label=\"Back\"",
            "aria-label=\"Card size\"",
            "aria-label=\"Open in a new tab\"",
        ] {
            assert!(
                ui.page().contains(named),
                "{named} is missing: {}",
                ui.page()
            );
        }
        for glyph in ["←", "↕", "↗", "⋮"] {
            assert!(
                ui.page()
                    .contains(&format!("aria-hidden=\"true\">{glyph}<")),
                "the glyph {glyph} must be hidden from a reader that has the name: {}",
                ui.page()
            );
        }
        assert!(style.contains(".card:has(.act-menu[open])"), "{style}");
        assert!(
            super::super::shell::SHELL_JS.contains("Ending session…"),
            "network actions need immediate feedback"
        );
        assert!(ui.page().contains("Open interfaces"), "{}", ui.page());
    }

    /// Every size the runtime can put on a card is one the stylesheet has a
    /// rule for, or a person picks a height and nothing moves.
    ///
    /// Read out of `shell.js` rather than listed here, so the two cannot drift:
    /// a size added to the runtime and styled nowhere fails this.
    #[test]
    fn every_size_the_runtime_offers_is_one_the_stylesheet_knows() {
        let sizes = super::super::shell::SHELL_JS
            .split_once("const SIZES = [")
            .and_then(|(_, rest)| rest.split_once(']'))
            .map(|(list, _)| list.to_owned())
            .expect("shell.js declares its sizes");
        let names: Vec<String> = sizes
            .split(',')
            .map(|name| name.trim().trim_matches('\'').to_owned())
            .filter(|name| !name.is_empty())
            .collect();
        assert_eq!(names.len(), 3, "{names:?}");
        for name in names {
            assert!(
                BUILTIN_STYLE.contains(&format!("[data-size=\"{name}\"]")),
                "`{name}` is offered but styled nowhere"
            );
        }
    }

    /// A replacement that drops something the runtime binds to is refused
    /// before a port is bound, not discovered by a person staring at a page
    /// that does nothing.
    #[test]
    fn a_replacement_missing_what_the_runtime_binds_to_is_refused() {
        // The built-in page with one thing taken out at a time, which is the
        // shape of every real mistake here: somebody restructures the page and
        // loses a piece without noticing.
        let cases = [
            (
                TRUSTED_RUNTIME_MARKER,
                "nowhere to load its trusted runtime",
            ),
            (
                "id=\"{{ document.elements.slot }}\"",
                "no element with id `ui-slot`",
            ),
            ("{{ document.card.frame }}", "class `frame`"),
        ];
        for (removed, expected) in cases {
            let source = BUILTIN_PAGE.replace(removed, "");
            let error = render_source(&source, Some("mine"))
                .unwrap_err()
                .to_string();
            assert!(
                error.contains(expected),
                "removing `{removed}` should say `{expected}`, said: {error}"
            );
            assert!(error.contains("mine"), "{error}");
        }
        // And the whole page, unaltered, still passes the same checks.
        assert!(render_source(BUILTIN_PAGE, None).is_ok());
    }

    /// A replacement may not bring behaviour, whatever it calls the file.
    #[test]
    fn a_replacement_may_not_smuggle_a_script() {
        let source = BUILTIN_PAGE.replace("</body>", "<script>fetch('/steal')</script></body>");
        assert!(reject_frontend_script(ENTRY_TEMPLATE, &source).is_err());
        assert!(render_source(&source, Some("mine")).is_err());
    }
}

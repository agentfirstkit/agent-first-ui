//! The stylesheet every AFUI page is served on top of.
//!
//! Measured across the pages in this kit, the same two hundred lines were
//! written again in every one of them: a palette, a box-sizing reset, a body
//! type scale, a focus ring, and — in the pages that remembered — a `[hidden]`
//! that actually hides and a phone's unreachable edges. The palettes had
//! already been copied from one page to the next by hand, which is how one of
//! them came to disagree about the focus colour with nothing to notice.
//!
//! What is collected here is not a design system. It is the part of a page's
//! appearance that either AFUI's own runtime decides — what a control looks
//! like while a call is in flight, what `hidden` does — or that is *wrong*
//! rather than merely plain when a page forgets it. Anything a page could
//! reasonably disagree about is left to the page.
//!
//! Deliberately not a component library. No button variants, no layout system,
//! no utility classes. The one state vocabulary here is the runtime's own
//! connection state, which AFUI produces and is therefore AFUI's to dress; the
//! Provider's word on `data-state` is styled nowhere, because what "busy"
//! looks like is not this crate's answer to give. A rule that needed to know
//! what a panel contains would not be collected — it would be declined.

/// The baseline stylesheet's source.
///
/// Sessions serve this themselves at [`crate::BASE_STYLE_PATH`], so a page
/// links it and needs no route of its own:
///
/// ```jinja
/// <link rel="stylesheet" href="{{ afui.base_style_path }}">
/// <link rel="stylesheet" href="style.css">
/// ```
///
/// In that order: the page's own stylesheet loads second and outranks this one
/// at equal specificity, and most selectors here are wrapped in `:where()` so
/// that overriding one never turns into a specificity fight. Retuning the
/// palette is a single declaration, because every value is a custom property
/// on `:root` — `:root { --afui-accent: #7a3cff; }`.
///
/// This accessor exists for the build that assembles a page outside a session
/// — a preview, a snapshot test, a Provider serving its own bytes. Inside a
/// session there is nothing to call: the route is already answered.
#[must_use]
pub fn page_base_style_source() -> &'static str {
    BASE_STYLE
}

pub(crate) const BASE_STYLE: &str = include_str!("serve/assets/base.css");

#[cfg(test)]
mod tests {
    use super::BASE_STYLE;

    /// The one declaration in the baseline that has to win outright, because
    /// the browser's own `[hidden]` rule loses to any author `display` and an
    /// element a runtime believes it removed would still take up room.
    #[test]
    fn hiding_an_element_beats_a_page_that_gave_it_a_display() {
        assert!(
            BASE_STYLE.contains("[hidden] {\n  display: none !important;\n}"),
            "the baseline must neutralise `[hidden]` unconditionally",
        );
    }

    /// Everything a page might retune is a custom property, which is what
    /// makes a different palette one line in the page's own stylesheet rather
    /// than a fork of this file. The ones that describe a colour need a second
    /// value, because a page whose light theme comes from here and whose dark
    /// theme does not is worse off than one that took neither.
    #[test]
    fn every_tunable_value_is_a_custom_property_and_every_colour_has_both_themes() {
        for token in [
            "--afui-page",
            "--afui-panel",
            "--afui-panel-strong",
            "--afui-ink",
            "--afui-muted",
            "--afui-line",
            "--afui-accent",
            "--afui-accent-ink",
            "--afui-focus",
            "--afui-warn-ink",
            "--afui-warn-bg",
            "--afui-danger-ink",
            "--afui-danger-bg",
            "--afui-ok-ink",
            "--afui-ok-bg",
            "--afui-shadow",
        ] {
            assert_eq!(
                BASE_STYLE.matches(&format!("{token}:")).count(),
                2,
                "`{token}` needs a light and a dark value",
            );
        }
        for token in [
            "--afui-radius",
            "--afui-font",
            "--afui-mono",
            "--afui-page-padding",
        ] {
            assert!(BASE_STYLE.contains(&format!("{token}:")), "`{token}`");
        }
        assert!(BASE_STYLE.contains("@media (prefers-color-scheme: dark)"));
        assert!(BASE_STYLE.contains("color-scheme: light dark;"));
    }

    /// A page that opens on a phone is the common case for two of the three
    /// deliveries, and the inset it needs is not something each page should
    /// have to remember.
    #[test]
    fn the_page_inset_clears_the_hardware() {
        for edge in ["top", "right", "bottom", "left"] {
            assert!(
                BASE_STYLE.contains(&format!(
                    "padding-{edge}: max(var(--afui-page-padding), env(safe-area-inset-{edge}));"
                )),
                "the `{edge}` inset must take the larger of the page's own and the hardware's",
            );
        }
    }

    /// The line between the one vocabulary this file may dress and the one it
    /// may not. `data-afui-connection` carries a word the kernel itself
    /// produces, so the two that mean "this may already be wrong" are styled
    /// here rather than in five stylesheets that each guessed. `data-state`
    /// carries whatever word a Provider chose, and styling that would mean
    /// inventing a vocabulary for somebody else's domain first.
    #[test]
    fn the_only_state_vocabulary_is_the_one_the_runtime_produces() {
        for word in ["reconnecting", "closed"] {
            assert!(
                BASE_STYLE.contains(&format!(
                    "[data-afui-connection=\"{word}\"][data-afui-connection]"
                )),
                "`{word}` must outrank the one class every page gives its own status line, or a \
                 page that stopped being current keeps its ordinary colour",
            );
        }
        for word in ["live", "connecting"] {
            assert!(
                !BASE_STYLE.contains(&format!("[data-afui-connection=\"{word}\"]")),
                "being connected is the ordinary case and needs no appearance of its own",
            );
        }
        assert!(
            !BASE_STYLE.contains("[data-state"),
            "the word in `data-state` is the Provider's, and this file does not know what it means",
        );
    }

    /// The boundary this module is under: appearance AFUI's runtime decides,
    /// or that is wrong when missing. A component vocabulary would be the
    /// beginning of a design system nobody asked this crate for.
    #[test]
    fn the_baseline_names_no_components() {
        for component in [
            ".btn", ".card", ".panel", ".row", ".col-", ".badge", ".chip",
        ] {
            assert!(
                !BASE_STYLE.contains(component),
                "`{component}` is a component, and components belong to the page",
            );
        }
    }
}

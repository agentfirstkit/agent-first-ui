//! The building material a Provider's page runtime is made of.
//!
//! Measured across the pages in this kit, roughly seven hundred lines were the
//! same work done again: deriving the session root, maintaining the typed AFUI
//! runtime across reconnects, correlating calls, replacing retained state,
//! replaying ordered events, writing an aria-live region, disabling controls
//! while something is in flight, debouncing a save around a base hash,
//! surviving a reload through `location.hash`, and warning before leaving with
//! an unsaved edit.
//!
//! None of that is page behaviour. It is what page behaviour is written *in*,
//! and the boundary is unchanged by moving it here: a frontend still cannot
//! supply JavaScript, and this source only ever reaches a browser because a
//! Provider spliced it into its own trusted runtime.
//!
//! Deliberately not a framework. There is no template here, no DOM diff and no
//! state store; every text write is `textContent` and every element comes from
//! the caller. A primitive that needed a third-party library would not be
//! collected — it would be declined.

/// The kernel's JavaScript source, to concatenate ahead of a Provider's own
/// runtime.
///
/// It defines one lexical `afui` binding and writes nothing to `window`, so
/// place both inside a single function scope:
///
/// ```rust
/// # use agent_first_ui::page_kernel_source;
/// # fn example(nonce: &str, provider_runtime: &str) -> String {
/// format!(
///     "<script nonce=\"{nonce}\">(function(){{\n{}\n{provider_runtime}\n}})();</script>",
///     page_kernel_source(),
/// )
/// # }
/// ```
///
/// One kernel rather than a menu of separately includable pieces: every
/// primitive is built from the same session root, and a page that took three
/// of them would otherwise get three copies of that derivation. What a page
/// does not call costs it a few kilobytes over loopback, which is not a trade
/// worth splitting an API over.
#[must_use]
pub fn page_kernel_source() -> &'static str {
    include_str!("page_runtime/kernel.js")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The kernel is spliced into a `<script>` element, so it must not be able
    /// to end one — and it must not reach for `window`, which is what keeps
    /// two pages on one origin from seeing each other's kernels.
    #[test]
    fn the_kernel_stays_inside_the_element_and_the_scope_it_is_spliced_into() {
        let source = page_kernel_source();
        assert!(
            !source.contains("</script"),
            "the kernel closes its element"
        );
        assert!(
            !source.contains("window.afui"),
            "the kernel must not publish itself as a global"
        );
        assert!(
            source.contains("const afui = (function ()"),
            "the kernel must define exactly one lexical binding"
        );
    }

    /// Every URL the kernel builds is relative to the session root, which is
    /// what keeps a request under this session's own credential — and the
    /// reserved routes it names are the ones this crate actually answers.
    ///
    /// The blob prefix belongs on this list as much as the other two: the
    /// kernel refuses a blob reference that does not match it, so renaming the
    /// route on the Rust side without this would not break a build — it would
    /// make every managed byte body unreachable from every page at once.
    ///
    /// The blob prefix arrives inside a regular-expression literal, where a
    /// `/` is written `\/`. Undoing that escaping is what lets one list cover
    /// all three routes instead of two plus a hand-written variant of the
    /// third — which is how the missing one stayed missing.
    #[test]
    fn the_kernel_names_the_reserved_routes_this_crate_answers() {
        let source = page_kernel_source().replace("\\/", "/");
        for path in [
            crate::reserved::SESSION_RUNTIME_PATH,
            crate::reserved::SESSION_BLOB_PATH_PREFIX,
            crate::SESSION_END_PATH,
        ] {
            assert!(source.contains(path), "the kernel does not name `{path}`");
        }
    }

    /// The application icon reaches a page as `{{ afui.app_icon_path }}`, from
    /// the template environment. A second way to name it from JavaScript was
    /// exported here and called by nothing, which is a reserved path written
    /// down one more time than it is used.
    #[test]
    fn the_kernel_leaves_the_application_icon_to_the_template_global() {
        assert!(
            !page_kernel_source().contains(crate::APP_ICON_PATH),
            "the kernel names the icon route the page template already carries"
        );
    }

    /// A library that started writing markup would be deciding what a person
    /// sees. Every text write is `textContent`.
    #[test]
    fn the_kernel_never_writes_markup() {
        let source = page_kernel_source();
        for forbidden in [
            "innerHTML",
            "outerHTML",
            "insertAdjacentHTML",
            "document.write",
        ] {
            assert!(!source.contains(forbidden), "the kernel uses {forbidden}");
        }
    }
}

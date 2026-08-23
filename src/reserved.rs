//! The routes AFUI answers under every session's own credential.
//!
//! One list, in the tier with no server in it, because these paths are needed
//! in three places that do not otherwise meet: the dispatch that answers them,
//! the page that references them, and a Provider that has to route around
//! them. Provider pages never name the runtime route themselves;
//! `afui.connect()` is the public browser boundary.

/// The prefix every reserved AFUI route lives under, on every session's own
/// credential — checked before the Provider's own router is ever reached, so
/// nothing a Provider routes there is reachable, shadowed or not. A Provider
/// must never claim any name under it. Reserving the whole prefix rather than
/// only today's paths is what lets AFUI add another one later without it
/// silently colliding with something a Provider already routed there.
#[cfg_attr(not(feature = "session"), allow(dead_code))]
pub(crate) const RESERVED_PATH_PREFIX: &str = "__afui/";

/// Reserved for every session, under its own credential, on every delivery —
/// `window`, `link`, and `session` alike, because they all share one dispatch.
/// `POST <access_url>SESSION_END_PATH` ends the session: whoever created it
/// gets `Outcome::Closed`, the same as if a person had closed its window.
///
/// Written without a leading slash because every caller appends it to a
/// credential URL that already ends in one — and because that is what makes it
/// resolve correctly as a relative reference from inside the page.
pub const SESSION_END_PATH: &str = "__afui/end";

/// The AFUI-owned state, call, and ordered-event protocol attached to a live
/// session. Provider pages use `afui.connect()` and never name this path.
#[cfg_attr(not(feature = "session"), allow(dead_code))]
pub(crate) const SESSION_RUNTIME_PATH: &str = "__afui/runtime";

/// Reserved prefix for bytes retained by an AFUI session runtime.
#[cfg_attr(not(feature = "session"), allow(dead_code))]
pub(crate) const SESSION_BLOB_PATH_PREFIX: &str = "__afui/blobs/";

/// Where every session serves the floor its page stands on.
///
/// A reserved route rather than bytes handed to the Provider to serve, for one
/// reason that decides it: a frontend override replaces the Provider's own
/// stylesheet wholesale, and a baseline concatenated into that file would go
/// with it. Answered here, the floor survives the replacement — an override
/// can be one template and no CSS at all, and still open as a page that
/// belongs to this kit.
///
/// Written without a leading slash for the same reason as the others: a page
/// references it relative to its own session root.
pub const BASE_STYLE_PATH: &str = "__afui/base.css";

/// Where a hosted session exposes its resolved application icon.
///
/// Written without a leading slash for the same reason as the two above: a
/// page references it relative to its own session root, and the window
/// launcher appends it to a credential URL that already ends in `/`.
pub const APP_ICON_PATH: &str = "__afui/app-icon.svg";

#[cfg(test)]
mod tests {
    use super::*;

    /// Every reserved route is under the reserved prefix, without a leading
    /// slash. A path that broke either rule would be routed to the Provider
    /// instead of answered here, or would resolve against the listener root
    /// instead of the session's.
    #[test]
    fn every_reserved_route_is_a_relative_path_under_the_reserved_prefix() {
        for path in [
            SESSION_END_PATH,
            SESSION_RUNTIME_PATH,
            SESSION_BLOB_PATH_PREFIX,
            APP_ICON_PATH,
            BASE_STYLE_PATH,
        ] {
            assert!(path.starts_with(RESERVED_PATH_PREFIX), "{path}");
            assert!(!path.starts_with('/'), "{path}");
        }
    }
}

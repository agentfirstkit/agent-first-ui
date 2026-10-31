//! Small, domain-agnostic primitives for handing a web UI to a person.
//!
//! AFUI owns delivery, lifetime, and the live session protocol. Providers own
//! their domain payload types, validation, presentation, and the meaning of a
//! completed result.

mod app_icon;
/// AFUI's standard question page: one or more questions, each answered by
/// choosing options or writing an answer, with optional context blocks.
#[cfg(feature = "session")]
pub mod ask;
/// Which of the three ways a session reaches a person. The word list is a
/// parse over three strings and one environment variable, so it costs nothing
/// to keep available in a build with no session at all — a Provider whose
/// human UI is a compiled-out option still has a `--mode` flag. The executor
/// underneath it needs a server, and stays behind `session`.
mod delivery;
mod error;
/// Hands a URL to the system browser on the runtime's behalf; only a served
/// session has a page that could ask.
#[cfg(feature = "session")]
mod external;
mod frontend;
mod frontend_script;
/// Address discovery: which of this machine's addresses another device on
/// the same physical LAN can actually reach. A plain computation over
/// `std::net` — no port bound, no credential minted — so it costs nothing to
/// keep available in a build that has no session at all, and
/// [`delivery::UiDeliveryPlan::deliver`]'s `link` branch is only one caller
/// among any that need a LAN-reachable address for a reason of their own.
mod lan;
/// The floor a Provider's page stands on: the appearance AFUI's own runtime
/// decides, and the handful of rules that are wrong rather than plain when a
/// page forgets them. A stylesheet is a string, so it costs a build with no
/// server nothing to carry — and the tier that only renders a page still has
/// to be able to hand it out.
mod page_style;
/// The routes AFUI answers under every session's credential, as one list
/// in the tier with no server in it: the dispatch that answers them, the page
/// that references them, and a Provider that routes around them all read the
/// same constants.
mod reserved;

// Handing a UI to a person costs a server; keeping one honest does not.
//
// Everything under `session` binds a port, mints a credential, or launches a
// browser, and that is where axum and tokio come from. The guard and the
// frontend overlay above need neither, so a Provider that only has templates
// to check — in a build with its human UI compiled out — takes this
// crate without them.
#[cfg(feature = "session")]
mod access_token;
/// Declaring the delivery flag, reading it back, and getting the readiness
/// facts into an event: the half of handing over a UI that happens on the
/// command line, so a Provider's flags and readiness events read the same as
/// every other's.
#[cfg(feature = "cli")]
pub mod cli;
/// Generating a Provider's decide-page runtime: the rules any page that asks a
/// person to decide needs, so a page does not write them again. Behind `page`
/// because it produces source a page splices in, and needs no server to do it.
#[cfg(feature = "page")]
mod decision;
#[cfg(feature = "session")]
mod host;
/// Keeping a credential someone else issued from lapsing: the cadence, and the
/// shape of the credential itself. The call that renews it stays the
/// Provider's, because the endpoint it renews against is the Provider's.
#[cfg(feature = "session")]
mod lease;
/// Assembling a frontend's templates into a page: the escaping policy, the
/// render, the rendered-markup guard, and the element/runtime-marker
/// contract check. Orthogonal to `session` — it renders and checks a page,
/// it does not bind a port, so a build with a frontend but no server still
/// gets it.
#[cfg(feature = "page")]
mod page;
/// The building material for custom pages and AFUI's standard question page.
/// Available in `page` without a server, and in `session` for the built-in UI.
#[cfg(any(feature = "page", feature = "session"))]
mod page_runtime;
#[cfg(feature = "session")]
mod registry;
/// The one versioned state/action/event protocol every first-party AFUI page
/// uses. Domain payloads remain Provider-owned; transport semantics do not.
#[cfg(feature = "session")]
mod runtime;
#[cfg(feature = "session")]
mod security;
/// The AFUI-owned remote page and streaming proxy used by direct `link`
/// delivery and by `afui session serve`.
#[cfg(feature = "session")]
mod serve;
/// The routes every Provider page serves, and the one bounded response shape a
/// page's own bytes travel back in. Behind `session` because both are routers
/// and responses; neither decides anything about what a page contains.
#[cfg(feature = "session")]
mod serving;
#[cfg(feature = "session")]
mod session;
/// One rendered page, handed to a person, waited on: resolve, render, policy,
/// routes, attach, deliver and wait in one call. Deliberately the last layer —
/// everything it does is reachable directly, and a page that does not fit this
/// shape should use the pieces instead of bending to it.
#[cfg(feature = "session")]
mod surface;
/// What a Provider's own tests need to arrange the world AFUI reads from.
/// Behind its own feature, out of the default set, so nothing here reaches a
/// release binary.
#[cfg(feature = "test-support")]
pub mod test_support;
#[cfg(feature = "session")]
mod upstream;
#[cfg(feature = "session")]
mod window;

/// `AFUI_CONFIG_DIR`, `AFUI_NO_REGISTRY`, and `AFUI_DELIVERY` are process-wide,
/// so every unit test that points one at its own temp tree, or sets one to a
/// fixed value, takes this first. One lock for the whole crate rather than one
/// per module: the modules share the variables, so separate locks would not
/// serialise anything.
#[cfg(test)]
pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub use app_icon::{APP_ICON_ASSET_PATH, UiAppIcon};
pub use delivery::{DELIVERY_ENV, UiDeliveryMode};
pub use error::{Error, Result, UiErrorKind};
pub use frontend::{
    CONFIG_DIR_ENV, FrontendLocation, FrontendManifest, FrontendScope, SAFE_MODE_ENV,
    SAFE_MODE_HINT, UiFrontend, discover_frontend, frontend_root, global_afui_root, load_frontend,
    safe_mode_from_env, validate_frontend_identifier,
};
pub use frontend_script::{reject_frontend_script, reject_rendered_script};
pub use lan::primary_lan_ipv4;
pub use page_style::page_base_style_source;
pub use reserved::{APP_ICON_PATH, BASE_STYLE_PATH, SESSION_END_PATH};

#[cfg(feature = "session")]
pub use access_token::{UiAccessToken, UiExpiry};
#[cfg(feature = "page")]
pub use decision::{DECISION_PENDING_DATASET_KEY, UiDecisionRuntime};
#[cfg(feature = "session")]
pub use delivery::{ActiveUiDelivery, ActiveUiUpstreamDelivery, UiDeliveryFacts, UiDeliveryPlan};
#[cfg(feature = "session")]
pub use host::{UiHost, UiMount, UiMountAccess, UiMountCredential};
#[cfg(feature = "session")]
pub use lease::{UiCredentialLease, is_ui_credential};
#[cfg(feature = "page")]
pub use page::{
    UiPage, UiPageBuilder, UiPageEnvironment, UiPageFilterResult, UiPageSerializedValue,
    UiPageValue,
};
#[cfg(any(feature = "page", feature = "session"))]
pub use page_runtime::page_kernel_source;
#[cfg(feature = "session")]
pub use registry::{
    NO_REGISTRY_ENV, UiSessionEntry, find_session, list_sessions, registry_opted_out,
    sessions_directory,
};
#[cfg(feature = "session")]
pub use runtime::{
    DEFAULT_UI_RUNTIME_CALL_CAPACITY, DEFAULT_UI_RUNTIME_EVENT_CAPACITY,
    MAX_UI_RUNTIME_MESSAGE_BYTES, UiBlobRef, UiCall, UiCallError, UiSessionRuntime,
};
#[cfg(feature = "session")]
pub use security::{OriginDecision, UiCspNonce, UiPagePolicy, UiPageScript, UiSecurityPolicy};
#[cfg(feature = "session")]
pub use serve::{ServedSessions, SessionServer, UiAttentionPolicy};
#[cfg(feature = "session")]
pub use serving::{
    PAGE_ASSETS_PREFIX, PAGE_STYLESHEET_PATH, UiBlobPolicy, page_asset_routes, page_routes,
};
#[cfg(feature = "session")]
pub use session::{
    ActiveUiSession, Outcome, UiCompletion, UiSession, UiSessionId, UiSessionMetadata,
    UiUrlDelivery,
};
#[cfg(feature = "session")]
pub use surface::{ActiveUiSurface, UiSurface, surface_page};
#[cfg(feature = "session")]
pub use upstream::{AnnouncedUiUpstream, UiUpstream};
#[cfg(feature = "session")]
pub use window::{BROWSER_BINARY_ENV, UiWindow, UiWindowConfig};

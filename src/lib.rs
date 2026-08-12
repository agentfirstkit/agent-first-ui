//! Small, domain-agnostic primitives for handing a web UI to a person.
//!
//! AFUI owns delivery, lifetime, and optional bounded channel transport.
//! Providers own their Axum router, both channel value types, domain
//! validation, presentation, and the meaning of a completed result.

mod app_icon;
mod error;
mod frontend;
mod frontend_script;
/// Address discovery: which of this machine's addresses another device on
/// the same physical LAN can actually reach. A plain computation over
/// `std::net` — no port bound, no credential minted — so it costs nothing to
/// keep available in a build that has no session at all, and
/// [`delivery::UiDeliveryPlan::deliver`]'s `link` branch is only one caller
/// among any that need a LAN-reachable address for a reason of their own.
mod lan;

/// Handing a UI to a person costs a server; keeping one honest does not.
///
/// Everything under `session` binds a port, mints a credential, or launches a
/// browser, and that is where axum and tokio come from. The guard and the
/// frontend overlay above need neither, so a Provider that only has templates
/// to check — in a build with its human UI compiled out — takes this
/// crate without them.
#[cfg(feature = "session")]
mod access_token;
#[cfg(feature = "session")]
mod channel;
#[cfg(feature = "session")]
mod delivery;
#[cfg(feature = "session")]
mod host;
/// Assembling a frontend's templates into a page: the escaping policy, the
/// render, the rendered-markup guard, and the element/runtime-marker
/// contract check. Orthogonal to `session` — it renders and checks a page,
/// it does not bind a port, so a build with a frontend but no server still
/// gets it.
#[cfg(feature = "page")]
mod page;
#[cfg(feature = "session")]
mod registry;
#[cfg(feature = "session")]
mod security;
/// Serving this machine's sessions to a browser that is not on it costs a
/// streaming HTTP client that no Provider consuming this crate as a library has
/// any use for, so it is opt-in.
#[cfg(feature = "serve")]
mod serve;
#[cfg(feature = "session")]
mod session;
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

pub use app_icon::{APP_ICON_ASSET_PATH, APP_ICON_PATH, UiAppIcon};
pub use error::{Error, Result};
pub use frontend::{
    CONFIG_DIR_ENV, FrontendLocation, FrontendManifest, FrontendScope, SAFE_MODE_ENV,
    SAFE_MODE_HINT, UiFrontend, discover_frontend, frontend_root, global_afui_root, load_frontend,
    safe_mode_from_env, validate_frontend_identifier,
};
pub use frontend_script::{reject_frontend_script, reject_rendered_script};
pub use lan::primary_lan_ipv4;

#[cfg(feature = "session")]
pub use access_token::{UiAccessToken, UiExpiry};
#[cfg(feature = "session")]
pub use channel::{
    DEFAULT_UI_SESSION_CHANNEL_CAPACITY, MAX_UI_SESSION_CHANNEL_MESSAGE_BYTES, UiSessionChannel,
};
#[cfg(feature = "session")]
pub use delivery::{ActiveUiDelivery, DELIVERY_ENV, UiDeliveryMode, UiDeliveryPlan};
#[cfg(feature = "session")]
pub use host::{SESSION_CHANNEL_PATH, SESSION_END_PATH, UiHost, UiMount};
#[cfg(feature = "page")]
pub use page::{
    UiPage, UiPageBuilder, UiPageEnvironment, UiPageFilterResult, UiPageSerializedValue,
    UiPageValue,
};
#[cfg(feature = "session")]
pub use registry::{
    NO_REGISTRY_ENV, UiSessionEntry, find_session, list_sessions, sessions_directory,
};
#[cfg(feature = "session")]
pub use security::{OriginDecision, UiCspNonce, UiSecurityPolicy};
#[cfg(feature = "serve")]
pub use serve::{ServedSessions, SessionServer, UiIdlePolicy};
#[cfg(feature = "session")]
pub use session::{
    ActiveUiSession, Outcome, UiCompletion, UiSession, UiSessionId, UiSessionMetadata,
    UiUrlDelivery,
};
#[cfg(feature = "session")]
pub use upstream::{AnnouncedUiUpstream, UiUpstream};
#[cfg(feature = "session")]
pub use window::{BROWSER_BINARY_ENV, UiWindow, UiWindowConfig};

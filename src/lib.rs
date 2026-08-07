//! Small, domain-agnostic primitives for handing a web UI to a person.
//!
//! AFUI owns delivery and lifetime. Providers own their Axum router, domain
//! validation, and the meaning of a completed result.

mod error;
mod frontend;
mod frontend_script;

/// Handing a UI to a person costs a server; keeping one honest does not.
///
/// Everything under `session` binds a port, mints a credential, or launches a
/// browser, and that is where axum and tokio come from. The guard and the
/// frontend overlay above need neither, so a Provider that only has templates
/// to check — afmail's, in a build with its Review UI compiled out — takes this
/// crate without them.
#[cfg(feature = "session")]
mod access_token;
#[cfg(feature = "session")]
mod host;
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

/// `AFUI_CONFIG_DIR` and `AFUI_NO_REGISTRY` are process-wide, so every unit
/// test that points one at its own temp tree takes this first. One lock for the
/// whole crate rather than one per module: the modules share the variables, so
/// separate locks would not serialise anything.
#[cfg(test)]
pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub use error::{Error, Result};
pub use frontend::{
    CONFIG_DIR_ENV, FrontendLocation, FrontendManifest, FrontendScope, SAFE_MODE_ENV,
    SAFE_MODE_HINT, UiFrontend, discover_frontend, frontend_root, global_afui_root, load_frontend,
    safe_mode_from_env, validate_frontend_identifier,
};
pub use frontend_script::{reject_frontend_script, reject_rendered_script};

#[cfg(feature = "session")]
pub use access_token::{UiAccessToken, UiExpiry};
#[cfg(feature = "session")]
pub use host::{UiHost, UiMount};
#[cfg(feature = "session")]
pub use registry::{
    NO_REGISTRY_ENV, UiSessionEntry, find_session, list_sessions, sessions_directory,
};
#[cfg(feature = "session")]
pub use security::{OriginDecision, UiCspNonce, UiSecurityPolicy};
#[cfg(feature = "serve")]
pub use serve::{ServedSessions, SessionServer};
#[cfg(feature = "session")]
pub use session::{
    ActiveUiSession, Outcome, UiCompletion, UiSession, UiSessionId, UiSessionMetadata,
    UiUrlDelivery,
};
#[cfg(feature = "session")]
pub use upstream::{AnnouncedUiUpstream, UiUpstream};
#[cfg(feature = "session")]
pub use window::{BROWSER_BINARY_ENV, UiWindow, UiWindowConfig};

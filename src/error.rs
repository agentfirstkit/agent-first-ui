use std::fmt;
use std::io;

use crate::frontend::SAFE_MODE_HINT;

/// An AFUI delivery or lifecycle error.
#[derive(Debug)]
pub enum Error {
    InvalidIdentifier {
        field: &'static str,
        value: String,
    },
    InvalidBaseUrl {
        value: String,
    },
    InvalidUpstreamUrl {
        value: String,
    },
    /// A host that cannot carry one origin per session.
    ///
    /// Every session becomes a name under this one, so it has to be a name
    /// something can be put *under*: an address literal has no subdomains, and a
    /// label longer than DNS allows would resolve nowhere and match no wildcard
    /// certificate.
    InvalidSessionOriginHost {
        value: String,
        reason: &'static str,
    },
    InvalidTtl,
    /// `--mode` or `AFUI_DELIVERY` named something other than `window`,
    /// `link`, or `session`.
    InvalidDeliveryMode {
        value: String,
    },
    /// Provider-owned application icon bytes that cannot be handed to a
    /// browser or desktop window as SVG.
    InvalidAppIcon {
        reason: &'static str,
    },
    /// The resolved delivery is `link`, but the Provider never called
    /// `UiDeliveryPlan::with_link_expiry` to say it offers one.
    LinkDeliveryNotDeclared,
    /// A session can expose at most one bidirectional channel.
    SessionChannelAlreadyAttached,
    /// A bounded channel must have somewhere to put a message.
    InvalidSessionChannelCapacity,
    /// The session ended, or the surface/controller side dropped its channel.
    SessionChannelClosed,
    /// One payload exceeded the channel's fixed memory and wire bound.
    SessionChannelMessageTooLarge {
        size_bytes: usize,
        max_bytes: usize,
    },
    /// A controller value could not be encoded for the surface.
    SessionChannelEncode(serde_json::Error),
    /// A surface payload could not be decoded into the Provider's input type.
    SessionChannelDecode(serde_json::Error),
    /// No LAN-reachable IPv4 address could be found to advertise for `link`
    /// delivery.
    LinkAddressUnavailable(io::Error),
    FrontendManifest {
        message: String,
    },
    /// A frontend that exists and is trusted, but that AFUI could not read.
    ///
    /// Never a quiet built-in page: a person looking at the Provider's own UI
    /// while believing they are looking at theirs is the one outcome this
    /// whole layer has to avoid, so the failure is loud and names the way out.
    FrontendUnreadable {
        provider_id: String,
        ui_kind: String,
        message: String,
    },
    /// A frontend written against a different version of the Provider's typed
    /// UI contract.
    FrontendIncompatible {
        provider_id: String,
        ui_kind: String,
        frontend_id: String,
        declared: String,
        implemented: String,
    },
    /// Markup an override may not supply, found inside a file whose name did
    /// not say it was code.
    FrontendScript {
        label: String,
        violation: &'static str,
    },
    /// A file name an override may not supply, or may not reach.
    FrontendFileName {
        name: String,
        reason: &'static str,
    },
    /// A `UiPage` template failed to compile or render. `frontend_id` is
    /// `None` when the failing source was the Provider's own fallback rather
    /// than an override.
    PageRender {
        frontend_id: Option<String>,
        template: String,
        detail: String,
    },
    /// A `UiPage` rendered, but left out something its own runtime depends
    /// on: a required element id, or the runtime marker missing or appearing
    /// more than once.
    PageIncomplete {
        frontend_id: Option<String>,
        what: String,
    },
    ConfigDirectoryUnavailable,
    Random(io::Error),
    Io(io::Error),
    WindowBinaryNotFound,
    WindowLaunch {
        binary: String,
        source: io::Error,
    },
    WindowWait(io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidIdentifier { field, value } => {
                write!(
                    formatter,
                    "{field} `{value}` must match [a-z][a-z0-9_-]{{0,63}}"
                )
            }
            Self::InvalidBaseUrl { value } => write!(
                formatter,
                "UI mount base URL `{value}` must be an absolute http:// URL with a host and no \
                 query or fragment, for example http://127.0.0.1:8080/ui/"
            ),
            Self::InvalidUpstreamUrl { value } => write!(
                formatter,
                "announced UI upstream URL `{value}` must be an absolute http:// URL with a host \
                 and no fragment, for example http://127.0.0.1:9222/takeover/panel?handoff_secret=…"
            ),
            Self::InvalidSessionOriginHost { value, reason } => write!(
                formatter,
                "`{value}` cannot host one origin per session: {reason}; give a hostname that \
                 `*.<host>` resolves under, for example localhost"
            ),
            Self::InvalidTtl => write!(formatter, "UI session TTL must be greater than zero"),
            Self::InvalidDeliveryMode { value } => write!(
                formatter,
                "`{value}` is not a UI delivery mode; use one of `window`, `link`, `session`"
            ),
            Self::InvalidAppIcon { reason } => {
                write!(formatter, "invalid Provider application icon: {reason}")
            }
            Self::LinkDeliveryNotDeclared => write!(
                formatter,
                "this Provider does not offer `link` delivery: it never called \
                 `UiDeliveryPlan::with_link_expiry`"
            ),
            Self::SessionChannelAlreadyAttached => {
                write!(formatter, "this UI session already has a channel attached")
            }
            Self::InvalidSessionChannelCapacity => {
                write!(
                    formatter,
                    "UI session channel capacity must be greater than zero"
                )
            }
            Self::SessionChannelClosed => {
                write!(formatter, "the UI session channel is closed")
            }
            Self::SessionChannelMessageTooLarge {
                size_bytes,
                max_bytes,
            } => write!(
                formatter,
                "UI session channel message is {size_bytes} bytes, exceeding the {max_bytes}-byte limit"
            ),
            Self::SessionChannelEncode(error) => {
                write!(
                    formatter,
                    "could not encode UI session channel message: {error}"
                )
            }
            Self::SessionChannelDecode(error) => {
                write!(
                    formatter,
                    "could not decode UI session channel message: {error}"
                )
            }
            Self::LinkAddressUnavailable(error) => write!(
                formatter,
                "could not find a LAN-reachable address to advertise for `link` delivery: \
                 {error}; `link` delivery is not available on this machine"
            ),
            Self::FrontendManifest { message } => {
                write!(formatter, "invalid frontend.json: {message}")
            }
            Self::FrontendUnreadable {
                provider_id,
                ui_kind,
                message,
            } => write!(
                formatter,
                "could not load the {provider_id} {ui_kind} frontend: {message}; the built-in UI \
                 was not quietly substituted"
            ),
            Self::FrontendIncompatible {
                provider_id,
                ui_kind,
                frontend_id,
                declared,
                implemented,
            } => write!(
                formatter,
                "{provider_id} {ui_kind} frontend `{frontend_id}` declares ui_api_version \
                 {declared}, but {provider_id} implements {implemented}; the built-in UI was not \
                 quietly substituted"
            ),
            Self::FrontendScript { label, violation } => write!(
                formatter,
                "frontend file `{label}` contains forbidden JavaScript-capable content: \
                 {violation}; a Provider supplies the behaviour, a frontend supplies the page"
            ),
            Self::FrontendFileName { name, reason } => {
                write!(formatter, "a frontend may not supply `{name}`: {reason}")
            }
            Self::PageRender {
                frontend_id,
                template,
                detail,
            } => match frontend_id {
                Some(frontend_id) => write!(
                    formatter,
                    "frontend `{frontend_id}` could not render `{template}`: {detail}; \
                     {SAFE_MODE_HINT}"
                ),
                None => write!(
                    formatter,
                    "could not render the built-in `{template}`: {detail}"
                ),
            },
            Self::PageIncomplete { frontend_id, what } => match frontend_id {
                Some(frontend_id) => write!(
                    formatter,
                    "frontend `{frontend_id}` is incomplete: {what}; {SAFE_MODE_HINT}"
                ),
                None => write!(formatter, "the built-in page is incomplete: {what}"),
            },
            Self::ConfigDirectoryUnavailable => {
                write!(
                    formatter,
                    "could not determine the system configuration directory"
                )
            }
            Self::Random(error) => write!(
                formatter,
                "could not generate secure UI credential: {error}"
            ),
            Self::Io(error) => write!(formatter, "UI session I/O failed: {error}"),
            Self::WindowBinaryNotFound => write!(
                formatter,
                "could not find Chromium, Google Chrome, or Microsoft Edge; set `AFUI_BROWSER_BINARY` \
                 to an explicit browser binary, or call UiWindowConfig::with_browser_binary"
            ),
            Self::WindowLaunch { binary, source } => {
                write!(
                    formatter,
                    "could not launch UI window with `{binary}`: {source}"
                )
            }
            Self::WindowWait(error) => write!(formatter, "could not wait for UI window: {error}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Random(error) | Self::Io(error) | Self::WindowWait(error) => Some(error),
            Self::SessionChannelEncode(error) | Self::SessionChannelDecode(error) => Some(error),
            Self::WindowLaunch { source, .. } => Some(source),
            Self::LinkAddressUnavailable(error) => Some(error),
            Self::InvalidIdentifier { .. }
            | Self::InvalidBaseUrl { .. }
            | Self::InvalidUpstreamUrl { .. }
            | Self::InvalidSessionOriginHost { .. }
            | Self::InvalidTtl
            | Self::InvalidDeliveryMode { .. }
            | Self::InvalidAppIcon { .. }
            | Self::LinkDeliveryNotDeclared
            | Self::SessionChannelAlreadyAttached
            | Self::InvalidSessionChannelCapacity
            | Self::SessionChannelClosed
            | Self::SessionChannelMessageTooLarge { .. }
            | Self::FrontendManifest { .. }
            | Self::FrontendUnreadable { .. }
            | Self::FrontendIncompatible { .. }
            | Self::FrontendScript { .. }
            | Self::FrontendFileName { .. }
            | Self::PageRender { .. }
            | Self::PageIncomplete { .. }
            | Self::ConfigDirectoryUnavailable
            | Self::WindowBinaryNotFound => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

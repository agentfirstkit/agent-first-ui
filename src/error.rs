use std::fmt;
use std::io;
use std::path::PathBuf;

use crate::frontend::SAFE_MODE_HINT;

/// An AFUI delivery or lifecycle error.
#[derive(Debug)]
pub enum Error {
    /// A standard question violates its bounded input contract.
    InvalidAsk {
        reason: &'static str,
    },
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
    /// A remote shell can proxy plain HTTP upstreams only. HTTPS remains a
    /// valid source for a local window, where the browser talks to it directly.
    UpstreamDeliveryRequiresHttp {
        mode: &'static str,
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
    /// The delivery a `--mode` flag or `AFUI_DELIVERY` resolved to is one this
    /// Provider does not offer.
    ///
    /// Today that can only be `link`, which is the one mode that widens a
    /// listener beyond loopback and therefore the one a Provider has to ask
    /// for. Raised while the plan is being built rather than when delivery
    /// starts, so a Provider that announces its resolved mode first never
    /// announces one that was already refused.
    DeliveryModeNotOffered {
        requested: crate::UiDeliveryMode,
    },
    /// A session can expose at most one AFUI runtime.
    SessionRuntimeAlreadyAttached,
    /// A runtime was attached to a page whose CSP forbids same-origin
    /// connections.
    SessionRuntimeUnreachable,
    /// Runtime call and event capacities must both be non-zero.
    InvalidSessionRuntimeCapacity,
    /// The session ended or its AFUI runtime was closed.
    SessionRuntimeClosed,
    /// A call already reached a terminal reply or no longer exists.
    SessionRuntimeCallClosed,
    /// An in-process runtime operation could not take its bounded state lock.
    SessionRuntimeBusy,
    /// One runtime frame exceeded the fixed memory and wire bound.
    SessionRuntimeMessageTooLarge {
        size_bytes: usize,
        max_bytes: usize,
    },
    /// This runtime was attached without a managed-blob policy.
    SessionRuntimeBlobsDisabled,
    /// Managed bytes exceeded the declared bound or used an undeclared type.
    SessionRuntimeBlobRejected {
        media_type: String,
        size_bytes: usize,
        max_bytes: u64,
    },
    /// A Provider value could not be encoded into an AFUI runtime frame.
    SessionRuntimeEncode(serde_json::Error),
    /// A surface frame could not be decoded as the AFUI runtime protocol.
    SessionRuntimeDecode(serde_json::Error),
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
    AttentionConfigInvalid {
        path: PathBuf,
        source: serde_json::Error,
    },
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
            Self::InvalidAsk { reason } => write!(formatter, "invalid question: {reason}"),
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
            Self::InvalidUpstreamUrl { .. } => write!(
                formatter,
                "announced UI upstream URL must be an absolute http:// or https:// URL with a \
                 host and no fragment"
            ),
            Self::UpstreamDeliveryRequiresHttp { mode } => write!(
                formatter,
                "`{mode}` delivery can proxy only an http:// upstream; use `window` for this \
                 https:// upstream or expose a private http:// source to AFUI"
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
            Self::DeliveryModeNotOffered { requested } => write!(
                formatter,
                "`{}` delivery is not offered here; this Provider offers {}",
                requested.as_str(),
                offered_modes()
            ),
            Self::SessionRuntimeAlreadyAttached => {
                write!(formatter, "this UI session already has a runtime attached")
            }
            Self::SessionRuntimeUnreachable => write!(
                formatter,
                "this UI session has a runtime attached, but its page is served under a policy \
                 that forbids connecting back to the session; build the page policy with \
                 `UiPagePolicy::allow_runtime`"
            ),
            Self::InvalidSessionRuntimeCapacity => write!(
                formatter,
                "UI session runtime call and event capacities must be greater than zero"
            ),
            Self::SessionRuntimeClosed => write!(formatter, "the UI session runtime is closed"),
            Self::SessionRuntimeCallClosed => {
                write!(formatter, "the UI session runtime call is already closed")
            }
            Self::SessionRuntimeBusy => {
                write!(formatter, "the UI session runtime is busy")
            }
            Self::SessionRuntimeMessageTooLarge {
                size_bytes,
                max_bytes,
            } => write!(
                formatter,
                "UI session runtime frame is {size_bytes} bytes, exceeding the {max_bytes}-byte limit"
            ),
            Self::SessionRuntimeBlobsDisabled => write!(
                formatter,
                "this UI session runtime has no managed-blob policy"
            ),
            Self::SessionRuntimeBlobRejected {
                media_type,
                size_bytes,
                max_bytes,
            } => write!(
                formatter,
                "UI session blob type `{media_type}` or its {size_bytes}-byte body is outside the declared {max_bytes}-byte policy"
            ),
            Self::SessionRuntimeEncode(error) => {
                write!(
                    formatter,
                    "could not encode UI session runtime value: {error}"
                )
            }
            Self::SessionRuntimeDecode(error) => {
                write!(
                    formatter,
                    "could not decode UI session runtime frame: {error}"
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
                    "frontend `{frontend_id}` could not render `{template}`: {detail}"
                ),
                None => write!(
                    formatter,
                    "could not render the built-in `{template}`: {detail}"
                ),
            },
            Self::PageIncomplete { frontend_id, what } => match frontend_id {
                Some(frontend_id) => {
                    write!(formatter, "frontend `{frontend_id}` is incomplete: {what}")
                }
                None => write!(formatter, "the built-in page is incomplete: {what}"),
            },
            Self::ConfigDirectoryUnavailable => {
                write!(
                    formatter,
                    "could not determine the system configuration directory"
                )
            }
            Self::AttentionConfigInvalid { path, source } => write!(
                formatter,
                "invalid AFUI attention config at `{}`: {source}",
                path.display()
            ),
            Self::Random(error) => write!(
                formatter,
                "could not generate secure UI credential: {error}"
            ),
            Self::Io(error) => write!(formatter, "UI session I/O failed: {error}"),
            Self::WindowBinaryNotFound => write!(
                formatter,
                "could not find Chromium, Google Chrome, or Microsoft Edge"
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

/// Suggested by [`Error::hint`] for a window that could not be opened because
/// nothing on this machine can open one.
const BROWSER_HINT: &str = "Install a Chromium-family browser, set `AFUI_BROWSER_BINARY` to an \
                            explicit browser binary, or call \
                            `UiWindowConfig::with_browser_binary`.";

/// What went wrong, classified — the axis a Provider maps into its own error
/// vocabulary.
///
/// A closed enum rather than a string, for the reason the string version was
/// replaced. [`Error`]'s variants are AFUI's business and change; this is the
/// stable axis above them, and a Provider translating AFUI's failures into its
/// own codes is doing the right thing — but it can only do that *completely*
/// if the compiler can tell it when it has missed one. A `&'static str` made
/// that impossible: nothing exhaustively matches a string, so every consumer
/// was forced into a wildcard arm, and a wildcard is where a new AFUI
/// classification goes to be mis-reported. Three Providers here defaulted
/// theirs to "the frontend could not be read", which is what a closed session
/// runtime and a machine with no browser were both reported as.
///
/// Deliberately not `#[non_exhaustive]`. Adding a classification *should*
/// stop a Provider's build until someone decides what it means to them —
/// that is the same promise the match below makes inside this crate.
///
/// [`as_str`](Self::as_str) and [`Display`](fmt::Display) give the stable
/// `snake_case` word for an error code built by prefixing:
/// `format!("myprovider_{}", error.kind())`. Those words carry no domain —
/// there is no "review", no "panel", no "payment" here — and the set is
/// written down in `docs/providers.md` and pinned by a test.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum UiErrorKind {
    /// An identifier, URL, TTL, or icon AFUI cannot accept.
    InvalidArgument,
    /// `--mode` or `AFUI_DELIVERY` named something other than the three words.
    DeliveryModeInvalid,
    /// The resolved delivery is one this Provider did not offer.
    DeliveryModeNotOffered,
    /// An `https://` upstream behind a delivery that proxies plain HTTP.
    UpstreamNotProxyable,
    /// No LAN-reachable address to advertise a `link` on.
    LinkAddressUnavailable,
    /// A second runtime, a zero capacity, or a page that cannot reach its
    /// runtime.
    RuntimeMisconfigured,
    /// The session or one of its calls has ended.
    RuntimeClosed,
    /// A bounded in-process runtime state lock was temporarily unavailable.
    RuntimeBusy,
    /// One encoded runtime frame exceeded the fixed wire bound.
    RuntimeMessageTooLarge,
    /// Managed blobs were not enabled, or bytes violated their declared policy.
    RuntimeBlob,
    /// A runtime payload could not be encoded or decoded.
    RuntimePayload,
    /// A frontend that exists and is trusted but could not be read.
    FrontendUnreadable,
    /// A frontend written against another `ui_api_version`.
    FrontendIncompatible,
    /// Markup or a file name an override may not supply.
    FrontendUnsafe,
    /// A template failed to compile or render.
    PageRender,
    /// A rendered page is missing an element id or its runtime marker.
    PageIncomplete,
    /// AFUI's own configuration directory or attention config.
    ConfigUnreadable,
    /// No browser found, or one that would not launch.
    WindowUnavailable,
    /// Waiting on an open window failed.
    WindowWaitFailed,
    /// Anything else AFUI could not read, write, or generate.
    Io,
}

impl UiErrorKind {
    /// The stable `snake_case` word, for a Provider building its own code out
    /// of this one.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidArgument => "invalid_argument",
            Self::DeliveryModeInvalid => "delivery_mode_invalid",
            Self::DeliveryModeNotOffered => "delivery_mode_not_offered",
            Self::UpstreamNotProxyable => "upstream_not_proxyable",
            Self::LinkAddressUnavailable => "link_address_unavailable",
            Self::RuntimeMisconfigured => "runtime_misconfigured",
            Self::RuntimeClosed => "runtime_closed",
            Self::RuntimeBusy => "runtime_busy",
            Self::RuntimeMessageTooLarge => "runtime_message_too_large",
            Self::RuntimeBlob => "runtime_blob",
            Self::RuntimePayload => "runtime_payload",
            Self::FrontendUnreadable => "frontend_unreadable",
            Self::FrontendIncompatible => "frontend_incompatible",
            Self::FrontendUnsafe => "frontend_unsafe",
            Self::PageRender => "page_render",
            Self::PageIncomplete => "page_incomplete",
            Self::ConfigUnreadable => "config_unreadable",
            Self::WindowUnavailable => "window_unavailable",
            Self::WindowWaitFailed => "window_wait_failed",
            Self::Io => "io",
        }
    }

    /// Every classification, so a caller — or a test — can walk the axis
    /// rather than keep a second copy of it.
    pub const ALL: &'static [Self] = &[
        Self::InvalidArgument,
        Self::DeliveryModeInvalid,
        Self::DeliveryModeNotOffered,
        Self::UpstreamNotProxyable,
        Self::LinkAddressUnavailable,
        Self::RuntimeMisconfigured,
        Self::RuntimeClosed,
        Self::RuntimeBusy,
        Self::RuntimeMessageTooLarge,
        Self::RuntimeBlob,
        Self::RuntimePayload,
        Self::FrontendUnreadable,
        Self::FrontendIncompatible,
        Self::FrontendUnsafe,
        Self::PageRender,
        Self::PageIncomplete,
        Self::ConfigUnreadable,
        Self::WindowUnavailable,
        Self::WindowWaitFailed,
        Self::Io,
    ];
}

impl fmt::Display for UiErrorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Error {
    /// A stable, domain-free classification for this failure.
    ///
    /// Consumers were writing 20-to-70-line matches over every variant only to
    /// arrive at their own error code, and every new variant silently fell into
    /// whichever arm was last. This turns that into a prefix — a Provider maps
    /// [`UiErrorKind::FrontendUnreadable`] to `<provider>_frontend_unreadable`
    /// and stops caring which of several variants produced it — or into an
    /// exhaustive match that stops compiling when AFUI adds one.
    #[must_use]
    pub fn kind(&self) -> UiErrorKind {
        // No wildcard arm, deliberately: this is the compile-time half of the
        // promise. A new variant does not compile until someone decides what
        // it is classified as, and `tests/error_kinds.rs` is the other half —
        // it fails if that decision invents a code nobody wrote down.
        match self {
            Self::InvalidAsk { .. }
            | Self::InvalidIdentifier { .. }
            | Self::InvalidBaseUrl { .. }
            | Self::InvalidUpstreamUrl { .. }
            | Self::InvalidSessionOriginHost { .. }
            | Self::InvalidTtl
            | Self::InvalidAppIcon { .. } => UiErrorKind::InvalidArgument,
            Self::InvalidDeliveryMode { .. } => UiErrorKind::DeliveryModeInvalid,
            Self::DeliveryModeNotOffered { .. } => UiErrorKind::DeliveryModeNotOffered,
            Self::UpstreamDeliveryRequiresHttp { .. } => UiErrorKind::UpstreamNotProxyable,
            Self::LinkAddressUnavailable(_) => UiErrorKind::LinkAddressUnavailable,
            Self::SessionRuntimeAlreadyAttached
            | Self::InvalidSessionRuntimeCapacity
            | Self::SessionRuntimeUnreachable => UiErrorKind::RuntimeMisconfigured,
            Self::SessionRuntimeClosed | Self::SessionRuntimeCallClosed => {
                UiErrorKind::RuntimeClosed
            }
            Self::SessionRuntimeBusy => UiErrorKind::RuntimeBusy,
            Self::SessionRuntimeMessageTooLarge { .. } => UiErrorKind::RuntimeMessageTooLarge,
            Self::SessionRuntimeBlobsDisabled | Self::SessionRuntimeBlobRejected { .. } => {
                UiErrorKind::RuntimeBlob
            }
            Self::SessionRuntimeEncode(_) | Self::SessionRuntimeDecode(_) => {
                UiErrorKind::RuntimePayload
            }
            Self::FrontendManifest { .. } | Self::FrontendUnreadable { .. } => {
                UiErrorKind::FrontendUnreadable
            }
            Self::FrontendIncompatible { .. } => UiErrorKind::FrontendIncompatible,
            Self::FrontendScript { .. } | Self::FrontendFileName { .. } => {
                UiErrorKind::FrontendUnsafe
            }
            Self::PageRender { .. } => UiErrorKind::PageRender,
            Self::PageIncomplete { .. } => UiErrorKind::PageIncomplete,
            Self::ConfigDirectoryUnavailable | Self::AttentionConfigInvalid { .. } => {
                UiErrorKind::ConfigUnreadable
            }
            Self::WindowBinaryNotFound | Self::WindowLaunch { .. } => {
                UiErrorKind::WindowUnavailable
            }
            Self::WindowWait(_) => UiErrorKind::WindowWaitFailed,
            Self::Random(_) | Self::Io(_) => UiErrorKind::Io,
        }
    }

    /// What a person can actually do about this, when there is something.
    ///
    /// Kept out of [`Display`](fmt::Display) rather than embedded in it. A
    /// caller that has a hint field of its own would otherwise print the same
    /// sentence twice — once inside the message, once beside it — and the only
    /// way out was a second conversion path that stripped it back off. Now the
    /// message says what happened and this says what to do, and a caller with
    /// nowhere to put a recovery action simply does not ask.
    #[must_use]
    pub fn hint(&self) -> Option<&'static str> {
        match self {
            // Only an override can be turned off; a built-in page that fails
            // to render is the Provider's own bug, and safe mode would change
            // nothing about it.
            Self::PageRender {
                frontend_id: Some(_),
                ..
            }
            | Self::PageIncomplete {
                frontend_id: Some(_),
                ..
            }
            | Self::FrontendManifest { .. }
            | Self::FrontendUnreadable { .. }
            | Self::FrontendIncompatible { .. }
            | Self::FrontendScript { .. }
            | Self::FrontendFileName { .. } => Some(SAFE_MODE_HINT),
            Self::WindowBinaryNotFound | Self::WindowLaunch { .. } => Some(BROWSER_HINT),
            _ => None,
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Random(error) | Self::Io(error) | Self::WindowWait(error) => Some(error),
            Self::SessionRuntimeEncode(error) | Self::SessionRuntimeDecode(error) => Some(error),
            Self::AttentionConfigInvalid { source, .. } => Some(source),
            Self::WindowLaunch { source, .. } => Some(source),
            Self::LinkAddressUnavailable(error) => Some(error),
            Self::InvalidAsk { .. }
            | Self::InvalidIdentifier { .. }
            | Self::InvalidBaseUrl { .. }
            | Self::InvalidUpstreamUrl { .. }
            | Self::UpstreamDeliveryRequiresHttp { .. }
            | Self::InvalidSessionOriginHost { .. }
            | Self::InvalidTtl
            | Self::InvalidDeliveryMode { .. }
            | Self::InvalidAppIcon { .. }
            | Self::DeliveryModeNotOffered { .. }
            | Self::SessionRuntimeAlreadyAttached
            | Self::SessionRuntimeUnreachable
            | Self::InvalidSessionRuntimeCapacity
            | Self::SessionRuntimeClosed
            | Self::SessionRuntimeCallClosed
            | Self::SessionRuntimeBusy
            | Self::SessionRuntimeMessageTooLarge { .. }
            | Self::SessionRuntimeBlobsDisabled
            | Self::SessionRuntimeBlobRejected { .. }
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

/// The deliveries a plan built without the `link` opt-in resolves within,
/// spelled the way a person types them. Built from the word list rather than
/// written out, so a refusal cannot name a mode that no longer parses.
fn offered_modes() -> String {
    crate::UiDeliveryMode::LOCAL_ONLY
        .iter()
        .map(|mode| format!("`{}`", mode.as_str()))
        .collect::<Vec<_>>()
        .join(", ")
}

impl From<io::Error> for Error {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

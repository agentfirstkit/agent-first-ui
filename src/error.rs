use std::fmt;
use std::io;

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
                "could not find Chromium, Google Chrome, or Microsoft Edge; set an explicit browser binary"
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
            Self::WindowLaunch { source, .. } => Some(source),
            Self::InvalidIdentifier { .. }
            | Self::InvalidBaseUrl { .. }
            | Self::InvalidUpstreamUrl { .. }
            | Self::InvalidSessionOriginHost { .. }
            | Self::InvalidTtl
            | Self::FrontendManifest { .. }
            | Self::FrontendUnreadable { .. }
            | Self::FrontendIncompatible { .. }
            | Self::FrontendScript { .. }
            | Self::FrontendFileName { .. }
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

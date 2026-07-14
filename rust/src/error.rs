use std::error::Error;
use std::fmt;

/// Every fallible operation in this crate returns this error. There are no
/// panics: untrusted publisher data must never crash a participant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentFirstUiError {
    UnsupportedVersion { found: String },
    MissingSpace,
    UnsupportedMessageType { message_type: String },
    CrossSectionEmit { author: String, path: String },
    InvalidIdentifier { identifier: String },
    DuplicateIdentifier { identifier: String },
    InvalidBinding { binding: String, reason: String },
    InvalidJsonPointer { pointer: String, reason: String },
    PathNotFound { path: String },
    InvalidArrayIndex { path: String, index: String },
    NonMonotonicSequence { current: u64, incoming: u64 },
    InvalidPatch { message: String },
    PatchTestFailed { path: String },
    InvalidSpace { message: String },
    InvalidRequest { message: String },
    Serde { message: String },
    WellFormedness { issues: Vec<String> },
}

impl fmt::Display for AgentFirstUiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedVersion { found } => {
                write!(f, "unsupported protocol version `{found}`")
            }
            Self::MissingSpace => write!(f, "cannot apply an emit without a current space"),
            Self::UnsupportedMessageType { message_type } => {
                write!(
                    f,
                    "unsupported message type `{message_type}` in this context"
                )
            }
            Self::CrossSectionEmit { author, path } => write!(
                f,
                "author `{author}` emitted into a section it does not own: `{path}`"
            ),
            Self::InvalidIdentifier { identifier } => {
                write!(f, "invalid identifier `{identifier}`")
            }
            Self::DuplicateIdentifier { identifier } => {
                write!(f, "duplicate identifier `{identifier}`")
            }
            Self::InvalidBinding { binding, reason } => {
                write!(f, "invalid binding `{binding}`: {reason}")
            }
            Self::InvalidJsonPointer { pointer, reason } => {
                write!(f, "invalid JSON Pointer `{pointer}`: {reason}")
            }
            Self::PathNotFound { path } => write!(f, "JSON Pointer path not found: {path}"),
            Self::InvalidArrayIndex { path, index } => {
                write!(f, "invalid array index `{index}` at JSON Pointer `{path}`")
            }
            Self::NonMonotonicSequence { current, incoming } => write!(
                f,
                "emit sequence `{incoming}` must be greater than current sequence `{current}`"
            ),
            Self::InvalidPatch { message } => write!(f, "invalid JSON Patch: {message}"),
            Self::PatchTestFailed { path } => {
                write!(f, "JSON Patch test operation failed at `{path}`")
            }
            Self::InvalidSpace { message } => write!(f, "invalid space: {message}"),
            Self::InvalidRequest { message } => write!(f, "invalid request: {message}"),
            Self::Serde { message } => write!(f, "serde error: {message}"),
            Self::WellFormedness { issues } => {
                write!(f, "space is not well-formed: {}", issues.join("; "))
            }
        }
    }
}

impl Error for AgentFirstUiError {}

impl From<serde_json::Error> for AgentFirstUiError {
    fn from(value: serde_json::Error) -> Self {
        Self::Serde {
            message: value.to_string(),
        }
    }
}

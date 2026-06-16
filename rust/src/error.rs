use std::error::Error;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AfuiError {
    UnsupportedVersion { found: String },
    MissingSnapshot,
    UnsupportedMessageType { message_type: String },
    InvalidId { id: String },
    DuplicateId { id: String },
    InvalidBinding { binding: String, reason: String },
    InvalidJsonPointer { pointer: String, reason: String },
    PathNotFound { path: String },
    InvalidArrayIndex { path: String, index: String },
    InvalidPatch { message: String },
    PatchTestFailed { path: String },
    InvalidState { message: String },
    InvalidTheme { message: String },
    Serde { message: String },
    WellFormedness { issues: Vec<String> },
}

impl fmt::Display for AfuiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedVersion { found } => {
                write!(f, "unsupported AFUI version `{found}`")
            }
            Self::MissingSnapshot => write!(f, "cannot apply a patch without a current snapshot"),
            Self::UnsupportedMessageType { message_type } => {
                write!(
                    f,
                    "unsupported AFUI message type `{message_type}` in this context"
                )
            }
            Self::InvalidId { id } => write!(f, "invalid AFUI id `{id}`"),
            Self::DuplicateId { id } => write!(f, "duplicate AFUI id `{id}`"),
            Self::InvalidBinding { binding, reason } => {
                write!(f, "invalid AFUI binding `{binding}`: {reason}")
            }
            Self::InvalidJsonPointer { pointer, reason } => {
                write!(f, "invalid JSON Pointer `{pointer}`: {reason}")
            }
            Self::PathNotFound { path } => write!(f, "JSON Pointer path not found: {path}"),
            Self::InvalidArrayIndex { path, index } => {
                write!(f, "invalid array index `{index}` at JSON Pointer `{path}`")
            }
            Self::InvalidPatch { message } => write!(f, "invalid JSON Patch: {message}"),
            Self::PatchTestFailed { path } => {
                write!(f, "JSON Patch test operation failed at `{path}`")
            }
            Self::InvalidState { message } => write!(f, "invalid AFUI state: {message}"),
            Self::InvalidTheme { message } => write!(f, "invalid AFUI theme: {message}"),
            Self::Serde { message } => write!(f, "serde error: {message}"),
            Self::WellFormedness { issues } => {
                write!(f, "AFUI document is not well-formed: {}", issues.join("; "))
            }
        }
    }
}

impl Error for AfuiError {}

impl From<serde_json::Error> for AfuiError {
    fn from(value: serde_json::Error) -> Self {
        Self::Serde {
            message: value.to_string(),
        }
    }
}

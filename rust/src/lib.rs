//! Rust support for Agent-First UI (AFUI) v0.1.
//!
//! AFUI is a data-only operation-surface protocol for trusted host renderers.
//! This crate mirrors the small programmatic layer: serde wire types, scoped
//! binding resolution, RFC 6902 JSON Patch helpers, document well-formedness
//! checks, and deterministic `user_action` assembly.

pub mod action;
pub mod binding;
pub mod error;
pub mod host;
pub mod patch;
pub mod report;
pub mod runtime;
pub mod types;
pub mod validation;

pub use action::{
    action_ids_from_view, assemble_action_input, declared_action_ids, find_action,
    prepare_user_action, redacted_input_for_display, AssembledActionInput,
    PrepareUserActionOptions, PreparedStatus, PreparedUserAction,
};
pub use binding::{get_dot_path, resolve_binding, Binding, BindingContext, BindingScope};
pub use error::AfuiError;
pub use host::{prepare_action_against_snapshot, HostStore, PrepareActionRequest};
pub use patch::{apply_patch, parse_pointer, Patch, PatchOperation};
pub use report::{
    inspect_snapshot, replay_jsonl, replay_messages, AfuiReport, ReplayFailure, ReplayReport,
    ValidationReport,
};
pub use runtime::apply_afui_message;
pub use serde_json::{json, Value as JsonValue};
pub use types::{
    Action, AfuiMessage, AfuiPayload, Document, Id, JsonMap, Risk, Screen, Snapshot, State,
    Surface, Theme, UserAction, View, AFUI_VERSION,
};
pub use validation::{is_valid_id, validate_document, validate_state_bindings};

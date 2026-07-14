//! Rust support for Agent-First UI -- the Space model.
//!
//! A **space** is a shared, addressable tree of facts partitioned into sections,
//! each owned by exactly one participant (its sole writer). Three wire messages
//! carry the protocol: **snapshot** (one delivered projection plus emit-log position),
//! **emit** (an owner's JSON Patch change to its own section), and **request**
//! (a participant asking another owner to invoke a capability).
//!
//! This crate implements the protocol-level types, RFC 6902 JSON Patch helpers,
//! scoped binding resolution, space well-formedness validation, request
//! preparation, projection materialization helpers, and replay/reporting
//! utilities. Audience membership decisions belong to an owner or transport
//! before a message reaches the store; the store validates and advances one
//! delivered projection.
//!
//! For the full normative specification see `spec/protocol.md`; the optional
//! display contract these helpers implement is `spec/view-hint.md`.

pub mod binding;
pub mod builder;
pub mod compose;
pub mod composition;
pub mod error;
pub mod operation;
pub mod profile;
pub mod projection;
pub mod report;
pub mod request;
pub mod runtime;
pub mod store;
pub mod types;
pub mod unit_profile;
pub mod validation;

pub use binding::{Binding, BindingContext, BindingScope, get_dot_path, resolve_binding};
pub use builder::{
    CapabilityBuilder, InputSchemaBuilder, LayoutBuilder, SectionBuilder, SpaceBuilder,
    UnitBuilder, layout,
};
pub use composition::{compose_spaces, ensure_owner_available, insert_owner_section};
pub use error::AgentFirstUiError;
pub use operation::{
    Operation, Operations, actor_state_pointer, apply_operations, parse_pointer,
    set_actor_state_operation,
};
pub use profile::{
    ProfileIssue, ProfileIssueLevel, ProfileValidationReport, ValidationProfile,
    validate_space_profile,
};
pub use projection::{
    Audience, EmitAudienceOverride, EmitProjectionSet, ProjectionPatch, ProjectionSet,
    project_emit_for, project_snapshot_for, project_space_for,
};
pub use report::{
    ReplayFailure, ReplayReport, SpaceReport, ValidationReport, inspect_space, replay_jsonl,
    replay_messages,
};
pub use request::{
    AssembledInput, OwnerRequest, OwnerRequestHandler, OwnerRequestResult, PrepareRequestOptions,
    PrepareRequestSpec, PreparedRequest, PreparedStatus, RequestExecutionStatus,
    assemble_request_input, assemble_request_input_with_actor, find_capability, find_unit,
    owner_request_from_message, owner_request_from_prepared, prepare_request,
    record_path_for_unit_row, redacted_input_for_display, space_binding_to_pointer,
};
pub use runtime::{apply_message, ensure_emit_write_scope};
pub use serde_json::{Value as JsonValue, json};
pub use store::{SpaceStore, prepare_request_against_space};
pub use types::{
    AgentFirstUiMessage, Capability, Id, JsonMap, Layout, MessagePayload, OwnerKey,
    PROTOCOL_VERSION, Prominence, Risk, Salience, SalienceState, Space, Unit,
};
pub use unit_profile::{
    CanonicalUnitKind, FIELD_ACTIVATE_CAPABILITY, FIELD_ALT_BINDING, FIELD_ATTENTION_BINDING,
    FIELD_CAPABILITIES, FIELD_CAPABILITY, FIELD_COUNT_BINDING, FIELD_DEFAULT_PRESENTATION,
    FIELD_FALLBACK, FIELD_FIELDS, FIELD_GENRE, FIELD_INPUT_BINDINGS, FIELD_INTERACTIONS,
    FIELD_LABEL_BINDING, FIELD_PRIMARY_CAPABILITY, FIELD_REFERENCE_BINDING, FIELD_REFERENCE_KIND,
    FIELD_RELATION, FIELD_ROLE, FIELD_ROW_CAPABILITIES, FIELD_SELECTOR_BINDING,
    FIELD_SESSION_ID_BINDING, FIELD_SOURCE_BINDING, FIELD_STATE_BINDING, FIELD_STATUS_BINDING,
    FIELD_SUBMIT_CAPABILITY, FIELD_SURFACE, FIELD_TEXT_BINDING, FIELD_TITLE_BINDING,
    FIELD_VALUE_BINDING, FormField, FormFieldFormat, FormFieldOption, FormFieldType,
    InteractionEffect, InteractionEvent, KIND_CAPABILITIES, KIND_FACTS, KIND_FORM, KIND_LIST,
    KIND_LOG, KIND_MEDIA, KIND_SELECT, KIND_TABLE, KIND_TERMINAL, KIND_TEXT, LayoutRelation,
    LayoutRole, PresentationLevel, ReferenceEnvelope, SurfaceGenre,
};
pub use validation::{is_valid_id, is_valid_owner_key, validate_space, validate_space_bindings};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::types::{Id, JsonMap, OwnerKey, Unit};

pub const KIND_FACTS: &str = "facts";
pub const KIND_TABLE: &str = "table";
pub const KIND_LIST: &str = "list";
pub const KIND_TEXT: &str = "text";
pub const KIND_MEDIA: &str = "media";
pub const KIND_LOG: &str = "log";
pub const KIND_TERMINAL: &str = "terminal";
pub const KIND_SELECT: &str = "select";
pub const KIND_FORM: &str = "form";
pub const KIND_CAPABILITIES: &str = "capabilities";

pub const FIELD_SOURCE_BINDING: &str = "source_binding";
pub const FIELD_SELECTOR_BINDING: &str = "selector_binding";
pub const FIELD_COUNT_BINDING: &str = "count_binding";
pub const FIELD_ATTENTION_BINDING: &str = "attention_binding";
pub const FIELD_VALUE_BINDING: &str = "value_binding";
pub const FIELD_LABEL_BINDING: &str = "label_binding";
pub const FIELD_TEXT_BINDING: &str = "text_binding";
pub const FIELD_ALT_BINDING: &str = "alt_binding";
pub const FIELD_SESSION_ID_BINDING: &str = "session_id_binding";
pub const FIELD_TITLE_BINDING: &str = "title_binding";
pub const FIELD_STATUS_BINDING: &str = "status_binding";
pub const FIELD_STATE_BINDING: &str = "state_binding";
pub const FIELD_INPUT_BINDINGS: &str = "input_bindings";
pub const FIELD_CAPABILITY: &str = "capability";
pub const FIELD_CAPABILITIES: &str = "capabilities";
pub const FIELD_ROW_CAPABILITIES: &str = "row_capabilities";
pub const FIELD_ACTIVATE_CAPABILITY: &str = "activate_capability";
pub const FIELD_PRIMARY_CAPABILITY: &str = "primary_capability";
pub const FIELD_SUBMIT_CAPABILITY: &str = "submit_capability";
pub const FIELD_REFERENCE_BINDING: &str = "reference_binding";
pub const FIELD_REFERENCE_KIND: &str = "reference_kind";
pub const FIELD_DEFAULT_PRESENTATION: &str = "default_presentation";
pub const FIELD_FIELDS: &str = "fields";
pub const FIELD_VISIBLE_WHEN: &str = "visible_when";
pub const FIELD_FALLBACK: &str = "fallback";
pub const FIELD_RELATION: &str = "relation";
pub const FIELD_ROLE: &str = "role";
pub const FIELD_SURFACE: &str = "surface";
pub const FIELD_GENRE: &str = "genre";
pub const FIELD_INTERACTIONS: &str = "interactions";

/// Canonical unit kinds of the View Hint Profile (`spec/view-hint.md`).
///
/// The protocol remains open-ended: unknown kinds are inert data and must render
/// via a safe fallback. This enum is an SDK convenience for producers that want
/// the canonical profile without stringly typed constants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CanonicalUnitKind {
    Facts,
    Table,
    List,
    Text,
    Media,
    Log,
    Terminal,
    Select,
    Form,
    Capabilities,
}

impl CanonicalUnitKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Facts => KIND_FACTS,
            Self::Table => KIND_TABLE,
            Self::List => KIND_LIST,
            Self::Text => KIND_TEXT,
            Self::Media => KIND_MEDIA,
            Self::Log => KIND_LOG,
            Self::Terminal => KIND_TERMINAL,
            Self::Select => KIND_SELECT,
            Self::Form => KIND_FORM,
            Self::Capabilities => KIND_CAPABILITIES,
        }
    }

    pub fn unit(self, id: impl Into<String>) -> Unit {
        Unit::new(id, self.as_str())
    }
}

/// The viewer-owned presentation states every conforming viewer makes reachable.
///
/// An owner may recommend one through `default_presentation`, but this value is
/// never authority and does not constrain how a viewer realizes the state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresentationLevel {
    Glance,
    Compact,
    Full,
}

impl PresentationLevel {
    pub const VALUES: &'static [&'static str] = &["glance", "compact", "full"];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Glance => "glance",
            Self::Compact => "compact",
            Self::Full => "full",
        }
    }
}

/// JSON value type collected by one canonical `form` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FormFieldType {
    String,
    Boolean,
    Number,
    Array,
}

impl FormFieldType {
    pub const VALUES: &'static [&'static str] = &["string", "boolean", "number", "array"];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Boolean => "boolean",
            Self::Number => "number",
            Self::Array => "array",
        }
    }
}

/// Advisory editor treatment for a string- or array-valued canonical `form` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FormFieldFormat {
    Text,
    Textarea,
    Markdown,
    Email,
    Enum,
    EmailList,
    Tags,
}

impl FormFieldFormat {
    pub const VALUES: &'static [&'static str] = &[
        "text",
        "textarea",
        "markdown",
        "email",
        "enum",
        "email_list",
        "tags",
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Textarea => "textarea",
            Self::Markdown => "markdown",
            Self::Email => "email",
            Self::Enum => "enum",
            Self::EmailList => "email_list",
            Self::Tags => "tags",
        }
    }
}

/// One choice for an enum-formatted string field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormFieldOption {
    pub value: String,
    pub label: String,
}

impl FormFieldOption {
    pub fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
        }
    }
}

/// One named value collected by a canonical `form` unit.
///
/// The viewer seeds the editor from `value_binding`, then places the current
/// value at `input:<name>`. On submit, all collected values are direct request
/// input and therefore override same-named `input_bindings`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormField {
    pub name: String,
    pub label: String,
    #[serde(rename = "type")]
    pub field_type: FormFieldType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<FormFieldFormat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_binding: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<FormFieldOption>,
}

impl FormField {
    pub fn new(
        name: impl Into<String>,
        label: impl Into<String>,
        field_type: FormFieldType,
    ) -> Self {
        Self {
            name: name.into(),
            label: label.into(),
            field_type,
            format: None,
            value_binding: None,
            required: false,
            options: Vec::new(),
        }
    }

    pub fn format(mut self, format: FormFieldFormat) -> Self {
        self.format = Some(format);
        self
    }

    pub fn value_binding(mut self, binding: impl Into<String>) -> Self {
        self.value_binding = Some(binding.into());
        self
    }

    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }

    pub fn options<I>(mut self, options: I) -> Self
    where
        I: IntoIterator<Item = FormFieldOption>,
    {
        self.options = options.into_iter().collect();
        self
    }
}

/// Transferable, inert context synthesized by a viewer for its context sink.
///
/// `reference_binding` supplies only `id`; the viewer supplies provenance and
/// domain kind from the reference-bearing unit and may derive `label` from the
/// current record. This is not a repeated source-record shape. It is not a
/// capability, locator, or instruction and never executes or auto-navigates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferenceEnvelope {
    pub owner: OwnerKey,
    pub unit: Id,
    pub kind: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl ReferenceEnvelope {
    pub fn new(
        owner: impl Into<OwnerKey>,
        unit: impl Into<Id>,
        kind: impl Into<String>,
        id: impl Into<String>,
    ) -> Self {
        Self {
            owner: owner.into(),
            unit: unit.into(),
            kind: kind.into(),
            id: id.into(),
            label: None,
        }
    }

    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn from_unit(owner: impl Into<OwnerKey>, unit: &Unit, id: impl Into<String>) -> Self {
        let kind = unit
            .field_as_str(FIELD_REFERENCE_KIND)
            .filter(|kind| !kind.is_empty())
            .unwrap_or(&unit.kind)
            .to_string();
        Self::new(owner, unit.id.clone(), kind, id)
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// Cross-media relationship between members of a layout region.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutRelation {
    Sequence,
    Workspace,
    MasterDetail,
    OverviewDetail,
    Comparison,
    Flow,
    Supporting,
}

impl LayoutRelation {
    pub const VALUES: &'static [&'static str] = &[
        "sequence",
        "workspace",
        "master_detail",
        "overview_detail",
        "comparison",
        "flow",
        "supporting",
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sequence => "sequence",
            Self::Workspace => "workspace",
            Self::MasterDetail => "master_detail",
            Self::OverviewDetail => "overview_detail",
            Self::Comparison => "comparison",
            Self::Flow => "flow",
            Self::Supporting => "supporting",
        }
    }
}

/// Cross-media semantic role for one layout region.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutRole {
    Primary,
    Navigation,
    Detail,
    Inspector,
    Actions,
    Status,
    History,
    Input,
    Summary,
    Supporting,
}

impl LayoutRole {
    pub const VALUES: &'static [&'static str] = &[
        "primary",
        "navigation",
        "detail",
        "inspector",
        "actions",
        "status",
        "history",
        "input",
        "summary",
        "supporting",
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Navigation => "navigation",
            Self::Detail => "detail",
            Self::Inspector => "inspector",
            Self::Actions => "actions",
            Self::Status => "status",
            Self::History => "history",
            Self::Input => "input",
            Self::Summary => "summary",
            Self::Supporting => "supporting",
        }
    }
}

/// Advisory shell genre for an owner section.
///
/// A genre is a host hint, not authority: viewers may ignore it, and unknown
/// values must not change the load-bearing protocol boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceGenre {
    Triage,
    CollectionAdmin,
    Monitor,
    Reader,
    Conversation,
    Board,
    Console,
}

impl SurfaceGenre {
    pub const VALUES: &'static [&'static str] = &[
        "triage",
        "collection_admin",
        "monitor",
        "reader",
        "conversation",
        "board",
        "console",
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Triage => "triage",
            Self::CollectionAdmin => "collection_admin",
            Self::Monitor => "monitor",
            Self::Reader => "reader",
            Self::Conversation => "conversation",
            Self::Board => "board",
            Self::Console => "console",
        }
    }
}

/// Canonical viewer interaction events recognized by the View Hint Profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionEvent {
    Activate,
    Select,
    Submit,
}

impl InteractionEvent {
    pub const VALUES: &'static [&'static str] = &["activate", "select", "submit"];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Activate => "activate",
            Self::Select => "select",
            Self::Submit => "submit",
        }
    }
}

/// A viewer-executed effect intent published as inert View Hint data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InteractionEffect {
    SetActorState {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value_binding: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value: Option<Value>,
    },
    Request {
        capability: Id,
        #[serde(default, skip_serializing_if = "JsonMap::is_empty")]
        input_bindings: JsonMap,
    },
}

impl InteractionEffect {
    pub fn set_actor_state(path: impl Into<String>, value_binding: impl Into<String>) -> Self {
        Self::SetActorState {
            path: path.into(),
            value_binding: Some(value_binding.into()),
            value: None,
        }
    }

    pub fn set_actor_state_value(path: impl Into<String>, value: Value) -> Self {
        Self::SetActorState {
            path: path.into(),
            value_binding: None,
            value: Some(value),
        }
    }

    pub fn request(capability: impl Into<Id>) -> Self {
        Self::Request {
            capability: capability.into(),
            input_bindings: JsonMap::new(),
        }
    }

    pub fn input_binding(mut self, name: impl Into<String>, binding: impl Into<String>) -> Self {
        if let Self::Request { input_bindings, .. } = &mut self {
            input_bindings.insert(name.into(), json!(binding.into()));
        }
        self
    }

    pub fn into_value(self) -> Value {
        match self {
            Self::SetActorState {
                path,
                value_binding,
                value,
            } => {
                let mut map = JsonMap::new();
                map.insert("type".to_string(), json!("set_actor_state"));
                map.insert("path".to_string(), json!(path));
                if let Some(value_binding) = value_binding {
                    map.insert("value_binding".to_string(), json!(value_binding));
                }
                if let Some(value) = value {
                    map.insert("value".to_string(), value);
                }
                Value::Object(map)
            }
            Self::Request {
                capability,
                input_bindings,
            } => {
                let mut map = JsonMap::new();
                map.insert("type".to_string(), json!("request"));
                map.insert("capability".to_string(), json!(capability));
                if !input_bindings.is_empty() {
                    map.insert("input_bindings".to_string(), Value::Object(input_bindings));
                }
                Value::Object(map)
            }
        }
    }
}

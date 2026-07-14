//! Namespaced composition for complete AFUI space snapshots.
//!
//! Composition consumes ordinary `snapshot` messages and emits another ordinary
//! `snapshot`. It does not add a fragment type or merge owner authority: every
//! source owner is remapped under a separator-safe namespace prefix and all
//! unit/capability references are rewritten into that local namespace.

use std::collections::HashSet;
use std::error::Error;
use std::fmt;

use crate::unit_profile::{
    FIELD_ACTIVATE_CAPABILITY, FIELD_CAPABILITY, FIELD_INTERACTIONS, FIELD_PRIMARY_CAPABILITY,
    FIELD_SUBMIT_CAPABILITY,
};
use crate::{
    AgentFirstUiError, AgentFirstUiMessage, JsonMap, JsonValue as Value, MessagePayload, Space,
    is_valid_id, validate_space,
};

fn namespace_owner(namespace: &str, owner: &str) -> String {
    namespace_join(namespace, owner)
}

fn prefix_id(namespace: &str, id: &str) -> String {
    namespace_join(namespace, id)
}

fn namespace_join(namespace: &str, value: &str) -> String {
    format!("ns{}_{}__{}", namespace.len(), namespace, value)
}

fn prefix_id_array(namespace: &str, arr: &[Value]) -> Vec<Value> {
    arr.iter()
        .map(|v| match v.as_str() {
            Some(s) => Value::String(prefix_id(namespace, s)),
            None => v.clone(),
        })
        .collect()
}

fn rewrite_binding(namespace: &str, owner_keys: &[&str], s: &str) -> String {
    if let Some(rest) = s.strip_prefix("space:") {
        for key in owner_keys {
            if rest == *key {
                return format!("space:{}", namespace_owner(namespace, key));
            }
            if let Some(suffix) = rest.strip_prefix(&format!("{key}.")) {
                return format!("space:{}.{suffix}", namespace_owner(namespace, key));
            }
        }
    }
    s.to_string()
}

fn namespace_value(namespace: &str, owner_keys: &[&str], value: &Value) -> Value {
    match value {
        Value::String(s) => Value::String(rewrite_binding(namespace, owner_keys, s)),
        Value::Array(arr) => Value::Array(
            arr.iter()
                .map(|v| namespace_value(namespace, owner_keys, v))
                .collect(),
        ),
        Value::Object(obj) => Value::Object(
            obj.iter()
                .map(|(k, v)| (k.clone(), namespace_value(namespace, owner_keys, v)))
                .collect(),
        ),
        _ => value.clone(),
    }
}

fn namespace_capability_ref(namespace: &str, value: &Value) -> Value {
    match value.as_str() {
        Some(s) => Value::String(prefix_id(namespace, s)),
        None => value.clone(),
    }
}

fn namespace_interaction_value(namespace: &str, owner_keys: &[&str], value: &Value) -> Value {
    match value {
        Value::Array(arr) => Value::Array(
            arr.iter()
                .map(|v| namespace_interaction_value(namespace, owner_keys, v))
                .collect(),
        ),
        Value::Object(obj) => Value::Object(
            obj.iter()
                .map(|(key, value)| {
                    let value = if key == FIELD_CAPABILITY {
                        namespace_capability_ref(namespace, value)
                    } else {
                        namespace_interaction_value(namespace, owner_keys, value)
                    };
                    (key.clone(), value)
                })
                .collect(),
        ),
        _ => namespace_value(namespace, owner_keys, value),
    }
}

fn namespace_item(namespace: &str, owner_keys: &[&str], obj: &JsonMap) -> JsonMap {
    let mut result = JsonMap::new();
    for (key, value) in obj {
        let new_val = match key.as_str() {
            "id" => match value.as_str() {
                Some(s) => Value::String(prefix_id(namespace, s)),
                None => value.clone(),
            },
            "capabilities" | "row_capabilities" | "members" => match value.as_array() {
                Some(arr) => Value::Array(prefix_id_array(namespace, arr)),
                None => value.clone(),
            },
            FIELD_ACTIVATE_CAPABILITY
            | FIELD_PRIMARY_CAPABILITY
            | FIELD_SUBMIT_CAPABILITY
            | FIELD_CAPABILITY => namespace_capability_ref(namespace, value),
            FIELD_INTERACTIONS => namespace_interaction_value(namespace, owner_keys, value),
            _ => namespace_value(namespace, owner_keys, value),
        };
        result.insert(key.clone(), new_val);
    }
    result
}

fn namespace_section(namespace: &str, owner_keys: &[&str], section: &JsonMap) -> JsonMap {
    let mut result = JsonMap::new();
    for (key, value) in section {
        let new_val = match key.as_str() {
            "root_layout" => match value.as_str() {
                Some(s) => Value::String(prefix_id(namespace, s)),
                None => value.clone(),
            },
            "capabilities" | "units" | "layouts" => match value.as_array() {
                Some(arr) => Value::Array(
                    arr.iter()
                        .map(|item| match item.as_object() {
                            Some(obj) => Value::Object(namespace_item(namespace, owner_keys, obj)),
                            None => item.clone(),
                        })
                        .collect(),
                ),
                None => value.clone(),
            },
            _ => namespace_value(namespace, owner_keys, value),
        };
        result.insert(key.clone(), new_val);
    }
    result
}

fn namespace_space(space: &Space, namespace: &str) -> Result<Space, AgentFirstUiError> {
    let sections = space
        .as_value()
        .as_object()
        .ok_or_else(|| AgentFirstUiError::InvalidSpace {
            message: "space must be a JSON object".to_string(),
        })?;
    let owner_keys: Vec<&str> = sections.keys().map(String::as_str).collect();
    let mut new_sections = JsonMap::new();
    for (owner_key, section_val) in sections {
        let new_key = namespace_owner(namespace, owner_key);
        let new_section = match section_val.as_object() {
            Some(section) => Value::Object(namespace_section(namespace, &owner_keys, section)),
            None => section_val.clone(),
        };
        new_sections.insert(new_key, new_section);
    }
    Space::try_from_value(Value::Object(new_sections))
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComposeSource {
    pub namespace: String,
    pub space: Space,
    pub sequence: u64,
}

impl ComposeSource {
    pub fn new(namespace: impl Into<String>, space: Space) -> Self {
        Self {
            namespace: namespace.into(),
            space,
            sequence: 0,
        }
    }

    pub fn with_sequence(mut self, sequence: u64) -> Self {
        self.sequence = sequence;
        self
    }
}

#[derive(Debug)]
pub enum ComposeError {
    Empty,
    InvalidNamespace(String),
    DuplicateNamespace(String),
    DuplicateOwner(String),
    SourceNotSnapshot {
        namespace: String,
        message_type: String,
    },
    SourceMessage {
        namespace: String,
        error: AgentFirstUiError,
    },
    Validation(AgentFirstUiError),
}

impl fmt::Display for ComposeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "at least one AFUI source is required"),
            Self::InvalidNamespace(namespace) => {
                write!(f, "invalid AFUI source namespace `{namespace}`")
            }
            Self::DuplicateNamespace(namespace) => {
                write!(f, "duplicate AFUI source namespace `{namespace}`")
            }
            Self::DuplicateOwner(owner) => write!(f, "duplicate composed owner `{owner}`"),
            Self::SourceNotSnapshot {
                namespace,
                message_type,
            } => write!(
                f,
                "source `{namespace}` must be a snapshot, got `{message_type}`"
            ),
            Self::SourceMessage { namespace, error } => {
                write!(
                    f,
                    "source `{namespace}` is not a valid AFUI snapshot: {error}"
                )
            }
            Self::Validation(error) => write!(f, "composed AFUI snapshot is invalid: {error}"),
        }
    }
}

impl Error for ComposeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::SourceMessage { error, .. } | Self::Validation(error) => Some(error),
            _ => None,
        }
    }
}

pub fn compose_messages(
    sources: Vec<(impl Into<String>, AgentFirstUiMessage)>,
) -> Result<AgentFirstUiMessage, ComposeError> {
    let mut spaces = Vec::with_capacity(sources.len());
    for (namespace, message) in sources {
        let namespace = namespace.into();
        message
            .ensure_version()
            .map_err(|error| ComposeError::SourceMessage {
                namespace: namespace.clone(),
                error,
            })?;
        let message_type = message.payload.message_type().to_string();
        let (space, sequence) = match message.payload {
            MessagePayload::Snapshot { space, sequence } => (space, sequence),
            _ => {
                return Err(ComposeError::SourceNotSnapshot {
                    namespace,
                    message_type,
                });
            }
        };
        validate_space(&space).map_err(|error| ComposeError::SourceMessage {
            namespace: namespace.clone(),
            error,
        })?;
        spaces.push(ComposeSource::new(namespace, space).with_sequence(sequence));
    }
    let sequence = spaces
        .iter()
        .map(|source| source.sequence)
        .max()
        .unwrap_or(0);
    Ok(AgentFirstUiMessage::snapshot(
        compose_spaces(spaces)?,
        sequence,
    ))
}

pub fn compose_spaces(sources: Vec<ComposeSource>) -> Result<Space, ComposeError> {
    if sources.is_empty() {
        return Err(ComposeError::Empty);
    }

    let mut namespaces = HashSet::new();
    let mut composed_sections = JsonMap::new();
    let mut sources_metadata = Vec::new();

    for source in sources {
        validate_namespace(&source.namespace)?;
        if !namespaces.insert(source.namespace.clone()) {
            return Err(ComposeError::DuplicateNamespace(source.namespace));
        }
        validate_space(&source.space).map_err(ComposeError::Validation)?;

        let owner_keys = source.space.owner_keys();
        let namespaced =
            namespace_space(&source.space, &source.namespace).map_err(ComposeError::Validation)?;
        let Some(sections) = namespaced.as_value().as_object() else {
            return Err(ComposeError::Validation(AgentFirstUiError::InvalidSpace {
                message: "namespaced space was not an object".to_string(),
            }));
        };
        for (owner, section) in sections {
            if composed_sections
                .insert(owner.clone(), section.clone())
                .is_some()
            {
                return Err(ComposeError::DuplicateOwner(owner.clone()));
            }
        }
        sources_metadata.push(source_metadata(&source.namespace, &owner_keys));
    }

    composed_sections.insert(
        "composition".to_string(),
        Value::Object({
            let mut section = JsonMap::new();
            section.insert("sources".to_string(), Value::Array(sources_metadata));
            section
        }),
    );

    let composed = Space::from(composed_sections);
    validate_space(&composed).map_err(ComposeError::Validation)?;
    Ok(composed)
}

fn validate_namespace(namespace: &str) -> Result<(), ComposeError> {
    if is_valid_id(namespace) {
        Ok(())
    } else {
        Err(ComposeError::InvalidNamespace(namespace.to_string()))
    }
}

fn source_metadata(namespace: &str, owners: &[String]) -> Value {
    let mut object = JsonMap::new();
    object.insert(
        "namespace".to_string(),
        Value::String(namespace.to_string()),
    );
    object.insert(
        "owners".to_string(),
        Value::Array(
            owners
                .iter()
                .map(|owner| Value::String(namespace_owner(namespace, owner)))
                .collect(),
        ),
    );
    Value::Object(object)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MessagePayload, Risk, json};

    fn source_space() -> Result<Space, Box<dyn Error>> {
        Space::try_from_value(json!({
            "operations": {
                "summary": { "count": 3 },
                "tickets": [{ "id": "T-1" }],
                "capabilities": [
                    { "id": "open_ticket", "label": "Open", "risk": Risk::ReadOnly }
                ],
                "units": [
                    {
                        "id": "summary",
                        "kind": "facts",
                        "activate_capability": "open_ticket",
                        "primary_capability": "open_ticket",
                        "items": [
                            { "label": "Count", "value_binding": "space:operations.summary.count" }
                        ],
                        "capabilities": ["open_ticket"],
                        "interactions": {
                            "activate": [
                                { "type": "request", "capability": "open_ticket" }
                            ]
                        }
                    },
                    {
                        "id": "tickets",
                        "kind": "table",
                        "source_binding": "space:operations.tickets",
                        "ticket_id_binding": "record:id",
                        "row_capabilities": ["open_ticket"]
                    }
                ],
                "layouts": [
                    { "id": "root", "members": ["summary", "tickets"] }
                ],
                "root_layout": "root"
            }
        }))
        .map_err(Into::into)
    }

    #[test]
    fn compose_namespaces_ids_capabilities_and_space_bindings() -> Result<(), Box<dyn Error>> {
        let composed = compose_spaces(vec![
            ComposeSource::new("mail", source_space()?),
            ComposeSource::new("crm", source_space()?),
        ])?;

        validate_space(&composed)?;
        assert!(composed.section("ns4_mail__operations").is_some());
        assert!(composed.section("ns3_crm__operations").is_some());

        let mail = composed
            .section("ns4_mail__operations")
            .and_then(Value::as_object)
            .ok_or_else(|| std::io::Error::other("mail operations section exists"))?;
        let units = mail
            .get("units")
            .and_then(Value::as_array)
            .ok_or_else(|| std::io::Error::other("mail units exist"))?;
        assert_eq!(units[0].get("id"), Some(&json!("ns4_mail__summary")));
        assert_eq!(
            units[0].pointer("/items/0/value_binding"),
            Some(&json!("space:ns4_mail__operations.summary.count"))
        );
        assert_eq!(
            units[0].get("activate_capability"),
            Some(&json!("ns4_mail__open_ticket"))
        );
        assert_eq!(
            units[0].get("primary_capability"),
            Some(&json!("ns4_mail__open_ticket"))
        );
        assert_eq!(
            units[0].pointer("/interactions/activate/0/capability"),
            Some(&json!("ns4_mail__open_ticket"))
        );
        assert_eq!(
            units[1].get("row_capabilities"),
            Some(&json!(["ns4_mail__open_ticket"]))
        );
        assert_eq!(
            Value::Object(mail.clone()).pointer("/layouts/0/members"),
            Some(&json!(["ns4_mail__summary", "ns4_mail__tickets"]))
        );
        Ok(())
    }

    #[test]
    fn compose_messages_rejects_non_snapshot_sources() {
        let result = compose_messages(vec![(
            "mail",
            AgentFirstUiMessage::request(
                "session_operator",
                "mail_operations",
                "open_ticket",
                None,
            ),
        )]);
        assert!(matches!(
            result,
            Err(ComposeError::SourceNotSnapshot { .. })
        ));
    }

    #[test]
    fn compose_messages_emits_snapshot() -> Result<(), Box<dyn Error>> {
        let message = AgentFirstUiMessage::snapshot(source_space()?, 7);
        let composed = compose_messages(vec![("mail", message)])?;
        assert_eq!(composed.agent_first_ui, crate::PROTOCOL_VERSION);
        let MessagePayload::Snapshot { space, sequence } = composed.payload else {
            return Err(std::io::Error::other("compose produced non-snapshot").into());
        };
        assert_eq!(sequence, 7);
        assert!(space.section("ns4_mail__operations").is_some());
        Ok(())
    }
}

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use crate::error::AgentFirstUiError;
use crate::operation::{Operation, Operations};

/// The protocol version carried in every message envelope.
pub const PROTOCOL_VERSION: &str = "0.2";

/// A stable identifier for a capability, unit, or layout. These ids are unique
/// only within the owner section that declares them.
pub type Id = String;

/// A section key: the leading-level key under which an owner's facts live, for
/// example `owner_A7K9mN4pQ2`.
pub type OwnerKey = String;

pub type JsonMap = Map<String, Value>;

/// The shared tree of facts, split into sections keyed by owner. Always a JSON
/// object whose top-level entries are owner sections.
#[derive(Debug, Clone, PartialEq)]
pub struct Space(Value);

impl Space {
    pub fn empty() -> Self {
        Self(Value::Object(JsonMap::new()))
    }

    pub fn try_from_value(value: Value) -> Result<Self, AgentFirstUiError> {
        if value.is_object() {
            Ok(Self(value))
        } else {
            Err(AgentFirstUiError::InvalidSpace {
                message: "space must be a JSON object".to_string(),
            })
        }
    }

    pub fn as_value(&self) -> &Value {
        &self.0
    }

    pub fn as_value_mut(&mut self) -> &mut Value {
        &mut self.0
    }

    pub fn into_value(self) -> Value {
        self.0
    }

    /// The section owned by `owner`, if present.
    pub fn section(&self, owner: &str) -> Option<&Value> {
        self.0.get(owner)
    }

    /// The owner keys (section names) currently in the space.
    pub fn owner_keys(&self) -> Vec<String> {
        self.0
            .as_object()
            .map(|map| map.keys().cloned().collect())
            .unwrap_or_default()
    }
}

impl Default for Space {
    fn default() -> Self {
        Self::empty()
    }
}

impl From<JsonMap> for Space {
    fn from(value: JsonMap) -> Self {
        Self(Value::Object(value))
    }
}

impl TryFrom<Value> for Space {
    type Error = AgentFirstUiError;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        Self::try_from_value(value)
    }
}

impl Serialize for Space {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Space {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        Self::try_from_value(value).map_err(serde::de::Error::custom)
    }
}

/// One wire message. The envelope carries the protocol version and a flattened,
/// `type`-tagged payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentFirstUiMessage {
    pub agent_first_ui: String,
    #[serde(flatten)]
    pub payload: MessagePayload,
}

impl AgentFirstUiMessage {
    /// A snapshot of one participant's delivered projection at an emit-log position.
    pub fn snapshot(space: Space, sequence: u64) -> Self {
        Self {
            agent_first_ui: PROTOCOL_VERSION.to_string(),
            payload: MessagePayload::Snapshot { space, sequence },
        }
    }

    /// An owner's applied change to its own section.
    pub fn emit(author: impl Into<OwnerKey>, sequence: u64, operations: Operations) -> Self {
        Self {
            agent_first_ui: PROTOCOL_VERSION.to_string(),
            payload: MessagePayload::Emit {
                author: author.into(),
                sequence,
                operations,
            },
        }
    }

    /// A participant asking another owner to invoke a capability.
    pub fn request(
        author: impl Into<OwnerKey>,
        owner: impl Into<OwnerKey>,
        capability: impl Into<Id>,
        input: Option<Value>,
    ) -> Self {
        Self {
            agent_first_ui: PROTOCOL_VERSION.to_string(),
            payload: MessagePayload::Request {
                author: author.into(),
                owner: owner.into(),
                capability: capability.into(),
                input,
            },
        }
    }

    pub fn ensure_version(&self) -> Result<(), AgentFirstUiError> {
        if self.agent_first_ui == PROTOCOL_VERSION {
            Ok(())
        } else {
            Err(AgentFirstUiError::UnsupportedVersion {
                found: self.agent_first_ui.clone(),
            })
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MessagePayload {
    Snapshot {
        space: Space,
        sequence: u64,
    },
    Emit {
        author: OwnerKey,
        sequence: u64,
        operations: Vec<Operation>,
    },
    Request {
        author: OwnerKey,
        owner: OwnerKey,
        capability: Id,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input: Option<Value>,
    },
}

impl MessagePayload {
    pub fn message_type(&self) -> &'static str {
        match self {
            Self::Snapshot { .. } => "snapshot",
            Self::Emit { .. } => "emit",
            Self::Request { .. } => "request",
        }
    }
}

/// The single safety label on a capability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    ReadOnly,
    LocalMutation,
    ExternalEffect,
    Destructive,
}

impl Risk {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::LocalMutation => "local_mutation",
            Self::ExternalEffect => "external_effect",
            Self::Destructive => "destructive",
        }
    }
}

impl Default for Risk {
    /// Fail-safe default: a capability that declares no risk is treated as the
    /// most dangerous category until the owner proves otherwise.
    fn default() -> Self {
        Self::Destructive
    }
}

impl std::fmt::Display for Risk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Deserialize an owner-supplied `risk`, treating any unrecognized value as
/// `destructive`. Owners are untrusted: an unknown risk word must fail safe
/// rather than abort the whole space.
fn deserialize_risk<'de, D>(deserializer: D) -> Result<Risk, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    Ok(match value.as_str() {
        Some("read_only") => Risk::ReadOnly,
        Some("local_mutation") => Risk::LocalMutation,
        Some("external_effect") => Risk::ExternalEffect,
        _ => Risk::Destructive,
    })
}

/// An owner-declared affordance: what a participant may request from this owner.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Capability {
    pub id: Id,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, deserialize_with = "deserialize_risk")]
    pub risk: Risk,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applicable_binding: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<Value>,
    #[serde(flatten, default)]
    pub extra: JsonMap,
}

impl Capability {
    pub fn new(id: impl Into<Id>, label: impl Into<String>, risk: Risk) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            description: None,
            risk,
            applicable_binding: None,
            input_schema: None,
            extra: JsonMap::new(),
        }
    }
}

/// How much a unit, field, item, or layout region matters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Salience {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prominence: Option<Prominence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<SalienceState>,
}

impl Salience {
    pub fn new() -> Self {
        Self {
            prominence: None,
            state: None,
        }
    }

    pub fn prominence(mut self, value: Prominence) -> Self {
        self.prominence = Some(value);
        self
    }

    pub fn state(mut self, value: SalienceState) -> Self {
        self.state = Some(value);
        self
    }
}

impl Default for Salience {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Prominence {
    Primary,
    Normal,
    Subtle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SalienceState {
    Neutral,
    Info,
    Success,
    Warning,
    Danger,
}

/// A unit: an owner's advisory recommendation of a coherent slice of facts to
/// present, with an open semantic `kind`. Units are a flat pool and compose only
/// by id reference through layouts and `select` cases.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Unit {
    pub id: Id,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activate_capability: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_capability: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub salience: Option<Salience>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<Id>,
    #[serde(flatten, default)]
    pub extra: JsonMap,
}

impl Unit {
    pub fn new(id: impl Into<Id>, kind: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            kind: kind.into(),
            description: None,
            activate_capability: None,
            primary_capability: None,
            salience: None,
            fallback: None,
            extra: JsonMap::new(),
        }
    }

    pub fn field(&self, key: &str) -> Option<&Value> {
        self.extra.get(key)
    }

    pub fn field_as_str(&self, key: &str) -> Option<&str> {
        self.field(key).and_then(Value::as_str)
    }

    pub fn set_field(mut self, key: impl Into<String>, value: Value) -> Self {
        self.extra.insert(key.into(), value);
        self
    }

    pub fn child_units(&self) -> Vec<&Unit> {
        Vec::new()
    }

    /// Capability ids this unit references, read from typed capability fields
    /// plus marker fields (`capability` / `*_capability` and
    /// `capabilities` / `*_capabilities`).
    pub fn capability_ids(&self) -> Vec<Id> {
        let mut ids = Vec::new();
        if let Some(id) = &self.activate_capability {
            ids.push(id.clone());
        }
        if let Some(id) = &self.primary_capability {
            ids.push(id.clone());
        }
        collect_capability_ids_from_value(&Value::Object(self.extra.clone()), &mut ids);
        ids
    }
}

fn collect_capability_ids_from_value(value: &Value, ids: &mut Vec<Id>) {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                if key == "capability" || key.ends_with("_capability") {
                    if let Some(id) = value.as_str() {
                        ids.push(id.to_string());
                    }
                } else if (key == "capabilities" || key.ends_with("_capabilities"))
                    && let Some(items) = value.as_array()
                {
                    ids.extend(items.iter().filter_map(Value::as_str).map(str::to_string));
                }
                collect_capability_ids_from_value(value, ids);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_capability_ids_from_value(item, ids);
            }
        }
        _ => {}
    }
}

/// One named layout region in an owner's recommended layout pool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    pub id: Id,
    pub members: Vec<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub overlay: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub salience: Option<Salience>,
    #[serde(flatten, default)]
    pub extra: JsonMap,
}

impl Layout {
    pub fn new(id: impl Into<Id>, members: Vec<Id>) -> Self {
        Self {
            id: id.into(),
            members,
            description: None,
            overlay: false,
            salience: None,
            extra: JsonMap::new(),
        }
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn risk_of(value: Value) -> Result<Risk, serde_json::Error> {
        Ok(serde_json::from_value::<Capability>(value)?.risk)
    }

    #[test]
    fn known_risk_words_round_trip() -> Result<(), serde_json::Error> {
        for (word, risk) in [
            ("read_only", Risk::ReadOnly),
            ("local_mutation", Risk::LocalMutation),
            ("external_effect", Risk::ExternalEffect),
            ("destructive", Risk::Destructive),
        ] {
            assert_eq!(
                risk_of(json!({ "id": "a", "label": "A", "risk": word }))?,
                risk
            );
        }
        Ok(())
    }

    #[test]
    fn unknown_risk_word_falls_back_to_destructive() -> Result<(), serde_json::Error> {
        assert_eq!(
            risk_of(json!({ "id": "a", "label": "A", "risk": "totally_new" }))?,
            Risk::Destructive
        );
        Ok(())
    }

    #[test]
    fn missing_risk_falls_back_to_destructive() -> Result<(), serde_json::Error> {
        assert_eq!(
            risk_of(json!({ "id": "a", "label": "A" }))?,
            Risk::Destructive
        );
        Ok(())
    }

    #[test]
    fn non_string_risk_falls_back_to_destructive() -> Result<(), serde_json::Error> {
        assert_eq!(
            risk_of(json!({ "id": "a", "label": "A", "risk": 7 }))?,
            Risk::Destructive
        );
        Ok(())
    }

    #[test]
    fn snapshot_uses_space_key() -> Result<(), serde_json::Error> {
        let message = AgentFirstUiMessage::snapshot(Space::empty(), 0);
        let value = serde_json::to_value(message)?;
        assert!(value.get("space").is_some());
        assert!(value.get("room").is_none());
        Ok(())
    }

    #[test]
    fn request_carries_target_owner() -> Result<(), serde_json::Error> {
        let message = AgentFirstUiMessage::request("owner_operator", "owner_afmail", "open", None);
        let value = serde_json::to_value(message)?;
        assert_eq!(value.get("author"), Some(&json!("owner_operator")));
        assert_eq!(value.get("owner"), Some(&json!("owner_afmail")));
        assert_eq!(value.get("capability"), Some(&json!("open")));
        Ok(())
    }
}

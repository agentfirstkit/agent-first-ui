use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use crate::error::AfuiError;
use crate::patch::{Patch, PatchOperation};

pub const AFUI_VERSION: &str = "0.1";

pub type Id = String;
pub type JsonMap = Map<String, Value>;

#[derive(Debug, Clone, PartialEq)]
pub struct State(Value);

impl State {
    pub fn empty() -> Self {
        Self(Value::Object(JsonMap::new()))
    }

    pub fn try_from_value(value: Value) -> Result<Self, AfuiError> {
        if value.is_object() {
            Ok(Self(value))
        } else {
            Err(AfuiError::InvalidState {
                message: "state must be a JSON object".to_string(),
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
}

impl Default for State {
    fn default() -> Self {
        Self::empty()
    }
}

impl From<JsonMap> for State {
    fn from(value: JsonMap) -> Self {
        Self(Value::Object(value))
    }
}

impl TryFrom<Value> for State {
    type Error = AfuiError;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        Self::try_from_value(value)
    }
}

impl Serialize for State {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for State {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        Self::try_from_value(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Theme(Value);

impl Theme {
    pub fn empty() -> Self {
        Self(Value::Object(JsonMap::new()))
    }

    pub fn try_from_value(value: Value) -> Result<Self, AfuiError> {
        let Some(tokens) = value.as_object() else {
            return Err(AfuiError::InvalidTheme {
                message: "theme must be a JSON object".to_string(),
            });
        };
        let invalid = tokens
            .iter()
            .find(|(_, value)| !(value.is_string() || value.is_number()));
        if let Some((key, _)) = invalid {
            return Err(AfuiError::InvalidTheme {
                message: format!("theme token `{key}` must be a string or number"),
            });
        }
        Ok(Self(value))
    }

    pub fn as_value(&self) -> &Value {
        &self.0
    }

    pub fn into_value(self) -> Value {
        self.0
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::empty()
    }
}

impl From<JsonMap> for Theme {
    fn from(value: JsonMap) -> Self {
        Self(Value::Object(value))
    }
}

impl TryFrom<Value> for Theme {
    type Error = AfuiError;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        Self::try_from_value(value)
    }
}

impl Serialize for Theme {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Theme {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        Self::try_from_value(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AfuiMessage {
    pub afui: String,
    #[serde(flatten)]
    pub payload: AfuiPayload,
}

impl AfuiMessage {
    pub fn ui_snapshot(document: Document, state: State) -> Self {
        Self {
            afui: AFUI_VERSION.to_string(),
            payload: AfuiPayload::UiSnapshot {
                document,
                state,
                theme: None,
            },
        }
    }

    pub fn ui_snapshot_with_theme(document: Document, state: State, theme: Theme) -> Self {
        Self {
            afui: AFUI_VERSION.to_string(),
            payload: AfuiPayload::UiSnapshot {
                document,
                state,
                theme: Some(theme),
            },
        }
    }

    pub fn state_patch(patch: Patch) -> Self {
        Self {
            afui: AFUI_VERSION.to_string(),
            payload: AfuiPayload::StatePatch { patch },
        }
    }

    pub fn document_patch(patch: Patch) -> Self {
        Self {
            afui: AFUI_VERSION.to_string(),
            payload: AfuiPayload::DocumentPatch { patch },
        }
    }

    pub fn theme_patch(patch: Patch) -> Self {
        Self {
            afui: AFUI_VERSION.to_string(),
            payload: AfuiPayload::ThemePatch { patch },
        }
    }

    pub fn user_action(action_id: impl Into<Id>, input: Option<Value>) -> Self {
        Self {
            afui: AFUI_VERSION.to_string(),
            payload: AfuiPayload::UserAction(UserAction {
                action_id: action_id.into(),
                input,
            }),
        }
    }

    pub fn ensure_version(&self) -> Result<(), AfuiError> {
        if self.afui == AFUI_VERSION {
            Ok(())
        } else {
            Err(AfuiError::UnsupportedVersion {
                found: self.afui.clone(),
            })
        }
    }

    pub fn as_snapshot(&self) -> Result<Snapshot, AfuiError> {
        self.ensure_version()?;
        match &self.payload {
            AfuiPayload::UiSnapshot {
                document,
                state,
                theme,
            } => Ok(Snapshot {
                document: document.clone(),
                state: state.clone(),
                theme: theme.clone(),
            }),
            other => Err(AfuiError::UnsupportedMessageType {
                message_type: other.message_type().to_string(),
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AfuiPayload {
    #[serde(rename = "ui_snapshot")]
    UiSnapshot {
        document: Document,
        state: State,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        theme: Option<Theme>,
    },
    #[serde(rename = "state_patch")]
    StatePatch { patch: Vec<PatchOperation> },
    #[serde(rename = "document_patch")]
    DocumentPatch { patch: Vec<PatchOperation> },
    #[serde(rename = "theme_patch")]
    ThemePatch { patch: Vec<PatchOperation> },
    #[serde(rename = "user_action")]
    UserAction(UserAction),
}

impl AfuiPayload {
    pub fn message_type(&self) -> &'static str {
        match self {
            Self::UiSnapshot { .. } => "ui_snapshot",
            Self::StatePatch { .. } => "state_patch",
            Self::DocumentPatch { .. } => "document_patch",
            Self::ThemePatch { .. } => "theme_patch",
            Self::UserAction(_) => "user_action",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub document: Document,
    pub state: State,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<Theme>,
}

impl Snapshot {
    pub fn new(document: Document, state: State) -> Self {
        Self {
            document,
            state,
            theme: None,
        }
    }

    pub fn with_theme(document: Document, state: State, theme: Theme) -> Self {
        Self {
            document,
            state,
            theme: Some(theme),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserAction {
    pub action_id: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub surface: Surface,
    pub screens: Vec<Screen>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<Action>,
    #[serde(flatten, default)]
    pub extra: JsonMap,
}

impl Document {
    pub fn new(surface: Surface, screens: Vec<Screen>) -> Self {
        Self {
            surface,
            screens,
            actions: Vec::new(),
            extra: JsonMap::new(),
        }
    }

    pub fn with_actions(mut self, actions: Vec<Action>) -> Self {
        self.actions = actions;
        self
    }

    pub fn find_action(&self, id: &str) -> Option<&Action> {
        self.actions.iter().find(|action| action.id == id)
    }

    pub fn find_view(&self, id: &str) -> Option<&View> {
        self.screens
            .iter()
            .flat_map(|screen| screen.views.iter())
            .find_map(|view| find_view_recursive(view, id))
    }
}

fn find_view_recursive<'a>(view: &'a View, id: &str) -> Option<&'a View> {
    if view.id == id {
        return Some(view);
    }
    for child in view.child_views() {
        if let Some(found) = find_view_recursive(child, id) {
            return Some(found);
        }
    }
    None
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Surface {
    pub id: Id,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(flatten, default)]
    pub extra: JsonMap,
}

impl Surface {
    pub fn new(id: impl Into<Id>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            description: None,
            version: None,
            extra: JsonMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Screen {
    pub id: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub views: Vec<View>,
    #[serde(flatten, default)]
    pub extra: JsonMap,
}

impl Screen {
    pub fn new(id: impl Into<Id>, views: Vec<View>) -> Self {
        Self {
            id: id.into(),
            title: None,
            description: None,
            views,
            extra: JsonMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct View {
    pub id: Id,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<Box<View>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub footer: Option<Box<View>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub empty_view: Option<Box<View>>,
    #[serde(rename = "default", default, skip_serializing_if = "Option::is_none")]
    pub default_view: Option<Box<View>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub cases: BTreeMap<String, View>,
    #[serde(flatten, default)]
    pub extra: JsonMap,
}

impl View {
    pub fn new(id: impl Into<Id>, kind: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            kind: kind.into(),
            description: None,
            header: None,
            footer: None,
            empty_view: None,
            default_view: None,
            cases: BTreeMap::new(),
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

    pub fn child_views(&self) -> Vec<&View> {
        let mut children = Vec::new();
        if let Some(view) = self.header.as_deref() {
            children.push(view);
        }
        if let Some(view) = self.footer.as_deref() {
            children.push(view);
        }
        if let Some(view) = self.empty_view.as_deref() {
            children.push(view);
        }
        if let Some(view) = self.default_view.as_deref() {
            children.push(view);
        }
        children.extend(self.cases.values());
        children
    }
}

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
    /// Fail-safe default: an action that declares no risk is treated as the
    /// most dangerous category until the producer proves otherwise.
    fn default() -> Self {
        Self::Destructive
    }
}

impl std::fmt::Display for Risk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Deserialize a producer-supplied `risk`, treating any unrecognized value as
/// `destructive`. Producers are untrusted: an unknown risk word (for example
/// from a newer producer) must fail safe rather than abort the whole document.
/// This matches `spec/afui.schema.json` and the skill, which both state that an
/// absent or unrecognized risk is treated as `destructive`.
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Action {
    pub id: Id,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, deserialize_with = "deserialize_risk")]
    pub risk: Risk,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undoable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<Value>,
    #[serde(flatten, default)]
    pub extra: JsonMap,
}

impl Action {
    pub fn new(id: impl Into<Id>, label: impl Into<String>, risk: Risk) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            description: None,
            risk,
            undoable: None,
            input_schema: None,
            extra: JsonMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn risk_of(value: Value) -> Result<Risk, serde_json::Error> {
        Ok(serde_json::from_value::<Action>(value)?.risk)
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
}

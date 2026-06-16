use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::binding::{resolve_binding, BindingContext};
use crate::error::AfuiError;
use crate::types::{Action, AfuiMessage, Document, Id, JsonMap, Risk, Snapshot, View};

#[derive(Debug, Clone, Default)]
pub struct PrepareUserActionOptions {
    pub input_value: Option<Value>,
    pub provided_input: JsonMap,
}

impl PrepareUserActionOptions {
    pub fn with_input_value(mut self, value: Value) -> Self {
        self.input_value = Some(value);
        self
    }

    pub fn with_provided_input(mut self, input: JsonMap) -> Self {
        self.provided_input = input;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssembledActionInput {
    pub input: JsonMap,
    pub missing: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreparedStatus {
    Ready,
    NeedsInput,
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreparedUserAction {
    pub status: PreparedStatus,
    pub action_id: Id,
    pub risk: Risk,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<Action>,
    #[serde(default, skip_serializing_if = "JsonMap::is_empty")]
    pub input: JsonMap,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missing: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<AfuiMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub fn find_action<'a>(document: &'a Document, action_id: &str) -> Option<&'a Action> {
    document.find_action(action_id)
}

pub fn declared_action_ids(document: &Document) -> Vec<Id> {
    document
        .actions
        .iter()
        .map(|action| action.id.clone())
        .collect()
}

pub fn action_ids_from_view(view: &View) -> Vec<Id> {
    let mut ids = Vec::new();
    for (key, value) in &view.extra {
        if key == "action" || key.ends_with("_action") {
            if let Some(id) = value.as_str() {
                ids.push(id.to_string());
            }
        } else if key == "actions" || key.ends_with("_actions") {
            if let Some(items) = value.as_array() {
                ids.extend(items.iter().filter_map(Value::as_str).map(str::to_string));
            }
        }
    }
    ids
}

pub fn assemble_action_input(
    snapshot: &Snapshot,
    source_view: Option<&View>,
    action: &Action,
    record: Option<&Value>,
    options: &PrepareUserActionOptions,
) -> Result<AssembledActionInput, AfuiError> {
    let mut input = JsonMap::new();
    let properties = input_schema_properties(action.input_schema.as_ref());
    let required = input_schema_required(action.input_schema.as_ref());
    let ctx = BindingContext::new(snapshot.state.as_value())
        .with_record(record)
        .with_input(options.input_value.as_ref());

    for name in properties {
        let bind_name = format!("{name}_bind");
        let Some(view) = source_view else {
            continue;
        };
        let Some(value) = view.field(&bind_name) else {
            continue;
        };
        let Some(binding_value) = value.as_str() else {
            return Err(AfuiError::InvalidBinding {
                binding: bind_name,
                reason: "binding field must be a string".to_string(),
            });
        };
        if let Some(resolved) = resolve_binding(&ctx, binding_value)? {
            if !is_empty_string(resolved) {
                input.insert(name, resolved.clone());
            }
        }
    }

    for (key, value) in &options.provided_input {
        if !is_empty_string(value) {
            input.insert(key.clone(), value.clone());
        }
    }

    let missing = required
        .into_iter()
        .filter(|name| !input.contains_key(name))
        .collect();

    Ok(AssembledActionInput { input, missing })
}

pub fn prepare_user_action(
    snapshot: &Snapshot,
    source_view: Option<&View>,
    action_id: &str,
    record: Option<&Value>,
    options: &PrepareUserActionOptions,
) -> Result<PreparedUserAction, AfuiError> {
    let Some(action) = snapshot.document.find_action(action_id) else {
        return Ok(PreparedUserAction {
            status: PreparedStatus::Error,
            action_id: action_id.to_string(),
            risk: Risk::Destructive,
            action: None,
            input: JsonMap::new(),
            missing: Vec::new(),
            input_schema: None,
            event: None,
            error: Some("Unresolved action reference".to_string()),
        });
    };

    let assembled = assemble_action_input(snapshot, source_view, action, record, options)?;
    if !assembled.missing.is_empty() {
        return Ok(PreparedUserAction {
            status: PreparedStatus::NeedsInput,
            action_id: action.id.clone(),
            risk: action.risk.clone(),
            action: Some(action.clone()),
            input: assembled.input,
            missing: assembled.missing,
            input_schema: action.input_schema.clone(),
            event: None,
            error: None,
        });
    }

    let event = AfuiMessage::user_action(
        action.id.clone(),
        Some(Value::Object(assembled.input.clone())),
    );
    Ok(PreparedUserAction {
        status: PreparedStatus::Ready,
        action_id: action.id.clone(),
        risk: action.risk.clone(),
        action: Some(action.clone()),
        input: assembled.input,
        missing: Vec::new(),
        input_schema: None,
        event: Some(event),
        error: None,
    })
}

pub fn redacted_input_for_display(input: &JsonMap) -> JsonMap {
    input
        .iter()
        .map(|(key, value)| {
            if key.ends_with("_secret") {
                (key.clone(), Value::String("***".to_string()))
            } else {
                (key.clone(), value.clone())
            }
        })
        .collect()
}

fn input_schema_properties(schema: Option<&Value>) -> Vec<String> {
    schema
        .and_then(|schema| schema.get("properties"))
        .and_then(Value::as_object)
        .map(|properties| properties.keys().cloned().collect())
        .unwrap_or_default()
}

fn input_schema_required(schema: Option<&Value>) -> Vec<String> {
    schema
        .and_then(|schema| schema.get("required"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn is_empty_string(value: &Value) -> bool {
    matches!(value, Value::String(text) if text.is_empty())
}

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::binding::{Binding, BindingContext, BindingScope, resolve_binding};
use crate::error::AgentFirstUiError;
use crate::operation::parse_pointer;
use crate::types::{
    AgentFirstUiMessage, Capability, Id, JsonMap, MessagePayload, OwnerKey, Risk, Space, Unit,
};
use crate::unit_profile::FIELD_INPUT_BINDINGS;

/// Options for assembling request input.
#[derive(Debug, Clone, Default)]
pub struct PrepareRequestOptions {
    pub input_value: Option<Value>,
    pub provided_input: JsonMap,
}

impl PrepareRequestOptions {
    pub fn with_input_value(mut self, value: Value) -> Self {
        self.input_value = Some(value);
        self
    }

    pub fn with_provided_input(mut self, input: JsonMap) -> Self {
        self.provided_input = input;
        self
    }
}

/// The assembled input for a request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssembledInput {
    pub input: JsonMap,
    pub missing: Vec<String>,
}

/// Whether a prepared request is ready to send, needs more input, or hit an error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreparedStatus {
    Ready,
    NeedsInput,
    Error,
}

/// A fully prepared request, ready for review and dispatch (or with a status
/// indicating what is missing or wrong).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreparedRequest {
    pub status: PreparedStatus,
    pub capability: Id,
    pub risk: Risk,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<OwnerKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub definition: Option<Capability>,
    #[serde(default, skip_serializing_if = "JsonMap::is_empty")]
    pub input: JsonMap,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missing: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<AgentFirstUiMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Spec for preparing a request: who is asking, which owner is targeted, what
/// capability is invoked, and how to resolve the input.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PrepareRequestSpec {
    pub author: OwnerKey,
    pub owner: OwnerKey,
    pub capability: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_unit_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_value: Option<Value>,
    #[serde(default, skip_serializing_if = "JsonMap::is_empty")]
    pub provided_input: JsonMap,
}

/// Request payload routed to the owner that declared the capability.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OwnerRequest {
    pub request_id: String,
    pub author: OwnerKey,
    pub owner: OwnerKey,
    pub capability: Id,
    pub risk: Risk,
    #[serde(default, skip_serializing_if = "JsonMap::is_empty")]
    pub input: JsonMap,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestExecutionStatus {
    Accepted,
    Rejected,
    Failed,
}

/// Owner-produced result for a request. Any space changes caused by an accepted
/// request are represented as the emitted messages in `emits`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OwnerRequestResult {
    pub request_id: String,
    pub owner: OwnerKey,
    pub capability: Id,
    pub status: RequestExecutionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub emits: Vec<AgentFirstUiMessage>,
}

impl OwnerRequestResult {
    pub fn accepted(request: &OwnerRequest) -> Self {
        Self::new(request, RequestExecutionStatus::Accepted)
    }

    pub fn rejected(request: &OwnerRequest, error: impl Into<String>) -> Self {
        Self::new(request, RequestExecutionStatus::Rejected).with_error(error)
    }

    pub fn failed(request: &OwnerRequest, error: impl Into<String>) -> Self {
        Self::new(request, RequestExecutionStatus::Failed).with_error(error)
    }

    pub fn with_output(mut self, output: Value) -> Self {
        self.output = Some(output);
        self
    }

    pub fn with_emit(mut self, emit: AgentFirstUiMessage) -> Self {
        self.emits.push(emit);
        self
    }

    fn new(request: &OwnerRequest, status: RequestExecutionStatus) -> Self {
        Self {
            request_id: request.request_id.clone(),
            owner: request.owner.clone(),
            capability: request.capability.clone(),
            status,
            output: None,
            error: None,
            emits: Vec::new(),
        }
    }

    fn with_error(mut self, error: impl Into<String>) -> Self {
        self.error = Some(error.into());
        self
    }
}

/// A synchronous owner-side request handler. Async transports can wrap this
/// contract in their own task/runtime while preserving the same data shape.
pub trait OwnerRequestHandler {
    fn handle_owner_request(
        &mut self,
        request: OwnerRequest,
    ) -> Result<OwnerRequestResult, AgentFirstUiError>;
}

impl PrepareRequestSpec {
    pub fn new(
        author: impl Into<OwnerKey>,
        owner: impl Into<OwnerKey>,
        capability: impl Into<Id>,
    ) -> Self {
        Self {
            author: author.into(),
            owner: owner.into(),
            capability: capability.into(),
            source_unit_id: None,
            record_path: None,
            input_value: None,
            provided_input: JsonMap::new(),
        }
    }

    pub fn with_source_unit_id(mut self, id: impl Into<Id>) -> Self {
        self.source_unit_id = Some(id.into());
        self
    }

    pub fn with_record_path(mut self, path: impl Into<String>) -> Self {
        self.record_path = Some(path.into());
        self
    }

    pub fn with_input_value(mut self, value: Value) -> Self {
        self.input_value = Some(value);
        self
    }

    pub fn with_provided_input(mut self, provided: JsonMap) -> Self {
        self.provided_input = provided;
        self
    }
}

pub fn owner_request_from_prepared(
    request_id: impl Into<String>,
    prepared: &PreparedRequest,
) -> Result<OwnerRequest, AgentFirstUiError> {
    if prepared.status != PreparedStatus::Ready {
        return Err(AgentFirstUiError::InvalidRequest {
            message: format!("prepared request must be ready, got {:?}", prepared.status),
        });
    }
    let message = prepared
        .message
        .as_ref()
        .ok_or_else(|| AgentFirstUiError::InvalidRequest {
            message: "ready request is missing request message".to_string(),
        })?;
    let MessagePayload::Request {
        author,
        owner,
        capability,
        input,
    } = &message.payload
    else {
        return Err(AgentFirstUiError::InvalidRequest {
            message: "prepared message is not a request".to_string(),
        });
    };
    Ok(OwnerRequest {
        request_id: request_id.into(),
        author: author.clone(),
        owner: owner.clone(),
        capability: capability.clone(),
        risk: prepared.risk.clone(),
        input: input_as_map(input.as_ref())?,
    })
}

pub fn owner_request_from_message(
    space: &Space,
    request_id: impl Into<String>,
    message: &AgentFirstUiMessage,
) -> Result<OwnerRequest, AgentFirstUiError> {
    message.ensure_version()?;
    let MessagePayload::Request {
        author,
        owner,
        capability,
        input,
    } = &message.payload
    else {
        return Err(AgentFirstUiError::UnsupportedMessageType {
            message_type: message.payload.message_type().to_string(),
        });
    };
    let Some(definition) = find_capability(space, owner, capability) else {
        return Err(AgentFirstUiError::InvalidRequest {
            message: format!("unresolved capability `{owner}/{capability}`"),
        });
    };
    Ok(OwnerRequest {
        request_id: request_id.into(),
        author: author.clone(),
        owner: owner.clone(),
        capability: capability.clone(),
        risk: definition.risk,
        input: input_as_map(input.as_ref())?,
    })
}

/// Find a capability by owner-local id.
pub fn find_capability(space: &Space, owner: &str, capability_id: &str) -> Option<Capability> {
    let section = space.section(owner)?;
    let caps = section.get("capabilities")?.as_array()?;
    for cap_val in caps {
        if let Ok(cap) = serde_json::from_value::<Capability>(cap_val.clone())
            && cap.id == capability_id
        {
            return Some(cap);
        }
    }
    None
}

/// Find a unit by owner-local id.
pub fn find_unit(space: &Space, owner: &str, unit_id: &str) -> Option<Unit> {
    let section = space.section(owner)?;
    let units = section.get("units").and_then(|u| u.as_array())?;
    for unit_val in units {
        if let Ok(unit) = serde_json::from_value::<Unit>(unit_val.clone())
            && unit.id == unit_id
        {
            return Some(unit);
        }
    }
    None
}

/// Assemble the input for a request from a source unit's `input_bindings` plus
/// any directly provided values.
pub fn assemble_request_input(
    space: &Space,
    source_unit: Option<&Unit>,
    capability: &Capability,
    record: Option<&Value>,
    options: &PrepareRequestOptions,
) -> Result<AssembledInput, AgentFirstUiError> {
    assemble_request_input_with_actor(space, source_unit, capability, record, None, options)
}

/// Assemble request input with an optional current actor section for `actor:`
/// bindings. When `input_value` is absent, `provided_input` is also exposed as
/// the `input:` scope so canonical form values can feed contextual bindings.
pub fn assemble_request_input_with_actor(
    space: &Space,
    source_unit: Option<&Unit>,
    capability: &Capability,
    record: Option<&Value>,
    actor: Option<&Value>,
    options: &PrepareRequestOptions,
) -> Result<AssembledInput, AgentFirstUiError> {
    let mut input = JsonMap::new();
    let properties = input_schema_properties(capability.input_schema.as_ref());
    let required = input_schema_required(capability.input_schema.as_ref());

    let provided_input_value = Value::Object(options.provided_input.clone());
    let input_value = options.input_value.as_ref().or_else(|| {
        if options.provided_input.is_empty() {
            None
        } else {
            Some(&provided_input_value)
        }
    });
    let ctx = BindingContext::new(space.as_value())
        .with_record(record)
        .with_input(input_value)
        .with_actor(actor);

    if let Some(unit) = source_unit
        && let Some(bindings_val) = unit.extra.get(FIELD_INPUT_BINDINGS)
    {
        let Some(bindings) = bindings_val.as_object() else {
            return Err(AgentFirstUiError::InvalidBinding {
                binding: FIELD_INPUT_BINDINGS.to_string(),
                reason: "input_bindings must be an object".to_string(),
            });
        };
        for name in &properties {
            let Some(bind_val) = bindings.get(name) else {
                continue;
            };
            let Some(binding_str) = bind_val.as_str() else {
                return Err(AgentFirstUiError::InvalidBinding {
                    binding: format!("{FIELD_INPUT_BINDINGS}.{name}"),
                    reason: "input binding must be a string".to_string(),
                });
            };
            if let Some(resolved) = resolve_binding(&ctx, binding_str)?
                && !is_empty_string(resolved)
            {
                input.insert(name.clone(), resolved.clone());
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

    Ok(AssembledInput { input, missing })
}

/// Prepare a request against a space, resolving the capability, source unit,
/// record, and input.
pub fn prepare_request(
    space: &Space,
    spec: PrepareRequestSpec,
) -> Result<PreparedRequest, AgentFirstUiError> {
    let Some(capability) = find_capability(space, &spec.owner, &spec.capability) else {
        return Ok(PreparedRequest {
            status: PreparedStatus::Error,
            capability: spec.capability,
            risk: Risk::Destructive,
            owner: Some(spec.owner),
            definition: None,
            input: JsonMap::new(),
            missing: Vec::new(),
            input_schema: None,
            message: None,
            error: Some("Unresolved owner/capability reference".to_string()),
        });
    };

    let source_unit = match spec.source_unit_id.as_deref() {
        Some(uid) => Some(find_unit(space, &spec.owner, uid).ok_or_else(|| {
            AgentFirstUiError::PathNotFound {
                path: format!("unit `{}/{uid}`", spec.owner),
            }
        })?),
        None => None,
    };

    let record =
        match spec.record_path.as_deref() {
            Some(path) => {
                parse_pointer(path)?;
                Some(space.as_value().pointer(path).ok_or_else(|| {
                    AgentFirstUiError::PathNotFound {
                        path: path.to_string(),
                    }
                })?)
            }
            None => None,
        };

    let options = PrepareRequestOptions {
        input_value: spec.input_value,
        provided_input: spec.provided_input,
    };
    let actor_section = space.section(&spec.author);

    let assembled = assemble_request_input_with_actor(
        space,
        source_unit.as_ref(),
        &capability,
        record,
        actor_section,
        &options,
    )?;

    if !assembled.missing.is_empty() {
        return Ok(PreparedRequest {
            status: PreparedStatus::NeedsInput,
            capability: capability.id.clone(),
            risk: capability.risk.clone(),
            owner: Some(spec.owner),
            definition: Some(capability.clone()),
            input: assembled.input,
            missing: assembled.missing,
            input_schema: capability.input_schema.clone(),
            message: None,
            error: None,
        });
    }

    let input_value = Value::Object(assembled.input.clone());
    let message = AgentFirstUiMessage::request(
        spec.author,
        spec.owner.clone(),
        capability.id.clone(),
        Some(input_value),
    );
    Ok(PreparedRequest {
        status: PreparedStatus::Ready,
        capability: capability.id.clone(),
        risk: capability.risk.clone(),
        owner: Some(spec.owner),
        definition: Some(capability),
        input: assembled.input,
        missing: Vec::new(),
        input_schema: None,
        message: Some(message),
        error: None,
    })
}

/// Return the JSON Pointer to a row in a unit's `source_binding` array.
///
/// Unit bindings use dot paths (`space:owner.items`), while request preparation
/// consumes JSON Pointers (`/owner/items/0`). This helper is the canonical bridge
/// for viewers selecting a row.
pub fn record_path_for_unit_row(
    space: &Space,
    owner: &str,
    unit_id: &str,
    row_index: usize,
) -> Result<String, AgentFirstUiError> {
    let unit = find_unit(space, owner, unit_id).ok_or_else(|| AgentFirstUiError::PathNotFound {
        path: format!("unit `{owner}/{unit_id}`"),
    })?;
    let binding_value =
        unit.field_as_str("source_binding")
            .ok_or_else(|| AgentFirstUiError::InvalidBinding {
                binding: format!("{unit_id}.source_binding"),
                reason: "unit has no source_binding".to_string(),
            })?;
    let pointer = space_binding_to_pointer(binding_value)?;
    let source =
        space
            .as_value()
            .pointer(&pointer)
            .ok_or_else(|| AgentFirstUiError::PathNotFound {
                path: pointer.clone(),
            })?;
    let rows = source
        .as_array()
        .ok_or_else(|| AgentFirstUiError::InvalidBinding {
            binding: binding_value.to_string(),
            reason: "source_binding must resolve to an array for row addressing".to_string(),
        })?;
    if row_index >= rows.len() {
        return Err(AgentFirstUiError::InvalidArrayIndex {
            path: pointer,
            index: row_index.to_string(),
        });
    }
    Ok(append_pointer_token(
        &space_binding_to_pointer(binding_value)?,
        &row_index.to_string(),
    ))
}

pub fn space_binding_to_pointer(binding_value: &str) -> Result<String, AgentFirstUiError> {
    let binding = Binding::parse(binding_value)?;
    if binding.scope != BindingScope::Space {
        return Err(AgentFirstUiError::InvalidBinding {
            binding: binding_value.to_string(),
            reason: "record row paths require a space-scoped binding".to_string(),
        });
    }
    Ok(dot_path_to_pointer(&binding.path))
}

/// Redact `_secret` fields in the input map for display purposes.
pub fn redacted_input_for_display(input: &JsonMap) -> JsonMap {
    input
        .iter()
        .map(|(key, value)| (key.clone(), redact_secret_value(key, value)))
        .collect()
}

fn redact_secret_value(key: &str, value: &Value) -> Value {
    if key.ends_with("_secret") {
        return Value::String("***".to_string());
    }
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(child_key, child_value)| {
                    (
                        child_key.clone(),
                        redact_secret_value(child_key, child_value),
                    )
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| redact_secret_value("", item))
                .collect(),
        ),
        _ => value.clone(),
    }
}

fn input_schema_properties(schema: Option<&Value>) -> Vec<String> {
    schema
        .and_then(|s| s.get("properties"))
        .and_then(Value::as_object)
        .map(|props| props.keys().cloned().collect())
        .unwrap_or_default()
}

fn input_schema_required(schema: Option<&Value>) -> Vec<String> {
    schema
        .and_then(|s| s.get("required"))
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

fn input_as_map(input: Option<&Value>) -> Result<JsonMap, AgentFirstUiError> {
    match input {
        Some(Value::Object(map)) => Ok(map.clone()),
        Some(_) => Err(AgentFirstUiError::InvalidRequest {
            message: "request input must be a JSON object".to_string(),
        }),
        None => Ok(JsonMap::new()),
    }
}

fn dot_path_to_pointer(path: &str) -> String {
    if path == "." || path.is_empty() {
        return String::new();
    }
    path.split('.').fold(String::new(), |pointer, token| {
        append_pointer_token(&pointer, token)
    })
}

fn append_pointer_token(pointer: &str, token: &str) -> String {
    let escaped = token.replace('~', "~0").replace('/', "~1");
    if pointer.is_empty() {
        format!("/{escaped}")
    } else {
        format!("{pointer}/{escaped}")
    }
}

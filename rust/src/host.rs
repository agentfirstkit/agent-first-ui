use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::action::{prepare_user_action, PrepareUserActionOptions, PreparedUserAction};
use crate::error::AfuiError;
use crate::patch::parse_pointer;
use crate::runtime::apply_afui_message;
use crate::types::{AfuiMessage, AfuiPayload, Id, JsonMap, Snapshot};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PrepareActionRequest {
    pub action_id: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_view_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_value: Option<Value>,
    #[serde(default, skip_serializing_if = "JsonMap::is_empty")]
    pub provided_input: JsonMap,
}

impl PrepareActionRequest {
    pub fn new(action_id: impl Into<Id>) -> Self {
        Self {
            action_id: action_id.into(),
            source_view_id: None,
            record_path: None,
            input_value: None,
            provided_input: JsonMap::new(),
        }
    }

    pub fn with_source_view_id(mut self, source_view_id: impl Into<Id>) -> Self {
        self.source_view_id = Some(source_view_id.into());
        self
    }

    pub fn with_record_path(mut self, record_path: impl Into<String>) -> Self {
        self.record_path = Some(record_path.into());
        self
    }

    pub fn with_input_value(mut self, input_value: Value) -> Self {
        self.input_value = Some(input_value);
        self
    }

    pub fn with_provided_input(mut self, provided_input: JsonMap) -> Self {
        self.provided_input = provided_input;
        self
    }
}

#[derive(Debug, Clone, Default)]
pub struct HostStore {
    snapshot: Option<Snapshot>,
    audit: Vec<AfuiMessage>,
}

impl HostStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_snapshot(snapshot: Snapshot) -> Self {
        Self {
            snapshot: Some(snapshot),
            audit: Vec::new(),
        }
    }

    pub fn snapshot(&self) -> Option<&Snapshot> {
        self.snapshot.as_ref()
    }

    pub fn audit(&self) -> &[AfuiMessage] {
        &self.audit
    }

    pub fn replace_snapshot(&mut self, snapshot: Snapshot) {
        self.snapshot = Some(snapshot);
    }

    pub fn apply_message(&mut self, message: &AfuiMessage) -> Result<&Snapshot, AfuiError> {
        let next = apply_afui_message(self.snapshot.as_ref(), message)?;
        self.audit.push(message.clone());
        self.snapshot = Some(next);
        self.snapshot.as_ref().ok_or(AfuiError::MissingSnapshot)
    }

    pub fn prepare_action(
        &self,
        request: &PrepareActionRequest,
    ) -> Result<PreparedUserAction, AfuiError> {
        let snapshot = self.snapshot.as_ref().ok_or(AfuiError::MissingSnapshot)?;
        prepare_action_against_snapshot(snapshot, request)
    }

    pub fn emit_user_action(&mut self, event: AfuiMessage) -> Result<(), AfuiError> {
        event.ensure_version()?;
        match &event.payload {
            AfuiPayload::UserAction(_) => {
                self.audit.push(event);
                Ok(())
            }
            other => Err(AfuiError::UnsupportedMessageType {
                message_type: other.message_type().to_string(),
            }),
        }
    }
}

pub fn prepare_action_against_snapshot(
    snapshot: &Snapshot,
    request: &PrepareActionRequest,
) -> Result<PreparedUserAction, AfuiError> {
    let source_view = match request.source_view_id.as_deref() {
        Some(id) => {
            Some(
                snapshot
                    .document
                    .find_view(id)
                    .ok_or_else(|| AfuiError::PathNotFound {
                        path: format!("document view `{id}`"),
                    })?,
            )
        }
        None => None,
    };

    let record = match request.record_path.as_deref() {
        Some(path) => {
            parse_pointer(path)?;
            Some(snapshot.state.as_value().pointer(path).ok_or_else(|| {
                AfuiError::PathNotFound {
                    path: path.to_string(),
                }
            })?)
        }
        None => None,
    };

    let options =
        PrepareUserActionOptions::default().with_provided_input(request.provided_input.clone());
    let options = if let Some(input_value) = &request.input_value {
        options.with_input_value(input_value.clone())
    } else {
        options
    };

    prepare_user_action(snapshot, source_view, &request.action_id, record, &options)
}

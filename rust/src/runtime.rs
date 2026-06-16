use serde_json::Value;

use crate::error::AfuiError;
use crate::patch::apply_patch;
use crate::types::{AfuiMessage, AfuiPayload, Document, Snapshot, State, Theme};
use crate::validation::validate_document;

pub fn apply_afui_message(
    current: Option<&Snapshot>,
    message: &AfuiMessage,
) -> Result<Snapshot, AfuiError> {
    message.ensure_version()?;

    match &message.payload {
        AfuiPayload::UiSnapshot {
            document,
            state,
            theme,
        } => {
            validate_document(document)?;
            Ok(Snapshot {
                document: document.clone(),
                state: state.clone(),
                theme: theme.clone(),
            })
        }
        AfuiPayload::StatePatch { patch } => {
            let current = current.ok_or(AfuiError::MissingSnapshot)?;
            let next_state = apply_patch(current.state.as_value(), patch)?;
            Ok(Snapshot {
                document: current.document.clone(),
                state: State::try_from_value(next_state)?,
                theme: current.theme.clone(),
            })
        }
        AfuiPayload::DocumentPatch { patch } => {
            let current = current.ok_or(AfuiError::MissingSnapshot)?;
            let document_value = serde_json::to_value(&current.document)?;
            let next_document_value = apply_patch(&document_value, patch)?;
            let next_document: Document = serde_json::from_value(next_document_value)?;
            validate_document(&next_document)?;
            Ok(Snapshot {
                document: next_document,
                state: current.state.clone(),
                theme: current.theme.clone(),
            })
        }
        AfuiPayload::ThemePatch { patch } => {
            let current = current.ok_or(AfuiError::MissingSnapshot)?;
            let theme_value = current
                .theme
                .as_ref()
                .map(|theme| theme.as_value().clone())
                .unwrap_or_else(|| Value::Object(Default::default()));
            let next_theme = apply_patch(&theme_value, patch)?;
            Ok(Snapshot {
                document: current.document.clone(),
                state: current.state.clone(),
                theme: Some(Theme::try_from_value(next_theme)?),
            })
        }
        AfuiPayload::UserAction(_) => Err(AfuiError::UnsupportedMessageType {
            message_type: "user_action".to_string(),
        }),
    }
}

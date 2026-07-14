use crate::error::AgentFirstUiError;
use crate::operation::{Operation, apply_operations, parse_pointer};
use crate::types::{AgentFirstUiMessage, MessagePayload, Space};
use crate::validation::{is_valid_owner_key, validate_space};

/// Advance a space by one message.
///
/// - A **snapshot** replaces the space and resets the emit-log position; it is
///   checked for well-formedness first.
/// - An **emit** applies its JSON Patch operations to the current space, after
///   verifying the author writes only its own section, and advances the position.
/// - A **request** is an intent routed to an owner, not a space change, so it is
///   not applied here.
pub fn apply_message(
    current_space: Option<&Space>,
    message: &AgentFirstUiMessage,
) -> Result<(Space, u64), AgentFirstUiError> {
    message.ensure_version()?;

    match &message.payload {
        MessagePayload::Snapshot { space, sequence } => {
            validate_space(space)?;
            Ok((space.clone(), *sequence))
        }
        MessagePayload::Emit {
            author,
            sequence,
            operations,
        } => {
            let current = current_space.ok_or(AgentFirstUiError::MissingSpace)?;
            ensure_emit_write_scope(author, operations)?;
            let next = apply_operations(current.as_value(), operations)?;
            let next_space = Space::try_from_value(next)?;
            validate_space(&next_space)?;
            Ok((next_space, *sequence))
        }
        MessagePayload::Request { .. } => Err(AgentFirstUiError::UnsupportedMessageType {
            message_type: "request".to_string(),
        }),
    }
}

/// An owner is the sole writer of its own section. Every operation in an emit
/// must target a path under the author's section; a move also removes from its
/// `from` path, so that must be owned too. (A copy only reads from `from`, so it
/// may read another section.)
///
/// Exposed so owner/transport code that scopes emits per audience (see
/// `projection`) enforces the same write-scope before delivering an emit.
pub fn ensure_emit_write_scope(
    author: &str,
    operations: &[Operation],
) -> Result<(), AgentFirstUiError> {
    if !is_valid_owner_key(author) {
        return Err(AgentFirstUiError::InvalidIdentifier {
            identifier: author.to_string(),
        });
    }
    for operation in operations {
        ensure_owned(author, operation.path())?;
        if let Operation::Move { from, .. } = operation {
            ensure_owned(author, from)?;
        }
    }
    Ok(())
}

fn ensure_owned(author: &str, pointer: &str) -> Result<(), AgentFirstUiError> {
    let tokens = parse_pointer(pointer)?;
    match tokens.first() {
        Some(section) if section == author => Ok(()),
        _ => Err(AgentFirstUiError::CrossSectionEmit {
            author: author.to_string(),
            path: pointer.to_string(),
        }),
    }
}

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::AgentFirstUiError;

/// The operations of an emit: an ordered list of JSON Patch operations applied
/// to the space.
pub type Operations = Vec<Operation>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum Operation {
    Add { path: String, value: Value },
    Remove { path: String },
    Replace { path: String, value: Value },
    Move { from: String, path: String },
    Copy { from: String, path: String },
    Test { path: String, value: Value },
}

impl Operation {
    /// The primary target path of the operation (the path being written).
    pub fn path(&self) -> &str {
        match self {
            Self::Add { path, .. }
            | Self::Remove { path }
            | Self::Replace { path, .. }
            | Self::Move { path, .. }
            | Self::Copy { path, .. }
            | Self::Test { path, .. } => path,
        }
    }
}

pub fn apply_operations(target: &Value, patch: &[Operation]) -> Result<Value, AgentFirstUiError> {
    let mut document = target.clone();
    for operation in patch {
        apply_operation(&mut document, operation)?;
    }
    Ok(document)
}

pub fn parse_pointer(pointer: &str) -> Result<Vec<String>, AgentFirstUiError> {
    if pointer.is_empty() {
        return Ok(Vec::new());
    }
    if !pointer.starts_with('/') {
        return Err(AgentFirstUiError::InvalidJsonPointer {
            pointer: pointer.to_string(),
            reason: "pointer must be empty or start with '/'".to_string(),
        });
    }

    pointer
        .strip_prefix('/')
        .unwrap_or_default()
        .split('/')
        .map(|part| decode_pointer_token(pointer, part))
        .collect()
}

/// Convert an actor-relative dot path to an absolute JSON Pointer under `actor`.
///
/// This is the safe write target for viewer-owned interaction state: callers still
/// emit as `actor`, and the resulting operation remains inside that same section.
pub fn actor_state_pointer(actor: &str, path: &str) -> Result<String, AgentFirstUiError> {
    validate_actor_state_target(actor, path)?;
    let mut pointer = String::new();
    pointer.push('/');
    pointer.push_str(&encode_pointer_token(actor));
    for token in path.split('.') {
        pointer.push('/');
        pointer.push_str(&encode_pointer_token(token));
    }
    Ok(pointer)
}

/// Build the JSON Patch operation for setting viewer-owned actor state.
pub fn set_actor_state_operation(
    actor: &str,
    path: &str,
    value: Value,
) -> Result<Operation, AgentFirstUiError> {
    Ok(Operation::Add {
        path: actor_state_pointer(actor, path)?,
        value,
    })
}

fn validate_actor_state_target(actor: &str, path: &str) -> Result<(), AgentFirstUiError> {
    if !is_separator_safe(actor) {
        return Err(AgentFirstUiError::InvalidIdentifier {
            identifier: actor.to_string(),
        });
    }
    if !is_valid_actor_state_path(path) {
        return Err(AgentFirstUiError::InvalidBinding {
            binding: path.to_string(),
            reason: "actor state path must be a relative dot path with separator-safe segments"
                .to_string(),
        });
    }
    Ok(())
}

pub(crate) fn is_valid_actor_state_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains(':')
        && path.split('.').all(is_separator_safe)
}

fn is_separator_safe(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
}

fn encode_pointer_token(token: &str) -> String {
    token.replace('~', "~0").replace('/', "~1")
}

fn decode_pointer_token(pointer: &str, token: &str) -> Result<String, AgentFirstUiError> {
    let mut decoded = String::new();
    let mut chars = token.chars();
    while let Some(ch) = chars.next() {
        if ch == '~' {
            match chars.next() {
                Some('0') => decoded.push('~'),
                Some('1') => decoded.push('/'),
                Some(other) => {
                    return Err(AgentFirstUiError::InvalidJsonPointer {
                        pointer: pointer.to_string(),
                        reason: format!("invalid escape `~{other}`"),
                    });
                }
                None => {
                    return Err(AgentFirstUiError::InvalidJsonPointer {
                        pointer: pointer.to_string(),
                        reason: "trailing '~' escape".to_string(),
                    });
                }
            }
        } else {
            decoded.push(ch);
        }
    }
    Ok(decoded)
}

fn apply_operation(document: &mut Value, operation: &Operation) -> Result<(), AgentFirstUiError> {
    match operation {
        Operation::Test { path, value } => {
            if read_at(document, path)? == value {
                Ok(())
            } else {
                Err(AgentFirstUiError::PatchTestFailed { path: path.clone() })
            }
        }
        Operation::Add { path, value } => add_at(document, path, value.clone()),
        Operation::Remove { path } => remove_at(document, path).map(|_| ()),
        Operation::Replace { path, value } => replace_at(document, path, value.clone()),
        Operation::Copy { from, path } => {
            let copied = read_at(document, from)?.clone();
            add_at(document, path, copied)
        }
        Operation::Move { from, path } => {
            if from == path {
                return Ok(());
            }
            ensure_not_moving_into_child(from, path)?;
            let moved = remove_at(document, from)?;
            add_at(document, path, moved)
        }
    }
}

fn ensure_not_moving_into_child(from: &str, path: &str) -> Result<(), AgentFirstUiError> {
    let from_tokens = parse_pointer(from)?;
    let path_tokens = parse_pointer(path)?;
    if !from_tokens.is_empty()
        && from_tokens.len() < path_tokens.len()
        && path_tokens.starts_with(&from_tokens)
    {
        return Err(AgentFirstUiError::InvalidPatch {
            message: "cannot move a value into one of its children".to_string(),
        });
    }
    Ok(())
}

fn read_at<'a>(document: &'a Value, pointer: &str) -> Result<&'a Value, AgentFirstUiError> {
    let tokens = parse_pointer(pointer)?;
    let mut current = document;
    for token in tokens {
        current = match current {
            Value::Object(map) => {
                map.get(&token)
                    .ok_or_else(|| AgentFirstUiError::PathNotFound {
                        path: pointer.to_string(),
                    })?
            }
            Value::Array(items) => {
                let index = existing_array_index(pointer, &token, items.len())?;
                items
                    .get(index)
                    .ok_or_else(|| AgentFirstUiError::PathNotFound {
                        path: pointer.to_string(),
                    })?
            }
            _ => {
                return Err(AgentFirstUiError::PathNotFound {
                    path: pointer.to_string(),
                });
            }
        };
    }
    Ok(current)
}

fn parent_at_mut<'a>(
    document: &'a mut Value,
    pointer: &str,
) -> Result<Option<(&'a mut Value, String)>, AgentFirstUiError> {
    let mut tokens = parse_pointer(pointer)?;
    let Some(key) = tokens.pop() else {
        return Ok(None);
    };

    let mut current = document;
    for token in tokens {
        current = match current {
            Value::Object(map) => {
                map.get_mut(&token)
                    .ok_or_else(|| AgentFirstUiError::PathNotFound {
                        path: pointer.to_string(),
                    })?
            }
            Value::Array(items) => {
                let index = existing_array_index(pointer, &token, items.len())?;
                items
                    .get_mut(index)
                    .ok_or_else(|| AgentFirstUiError::PathNotFound {
                        path: pointer.to_string(),
                    })?
            }
            _ => {
                return Err(AgentFirstUiError::PathNotFound {
                    path: pointer.to_string(),
                });
            }
        };
    }

    Ok(Some((current, key)))
}

fn add_at(document: &mut Value, pointer: &str, value: Value) -> Result<(), AgentFirstUiError> {
    let Some((parent, key)) = parent_at_mut(document, pointer)? else {
        *document = value;
        return Ok(());
    };

    match parent {
        Value::Object(map) => {
            map.insert(key, value);
            Ok(())
        }
        Value::Array(items) => {
            if key == "-" {
                items.push(value);
                return Ok(());
            }
            let index = insertion_array_index(pointer, &key, items.len())?;
            items.insert(index, value);
            Ok(())
        }
        _ => Err(AgentFirstUiError::PathNotFound {
            path: pointer.to_string(),
        }),
    }
}

fn remove_at(document: &mut Value, pointer: &str) -> Result<Value, AgentFirstUiError> {
    let Some((parent, key)) = parent_at_mut(document, pointer)? else {
        return Ok(std::mem::take(document));
    };

    match parent {
        Value::Object(map) => map
            .remove(&key)
            .ok_or_else(|| AgentFirstUiError::PathNotFound {
                path: pointer.to_string(),
            }),
        Value::Array(items) => {
            if key == "-" {
                return Err(AgentFirstUiError::InvalidArrayIndex {
                    path: pointer.to_string(),
                    index: key,
                });
            }
            let index = existing_array_index(pointer, &key, items.len())?;
            Ok(items.remove(index))
        }
        _ => Err(AgentFirstUiError::PathNotFound {
            path: pointer.to_string(),
        }),
    }
}

fn replace_at(document: &mut Value, pointer: &str, value: Value) -> Result<(), AgentFirstUiError> {
    let Some((parent, key)) = parent_at_mut(document, pointer)? else {
        *document = value;
        return Ok(());
    };

    match parent {
        Value::Object(map) => {
            if !map.contains_key(&key) {
                return Err(AgentFirstUiError::PathNotFound {
                    path: pointer.to_string(),
                });
            }
            map.insert(key, value);
            Ok(())
        }
        Value::Array(items) => {
            if key == "-" {
                return Err(AgentFirstUiError::InvalidArrayIndex {
                    path: pointer.to_string(),
                    index: key,
                });
            }
            let index = existing_array_index(pointer, &key, items.len())?;
            if let Some(slot) = items.get_mut(index) {
                *slot = value;
                Ok(())
            } else {
                Err(AgentFirstUiError::PathNotFound {
                    path: pointer.to_string(),
                })
            }
        }
        _ => Err(AgentFirstUiError::PathNotFound {
            path: pointer.to_string(),
        }),
    }
}

fn existing_array_index(path: &str, token: &str, len: usize) -> Result<usize, AgentFirstUiError> {
    let index = parse_array_index(path, token)?;
    if index < len {
        Ok(index)
    } else {
        Err(AgentFirstUiError::PathNotFound {
            path: path.to_string(),
        })
    }
}

fn insertion_array_index(path: &str, token: &str, len: usize) -> Result<usize, AgentFirstUiError> {
    let index = parse_array_index(path, token)?;
    if index <= len {
        Ok(index)
    } else {
        Err(AgentFirstUiError::InvalidArrayIndex {
            path: path.to_string(),
            index: token.to_string(),
        })
    }
}

fn parse_array_index(path: &str, token: &str) -> Result<usize, AgentFirstUiError> {
    if token.starts_with('0') && token.len() > 1 {
        return Err(AgentFirstUiError::InvalidArrayIndex {
            path: path.to_string(),
            index: token.to_string(),
        });
    }
    token
        .parse::<usize>()
        .map_err(|_| AgentFirstUiError::InvalidArrayIndex {
            path: path.to_string(),
            index: token.to_string(),
        })
}

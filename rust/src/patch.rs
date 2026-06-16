use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::AfuiError;

pub type Patch = Vec<PatchOperation>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum PatchOperation {
    Add { path: String, value: Value },
    Remove { path: String },
    Replace { path: String, value: Value },
    Move { from: String, path: String },
    Copy { from: String, path: String },
    Test { path: String, value: Value },
}

pub fn apply_patch(target: &Value, patch: &[PatchOperation]) -> Result<Value, AfuiError> {
    let mut document = target.clone();
    for operation in patch {
        apply_operation(&mut document, operation)?;
    }
    Ok(document)
}

pub fn parse_pointer(pointer: &str) -> Result<Vec<String>, AfuiError> {
    if pointer.is_empty() {
        return Ok(Vec::new());
    }
    if !pointer.starts_with('/') {
        return Err(AfuiError::InvalidJsonPointer {
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

fn decode_pointer_token(pointer: &str, token: &str) -> Result<String, AfuiError> {
    let mut decoded = String::new();
    let mut chars = token.chars();
    while let Some(ch) = chars.next() {
        if ch == '~' {
            match chars.next() {
                Some('0') => decoded.push('~'),
                Some('1') => decoded.push('/'),
                Some(other) => {
                    return Err(AfuiError::InvalidJsonPointer {
                        pointer: pointer.to_string(),
                        reason: format!("invalid escape `~{other}`"),
                    })
                }
                None => {
                    return Err(AfuiError::InvalidJsonPointer {
                        pointer: pointer.to_string(),
                        reason: "trailing '~' escape".to_string(),
                    })
                }
            }
        } else {
            decoded.push(ch);
        }
    }
    Ok(decoded)
}

fn apply_operation(document: &mut Value, operation: &PatchOperation) -> Result<(), AfuiError> {
    match operation {
        PatchOperation::Test { path, value } => {
            if read_at(document, path)? == value {
                Ok(())
            } else {
                Err(AfuiError::PatchTestFailed { path: path.clone() })
            }
        }
        PatchOperation::Add { path, value } => add_at(document, path, value.clone()),
        PatchOperation::Remove { path } => remove_at(document, path).map(|_| ()),
        PatchOperation::Replace { path, value } => replace_at(document, path, value.clone()),
        PatchOperation::Copy { from, path } => {
            let copied = read_at(document, from)?.clone();
            add_at(document, path, copied)
        }
        PatchOperation::Move { from, path } => {
            if from == path {
                return Ok(());
            }
            ensure_not_moving_into_child(from, path)?;
            let moved = remove_at(document, from)?;
            add_at(document, path, moved)
        }
    }
}

fn ensure_not_moving_into_child(from: &str, path: &str) -> Result<(), AfuiError> {
    let from_tokens = parse_pointer(from)?;
    let path_tokens = parse_pointer(path)?;
    if !from_tokens.is_empty()
        && from_tokens.len() < path_tokens.len()
        && path_tokens.starts_with(&from_tokens)
    {
        return Err(AfuiError::InvalidPatch {
            message: "cannot move a value into one of its children".to_string(),
        });
    }
    Ok(())
}

fn read_at<'a>(document: &'a Value, pointer: &str) -> Result<&'a Value, AfuiError> {
    let tokens = parse_pointer(pointer)?;
    let mut current = document;
    for token in tokens {
        current = match current {
            Value::Object(map) => map.get(&token).ok_or_else(|| AfuiError::PathNotFound {
                path: pointer.to_string(),
            })?,
            Value::Array(items) => {
                let index = existing_array_index(pointer, &token, items.len())?;
                items.get(index).ok_or_else(|| AfuiError::PathNotFound {
                    path: pointer.to_string(),
                })?
            }
            _ => {
                return Err(AfuiError::PathNotFound {
                    path: pointer.to_string(),
                })
            }
        };
    }
    Ok(current)
}

fn parent_at_mut<'a>(
    document: &'a mut Value,
    pointer: &str,
) -> Result<Option<(&'a mut Value, String)>, AfuiError> {
    let mut tokens = parse_pointer(pointer)?;
    let Some(key) = tokens.pop() else {
        return Ok(None);
    };

    let mut current = document;
    for token in tokens {
        current = match current {
            Value::Object(map) => map.get_mut(&token).ok_or_else(|| AfuiError::PathNotFound {
                path: pointer.to_string(),
            })?,
            Value::Array(items) => {
                let index = existing_array_index(pointer, &token, items.len())?;
                items
                    .get_mut(index)
                    .ok_or_else(|| AfuiError::PathNotFound {
                        path: pointer.to_string(),
                    })?
            }
            _ => {
                return Err(AfuiError::PathNotFound {
                    path: pointer.to_string(),
                })
            }
        };
    }

    Ok(Some((current, key)))
}

fn add_at(document: &mut Value, pointer: &str, value: Value) -> Result<(), AfuiError> {
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
        _ => Err(AfuiError::PathNotFound {
            path: pointer.to_string(),
        }),
    }
}

fn remove_at(document: &mut Value, pointer: &str) -> Result<Value, AfuiError> {
    let Some((parent, key)) = parent_at_mut(document, pointer)? else {
        return Ok(std::mem::take(document));
    };

    match parent {
        Value::Object(map) => map.remove(&key).ok_or_else(|| AfuiError::PathNotFound {
            path: pointer.to_string(),
        }),
        Value::Array(items) => {
            if key == "-" {
                return Err(AfuiError::InvalidArrayIndex {
                    path: pointer.to_string(),
                    index: key,
                });
            }
            let index = existing_array_index(pointer, &key, items.len())?;
            Ok(items.remove(index))
        }
        _ => Err(AfuiError::PathNotFound {
            path: pointer.to_string(),
        }),
    }
}

fn replace_at(document: &mut Value, pointer: &str, value: Value) -> Result<(), AfuiError> {
    let Some((parent, key)) = parent_at_mut(document, pointer)? else {
        *document = value;
        return Ok(());
    };

    match parent {
        Value::Object(map) => {
            if !map.contains_key(&key) {
                return Err(AfuiError::PathNotFound {
                    path: pointer.to_string(),
                });
            }
            map.insert(key, value);
            Ok(())
        }
        Value::Array(items) => {
            if key == "-" {
                return Err(AfuiError::InvalidArrayIndex {
                    path: pointer.to_string(),
                    index: key,
                });
            }
            let index = existing_array_index(pointer, &key, items.len())?;
            if let Some(slot) = items.get_mut(index) {
                *slot = value;
                Ok(())
            } else {
                Err(AfuiError::PathNotFound {
                    path: pointer.to_string(),
                })
            }
        }
        _ => Err(AfuiError::PathNotFound {
            path: pointer.to_string(),
        }),
    }
}

fn existing_array_index(path: &str, token: &str, len: usize) -> Result<usize, AfuiError> {
    let index = parse_array_index(path, token)?;
    if index < len {
        Ok(index)
    } else {
        Err(AfuiError::PathNotFound {
            path: path.to_string(),
        })
    }
}

fn insertion_array_index(path: &str, token: &str, len: usize) -> Result<usize, AfuiError> {
    let index = parse_array_index(path, token)?;
    if index <= len {
        Ok(index)
    } else {
        Err(AfuiError::InvalidArrayIndex {
            path: path.to_string(),
            index: token.to_string(),
        })
    }
}

fn parse_array_index(path: &str, token: &str) -> Result<usize, AfuiError> {
    if token.starts_with('0') && token.len() > 1 {
        return Err(AfuiError::InvalidArrayIndex {
            path: path.to_string(),
            index: token.to_string(),
        });
    }
    token
        .parse::<usize>()
        .map_err(|_| AfuiError::InvalidArrayIndex {
            path: path.to_string(),
            index: token.to_string(),
        })
}

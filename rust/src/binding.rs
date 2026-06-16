use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::AfuiError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BindingScope {
    State,
    Record,
    Input,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub scope: BindingScope,
    pub path: String,
}

impl Binding {
    pub fn parse(value: &str) -> Result<Self, AfuiError> {
        let (scope, path) = if let Some((prefix, path)) = value.split_once(':') {
            let scope = match prefix {
                "state" => BindingScope::State,
                "record" => BindingScope::Record,
                "input" => BindingScope::Input,
                other => {
                    return Err(AfuiError::InvalidBinding {
                        binding: value.to_string(),
                        reason: format!("unknown scope `{other}`"),
                    })
                }
            };
            (scope, path)
        } else {
            (BindingScope::State, value)
        };

        if path.is_empty() {
            return Err(AfuiError::InvalidBinding {
                binding: value.to_string(),
                reason: "path must not be empty".to_string(),
            });
        }
        if path.contains(':') {
            return Err(AfuiError::InvalidBinding {
                binding: value.to_string(),
                reason: "path must not contain ':'".to_string(),
            });
        }

        Ok(Self {
            scope,
            path: path.to_string(),
        })
    }
}

impl std::str::FromStr for Binding {
    type Err = AfuiError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct BindingContext<'a> {
    pub state: &'a Value,
    pub record: Option<&'a Value>,
    pub input: Option<&'a Value>,
}

impl<'a> BindingContext<'a> {
    pub fn new(state: &'a Value) -> Self {
        Self {
            state,
            record: None,
            input: None,
        }
    }

    pub fn with_record(mut self, record: Option<&'a Value>) -> Self {
        self.record = record;
        self
    }

    pub fn with_input(mut self, input: Option<&'a Value>) -> Self {
        self.input = input;
        self
    }
}

pub fn resolve_binding<'a>(
    ctx: &'a BindingContext<'a>,
    binding_value: &str,
) -> Result<Option<&'a Value>, AfuiError> {
    let binding = Binding::parse(binding_value)?;
    let root = match binding.scope {
        BindingScope::State => Some(ctx.state),
        BindingScope::Record => ctx.record,
        BindingScope::Input => ctx.input,
    };
    let Some(root) = root else {
        return Ok(None);
    };

    if binding.scope == BindingScope::Input && binding.path == "value" && !root.is_object() {
        return Ok(Some(root));
    }

    Ok(get_dot_path(root, &binding.path))
}

pub fn get_dot_path<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    if path == "." || path.is_empty() {
        return Some(root);
    }

    let mut current = root;
    for token in path.split('.') {
        current = match current {
            Value::Object(map) => map.get(token)?,
            Value::Array(items) => {
                let index = token.parse::<usize>().ok()?;
                items.get(index)?
            }
            _ => return None,
        };
    }
    Some(current)
}

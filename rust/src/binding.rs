use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::AgentFirstUiError;

/// The scopes a binding may read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BindingScope {
    /// The shared space tree.
    Space,
    /// The current row of a list or table.
    Record,
    /// The request input being assembled.
    Input,
    /// The current actor/viewer's own section.
    Actor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub scope: BindingScope,
    pub path: String,
}

impl Binding {
    pub fn parse(value: &str) -> Result<Self, AgentFirstUiError> {
        let (scope, path) = if let Some((prefix, path)) = value.split_once(':') {
            let scope = match prefix {
                "space" => BindingScope::Space,
                "record" => BindingScope::Record,
                "input" => BindingScope::Input,
                "actor" => BindingScope::Actor,
                other => {
                    return Err(AgentFirstUiError::InvalidBinding {
                        binding: value.to_string(),
                        reason: format!("unknown scope `{other}`"),
                    });
                }
            };
            (scope, path)
        } else {
            (BindingScope::Space, value)
        };

        if path.is_empty() {
            return Err(AgentFirstUiError::InvalidBinding {
                binding: value.to_string(),
                reason: "path must not be empty".to_string(),
            });
        }
        if path.contains(':') {
            return Err(AgentFirstUiError::InvalidBinding {
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
    type Err = AgentFirstUiError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct BindingContext<'a> {
    pub space: &'a Value,
    pub record: Option<&'a Value>,
    pub input: Option<&'a Value>,
    pub actor: Option<&'a Value>,
}

impl<'a> BindingContext<'a> {
    pub fn new(space: &'a Value) -> Self {
        Self {
            space,
            record: None,
            input: None,
            actor: None,
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

    pub fn with_actor(mut self, actor: Option<&'a Value>) -> Self {
        self.actor = actor;
        self
    }
}

pub fn resolve_binding<'a>(
    context: &'a BindingContext<'a>,
    binding_value: &str,
) -> Result<Option<&'a Value>, AgentFirstUiError> {
    let binding = Binding::parse(binding_value)?;
    let root = match binding.scope {
        BindingScope::Space => Some(context.space),
        BindingScope::Record => context.record,
        BindingScope::Input => context.input,
        BindingScope::Actor => context.actor,
    };
    let Some(root) = root else {
        return Ok(None);
    };

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

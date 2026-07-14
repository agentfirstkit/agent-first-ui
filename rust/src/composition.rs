use serde_json::Value;

use crate::error::AgentFirstUiError;
use crate::types::{JsonMap, OwnerKey, Space};
use crate::validation::{is_valid_owner_key, validate_space};

/// Ensure a new owner id can join a space without colliding with an existing
/// top-level section.
pub fn ensure_owner_available(space: &Space, owner: &str) -> Result<(), AgentFirstUiError> {
    if !is_valid_owner_key(owner) {
        return Err(AgentFirstUiError::InvalidIdentifier {
            identifier: owner.to_string(),
        });
    }
    if space.section(owner).is_some() {
        return Err(AgentFirstUiError::DuplicateIdentifier {
            identifier: owner.to_string(),
        });
    }
    Ok(())
}

/// Insert one owner section into a space, rejecting owner-id collisions.
pub fn insert_owner_section(
    space: &mut Space,
    owner: impl Into<OwnerKey>,
    section: Value,
) -> Result<(), AgentFirstUiError> {
    let owner = owner.into();
    ensure_owner_available(space, &owner)?;
    let mut next = space.as_value().clone();
    let Some(sections) = next.as_object_mut() else {
        return Err(AgentFirstUiError::InvalidSpace {
            message: "space must be a JSON object".to_string(),
        });
    };
    sections.insert(owner, section);
    let next_space = Space::try_from_value(next)?;
    validate_space(&next_space)?;
    *space = next_space;
    Ok(())
}

/// Compose complete spaces by placing owner sections under their existing owner
/// ids. No owner or local id rewriting is performed.
pub fn compose_spaces<I>(sources: I) -> Result<Space, AgentFirstUiError>
where
    I: IntoIterator<Item = Space>,
{
    let mut sections = JsonMap::new();
    for source in sources {
        validate_space(&source)?;
        let Some(source_sections) = source.as_value().as_object() else {
            return Err(AgentFirstUiError::InvalidSpace {
                message: "source space was not an object".to_string(),
            });
        };
        for (owner, section) in source_sections {
            if sections.insert(owner.clone(), section.clone()).is_some() {
                return Err(AgentFirstUiError::DuplicateIdentifier {
                    identifier: owner.clone(),
                });
            }
        }
    }
    let composed = Space::from(sections);
    validate_space(&composed)?;
    Ok(composed)
}

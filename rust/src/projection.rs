use serde::{Deserialize, Serialize};

use crate::error::AgentFirstUiError;
use crate::operation::{Operations, apply_operations};
use crate::runtime::ensure_emit_write_scope;
use crate::types::{AgentFirstUiMessage, OwnerKey, Space};
use crate::validation::validate_space;

/// A transport-defined delivery audience identifier.
///
/// This is intentionally out-of-band metadata: it is not serialized into AFUI
/// snapshot or emit messages.
pub type Audience = String;

/// A JSON Patch overlay applied only when materializing one audience's delivery.
///
/// Projection patches are owner/transport metadata, not part of the AFUI wire
/// format. Applying them produces an ordinary `Space` that can be wrapped in a
/// `snapshot` and delivered without any hidden audience marker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectionPatch {
    pub audience: Audience,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub operations: Operations,
}

impl ProjectionPatch {
    pub fn new(audience: impl Into<Audience>, operations: Operations) -> Self {
        Self {
            audience: audience.into(),
            operations,
        }
    }
}

/// A public/base space plus out-of-band overlays for individual audiences.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectionSet {
    pub base: Space,
    pub patches: Vec<ProjectionPatch>,
}

impl ProjectionSet {
    pub fn new(base: Space) -> Self {
        Self {
            base,
            patches: Vec::new(),
        }
    }

    pub fn with_patch(mut self, patch: ProjectionPatch) -> Self {
        self.patches.push(patch);
        self
    }

    pub fn push_patch(&mut self, patch: ProjectionPatch) {
        self.patches.push(patch);
    }

    /// Materialize the already-projected space for one audience.
    pub fn project_for(&self, audience: &str) -> Result<Space, AgentFirstUiError> {
        project_space_for(&self.base, audience, &self.patches)
    }

    /// Materialize a wire snapshot for one audience.
    pub fn snapshot_for(
        &self,
        audience: &str,
        sequence: u64,
    ) -> Result<AgentFirstUiMessage, AgentFirstUiError> {
        project_snapshot_for(&self.base, audience, &self.patches, sequence)
    }
}

/// Apply the patches whose `audience` exactly matches `audience` and return a
/// normal delivered projection.
///
/// The input space is never mutated. Any patch/path/validation failure returns an
/// error and no partial projection is exposed to the caller.
pub fn project_space_for(
    base: &Space,
    audience: &str,
    patches: &[ProjectionPatch],
) -> Result<Space, AgentFirstUiError> {
    validate_space(base)?;

    let mut projected = base.as_value().clone();
    for patch in patches
        .iter()
        .filter(|patch| patch.audience.as_str() == audience)
    {
        projected = apply_operations(&projected, &patch.operations)?;
    }

    let space = Space::try_from_value(projected)?;
    validate_space(&space)?;
    Ok(space)
}

/// Materialize a projected space and wrap it in a `snapshot` message.
pub fn project_snapshot_for(
    base: &Space,
    audience: &str,
    patches: &[ProjectionPatch],
    sequence: u64,
) -> Result<AgentFirstUiMessage, AgentFirstUiError> {
    Ok(AgentFirstUiMessage::snapshot(
        project_space_for(base, audience, patches)?,
        sequence,
    ))
}

/// The operations one audience receives in place of the base — how an owner
/// whispers a different change (or, with an empty list, nothing) to a subset of
/// participants.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmitAudienceOverride {
    pub audience: Audience,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub operations: Operations,
}

impl EmitAudienceOverride {
    pub fn new(audience: impl Into<Audience>, operations: Operations) -> Self {
        Self {
            audience: audience.into(),
            operations,
        }
    }
}

/// A base emit plus out-of-band per-audience operation overlays — the emit-log
/// analogue of [`ProjectionSet`]. The owner/transport authors one base operation
/// list (delivered to every audience without an override) and, per whisper
/// audience, an alternate list. Like [`ProjectionPatch`], the overrides are
/// owner/transport metadata and never reach the wire: [`Self::emit_for`] returns
/// an ordinary `emit` carrying just that audience's operations, with no audience
/// marker.
#[derive(Debug, Clone, PartialEq)]
pub struct EmitProjectionSet {
    pub author: OwnerKey,
    pub sequence: u64,
    pub base: Operations,
    pub overrides: Vec<EmitAudienceOverride>,
}

impl EmitProjectionSet {
    pub fn new(author: impl Into<OwnerKey>, sequence: u64, base: Operations) -> Self {
        Self {
            author: author.into(),
            sequence,
            base,
            overrides: Vec::new(),
        }
    }

    pub fn with_override(mut self, over: EmitAudienceOverride) -> Self {
        self.overrides.push(over);
        self
    }

    pub fn push_override(&mut self, over: EmitAudienceOverride) {
        self.overrides.push(over);
    }

    /// Materialize the emit delivered to one audience.
    pub fn emit_for(&self, audience: &str) -> Result<AgentFirstUiMessage, AgentFirstUiError> {
        project_emit_for(
            &self.author,
            self.sequence,
            &self.base,
            audience,
            &self.overrides,
        )
    }
}

/// Select one audience's emit operations (its override if any, else the base),
/// enforce emit write-scope, and wrap them in an `emit` message.
///
/// The input is never mutated, and a cross-section operation is rejected rather
/// than delivered, so an owner cannot whisper a write into another owner's
/// section. An empty operation list yields a no-op emit a transport may drop.
pub fn project_emit_for(
    author: &str,
    sequence: u64,
    base: &Operations,
    audience: &str,
    overrides: &[EmitAudienceOverride],
) -> Result<AgentFirstUiMessage, AgentFirstUiError> {
    let operations = overrides
        .iter()
        .find(|over| over.audience.as_str() == audience)
        .map(|over| over.operations.clone())
        .unwrap_or_else(|| base.clone());
    ensure_emit_write_scope(author, &operations)?;
    Ok(AgentFirstUiMessage::emit(
        author.to_string(),
        sequence,
        operations,
    ))
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use serde_json::{Value, json};

    use super::*;
    use crate::operation::Operation;

    fn base_space() -> Result<Space, AgentFirstUiError> {
        Space::try_from_value(json!({
            "owner_afmail_7Qk2x9fL": {
                "status": "public",
                "drafts": [
                    { "id": "d1", "subject": "shared" },
                    { "id": "d2", "subject": "sensitive" }
                ],
                "capabilities": [
                    { "id": "refresh", "label": "Refresh", "risk": "read_only" },
                    { "id": "send_draft", "label": "Send Draft", "risk": "external_effect" }
                ],
                "units": [
                    { "id": "mailbox_status", "kind": "facts", "source_binding": "space:owner_afmail_7Qk2x9fL.status" }
                ],
                "layouts": [
                    { "id": "root", "members": ["mailbox_status"] }
                ],
                "root_layout": "root"
            },
            "owner_operator_Vt3pN1aE7": { "selection": null }
        }))
    }

    fn has_key(value: &Value, key: &str) -> bool {
        match value {
            Value::Object(map) => map
                .iter()
                .any(|(candidate, child)| candidate == key || has_key(child, key)),
            Value::Array(items) => items.iter().any(|item| has_key(item, key)),
            _ => false,
        }
    }

    #[test]
    fn public_base_is_visible_when_no_patch_matches() -> Result<(), Box<dyn Error>> {
        let base = base_space()?;
        let projected = project_space_for(
            &base,
            "session/bob",
            &[ProjectionPatch::new(
                "session/alice",
                vec![Operation::Replace {
                    path: "/owner_afmail_7Qk2x9fL/status".to_string(),
                    value: json!("alice-only"),
                }],
            )],
        )?;

        assert_eq!(
            projected
                .as_value()
                .pointer("/owner_afmail_7Qk2x9fL/status"),
            Some(&json!("public"))
        );
        validate_space(&projected)?;
        Ok(())
    }

    #[test]
    fn matching_patch_changes_and_omits_values_before_delivery() -> Result<(), Box<dyn Error>> {
        let base = base_space()?;
        let projected = project_space_for(
            &base,
            "session/alice",
            &[ProjectionPatch::new(
                "session/alice",
                vec![
                    Operation::Replace {
                        path: "/owner_afmail_7Qk2x9fL/status".to_string(),
                        value: json!("alice-only"),
                    },
                    Operation::Remove {
                        path: "/owner_afmail_7Qk2x9fL/drafts/1".to_string(),
                    },
                    Operation::Remove {
                        path: "/owner_afmail_7Qk2x9fL/capabilities/1".to_string(),
                    },
                ],
            )],
        )?;

        assert_eq!(
            projected
                .as_value()
                .pointer("/owner_afmail_7Qk2x9fL/status"),
            Some(&json!("alice-only"))
        );
        assert_eq!(
            projected
                .as_value()
                .pointer("/owner_afmail_7Qk2x9fL/drafts"),
            Some(&json!([{ "id": "d1", "subject": "shared" }]))
        );
        assert_eq!(
            projected
                .as_value()
                .pointer("/owner_afmail_7Qk2x9fL/capabilities"),
            Some(&json!([{ "id": "refresh", "label": "Refresh", "risk": "read_only" }]))
        );
        validate_space(&projected)?;
        Ok(())
    }

    #[test]
    fn projected_snapshot_carries_no_audience_marker() -> Result<(), Box<dyn Error>> {
        let base = base_space()?;
        let snapshot = project_snapshot_for(
            &base,
            "session/alice",
            &[ProjectionPatch::new(
                "session/alice",
                vec![Operation::Add {
                    path: "/owner_afmail_7Qk2x9fL/personal_badge".to_string(),
                    value: json!("alice"),
                }],
            )],
            42,
        )?;
        let value = serde_json::to_value(snapshot)?;

        assert_eq!(value.get("type"), Some(&json!("snapshot")));
        assert!(value.get("space").is_some());
        assert!(value.get("room").is_none());
        assert!(!has_key(&value, "audience"));
        Ok(())
    }

    #[test]
    fn invalid_projection_patch_fails_without_mutating_base() -> Result<(), Box<dyn Error>> {
        let base = base_space()?;
        let result = project_space_for(
            &base,
            "session/alice",
            &[ProjectionPatch::new(
                "session/alice",
                vec![Operation::Remove {
                    path: "/owner_afmail_7Qk2x9fL/missing".to_string(),
                }],
            )],
        );

        assert!(matches!(
            result,
            Err(AgentFirstUiError::PathNotFound { .. })
        ));
        assert_eq!(
            base.as_value().pointer("/owner_afmail_7Qk2x9fL/status"),
            Some(&json!("public"))
        );
        assert_eq!(
            base.as_value()
                .pointer("/owner_afmail_7Qk2x9fL/drafts/1/subject"),
            Some(&json!("sensitive"))
        );
        Ok(())
    }

    #[test]
    fn projection_that_would_deliver_invalid_space_is_rejected() -> Result<(), Box<dyn Error>> {
        let base = base_space()?;
        let result = project_space_for(
            &base,
            "session/alice",
            &[ProjectionPatch::new(
                "session/alice",
                vec![Operation::Remove {
                    path: "/owner_afmail_7Qk2x9fL/units/0".to_string(),
                }],
            )],
        );

        assert!(matches!(
            result,
            Err(AgentFirstUiError::WellFormedness { .. })
        ));
        assert_eq!(
            base.as_value().pointer("/owner_afmail_7Qk2x9fL/units/0/id"),
            Some(&json!("mailbox_status"))
        );
        Ok(())
    }

    #[test]
    fn projection_set_projects_and_snapshots() -> Result<(), Box<dyn Error>> {
        let set = ProjectionSet::new(base_space()?).with_patch(ProjectionPatch::new(
            "session/alice",
            vec![Operation::Replace {
                path: "/owner_afmail_7Qk2x9fL/status".to_string(),
                value: json!("alice-only"),
            }],
        ));

        let projected = set.project_for("session/alice")?;
        assert_eq!(
            projected
                .as_value()
                .pointer("/owner_afmail_7Qk2x9fL/status"),
            Some(&json!("alice-only"))
        );

        let snapshot = set.snapshot_for("session/alice", 7)?;
        let value = serde_json::to_value(snapshot)?;
        assert_eq!(value.get("sequence"), Some(&json!(7)));
        assert_eq!(
            value.pointer("/space/owner_afmail_7Qk2x9fL/status"),
            Some(&json!("alice-only"))
        );
        assert!(!has_key(&value, "audience"));
        Ok(())
    }

    const AFMAIL: &str = "owner_afmail_7Qk2x9fL";

    fn set_status(value: &str) -> Operation {
        Operation::Replace {
            path: format!("/{AFMAIL}/status"),
            value: json!(value),
        }
    }

    #[test]
    fn emit_base_delivered_when_no_override_matches() -> Result<(), Box<dyn Error>> {
        let set = EmitProjectionSet::new(AFMAIL, 5, vec![set_status("public")]).with_override(
            EmitAudienceOverride::new("session/alice", vec![set_status("alice")]),
        );

        let value = serde_json::to_value(set.emit_for("session/bob")?)?;
        assert_eq!(value.get("type"), Some(&json!("emit")));
        assert_eq!(value.get("author"), Some(&json!(AFMAIL)));
        assert_eq!(value.get("sequence"), Some(&json!(5)));
        assert_eq!(value.pointer("/operations/0/value"), Some(&json!("public")));
        assert!(!has_key(&value, "audience"));
        Ok(())
    }

    #[test]
    fn emit_override_replaces_operations_for_its_audience() -> Result<(), Box<dyn Error>> {
        let set = EmitProjectionSet::new(AFMAIL, 5, vec![set_status("public")]).with_override(
            EmitAudienceOverride::new("session/alice", vec![set_status("alice")]),
        );

        let value = serde_json::to_value(set.emit_for("session/alice")?)?;
        assert_eq!(value.pointer("/operations/0/value"), Some(&json!("alice")));
        Ok(())
    }

    #[test]
    fn whisper_empty_base_delivers_nothing_to_other_audiences() -> Result<(), Box<dyn Error>> {
        // A pure whisper: only alice sees the change; everyone else gets a no-op.
        let set = EmitProjectionSet::new(AFMAIL, 9, Vec::new()).with_override(
            EmitAudienceOverride::new("session/alice", vec![set_status("secret")]),
        );

        let alice = serde_json::to_value(set.emit_for("session/alice")?)?;
        assert_eq!(alice.pointer("/operations/0/value"), Some(&json!("secret")));

        let bob = serde_json::to_value(set.emit_for("session/bob")?)?;
        assert_eq!(bob.pointer("/operations"), Some(&json!([])));
        Ok(())
    }

    #[test]
    fn emit_projection_rejects_cross_section_operation() {
        let set = EmitProjectionSet::new(
            AFMAIL,
            1,
            vec![Operation::Replace {
                path: "/owner_operator_Vt3pN1aE7/selection".to_string(),
                value: json!("pwned"),
            }],
        );
        assert!(matches!(
            set.emit_for("session/alice"),
            Err(AgentFirstUiError::CrossSectionEmit { .. })
        ));
    }
}

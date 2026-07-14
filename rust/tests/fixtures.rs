use std::error::Error;

use agent_first_ui::{
    AgentFirstUiError, AgentFirstUiMessage, BindingContext, CapabilityBuilder, FormField,
    FormFieldFormat, FormFieldOption, FormFieldType, InputSchemaBuilder, InteractionEffect,
    LayoutBuilder, LayoutRelation, LayoutRole, Operation, OwnerRequestResult, PrepareRequestSpec,
    PreparedStatus, PresentationLevel, ProfileIssueLevel, ReferenceEnvelope,
    RequestExecutionStatus, Risk, SectionBuilder, Space, SpaceBuilder, SpaceStore, SurfaceGenre,
    UnitBuilder, ValidationProfile, actor_state_pointer, apply_message, compose_spaces,
    ensure_owner_available, find_capability, find_unit, inspect_space, is_valid_id,
    is_valid_owner_key, layout, owner_request_from_message, owner_request_from_prepared,
    parse_pointer, prepare_request, record_path_for_unit_row, redacted_input_for_display,
    resolve_binding, set_actor_state_operation, space_binding_to_pointer, validate_space,
    validate_space_bindings, validate_space_profile,
};
use serde_json::json;

const OPS_OWNER: &str = "owner_operations_H8sJ3kL9mN";
const OPERATOR_OWNER: &str = "owner_operator_Vt3pN1aE7";

fn operator_console_message() -> Result<AgentFirstUiMessage, Box<dyn Error>> {
    Ok(serde_json::from_str(include_str!(
        "../../examples/operator_console.afui.json"
    ))?)
}

fn load_space(msg: &AgentFirstUiMessage) -> Result<(Space, u64), Box<dyn Error>> {
    let (space, sequence) = apply_message(None, msg)?;
    Ok((space, sequence))
}

#[test]
fn parses_all_example_snapshots() -> Result<(), Box<dyn Error>> {
    let examples = [
        include_str!("../../examples/operator_console.afui.json"),
        include_str!("../../examples/go_game.afui.json"),
        include_str!("../../examples/terminal_operations.afui.json"),
    ];
    for example in examples {
        let message: AgentFirstUiMessage = serde_json::from_str(example)?;
        let (space, _sequence) = apply_message(None, &message)?;
        validate_space(&space)?;
        validate_space_bindings(&space)?;
        let report = validate_space_profile(&space, ValidationProfile::ReferenceHost);
        assert!(report.valid(), "errors: {:?}", report.issues);
    }
    Ok(())
}

#[test]
fn space_has_expected_owners_and_capabilities() -> Result<(), Box<dyn Error>> {
    let msg = operator_console_message()?;
    let (space, _) = load_space(&msg)?;
    validate_space(&space)?;

    let report = inspect_space(&space);
    assert!(
        report.owner_count >= 2,
        "expected at least 2 owners, got {}",
        report.owner_count
    );
    assert!(
        report.capability_count >= 1,
        "expected at least 1 capability"
    );
    assert!(
        !report.unit_kinds.is_empty(),
        "expected at least one unit kind"
    );
    assert!(
        report.validation.valid,
        "validation failed: {:?}",
        report.validation.issues
    );

    Ok(())
}

#[test]
fn prepare_request_ready_path() -> Result<(), Box<dyn Error>> {
    let msg = operator_console_message()?;
    let (space, _) = load_space(&msg)?;

    let obj = space.as_value().as_object().ok_or("space not an object")?;
    let mut found_owner = None;
    let mut found_cap_id = None;
    'outer: for (owner_key, section) in obj {
        if let Some(caps) = section.get("capabilities").and_then(|c| c.as_array()) {
            for cap in caps {
                if let Some(id) = cap.get("id").and_then(|i| i.as_str()) {
                    found_owner = Some(owner_key.clone());
                    found_cap_id = Some(id.to_string());
                    break 'outer;
                }
            }
        }
    }
    let owner = found_owner.ok_or("no capabilities found in space")?;
    let cap_id = found_cap_id.ok_or("no capability id found")?;

    let capability = find_capability(&space, &owner, &cap_id).ok_or("capability not found")?;
    let required_keys: Vec<String> = capability
        .input_schema
        .as_ref()
        .and_then(|s| s.get("required"))
        .and_then(|r| r.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    let mut provided = serde_json::Map::new();
    for key in &required_keys {
        provided.insert(key.clone(), json!("test_value"));
    }

    let spec =
        PrepareRequestSpec::new(OPERATOR_OWNER, owner, &cap_id).with_provided_input(provided);
    let prepared = prepare_request(&space, spec)?;

    assert_eq!(
        prepared.status,
        PreparedStatus::Ready,
        "expected Ready, got {:?}: {:?}",
        prepared.status,
        prepared.error
    );
    assert!(
        prepared.message.is_some(),
        "expected a message in Ready state"
    );

    Ok(())
}

#[test]
fn prepare_request_needs_input_when_required_field_missing() -> Result<(), Box<dyn Error>> {
    let space = Space::try_from_value(json!({
        OPS_OWNER: {
            "capabilities": [{
                "id": "deploy",
                "label": "Deploy",
                "risk": "destructive",
                "input_schema": {
                    "type": "object",
                    "required": ["target"],
                    "properties": {
                        "target": { "type": "string" }
                    }
                }
            }]
        },
        OPERATOR_OWNER: { "selection": null }
    }))?;

    let spec = PrepareRequestSpec::new(OPERATOR_OWNER, OPS_OWNER, "deploy");
    let prepared = prepare_request(&space, spec)?;

    assert_eq!(prepared.status, PreparedStatus::NeedsInput);
    assert!(prepared.missing.contains(&"target".to_string()));
    assert!(prepared.message.is_none());

    Ok(())
}

#[test]
fn prepare_request_error_on_unresolved_capability() -> Result<(), Box<dyn Error>> {
    let space = Space::try_from_value(json!({
        OPS_OWNER: { "status": "idle" },
        OPERATOR_OWNER: { "selection": null }
    }))?;

    let spec = PrepareRequestSpec::new(OPERATOR_OWNER, OPS_OWNER, "nonexistent_capability");
    let prepared = prepare_request(&space, spec)?;

    assert_eq!(prepared.status, PreparedStatus::Error);
    assert_eq!(prepared.risk, Risk::Destructive);
    assert!(
        prepared
            .error
            .as_deref()
            .map(|e| e.contains("Unresolved"))
            .unwrap_or(false)
    );

    Ok(())
}

#[test]
fn emit_ownership_rejected_for_cross_section() -> Result<(), Box<dyn Error>> {
    let space = Space::try_from_value(json!({
        OPS_OWNER: { "status": "idle" },
        OPERATOR_OWNER: { "selection": null }
    }))?;

    let msg = AgentFirstUiMessage::snapshot(space.clone(), 0);
    let mut store = SpaceStore::new();
    store.apply(&msg)?;

    let bad_emit = AgentFirstUiMessage::emit(
        OPERATOR_OWNER,
        1,
        vec![Operation::Replace {
            path: "/owner_operations_H8sJ3kL9mN/status".to_string(),
            value: json!("pwned"),
        }],
    );

    let result = store.apply(&bad_emit);
    assert!(
        matches!(result, Err(AgentFirstUiError::CrossSectionEmit { .. })),
        "expected CrossSectionEmit, got {result:?}"
    );

    Ok(())
}

#[test]
fn emit_that_orphans_reference_is_rejected_after_patch() -> Result<(), Box<dyn Error>> {
    let space = Space::try_from_value(json!({
        OPS_OWNER: {
            "units": [{ "id": "main", "kind": "table" }],
            "layouts": [{ "id": "root", "members": ["main"] }],
            "root_layout": "root"
        }
    }))?;

    let msg = AgentFirstUiMessage::snapshot(space, 0);
    let mut store = SpaceStore::new();
    store.apply(&msg)?;

    let bad_emit = AgentFirstUiMessage::emit(
        OPS_OWNER,
        1,
        vec![Operation::Remove {
            path: "/owner_operations_H8sJ3kL9mN/units/0".to_string(),
        }],
    );
    let result = store.apply(&bad_emit);

    assert!(
        matches!(result, Err(AgentFirstUiError::WellFormedness { .. })),
        "expected WellFormedness, got {result:?}"
    );
    assert_eq!(store.sequence(), 0);
    Ok(())
}

#[test]
fn risk_fallback_to_destructive() -> Result<(), Box<dyn Error>> {
    let space = Space::try_from_value(json!({
        OPS_OWNER: {
            "capabilities": [
                { "id": "c1", "label": "C1", "risk": "unknown_future_risk" }
            ]
        }
    }))?;
    let cap = find_capability(&space, OPS_OWNER, "c1").ok_or("c1 not found")?;
    assert_eq!(cap.risk, Risk::Destructive);
    Ok(())
}

#[test]
fn redacted_input_hides_secret_keys() {
    let input = serde_json::Map::from_iter([
        ("api_key_secret".to_string(), json!("sk_live")),
        (
            "nested".to_string(),
            json!({
                "token_secret": "tok_live",
                "items": [
                    { "password_secret": "pw", "visible": "ok" }
                ]
            }),
        ),
        ("target".to_string(), json!("prod")),
    ]);
    let redacted = redacted_input_for_display(&input);
    let redacted_value = json!(redacted);
    assert_eq!(redacted.get("api_key_secret"), Some(&json!("***")));
    assert_eq!(
        redacted_value.pointer("/nested/token_secret"),
        Some(&json!("***"))
    );
    assert_eq!(
        redacted_value.pointer("/nested/items/0/password_secret"),
        Some(&json!("***"))
    );
    assert_eq!(
        redacted_value.pointer("/nested/items/0/visible"),
        Some(&json!("ok"))
    );
    assert_eq!(redacted.get("target"), Some(&json!("prod")));
}

#[test]
fn replay_jsonl_handles_snapshot_emit_request() -> Result<(), Box<dyn Error>> {
    let jsonl = include_str!("../../spec/fixtures/operator_console.afui.jsonl");
    let report = agent_first_ui::replay_jsonl(jsonl);
    assert!(
        report.failure_count == 0,
        "replay failures: {:?}",
        report.failures
    );
    assert!(report.snapshot_count >= 1);
    assert!(report.final_report.is_some());
    Ok(())
}

#[test]
fn applies_rfc6902_operations() -> Result<(), Box<dyn Error>> {
    let space_val = json!({
        OPS_OWNER: {
            "status": "idle",
            "items": ["a", "c"]
        }
    });
    let space = Space::try_from_value(space_val)?;
    let msg = AgentFirstUiMessage::snapshot(space, 0);
    let mut store = SpaceStore::new();
    store.apply(&msg)?;

    let emit = AgentFirstUiMessage::emit(
        OPS_OWNER,
        1,
        vec![
            Operation::Test {
                path: "/owner_operations_H8sJ3kL9mN/status".to_string(),
                value: json!("idle"),
            },
            Operation::Replace {
                path: "/owner_operations_H8sJ3kL9mN/status".to_string(),
                value: json!("running"),
            },
            Operation::Add {
                path: "/owner_operations_H8sJ3kL9mN/items/1".to_string(),
                value: json!("b"),
            },
        ],
    );
    let space_after = store.apply(&emit)?;
    assert_eq!(
        space_after
            .as_value()
            .pointer("/owner_operations_H8sJ3kL9mN/status"),
        Some(&json!("running"))
    );
    assert_eq!(
        space_after
            .as_value()
            .pointer("/owner_operations_H8sJ3kL9mN/items/1"),
        Some(&json!("b"))
    );

    Ok(())
}

#[test]
fn store_rejects_non_monotonic_emit_sequence() -> Result<(), Box<dyn Error>> {
    let space = Space::try_from_value(json!({
        OPS_OWNER: { "status": "idle" }
    }))?;
    let mut store = SpaceStore::from_snapshot(space, 2);
    let stale_emit = AgentFirstUiMessage::emit(
        OPS_OWNER,
        2,
        vec![Operation::Replace {
            path: "/owner_operations_H8sJ3kL9mN/status".to_string(),
            value: json!("stale"),
        }],
    );

    let result = store.apply(&stale_emit);
    assert!(
        matches!(result, Err(AgentFirstUiError::NonMonotonicSequence { .. })),
        "expected NonMonotonicSequence, got {result:?}"
    );
    assert_eq!(
        store.space().and_then(|space| space
            .as_value()
            .pointer("/owner_operations_H8sJ3kL9mN/status")),
        Some(&json!("idle"))
    );
    Ok(())
}

#[test]
fn json_pointer_helpers() -> Result<(), Box<dyn Error>> {
    assert_eq!(parse_pointer("/")?, vec!["".to_string()]);
    assert_eq!(
        parse_pointer("/a~1b/c~0d")?,
        vec!["a/b".to_string(), "c~d".to_string()]
    );
    Ok(())
}

#[test]
fn find_unit_searches_flat_units() -> Result<(), Box<dyn Error>> {
    let space = Space::try_from_value(json!({
        OPS_OWNER: {
            "units": [
                { "id": "main_table", "kind": "table" },
                { "id": "table_header", "kind": "text", "text": "Header" }
            ]
        }
    }))?;

    let unit = find_unit(&space, OPS_OWNER, "main_table").ok_or("main_table not found")?;
    assert_eq!(unit.kind, "table");
    let header = find_unit(&space, OPS_OWNER, "table_header").ok_or("table_header not found")?;
    assert_eq!(header.kind, "text");
    assert!(find_unit(&space, OPS_OWNER, "nonexistent").is_none());
    Ok(())
}

#[test]
fn builders_create_valid_space() -> Result<(), Box<dyn Error>> {
    let cap = CapabilityBuilder::read_only("open_ticket", "Open Ticket")
        .input_schema(InputSchemaBuilder::new().string("ticket_id", true).build())
        .build();
    let unit = UnitBuilder::table("tickets")
        .title("Tickets")
        .source_binding("space:owner_operations_H8sJ3kL9mN.tickets")
        .input_binding("ticket_id", "record:id")
        .row_capabilities(["open_ticket"])
        .activate_capability("open_ticket")
        .build();
    let form = UnitBuilder::form("ticket_form")
        .title("Open ticket")
        .form_field(
            FormField::new("ticket_id", "Ticket", FormFieldType::String)
                .format(FormFieldFormat::Enum)
                .options([FormFieldOption::new("t1", "Ticket A")])
                .required(),
        )
        .submit_capability("open_ticket")
        .default_presentation(PresentationLevel::Compact)
        .build();

    let space = SpaceBuilder::new()
        .section(
            OPS_OWNER,
            SectionBuilder::new()
                .fact("tickets", json!([{ "id": "t1", "title": "A" }]))
                .capability(cap)
                .unit(unit)
                .unit(form)
                .layout(
                    LayoutBuilder::new("root", ["tickets", "ticket_form"])
                        .relation(LayoutRelation::OverviewDetail)
                        .role(LayoutRole::Primary)
                        .primary()
                        .build(),
                )
                .root_layout("root"),
        )?
        .build()?;

    validate_space(&space)?;
    let profile_report = validate_space_profile(&space, ValidationProfile::ReferenceHost);
    assert!(
        profile_report.valid(),
        "errors: {:?}",
        profile_report.issues
    );
    assert_eq!(
        space
            .as_value()
            .pointer("/owner_operations_H8sJ3kL9mN/layouts/0/relation"),
        Some(&json!("overview_detail"))
    );
    assert_eq!(
        space
            .as_value()
            .pointer("/owner_operations_H8sJ3kL9mN/layouts/0/role"),
        Some(&json!("primary"))
    );
    let interaction_unit = UnitBuilder::list("interactive_tickets")
        .source_binding("space:owner_operations_H8sJ3kL9mN.tickets")
        .on_activate(InteractionEffect::set_actor_state(
            "selection.ticket_id",
            "record:id",
        ))
        .on_activate(
            InteractionEffect::request("open_ticket").input_binding("ticket_id", "record:id"),
        )
        .build();
    assert_eq!(
        interaction_unit
            .field("interactions")
            .and_then(|value| value.pointer("/activate/0/type")),
        Some(&json!("set_actor_state"))
    );
    assert_eq!(
        interaction_unit
            .field("interactions")
            .and_then(|value| value.pointer("/activate/1/capability")),
        Some(&json!("open_ticket"))
    );
    assert_eq!(
        record_path_for_unit_row(&space, OPS_OWNER, "tickets", 0)?,
        "/owner_operations_H8sJ3kL9mN/tickets/0"
    );

    Ok(())
}

#[test]
fn profile_rejects_legacy_names() -> Result<(), Box<dyn Error>> {
    let space = Space::try_from_value(json!({
        OPS_OWNER: {
            "units": [
                {
                    "id": "legacy",
                    "kind": "stats",
                    "source_bind": "space:owner_operations_H8sJ3kL9mN.tickets",
                    "actions": ["open_ticket"]
                }
            ]
        }
    }))?;

    let report = validate_space_profile(&space, ValidationProfile::ReferenceHost);
    assert_eq!(report.error_count(), 3, "errors: {:?}", report.issues);
    assert!(!report.valid());
    assert!(
        report
            .issues
            .iter()
            .all(|issue| issue.level == ProfileIssueLevel::Error)
    );
    Ok(())
}

#[test]
fn profile_ignores_domain_fact_names() -> Result<(), Box<dyn Error>> {
    let space = Space::try_from_value(json!({
        OPS_OWNER: {
            "theme": { "color_primary": "red" },
            "role": "operator",
            "relation": "owner",
            "audit": {
                "action": "captured text",
                "source_bind": "not a hint binding",
                "record": { "kind": "stats" }
            },
            "units": [
                { "id": "main", "kind": "facts", "items": [] }
            ]
        }
    }))?;

    let report = validate_space_profile(&space, ValidationProfile::ReferenceHost);
    assert!(report.valid(), "errors: {:?}", report.issues);
    Ok(())
}

#[test]
fn profile_validates_layout_relation_and_role_only_on_layouts() -> Result<(), Box<dyn Error>> {
    let space = Space::try_from_value(json!({
        OPS_OWNER: {
            "units": [
                {
                    "id": "main",
                    "kind": "facts",
                    "role": "domain role text",
                    "metadata": {
                        "relation": "domain-specific relationship"
                    }
                }
            ],
            "layouts": [
                {
                    "id": "root",
                    "members": ["main"],
                    "relation": "columns",
                    "role": "panel"
                }
            ],
            "root_layout": "root"
        }
    }))?;

    let report = validate_space_profile(&space, ValidationProfile::ReferenceHost);
    assert_eq!(report.error_count(), 2, "errors: {:?}", report.issues);
    assert!(
        report
            .issues
            .iter()
            .any(|issue| issue.path.ends_with(".relation"))
    );
    assert!(
        report
            .issues
            .iter()
            .any(|issue| issue.path.ends_with(".role"))
    );
    Ok(())
}

#[test]
fn profile_rejects_noncanonical_interaction_events() -> Result<(), Box<dyn Error>> {
    let space = Space::try_from_value(json!({
        OPS_OWNER: {
            "units": [
                {
                    "id": "tickets",
                    "kind": "list",
                    "interactions": {
                        "double_click": [
                            { "type": "set_actor_state", "path": "selection.ticket_id", "value_binding": "record:id" }
                        ],
                        "activate": [
                            { "type": "run_script" }
                        ]
                    }
                }
            ]
        }
    }))?;

    let report = validate_space_profile(&space, ValidationProfile::ReferenceHost);
    assert_eq!(report.error_count(), 2, "errors: {:?}", report.issues);
    Ok(())
}

#[test]
fn profile_accepts_surface_genres() -> Result<(), Box<dyn Error>> {
    for genre in SurfaceGenre::VALUES {
        let space = Space::try_from_value(json!({
            OPS_OWNER: {
                "surface": { "genre": genre },
                "units": [
                    { "id": "main", "kind": "facts", "items": [] }
                ]
            }
        }))?;

        let report = validate_space_profile(&space, ValidationProfile::ReferenceHost);
        assert!(report.valid(), "genre {genre} errors: {:?}", report.issues);
    }
    Ok(())
}

#[test]
fn profile_rejects_unknown_surface_genre() -> Result<(), Box<dyn Error>> {
    let space = Space::try_from_value(json!({
        OPS_OWNER: {
            "surface": { "genre": "spreadsheet" },
            "units": [
                { "id": "main", "kind": "facts", "items": [] }
            ]
        }
    }))?;

    let report = validate_space_profile(&space, ValidationProfile::ReferenceHost);
    assert_eq!(report.error_count(), 1, "errors: {:?}", report.issues);
    assert!(
        report
            .issues
            .iter()
            .any(|issue| issue.path.ends_with(".surface.genre"))
    );
    Ok(())
}

#[test]
fn profile_accepts_canonical_form_reference_and_presentation_contract() -> Result<(), Box<dyn Error>>
{
    let reference_unit = UnitBuilder::list("messages")
        .reference_kind("message")
        .build();
    let reference =
        ReferenceEnvelope::from_unit(OPS_OWNER, &reference_unit, "msg_1").label("Quarterly update");
    assert_eq!(
        json!(reference),
        json!({
            "owner": OPS_OWNER,
            "unit": "messages",
            "kind": "message",
            "id": "msg_1",
            "label": "Quarterly update"
        })
    );
    let space = Space::try_from_value(json!({
        OPS_OWNER: {
            "capabilities": [{
                "id": "save_draft",
                "label": "Save draft",
                "risk": "local_mutation"
            }],
            "messages": [{ "id": "msg_1", "title": "Quarterly update" }],
            "units": [
                {
                    "id": "messages",
                    "kind": "list",
                    "source_binding": "space:owner_operations_H8sJ3kL9mN.messages",
                    "reference_binding": "record:id",
                    "reference_kind": "message",
                    "row_capabilities": ["save_draft"],
                    "fields": [
                        {"key": "cc", "label": "Cc", "visible_when": {"empty": false}},
                        {"key": "attachment_count", "label": "Attachments", "visible_when": {"gt": 0}}
                    ],
                    "default_presentation": "glance"
                },
                {
                    "id": "draft_form",
                    "kind": "form",
                    "fields": [
                        {
                            "name": "to",
                            "label": "To",
                            "type": "array",
                            "format": "email_list",
                            "required": true
                        },
                        {
                            "name": "cc",
                            "label": "Cc",
                            "type": "array",
                            "format": "email_list"
                        },
                        {
                            "name": "body_markdown",
                            "label": "Message",
                            "type": "string",
                            "format": "markdown",
                            "value_binding": "actor:drafts.msg_1.body_markdown",
                            "required": true
                        },
                        {
                            "name": "visibility",
                            "label": "Visibility",
                            "type": "string",
                            "format": "enum",
                            "options": [
                                { "value": "private", "label": "Private" },
                                { "value": "public", "label": "Public" }
                            ]
                        },
                        {
                            "name": "notify",
                            "label": "Notify recipients",
                            "type": "boolean"
                        }
                    ],
                    "submit_capability": "save_draft",
                    "default_presentation": "full"
                }
            ]
        }
    }))?;

    let report = validate_space_profile(&space, ValidationProfile::ReferenceHost);
    assert!(report.valid(), "errors: {:?}", report.issues);
    Ok(())
}

#[test]
fn profile_rejects_invalid_field_visibility_predicates() -> Result<(), Box<dyn Error>> {
    let space = Space::try_from_value(json!({
        OPS_OWNER: {
            "units": [{
                "id": "messages",
                "kind": "list",
                "fields": [
                    {"key": "cc", "visible_when": []},
                    {"key": "attachments", "visible_when": {"gt": "zero"}},
                    {"key": "status", "visible_when": {"unknown": true}}
                ]
            }]
        }
    }))?;

    let report = validate_space_profile(&space, ValidationProfile::ReferenceHost);
    assert_eq!(report.error_count(), 3, "errors: {:?}", report.issues);
    assert!(
        report
            .issues
            .iter()
            .all(|issue| issue.path.contains("visible_when"))
    );
    Ok(())
}

#[test]
fn profile_rejects_retired_input_reference_label_and_invalid_form_fields()
-> Result<(), Box<dyn Error>> {
    let space = Space::try_from_value(json!({
        OPS_OWNER: {
            "capabilities": [{
                "id": "save",
                "label": "Save",
                "risk": "local_mutation"
            }],
            "units": [
                {
                    "id": "old_input",
                    "kind": "input",
                    "reference_label": "Copy"
                },
                {
                    "id": "bad_form",
                    "kind": "form",
                    "fields": [
                        {
                            "name": "recipients",
                            "label": "Recipients",
                            "type": "string",
                            "format": "email_list"
                        },
                        {
                            "name": "recipients",
                            "label": "Recipients again",
                            "type": "string",
                            "format": "enum"
                        }
                    ],
                    "submit_capability": "save",
                    "default_presentation": "expanded"
                },
                {
                    "id": "not_a_form",
                    "kind": "facts",
                    "submit_capability": "save"
                },
                {
                    "id": "empty_action_surface",
                    "kind": "capabilities",
                    "capabilities": []
                }
            ]
        }
    }))?;

    let report = validate_space_profile(&space, ValidationProfile::ReferenceHost);
    assert!(!report.valid());
    assert!(
        report
            .issues
            .iter()
            .any(|issue| issue.message.contains("retired unit kind `input`"))
    );
    assert!(
        report
            .issues
            .iter()
            .any(|issue| issue.message.contains("retired reference_label"))
    );
    assert!(
        report
            .issues
            .iter()
            .any(|issue| issue.message.contains("must be unique"))
    );
    assert!(
        report
            .issues
            .iter()
            .any(|issue| issue.message.contains("incompatible"))
    );
    assert!(
        report
            .issues
            .iter()
            .any(|issue| issue.message.contains("requires a non-empty options"))
    );
    assert!(
        report
            .issues
            .iter()
            .any(|issue| issue.path.ends_with(".default_presentation"))
    );
    assert!(
        report
            .issues
            .iter()
            .any(|issue| issue.message.contains("only valid on a canonical form"))
    );
    assert!(
        report
            .issues
            .iter()
            .any(|issue| issue.message.contains("requires a non-empty capabilities"))
    );
    Ok(())
}

#[test]
fn validation_accepts_actor_state_and_request_interactions() -> Result<(), Box<dyn Error>> {
    let space = Space::try_from_value(json!({
        OPS_OWNER: {
            "capabilities": [
                { "id": "open_ticket", "label": "Open", "risk": "read_only" }
            ],
            "units": [
                {
                    "id": "tickets",
                    "kind": "list",
                    "source_binding": "space:owner_operations_H8sJ3kL9mN.tickets",
                    "interactions": {
                        "activate": [
                            {
                                "type": "set_actor_state",
                                "path": "selection.ticket_id",
                                "value_binding": "record:id"
                            },
                            {
                                "type": "request",
                                "capability": "open_ticket",
                                "input_bindings": { "ticket_id": "record:id" }
                            }
                        ]
                    }
                }
            ],
            "tickets": [{ "id": "t1" }]
        }
    }))?;

    validate_space(&space)?;
    Ok(())
}

#[test]
fn validation_rejects_cross_owner_actor_state_targets() -> Result<(), Box<dyn Error>> {
    let space = Space::try_from_value(json!({
        OPS_OWNER: {
            "units": [
                {
                    "id": "tickets",
                    "kind": "list",
                    "interactions": {
                        "activate": [
                            {
                                "type": "set_actor_state",
                                "path": "space:owner_afmail.current_message",
                                "value_binding": "record:id"
                            }
                        ]
                    }
                }
            ]
        }
    }))?;

    assert!(matches!(
        validate_space(&space),
        Err(AgentFirstUiError::WellFormedness { .. })
    ));
    Ok(())
}

#[test]
fn request_input_resolves_record_binding() -> Result<(), Box<dyn Error>> {
    let space = Space::try_from_value(json!({
        OPS_OWNER: {
            "capabilities": [{
                "id": "open_ticket",
                "label": "Open",
                "risk": "read_only",
                "input_schema": {
                    "type": "object",
                    "required": ["ticket_id"],
                    "properties": { "ticket_id": { "type": "string" } }
                }
            }],
            "tickets": [{ "id": "t1" }, { "id": "t2" }],
            "units": [{
                "id": "tickets",
                "kind": "table",
                "source_binding": "space:owner_operations_H8sJ3kL9mN.tickets",
                "input_bindings": { "ticket_id": "record:id" },
                "row_capabilities": ["open_ticket"]
            }]
        }
    }))?;

    assert_eq!(
        space_binding_to_pointer("space:owner_operations_H8sJ3kL9mN.tickets")?,
        "/owner_operations_H8sJ3kL9mN/tickets"
    );
    let record_path = record_path_for_unit_row(&space, OPS_OWNER, "tickets", 1)?;
    assert_eq!(record_path, "/owner_operations_H8sJ3kL9mN/tickets/1");
    let prepared = prepare_request(
        &space,
        PrepareRequestSpec::new(OPERATOR_OWNER, OPS_OWNER, "open_ticket")
            .with_source_unit_id("tickets")
            .with_record_path(record_path),
    )?;
    assert_eq!(prepared.status, PreparedStatus::Ready);
    assert_eq!(prepared.input.get("ticket_id"), Some(&json!("t2")));
    let owner_request = owner_request_from_prepared("req-1", &prepared)?;
    assert_eq!(owner_request.capability, "open_ticket");

    Ok(())
}

#[test]
fn form_submission_uses_normalized_arrays_as_direct_provided_input() -> Result<(), Box<dyn Error>> {
    let space = Space::try_from_value(json!({
        OPS_OWNER: {
            "capabilities": [{
                "id": "save_draft",
                "label": "Save draft",
                "risk": "local_mutation",
                "input_schema": {
                    "type": "object",
                    "required": ["draft_id", "to", "subject", "body_markdown"],
                    "properties": {
                        "draft_id": { "type": "string" },
                        "to": { "type": "array", "items": { "type": "string" } },
                        "cc": { "type": "array", "items": { "type": "string" } },
                        "first_recipient": { "type": "string" },
                        "subject": { "type": "string" },
                        "body_markdown": { "type": "string", "format": "markdown" }
                    }
                }
            }],
            "units": [{
                "id": "draft_form",
                "kind": "form",
                "fields": [
                    { "name": "to", "label": "To", "type": "array", "format": "email_list", "required": true },
                    { "name": "cc", "label": "Cc", "type": "array", "format": "email_list" },
                    { "name": "subject", "label": "Subject", "type": "string", "format": "text", "required": true },
                    { "name": "body_markdown", "label": "Message", "type": "string", "format": "markdown", "required": true }
                ],
                "input_bindings": {
                    "draft_id": "actor:draft.id",
                    "first_recipient": "input:to.0",
                    "subject": "actor:draft.stale_subject"
                },
                "submit_capability": "save_draft"
            }]
        },
        OPERATOR_OWNER: {
            "draft": {
                "id": "draft_1",
                "stale_subject": "Old subject"
            }
        }
    }))?;
    let provided = serde_json::Map::from_iter([
        (
            "to".to_string(),
            json!(["one@example.com", "two@example.com"]),
        ),
        ("cc".to_string(), json!(["copy@example.com"])),
        ("subject".to_string(), json!("Current subject")),
        ("body_markdown".to_string(), json!("Hello **world**")),
    ]);

    let prepared = prepare_request(
        &space,
        PrepareRequestSpec::new(OPERATOR_OWNER, OPS_OWNER, "save_draft")
            .with_source_unit_id("draft_form")
            .with_provided_input(provided),
    )?;

    assert_eq!(prepared.status, PreparedStatus::Ready);
    assert_eq!(prepared.input.get("draft_id"), Some(&json!("draft_1")));
    assert_eq!(
        prepared.input.get("to"),
        Some(&json!(["one@example.com", "two@example.com"]))
    );
    assert_eq!(prepared.input.get("cc"), Some(&json!(["copy@example.com"])));
    assert_eq!(
        prepared.input.get("first_recipient"),
        Some(&json!("one@example.com"))
    );
    assert_eq!(
        prepared.input.get("subject"),
        Some(&json!("Current subject"))
    );
    Ok(())
}

#[test]
fn actor_bindings_resolve_against_author_section() -> Result<(), Box<dyn Error>> {
    let space = Space::try_from_value(json!({
        OPS_OWNER: {
            "capabilities": [{
                "id": "open_ticket",
                "label": "Open",
                "risk": "read_only",
                "input_schema": {
                    "type": "object",
                    "required": ["ticket_id"],
                    "properties": { "ticket_id": { "type": "string" } }
                }
            }],
            "tickets": [{ "id": "t1" }],
            "units": [{
                "id": "tickets",
                "kind": "table",
                "source_binding": "space:owner_operations_H8sJ3kL9mN.tickets",
                "input_bindings": { "ticket_id": "actor:selection.ticket_id" }
            }]
        },
        OPERATOR_OWNER: {
            "selection": { "ticket_id": "t1" }
        }
    }))?;

    let actor_section = space
        .section(OPERATOR_OWNER)
        .ok_or("missing actor section")?;
    let ctx = BindingContext::new(space.as_value()).with_actor(Some(actor_section));
    assert_eq!(
        resolve_binding(&ctx, "actor:selection.ticket_id")?,
        Some(&json!("t1"))
    );

    let prepared = prepare_request(
        &space,
        PrepareRequestSpec::new(OPERATOR_OWNER, OPS_OWNER, "open_ticket")
            .with_source_unit_id("tickets"),
    )?;
    assert_eq!(prepared.status, PreparedStatus::Ready);
    assert_eq!(prepared.input.get("ticket_id"), Some(&json!("t1")));
    Ok(())
}

#[test]
fn request_input_ignores_legacy_field_binding_convention() -> Result<(), Box<dyn Error>> {
    let space = Space::try_from_value(json!({
        OPS_OWNER: {
            "capabilities": [{
                "id": "open_ticket",
                "label": "Open",
                "risk": "read_only",
                "input_schema": {
                    "type": "object",
                    "required": ["ticket_id"],
                    "properties": { "ticket_id": { "type": "string" } }
                }
            }],
            "tickets": [{ "id": "t1" }],
            "units": [{
                "id": "tickets",
                "kind": "table",
                "source_binding": "space:owner_operations_H8sJ3kL9mN.tickets",
                "ticket_id_binding": "record:id",
                "row_capabilities": ["open_ticket"]
            }]
        }
    }))?;

    let record_path = record_path_for_unit_row(&space, OPS_OWNER, "tickets", 0)?;
    let prepared = prepare_request(
        &space,
        PrepareRequestSpec::new(OPERATOR_OWNER, OPS_OWNER, "open_ticket")
            .with_source_unit_id("tickets")
            .with_record_path(record_path),
    )?;
    assert_eq!(prepared.status, PreparedStatus::NeedsInput);
    assert_eq!(prepared.missing, vec!["ticket_id".to_string()]);

    Ok(())
}

#[test]
fn owner_request_from_message_resolves_risk() -> Result<(), Box<dyn Error>> {
    let space = Space::try_from_value(json!({
        OPS_OWNER: {
            "capabilities": [{ "id": "deploy", "label": "Deploy", "risk": "destructive" }]
        }
    }))?;
    let message = AgentFirstUiMessage::request(
        OPERATOR_OWNER,
        OPS_OWNER,
        "deploy",
        Some(json!({ "target": "prod" })),
    );
    let request = owner_request_from_message(&space, "req-1", &message)?;
    assert_eq!(request.owner, OPS_OWNER);
    assert_eq!(request.risk, Risk::Destructive);
    assert_eq!(request.input.get("target"), Some(&json!("prod")));
    Ok(())
}

#[test]
fn request_owner_disambiguates_duplicate_capability_ids() -> Result<(), Box<dyn Error>> {
    let other_owner = "owner_peer_Z4xY8wQ2";
    let space = Space::try_from_value(json!({
        OPS_OWNER: {
            "capabilities": [{ "id": "open", "label": "Open Ops", "risk": "read_only" }]
        },
        other_owner: {
            "capabilities": [{ "id": "open", "label": "Open Peer", "risk": "destructive" }]
        }
    }))?;
    validate_space(&space)?;

    let message = AgentFirstUiMessage::request(OPERATOR_OWNER, other_owner, "open", None);
    let request = owner_request_from_message(&space, "req-1", &message)?;
    assert_eq!(request.owner, other_owner);
    assert_eq!(request.risk, Risk::Destructive);

    Ok(())
}

#[test]
fn owner_request_result_wraps_emits() {
    let request = agent_first_ui::OwnerRequest {
        request_id: "req-1".to_string(),
        author: OPERATOR_OWNER.to_string(),
        owner: OPS_OWNER.to_string(),
        capability: "refresh".to_string(),
        risk: Risk::ReadOnly,
        input: serde_json::Map::new(),
    };
    let emit = AgentFirstUiMessage::emit(
        OPS_OWNER,
        2,
        vec![Operation::Replace {
            path: "/owner_operations_H8sJ3kL9mN/status".to_string(),
            value: json!("fresh"),
        }],
    );
    let result = OwnerRequestResult::accepted(&request).with_emit(emit);
    assert_eq!(result.status, RequestExecutionStatus::Accepted);
    assert_eq!(result.emits.len(), 1);
}

#[test]
fn compose_spaces_keeps_owner_local_ids_and_rejects_owner_collisions() -> Result<(), Box<dyn Error>>
{
    let space = Space::try_from_value(json!({
        OPS_OWNER: {
            "capabilities": [{ "id": "open_ticket", "label": "Open", "risk": "read_only" }],
            "tickets": [{ "id": "t1" }],
            "units": [{
                "id": "tickets",
                "kind": "table",
                "source_binding": "space:owner_operations_H8sJ3kL9mN.tickets",
                "row_capabilities": ["open_ticket"]
            }],
            "layouts": [
                { "id": "root", "members": ["tickets"] }
            ],
            "root_layout": "root"
        }
    }))?;

    let other_owner = "owner_peer_Z4xY8wQ2";
    let other = Space::try_from_value(json!({
        other_owner: {
            "name": "Peer",
            "capabilities": [{ "id": "open_ticket", "label": "Open", "risk": "read_only" }],
            "units": [{ "id": "tickets", "kind": "table" }]
        }
    }))?;

    let composed = compose_spaces(vec![space.clone(), other])?;
    validate_space(&composed)?;
    let section = composed
        .section(OPS_OWNER)
        .and_then(serde_json::Value::as_object)
        .ok_or("missing operations section")?;
    assert_eq!(section["units"][0]["id"], json!("tickets"));
    assert_eq!(
        section["units"][0]["source_binding"],
        json!("space:owner_operations_H8sJ3kL9mN.tickets")
    );
    assert_eq!(
        section["units"][0]["row_capabilities"],
        json!(["open_ticket"])
    );
    assert_eq!(section["layouts"][0]["members"], json!(["tickets"]));
    assert_eq!(section["root_layout"], json!("root"));
    assert!(ensure_owner_available(&composed, OPS_OWNER).is_err());

    let duplicate = compose_spaces(vec![space.clone(), space]);
    assert!(matches!(
        duplicate,
        Err(AgentFirstUiError::DuplicateIdentifier { .. })
    ));
    Ok(())
}

#[test]
fn owner_local_ids_can_repeat_across_sections_but_not_within_one() -> Result<(), Box<dyn Error>> {
    let other_owner = "owner_peer_Z4xY8wQ2";
    let ok_space = Space::try_from_value(json!({
        OPS_OWNER: {
            "capabilities": [{ "id": "open", "label": "Open", "risk": "read_only" }],
            "units": [{ "id": "root", "kind": "table" }],
            "layouts": [{ "id": "layout", "members": ["root"] }],
            "root_layout": "layout"
        },
        other_owner: {
            "capabilities": [{ "id": "open", "label": "Open", "risk": "destructive" }],
            "units": [{ "id": "root", "kind": "text" }],
            "layouts": [{ "id": "layout", "members": ["root"] }],
            "root_layout": "layout"
        }
    }))?;
    validate_space(&ok_space)?;

    let bad_space = Space::try_from_value(json!({
        OPS_OWNER: {
            "capabilities": [{ "id": "dup", "label": "Dup", "risk": "read_only" }],
            "units": [{ "id": "dup", "kind": "table" }]
        }
    }))?;
    assert!(matches!(
        validate_space(&bad_space),
        Err(AgentFirstUiError::WellFormedness { .. })
    ));

    Ok(())
}

#[test]
fn validation_helpers_accept_expected_identifiers() {
    assert!(is_valid_id("abc_DEF-123"));
    assert!(!is_valid_id("abc.def"));
    assert!(!is_valid_id("abc:def"));
    assert!(!is_valid_id(""));
    assert!(!is_valid_id("-"));
    assert!(is_valid_owner_key(OPERATOR_OWNER));
    assert!(!is_valid_owner_key("owner.with.dot"));
    assert!(!is_valid_owner_key("owner:with:colon"));
    assert!(!is_valid_owner_key("session/operator"));
    assert!(!is_valid_owner_key("session//operator"));
}

#[test]
fn actor_state_helpers_stay_inside_actor_section() -> Result<(), Box<dyn Error>> {
    assert_eq!(
        actor_state_pointer(OPERATOR_OWNER, "selection.message_id")?,
        "/owner_operator_Vt3pN1aE7/selection/message_id"
    );
    let operation = set_actor_state_operation(OPERATOR_OWNER, "selection.message_id", json!("m1"))?;
    assert_eq!(
        operation.path(),
        "/owner_operator_Vt3pN1aE7/selection/message_id"
    );
    assert!(actor_state_pointer(OPERATOR_OWNER, "space:owner_afmail.message").is_err());
    assert!(actor_state_pointer("owner/invalid", "selection.message_id").is_err());
    Ok(())
}

#[test]
fn layout_helper_builds_region() {
    let region = layout("root", ["main", "supporting"]);
    assert_eq!(region.id, "root");
    assert_eq!(
        region.members,
        vec!["main".to_string(), "supporting".to_string()]
    );
}

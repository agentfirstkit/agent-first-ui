use std::error::Error;

use agent_first_ui::{
    action_ids_from_view, apply_afui_message, apply_patch, assemble_action_input,
    declared_action_ids, find_action, get_dot_path, inspect_snapshot, is_valid_id, parse_pointer,
    prepare_user_action, redacted_input_for_display, resolve_binding, validate_document, Action,
    AfuiError, AfuiMessage, Binding, BindingContext, BindingScope, Document, HostStore,
    PatchOperation, PrepareActionRequest, PrepareUserActionOptions, PreparedStatus, Risk, Screen,
    Snapshot, State, Surface, Theme, View,
};
use serde_json::{json, Value};

fn admin_snapshot() -> Result<Snapshot, Box<dyn Error>> {
    let message: AfuiMessage =
        serde_json::from_str(include_str!("../../spec/fixtures/admin.afui.json"))?;
    Ok(message.as_snapshot()?)
}

fn minimal_document() -> Document {
    Document::new(
        Surface::new("surface", "Surface"),
        vec![Screen::new("main", vec![View::new("content", "text")])],
    )
    .with_actions(vec![Action::new("refresh", "Refresh", Risk::ReadOnly)])
}

#[test]
fn parses_and_validates_spec_fixtures() -> Result<(), Box<dyn Error>> {
    let fixtures = [
        include_str!("../../spec/fixtures/admin.afui.json"),
        include_str!("../../spec/fixtures/tendril.afui.json"),
        include_str!("../../spec/fixtures/hud.afui.json"),
        include_str!("../../spec/fixtures/image_editor.afui.json"),
        include_str!("../../examples/operator_console.afui.json"),
        include_str!("../../examples/tui_ops.afui.json"),
        include_str!("../../examples/go_game.afui.json"),
    ];

    for fixture in fixtures {
        let message: AfuiMessage = serde_json::from_str(fixture)?;
        let snapshot = message.as_snapshot()?;
        validate_document(&snapshot.document)?;
    }

    Ok(())
}

#[test]
fn prepares_user_action_from_record_binding() -> Result<(), Box<dyn Error>> {
    let snapshot = admin_snapshot()?;
    let orders_table = snapshot
        .document
        .find_view("orders_table")
        .ok_or("orders_table missing")?;
    let record = snapshot
        .state
        .as_value()
        .pointer("/orders/0")
        .ok_or("orders[0] missing")?;

    let mut provided = serde_json::Map::new();
    provided.insert("reason".to_string(), json!("customer request"));
    let options = PrepareUserActionOptions::default().with_provided_input(provided);
    let prepared = prepare_user_action(
        &snapshot,
        Some(orders_table),
        "refund_order",
        Some(record),
        &options,
    )?;

    assert_eq!(prepared.status, PreparedStatus::Ready);
    assert_eq!(prepared.risk, Risk::Destructive);
    assert_eq!(prepared.input.get("order_id"), Some(&json!("ord_123")));
    assert_eq!(
        prepared.input.get("reason"),
        Some(&json!("customer request"))
    );
    assert!(prepared.event.is_some());

    Ok(())
}

#[test]
fn reports_missing_required_action_input() -> Result<(), Box<dyn Error>> {
    let snapshot = admin_snapshot()?;
    let orders_table = snapshot
        .document
        .find_view("orders_table")
        .ok_or("orders_table missing")?;
    let record = snapshot
        .state
        .as_value()
        .pointer("/orders/0")
        .ok_or("orders[0] missing")?;

    let prepared = prepare_user_action(
        &snapshot,
        Some(orders_table),
        "refund_order",
        Some(record),
        &PrepareUserActionOptions::default(),
    )?;

    assert_eq!(prepared.status, PreparedStatus::NeedsInput);
    assert_eq!(prepared.input.get("order_id"), Some(&json!("ord_123")));
    assert_eq!(prepared.missing, vec!["reason".to_string()]);
    assert!(prepared.event.is_none());

    Ok(())
}

#[test]
fn resolves_bindings_for_all_scopes() -> Result<(), Box<dyn Error>> {
    let state = json!({
        "summary": { "pending_count": 12 },
        "orders": [{ "id": "ord_123" }]
    });
    let record = json!({ "id": "ord_999", "nested": { "status": "paid" } });
    let input = json!("hello");
    let ctx = BindingContext::new(&state)
        .with_record(Some(&record))
        .with_input(Some(&input));

    let parsed = Binding::parse("summary.pending_count")?;
    assert_eq!(parsed.scope, BindingScope::State);
    assert_eq!(parsed.path, "summary.pending_count");
    assert_eq!(
        resolve_binding(&ctx, "state:summary.pending_count")?,
        Some(&json!(12))
    );
    assert_eq!(
        resolve_binding(&ctx, "record:nested.status")?,
        Some(&json!("paid"))
    );
    assert_eq!(resolve_binding(&ctx, "input:.")?, Some(&json!("hello")));
    assert_eq!(resolve_binding(&ctx, "input:value")?, Some(&json!("hello")));
    assert_eq!(resolve_binding(&ctx, "state:missing")?, None);
    assert_eq!(get_dot_path(&state, "orders.0.id"), Some(&json!("ord_123")));

    assert!(Binding::parse("bad:field").is_err());
    assert!(Binding::parse("state:").is_err());
    assert!(Binding::parse("state:a:b").is_err());

    Ok(())
}

#[test]
fn prepares_input_bound_actions_and_unresolved_actions() -> Result<(), Box<dyn Error>> {
    let message: AfuiMessage =
        serde_json::from_str(include_str!("../../spec/fixtures/tendril.afui.json"))?;
    let snapshot = message.as_snapshot()?;
    let address_bar = snapshot
        .document
        .find_view("address_bar")
        .ok_or("address_bar missing")?;
    let options = PrepareUserActionOptions::default().with_input_value(json!("spore://demo/hello"));

    let prepared = prepare_user_action(
        &snapshot,
        Some(address_bar),
        "submit_address",
        None,
        &options,
    )?;
    assert_eq!(prepared.status, PreparedStatus::Ready);
    assert_eq!(
        prepared.input.get("text"),
        Some(&json!("spore://demo/hello"))
    );
    assert_eq!(
        prepared.event.as_ref().map(|event| event.afui.as_str()),
        Some("0.1")
    );

    let missing = prepare_user_action(
        &snapshot,
        Some(address_bar),
        "not_declared",
        None,
        &PrepareUserActionOptions::default(),
    )?;
    assert_eq!(missing.status, PreparedStatus::Error);
    assert_eq!(missing.risk, Risk::Destructive);
    assert_eq!(
        missing.error.as_deref(),
        Some("Unresolved action reference")
    );

    Ok(())
}

#[test]
fn action_helpers_collect_declared_refs_and_redact_input() -> Result<(), Box<dyn Error>> {
    let snapshot = admin_snapshot()?;
    let table = snapshot
        .document
        .find_view("orders_table")
        .ok_or("orders_table missing")?;
    let ids = action_ids_from_view(table);
    assert_eq!(ids, vec!["open_order", "approve_order", "refund_order"]);
    assert!(declared_action_ids(&snapshot.document).contains(&"refund_order".to_string()));
    assert_eq!(
        find_action(&snapshot.document, "approve_order").map(|action| action.risk.clone()),
        Some(Risk::ExternalEffect)
    );

    let redacted = redacted_input_for_display(&serde_json::Map::from_iter([
        ("api_key_secret".to_string(), json!("sk_live")),
        ("order_id".to_string(), json!("ord_123")),
    ]));
    assert_eq!(redacted.get("api_key_secret"), Some(&json!("***")));
    assert_eq!(redacted.get("order_id"), Some(&json!("ord_123")));

    Ok(())
}

#[test]
fn assemble_action_input_rejects_non_string_binding() -> Result<(), Box<dyn Error>> {
    let snapshot = Snapshot::new(
        Document::new(
            Surface::new("surface", "Surface"),
            vec![Screen::new(
                "main",
                vec![View::new("bad_input", "input").set_field("name_bind", json!(123))],
            )],
        )
        .with_actions(vec![Action {
            input_schema: Some(json!({
                "type": "object",
                "properties": { "name": { "type": "string" } }
            })),
            ..Action::new("submit", "Submit", Risk::ReadOnly)
        }]),
        State::try_from_value(json!({}))?,
    );
    let view = snapshot
        .document
        .find_view("bad_input")
        .ok_or("bad_input missing")?;
    let action = snapshot
        .document
        .find_action("submit")
        .ok_or("submit missing")?;

    let error = assemble_action_input(
        &snapshot,
        Some(view),
        action,
        None,
        &PrepareUserActionOptions::default(),
    )
    .err()
    .ok_or("expected invalid binding")?;
    assert!(matches!(error, AfuiError::InvalidBinding { .. }));

    Ok(())
}

#[test]
fn applies_rfc6902_patch_atomically() -> Result<(), Box<dyn Error>> {
    let state = json!({
        "orders": [{ "id": "ord_123", "status": "pending_review" }],
        "recent_events": []
    });
    let patch = vec![
        PatchOperation::Test {
            path: "/orders/0/status".to_string(),
            value: json!("pending_review"),
        },
        PatchOperation::Replace {
            path: "/orders/0/status".to_string(),
            value: json!("refunded"),
        },
        PatchOperation::Add {
            path: "/recent_events/-".to_string(),
            value: json!({ "message": "refunded" }),
        },
    ];

    let next = apply_patch(&state, &patch)?;
    assert_eq!(next.pointer("/orders/0/status"), Some(&json!("refunded")));
    assert_eq!(
        next.pointer("/recent_events/0/message"),
        Some(&json!("refunded"))
    );

    let failing = vec![PatchOperation::Test {
        path: "/orders/0/status".to_string(),
        value: json!("paid"),
    }];
    assert!(apply_patch(&state, &failing).is_err());
    assert_eq!(
        state.pointer("/orders/0/status"),
        Some(&json!("pending_review"))
    );

    Ok(())
}

#[test]
fn applies_all_json_patch_operations_and_root_replacement() -> Result<(), Box<dyn Error>> {
    let value = json!({
        "obj": { "a/b": 1, "tilde~key": 2 },
        "items": ["a", "c"],
        "copy_source": { "nested": true }
    });
    let patch = vec![
        PatchOperation::Add {
            path: "/items/1".to_string(),
            value: json!("b"),
        },
        PatchOperation::Copy {
            from: "/copy_source".to_string(),
            path: "/copied".to_string(),
        },
        PatchOperation::Move {
            from: "/copied".to_string(),
            path: "/moved".to_string(),
        },
        PatchOperation::Replace {
            path: "/obj/a~1b".to_string(),
            value: json!(10),
        },
        PatchOperation::Remove {
            path: "/obj/tilde~0key".to_string(),
        },
    ];

    let patched = apply_patch(&value, &patch)?;
    assert_eq!(patched.pointer("/items/0"), Some(&json!("a")));
    assert_eq!(patched.pointer("/items/1"), Some(&json!("b")));
    assert_eq!(patched.pointer("/items/2"), Some(&json!("c")));
    assert_eq!(patched.pointer("/moved/nested"), Some(&json!(true)));
    assert_eq!(patched.pointer("/obj/a~1b"), Some(&json!(10)));
    assert!(patched.pointer("/obj/tilde~0key").is_none());

    let root_replaced = apply_patch(
        &value,
        &[PatchOperation::Replace {
            path: String::new(),
            value: json!({"root": "new"}),
        }],
    )?;
    assert_eq!(root_replaced, json!({"root": "new"}));

    Ok(())
}

#[test]
fn rejects_invalid_json_patch_paths_and_moves() {
    let value = json!({ "items": ["a"], "obj": { "child": {} } });
    let bad_pointer = apply_patch(
        &value,
        &[PatchOperation::Add {
            path: "items/0".to_string(),
            value: json!("x"),
        }],
    );
    assert!(bad_pointer.is_err());

    let bad_index = apply_patch(
        &value,
        &[PatchOperation::Add {
            path: "/items/01".to_string(),
            value: json!("x"),
        }],
    );
    assert!(bad_index.is_err());

    let move_into_child = apply_patch(
        &value,
        &[PatchOperation::Move {
            from: "/obj".to_string(),
            path: "/obj/child/grandchild".to_string(),
        }],
    );
    assert!(move_into_child.is_err());

    let same_path_move = apply_patch(
        &value,
        &[PatchOperation::Move {
            from: "/obj".to_string(),
            path: "/obj".to_string(),
        }],
    );
    assert_eq!(same_path_move, Ok(value));
}

#[test]
fn parses_json_pointer_empty_tokens_and_escapes() -> Result<(), Box<dyn Error>> {
    assert_eq!(parse_pointer("/")?, vec!["".to_string()]);
    assert_eq!(
        parse_pointer("//nested")?,
        vec!["".to_string(), "nested".to_string()]
    );
    assert_eq!(
        parse_pointer("/a~1b/c~0d")?,
        vec!["a/b".to_string(), "c~d".to_string()]
    );

    Ok(())
}

#[test]
fn host_store_prepares_actions_and_reports_snapshot() -> Result<(), Box<dyn Error>> {
    let snapshot = admin_snapshot()?;
    let mut store = HostStore::from_snapshot(snapshot);
    let request = PrepareActionRequest::new("open_order")
        .with_source_view_id("orders_table")
        .with_record_path("/orders/0");
    let prepared = store.prepare_action(&request)?;

    assert_eq!(prepared.status, PreparedStatus::Ready);
    assert_eq!(prepared.input.get("order_id"), Some(&json!("ord_123")));
    assert!(prepared.event.is_some());

    let event = prepared.event.ok_or("missing prepared event")?;
    store.emit_user_action(event)?;
    assert_eq!(store.audit().len(), 1);

    let report = inspect_snapshot(store.snapshot().ok_or("missing snapshot")?);
    assert_eq!(report.surface_id, "order_admin");
    assert!(report.validation.valid);
    assert!(report.view_kinds.get("table").copied().unwrap_or_default() >= 1);

    Ok(())
}

#[test]
fn applies_afui_state_patch_message() -> Result<(), Box<dyn Error>> {
    let snapshot = admin_snapshot()?;
    let patch: Vec<PatchOperation> =
        serde_json::from_str(include_str!("../../spec/fixtures/admin.afui.patch.json"))?;
    let patch_message = AfuiMessage::state_patch(patch);

    let next = apply_afui_message(Some(&snapshot), &patch_message)?;
    assert_eq!(
        next.state.as_value().pointer("/orders/0/status"),
        Some(&json!("refunded"))
    );
    assert_eq!(
        next.state.as_value().pointer("/last_selected_order_id"),
        Some(&json!("ord_123"))
    );
    assert!(next
        .state
        .as_value()
        .pointer("/summary/failed_jobs_count")
        .is_none());

    Ok(())
}

#[test]
fn applies_snapshot_document_patch_and_theme_patch_messages() -> Result<(), Box<dyn Error>> {
    let snapshot_message =
        AfuiMessage::ui_snapshot(minimal_document(), State::try_from_value(json!({}))?);
    let snapshot = apply_afui_message(None, &snapshot_message)?;
    assert_eq!(snapshot.document.surface.id, "surface");

    let document_patch = AfuiMessage::document_patch(vec![
        PatchOperation::Test {
            path: "/screens/0/id".to_string(),
            value: json!("main"),
        },
        PatchOperation::Add {
            path: "/screens/0/views/-".to_string(),
            value: json!({ "id": "status", "kind": "text", "text": "Ready" }),
        },
    ]);
    let snapshot = apply_afui_message(Some(&snapshot), &document_patch)?;
    assert!(snapshot.document.find_view("status").is_some());

    let theme_patch = AfuiMessage::theme_patch(vec![PatchOperation::Add {
        path: "/color_primary".to_string(),
        value: json!("#00a36c"),
    }]);
    let snapshot = apply_afui_message(Some(&snapshot), &theme_patch)?;
    assert_eq!(
        snapshot
            .theme
            .as_ref()
            .and_then(|theme| theme.as_value().pointer("/color_primary")),
        Some(&json!("#00a36c"))
    );

    Ok(())
}

#[test]
fn rejects_invalid_runtime_messages() -> Result<(), Box<dyn Error>> {
    let state_patch = AfuiMessage::state_patch(Vec::new());
    assert!(matches!(
        apply_afui_message(None, &state_patch),
        Err(AfuiError::MissingSnapshot)
    ));

    let user_action = AfuiMessage::user_action("refresh", None);
    assert!(matches!(
        apply_afui_message(
            Some(&Snapshot::new(
                minimal_document(),
                State::try_from_value(json!({}))?
            )),
            &user_action
        ),
        Err(AfuiError::UnsupportedMessageType { .. })
    ));

    let mut unsupported =
        AfuiMessage::ui_snapshot(minimal_document(), State::try_from_value(json!({}))?);
    unsupported.afui = "9.9".to_string();
    assert!(matches!(
        unsupported.as_snapshot(),
        Err(AfuiError::UnsupportedVersion { .. })
    ));

    let invalid_theme = AfuiMessage::theme_patch(vec![PatchOperation::Add {
        path: "/nested".to_string(),
        value: json!({ "bad": true }),
    }]);
    let snapshot = Snapshot::new(minimal_document(), State::try_from_value(json!({}))?);
    assert!(matches!(
        apply_afui_message(Some(&snapshot), &invalid_theme),
        Err(AfuiError::InvalidTheme { .. })
    ));

    Ok(())
}

#[test]
fn rejects_document_patch_that_breaks_well_formedness() -> Result<(), Box<dyn Error>> {
    let snapshot = Snapshot::new(minimal_document(), State::try_from_value(json!({}))?);
    let orphan_patch = AfuiMessage::document_patch(vec![
        PatchOperation::Test {
            path: "/screens/0/views/0/id".to_string(),
            value: json!("content"),
        },
        PatchOperation::Add {
            path: "/screens/0/views/0/actions".to_string(),
            value: json!(["missing_action"]),
        },
    ]);

    assert!(matches!(
        apply_afui_message(Some(&snapshot), &orphan_patch),
        Err(AfuiError::WellFormedness { .. })
    ));

    Ok(())
}

#[test]
fn parses_user_action_wire_messages() -> Result<(), Box<dyn Error>> {
    let line = r#"{"afui":"0.1","type":"user_action","action_id":"submit_address","input":{"text":"spore://demo/hello"}}"#;
    let message: AfuiMessage = serde_json::from_str(line)?;
    let encoded = serde_json::to_value(&message)?;

    assert_eq!(encoded.get("type"), Some(&json!("user_action")));
    assert_eq!(encoded.get("action_id"), Some(&json!("submit_address")));
    assert_eq!(
        encoded.pointer("/input/text"),
        Some(&json!("spore://demo/hello"))
    );

    Ok(())
}

#[test]
fn validates_state_theme_ids_and_document_integrity() -> Result<(), Box<dyn Error>> {
    assert!(State::try_from_value(Value::Null).is_err());
    assert!(Theme::try_from_value(json!({ "color_primary": "#fff", "radius_px": 4 })).is_ok());
    assert!(Theme::try_from_value(json!({ "nested": {} })).is_err());
    assert!(is_valid_id("surface.screen-1:view"));
    assert!(!is_valid_id(""));
    assert!(!is_valid_id("-"));
    assert!(!is_valid_id("has space"));

    let valid = minimal_document();
    validate_document(&valid)?;

    let mut legacy_app = minimal_document();
    legacy_app.extra.insert("app".to_string(), json!({}));
    assert!(matches!(
        validate_document(&legacy_app),
        Err(AfuiError::WellFormedness { .. })
    ));

    let duplicate_id = Document::new(
        Surface::new("surface", "Surface"),
        vec![Screen::new("main", vec![View::new("main", "text")])],
    );
    assert!(matches!(
        validate_document(&duplicate_id),
        Err(AfuiError::WellFormedness { .. })
    ));

    let invalid = Document::new(
        Surface::new("bad id", "Surface"),
        vec![Screen::new(
            "main",
            vec![View::new("content", "").set_field("submit_action", json!("missing"))],
        )],
    )
    .with_actions(vec![Action {
        input_schema: Some(json!("not object")),
        extra: serde_json::Map::from_iter([("requires_confirmation".to_string(), json!(true))]),
        ..Action::new("refresh", "Refresh", Risk::ReadOnly)
    }]);
    assert!(matches!(
        validate_document(&invalid),
        Err(AfuiError::WellFormedness { .. })
    ));

    let marked_fields = Document::new(
        Surface::new("surface", "Surface"),
        vec![Screen::new(
            "main",
            vec![View::new("content", "text")
                .set_field("source_bind", json!("bad:scope"))
                .set_field("image_uri", json!(123))
                .set_field("component_uri", json!("https://example.test/widget.js"))
                .set_field("actions", json!("not array"))],
        )],
    );
    assert!(matches!(
        validate_document(&marked_fields),
        Err(AfuiError::WellFormedness { .. })
    ));

    Ok(())
}

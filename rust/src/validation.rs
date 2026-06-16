use std::collections::HashSet;

use serde_json::Value;

use crate::binding::{get_dot_path, Binding, BindingScope};
use crate::error::AfuiError;
use crate::types::{Action, Document, JsonMap, Screen, View};

pub fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id != "-"
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | ':' | '-'))
}

pub fn validate_document(document: &Document) -> Result<(), AfuiError> {
    let mut issues = Vec::new();
    let mut ids = HashSet::new();
    let action_ids = document
        .actions
        .iter()
        .map(|action| action.id.clone())
        .collect::<HashSet<_>>();

    validate_id("surface.id", &document.surface.id, &mut ids, &mut issues);
    validate_marked_fields(
        "surface",
        &document.surface.extra,
        false,
        &action_ids,
        &mut issues,
    );
    if document.extra.contains_key("app") {
        issues.push("document.app is not valid AFUI; use document.surface".to_string());
    }
    validate_marked_fields("document", &document.extra, false, &action_ids, &mut issues);

    for (screen_index, screen) in document.screens.iter().enumerate() {
        validate_screen(
            screen,
            &format!("screens[{screen_index}]"),
            &mut ids,
            &action_ids,
            &mut issues,
        );
    }

    for (action_index, action) in document.actions.iter().enumerate() {
        validate_action(
            action,
            &format!("actions[{action_index}]"),
            &mut ids,
            &action_ids,
            &mut issues,
        );
    }

    if issues.is_empty() {
        Ok(())
    } else {
        Err(AfuiError::WellFormedness { issues })
    }
}

/// Optional cross-check that every `state:`-scoped binding in `document`
/// resolves against `state`. This is the light, no-schema enforcement of the
/// data/render-guidance split: it catches a view that references data the
/// snapshot does not ship (a typo, a renamed field, presentation that was
/// expected as data). `record:` and `input:` bindings resolve at render time
/// and are intentionally not checked here. State stays opaque, producer-owned
/// data — this only asserts the document and its own state are consistent.
pub fn validate_state_bindings(document: &Document, state: &Value) -> Result<(), AfuiError> {
    let document_value = serde_json::to_value(document)?;
    let mut issues = Vec::new();
    collect_unresolved_state_bindings(&document_value, state, &mut issues);
    if issues.is_empty() {
        Ok(())
    } else {
        Err(AfuiError::WellFormedness { issues })
    }
}

fn collect_unresolved_state_bindings(node: &Value, state: &Value, issues: &mut Vec<String>) {
    match node {
        Value::Object(map) => {
            for (key, value) in map {
                if key.ends_with("_bind") {
                    if let Some(binding_str) = value.as_str() {
                        if let Ok(binding) = Binding::parse(binding_str) {
                            if binding.scope == BindingScope::State
                                && get_dot_path(state, &binding.path).is_none()
                            {
                                issues.push(format!(
                                    "{key} `{binding_str}` does not resolve against state"
                                ));
                            }
                        }
                    }
                }
                collect_unresolved_state_bindings(value, state, issues);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_unresolved_state_bindings(item, state, issues);
            }
        }
        _ => {}
    }
}

fn validate_screen(
    screen: &Screen,
    path: &str,
    ids: &mut HashSet<String>,
    action_ids: &HashSet<String>,
    issues: &mut Vec<String>,
) {
    validate_id(&format!("{path}.id"), &screen.id, ids, issues);
    validate_marked_fields(path, &screen.extra, false, action_ids, issues);
    for (view_index, view) in screen.views.iter().enumerate() {
        validate_view(
            view,
            &format!("{path}.views[{view_index}]"),
            ids,
            action_ids,
            issues,
        );
    }
}

fn validate_view(
    view: &View,
    path: &str,
    ids: &mut HashSet<String>,
    action_ids: &HashSet<String>,
    issues: &mut Vec<String>,
) {
    validate_id(&format!("{path}.id"), &view.id, ids, issues);
    if view.kind.is_empty() {
        issues.push(format!("{path}.kind must not be empty"));
    }
    validate_marked_fields(path, &view.extra, true, action_ids, issues);
    if view.kind == "terminal" {
        validate_terminal_view(path, &view.extra, issues);
    }

    if let Some(child) = view.header.as_deref() {
        validate_view(child, &format!("{path}.header"), ids, action_ids, issues);
    }
    if let Some(child) = view.footer.as_deref() {
        validate_view(child, &format!("{path}.footer"), ids, action_ids, issues);
    }
    if let Some(child) = view.empty_view.as_deref() {
        validate_view(
            child,
            &format!("{path}.empty_view"),
            ids,
            action_ids,
            issues,
        );
    }
    if let Some(child) = view.default_view.as_deref() {
        validate_view(child, &format!("{path}.default"), ids, action_ids, issues);
    }
    for (case_key, child) in &view.cases {
        validate_view(
            child,
            &format!("{path}.cases.{case_key}"),
            ids,
            action_ids,
            issues,
        );
    }
}

fn validate_action(
    action: &Action,
    path: &str,
    ids: &mut HashSet<String>,
    action_ids: &HashSet<String>,
    issues: &mut Vec<String>,
) {
    validate_id(&format!("{path}.id"), &action.id, ids, issues);
    if action.label.is_empty() {
        issues.push(format!("{path}.label must not be empty"));
    }
    if action.extra.contains_key("requires_confirmation") {
        issues.push(format!(
            "{path}.requires_confirmation is not valid AFUI; gating is runtime behavior derived from risk"
        ));
    }
    if let Some(schema) = &action.input_schema {
        if !schema.is_object() {
            issues.push(format!("{path}.input_schema must be a JSON object"));
        }
    }
    // An action's own extra fields must not smuggle code/package/script markers.
    // Action refs are not resolved here: an action does not reference actions.
    validate_marked_fields(path, &action.extra, false, action_ids, issues);
}

fn validate_id(path: &str, id: &str, ids: &mut HashSet<String>, issues: &mut Vec<String>) {
    if !is_valid_id(id) {
        issues.push(format!("{path} has invalid id `{id}`"));
    }
    if !ids.insert(id.to_string()) {
        issues.push(format!("{path} duplicates id `{id}`"));
    }
}

fn validate_marked_fields(
    path: &str,
    fields: &JsonMap,
    validate_actions: bool,
    action_ids: &HashSet<String>,
    issues: &mut Vec<String>,
) {
    for (key, value) in fields {
        let field_path = format!("{path}.{key}");
        validate_no_code_transport_field(&field_path, key, issues);
        if key.ends_with("_bind") {
            validate_binding(&field_path, value, issues);
        }
        if key.ends_with("_uri") && !value.is_string() {
            issues.push(format!("{field_path} must be a string URI"));
        }
        if validate_actions && (key == "action" || key.ends_with("_action")) {
            validate_single_action_ref(&field_path, value, action_ids, issues);
        }
        if validate_actions && (key == "actions" || key.ends_with("_actions")) {
            validate_action_ref_array(&field_path, value, action_ids, issues);
        }
    }
}

fn validate_no_code_transport_field(path: &str, key: &str, issues: &mut Vec<String>) {
    let forbidden = matches!(
        key,
        "html"
            | "css"
            | "javascript"
            | "script"
            | "script_uri"
            | "component_uri"
            | "plugin_uri"
            | "wasm_uri"
            | "package_json"
            | "build_script"
            | "shell_script"
            | "shell_command"
            | "npm_package"
            | "dependency_graph"
    ) || key.ends_with("_script")
        || key.ends_with("_script_uri")
        || key.ends_with("_component_uri")
        || key.ends_with("_plugin_uri")
        || key.ends_with("_wasm_uri");

    if forbidden {
        issues.push(format!(
            "{path} is not valid AFUI; ordinary AFUI documents cannot carry code, package, build, script, or remote component fields"
        ));
    }
}

/// A `terminal` view only references a host-owned session and presentation; it
/// must never embed a command to run. The generic code-transport blacklist
/// (`validate_no_code_transport_field`) already rejects `shell_command` etc.;
/// this adds the bare command-ish keys that are specifically meaningful on a
/// terminal. All other (open) view fields stay allowed — this is a blacklist,
/// not a whitelist, so `description`/`header`/appearance hints are unaffected.
fn validate_terminal_view(path: &str, fields: &JsonMap, issues: &mut Vec<String>) {
    for key in fields.keys() {
        let forbidden = matches!(
            key.as_str(),
            "command" | "cmd" | "exec" | "run" | "program" | "entrypoint" | "args" | "argv"
        );
        if forbidden {
            issues.push(format!(
                "{path}.{key} is not valid AFUI; a terminal view references a host session and must not embed a command to run"
            ));
        }
    }
}

fn validate_binding(path: &str, value: &Value, issues: &mut Vec<String>) {
    let Some(binding) = value.as_str() else {
        issues.push(format!("{path} must be a string binding"));
        return;
    };
    if let Err(error) = Binding::parse(binding) {
        issues.push(format!("{path} {error}"));
    }
}

fn validate_single_action_ref(
    path: &str,
    value: &Value,
    action_ids: &HashSet<String>,
    issues: &mut Vec<String>,
) {
    let Some(action_id) = value.as_str() else {
        issues.push(format!("{path} must be an action id string"));
        return;
    };
    if !action_ids.contains(action_id) {
        issues.push(format!("{path} references undeclared action `{action_id}`"));
    }
}

fn validate_action_ref_array(
    path: &str,
    value: &Value,
    action_ids: &HashSet<String>,
    issues: &mut Vec<String>,
) {
    let Some(items) = value.as_array() else {
        issues.push(format!("{path} must be an array of action id strings"));
        return;
    };
    for (index, item) in items.iter().enumerate() {
        let item_path = format!("{path}[{index}]");
        validate_single_action_ref(&item_path, item, action_ids, issues);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document_from(value: Value) -> Result<Document, serde_json::Error> {
        serde_json::from_value(value)
    }

    fn issues_of(document: &Document) -> Vec<String> {
        match validate_document(document) {
            Ok(()) => Vec::new(),
            Err(AfuiError::WellFormedness { issues }) => issues,
            Err(other) => vec![other.to_string()],
        }
    }

    fn binding_issues_of(document: &Document, state: &Value) -> Vec<String> {
        match validate_state_bindings(document, state) {
            Ok(()) => Vec::new(),
            Err(AfuiError::WellFormedness { issues }) => issues,
            Err(other) => vec![other.to_string()],
        }
    }

    #[test]
    fn flags_unresolved_state_binding() -> Result<(), serde_json::Error> {
        let document = document_from(serde_json::json!({
            "surface": { "id": "s", "title": "S" },
            "screens": [
                { "id": "scr", "views": [
                    { "id": "v", "kind": "table", "source_bind": "state:orders" }
                ] }
            ]
        }))?;
        let issues = binding_issues_of(&document, &serde_json::json!({ "customers": [] }));
        assert!(
            issues.iter().any(|issue| issue.contains("state:orders")),
            "expected unresolved state:orders, got {issues:?}"
        );
        Ok(())
    }

    #[test]
    fn accepts_resolved_state_and_skips_record_and_input() -> Result<(), serde_json::Error> {
        // Nested bindings (here under a stats `items` array) and resolved
        // top-level bindings pass; record:/input: are not checked statically.
        let document = document_from(serde_json::json!({
            "surface": { "id": "s", "title": "S" },
            "screens": [
                { "id": "scr", "views": [
                    {
                        "id": "v", "kind": "table",
                        "source_bind": "state:orders",
                        "order_id_bind": "record:id",
                        "query_bind": "input:.",
                        "items": [ { "value_bind": "state:summary.count" } ]
                    }
                ] }
            ]
        }))?;
        let state = serde_json::json!({ "orders": [], "summary": { "count": 0 } });
        assert!(binding_issues_of(&document, &state).is_empty());
        Ok(())
    }

    #[test]
    fn rejects_code_field_on_surface() -> Result<(), serde_json::Error> {
        let document = document_from(serde_json::json!({
            "surface": { "id": "s", "title": "S", "component_uri": "https://x/y.js" },
            "screens": []
        }))?;
        let issues = issues_of(&document);
        assert!(
            issues
                .iter()
                .any(|issue| issue.contains("surface.component_uri")),
            "expected surface.component_uri to be rejected, got {issues:?}"
        );
        Ok(())
    }

    #[test]
    fn rejects_code_field_on_action() -> Result<(), serde_json::Error> {
        let document = document_from(serde_json::json!({
            "surface": { "id": "s", "title": "S" },
            "screens": [],
            "actions": [
                { "id": "a", "label": "A", "risk": "read_only", "html": "<b>x</b>" }
            ]
        }))?;
        let issues = issues_of(&document);
        assert!(
            issues.iter().any(|issue| issue.contains("actions[0].html")),
            "expected actions[0].html to be rejected, got {issues:?}"
        );
        Ok(())
    }

    #[test]
    fn rejects_command_field_on_terminal_view() -> Result<(), serde_json::Error> {
        let document = document_from(serde_json::json!({
            "surface": { "id": "s", "title": "S" },
            "screens": [
                {
                    "id": "scr",
                    "views": [
                        { "id": "term1", "kind": "terminal", "command": "rm -rf /" }
                    ]
                }
            ]
        }))?;
        let issues = issues_of(&document);
        assert!(
            issues
                .iter()
                .any(|issue| issue.contains("views[0].command")),
            "expected terminal command field to be rejected, got {issues:?}"
        );
        Ok(())
    }

    #[test]
    fn accepts_terminal_view_with_generic_fields() -> Result<(), serde_json::Error> {
        let document = document_from(serde_json::json!({
            "surface": { "id": "s", "title": "S" },
            "screens": [
                {
                    "id": "scr",
                    "views": [
                        {
                            "id": "term1",
                            "kind": "terminal",
                            "description": "main shell",
                            "session_bind": "state:terminals.term1.session_id",
                            "title_bind": "state:terminals.term1.title",
                            "status_bind": "state:terminals.term1.status"
                        }
                    ]
                }
            ]
        }))?;
        assert!(
            issues_of(&document).is_empty(),
            "expected terminal view with generic + presentation fields to validate, got {:?}",
            issues_of(&document)
        );
        Ok(())
    }

    #[test]
    fn accepts_plain_surface_and_action() -> Result<(), serde_json::Error> {
        let document = document_from(serde_json::json!({
            "surface": { "id": "s", "title": "S" },
            "screens": [],
            "actions": [ { "id": "a", "label": "A", "risk": "read_only" } ]
        }))?;
        assert!(issues_of(&document).is_empty());
        Ok(())
    }
}

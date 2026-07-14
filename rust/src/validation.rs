use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::binding::{Binding, BindingScope, get_dot_path};
use crate::error::AgentFirstUiError;
use crate::operation::is_valid_actor_state_path;
use crate::types::{Capability, Layout, Space, Unit};
use crate::unit_profile::FIELD_INPUT_BINDINGS;
use crate::unit_profile::FIELD_INTERACTIONS;

pub fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id != "-"
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
}

pub fn is_valid_owner_key(owner: &str) -> bool {
    !owner.is_empty()
        && owner != "-"
        && owner
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
}

/// Validate a space for well-formedness. Collects all issues before returning.
pub fn validate_space(space: &Space) -> Result<(), AgentFirstUiError> {
    let mut issues = Vec::new();

    let Some(space_obj) = space.as_value().as_object() else {
        issues.push("space must be a JSON object".to_string());
        return Err(AgentFirstUiError::WellFormedness { issues });
    };

    for (owner_key, section_val) in space_obj {
        if !is_valid_owner_key(owner_key) {
            issues.push(format!(
                "owner key `{owner_key}` is not a separator-safe owner id"
            ));
        }
        let Some(section) = section_val.as_object() else {
            issues.push(format!("section `{owner_key}` must be a JSON object"));
            continue;
        };

        let mut section_ids: HashSet<String> = HashSet::new();
        let section_path = format!("space.{owner_key}");
        let section_capability_ids = collect_section_capability_ids(section);

        if let Some(caps_val) = section.get("capabilities") {
            match serde_json::from_value::<Vec<Capability>>(caps_val.clone()) {
                Ok(caps) => {
                    for (i, cap) in caps.iter().enumerate() {
                        validate_capability(
                            cap,
                            &format!("{section_path}.capabilities[{i}]"),
                            &mut section_ids,
                            &mut issues,
                        );
                    }
                }
                Err(e) => {
                    issues.push(format!(
                        "{section_path}.capabilities failed to deserialize: {e}"
                    ));
                }
            }
        }

        let section_unit_ids = collect_section_unit_ids(section);
        let section_layout_ids = collect_section_layout_ids(section);
        if let Some(units_val) = section.get("units") {
            match serde_json::from_value::<Vec<Unit>>(units_val.clone()) {
                Ok(units) => {
                    for (i, unit) in units.iter().enumerate() {
                        validate_unit(
                            unit,
                            &format!("{section_path}.units[{i}]"),
                            &mut section_ids,
                            &section_capability_ids,
                            &section_unit_ids,
                            &mut issues,
                        );
                    }
                }
                Err(e) => {
                    issues.push(format!("{section_path}.units failed to deserialize: {e}"));
                }
            }
        }

        if let Some(layouts_val) = section.get("layouts") {
            match serde_json::from_value::<Vec<Layout>>(layouts_val.clone()) {
                Ok(layouts) => validate_layouts(
                    &layouts,
                    section.get("root_layout"),
                    &section_unit_ids,
                    &section_layout_ids,
                    &format!("{section_path}.layouts"),
                    &mut section_ids,
                    &mut issues,
                ),
                Err(e) => {
                    issues.push(format!("{section_path}.layouts failed to deserialize: {e}"));
                }
            }
        } else if section.get("root_layout").is_some() {
            issues.push(format!(
                "{section_path}.root_layout is not valid without a layouts pool"
            ));
        }
    }

    if issues.is_empty() {
        Ok(())
    } else {
        Err(AgentFirstUiError::WellFormedness { issues })
    }
}

/// Check that every `space:`-scoped binding resolves against the space tree.
/// `record:`, `input:`, and `actor:` bindings resolve at render time and are not checked.
pub fn validate_space_bindings(space: &Space) -> Result<(), AgentFirstUiError> {
    let space_val = space.as_value();
    let mut issues = Vec::new();
    let Some(space_obj) = space_val.as_object() else {
        issues.push("space must be a JSON object".to_string());
        return Err(AgentFirstUiError::WellFormedness { issues });
    };
    for section_val in space_obj.values() {
        let Some(section) = section_val.as_object() else {
            continue;
        };
        if let Some(caps_val) = section.get("capabilities")
            && let Ok(caps) = serde_json::from_value::<Vec<Capability>>(caps_val.clone())
        {
            for cap in caps {
                if let Some(binding) = &cap.applicable_binding {
                    collect_unresolved_space_binding(
                        "applicable_binding",
                        &Value::String(binding.clone()),
                        space_val,
                        &mut issues,
                    );
                }
                collect_unresolved_space_bindings(
                    &Value::Object(cap.extra),
                    space_val,
                    &mut issues,
                );
            }
        }
        if let Some(units_val) = section.get("units")
            && let Ok(units) = serde_json::from_value::<Vec<Unit>>(units_val.clone())
        {
            for unit in units {
                collect_unresolved_space_bindings(
                    &Value::Object(unit.extra),
                    space_val,
                    &mut issues,
                );
            }
        }
        if let Some(layouts_val) = section.get("layouts")
            && let Ok(layouts) = serde_json::from_value::<Vec<Layout>>(layouts_val.clone())
        {
            for layout in layouts {
                collect_unresolved_space_bindings(
                    &Value::Object(layout.extra),
                    space_val,
                    &mut issues,
                );
            }
        }
    }
    if issues.is_empty() {
        Ok(())
    } else {
        Err(AgentFirstUiError::WellFormedness { issues })
    }
}

fn collect_unresolved_space_bindings(node: &Value, space: &Value, issues: &mut Vec<String>) {
    match node {
        Value::Object(map) => {
            for (key, value) in map {
                if key == FIELD_INPUT_BINDINGS {
                    collect_unresolved_input_bindings(value, space, issues);
                    continue;
                }
                if key == FIELD_INTERACTIONS {
                    collect_unresolved_interaction_bindings(value, space, issues);
                    continue;
                }
                if key.ends_with("_binding") {
                    collect_unresolved_space_binding(key, value, space, issues);
                }
                collect_unresolved_space_bindings(value, space, issues);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_unresolved_space_bindings(item, space, issues);
            }
        }
        _ => {}
    }
}

fn collect_unresolved_interaction_bindings(value: &Value, space: &Value, issues: &mut Vec<String>) {
    let Some(events) = value.as_object() else {
        return;
    };
    for (event, effects) in events {
        let Some(items) = effects.as_array() else {
            continue;
        };
        for (index, effect) in items.iter().enumerate() {
            let Some(effect) = effect.as_object() else {
                continue;
            };
            if let Some(value_binding) = effect.get("value_binding") {
                collect_unresolved_space_binding(
                    &format!("{FIELD_INTERACTIONS}.{event}[{index}].value_binding"),
                    value_binding,
                    space,
                    issues,
                );
            }
            if let Some(input_bindings) = effect.get(FIELD_INPUT_BINDINGS) {
                collect_unresolved_input_bindings(input_bindings, space, issues);
            }
        }
    }
}

fn collect_unresolved_input_bindings(value: &Value, space: &Value, issues: &mut Vec<String>) {
    let Some(bindings) = value.as_object() else {
        return;
    };
    for (name, binding_value) in bindings {
        collect_unresolved_space_binding(
            &format!("{FIELD_INPUT_BINDINGS}.{name}"),
            binding_value,
            space,
            issues,
        );
    }
}

fn collect_unresolved_space_binding(
    key: &str,
    value: &Value,
    space: &Value,
    issues: &mut Vec<String>,
) {
    if let Some(binding_str) = value.as_str()
        && let Ok(binding) = Binding::parse(binding_str)
        && binding.scope == BindingScope::Space
        && get_dot_path(space, &binding.path).is_none()
    {
        issues.push(format!(
            "`{key}` `{binding_str}` does not resolve against the space"
        ));
    }
}

fn collect_section_capability_ids(section: &serde_json::Map<String, Value>) -> HashSet<String> {
    let mut ids = HashSet::new();
    if let Some(caps_val) = section.get("capabilities")
        && let Ok(caps) = serde_json::from_value::<Vec<Capability>>(caps_val.clone())
    {
        for cap in caps {
            ids.insert(cap.id);
        }
    }
    ids
}

fn collect_section_unit_ids(section: &serde_json::Map<String, Value>) -> HashSet<String> {
    section
        .get("units")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|unit_val| serde_json::from_value::<Unit>(unit_val.clone()).ok())
        .map(|unit| unit.id)
        .collect()
}

fn collect_section_layout_ids(section: &serde_json::Map<String, Value>) -> HashSet<String> {
    section
        .get("layouts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|layout_val| serde_json::from_value::<Layout>(layout_val.clone()).ok())
        .map(|layout| layout.id)
        .collect()
}

fn validate_capability(
    cap: &Capability,
    path: &str,
    all_ids: &mut HashSet<String>,
    issues: &mut Vec<String>,
) {
    validate_id_field(&format!("{path}.id"), &cap.id, all_ids, issues);
    if cap.label.is_empty() {
        issues.push(format!("{path}.label must not be empty"));
    }
    if let Some(schema) = &cap.input_schema
        && !schema.is_object()
    {
        issues.push(format!(
            "{path}.input_schema must be a JSON object if present"
        ));
    }
    if let Some(binding) = &cap.applicable_binding {
        validate_binding_str(&format!("{path}.applicable_binding"), binding, issues);
    }
    validate_extra_value(path, &Value::Object(cap.extra.clone()), None, issues);
}

fn validate_unit(
    unit: &Unit,
    path: &str,
    all_ids: &mut HashSet<String>,
    all_capability_ids: &HashSet<String>,
    section_unit_ids: &HashSet<String>,
    issues: &mut Vec<String>,
) {
    validate_id_field(&format!("{path}.id"), &unit.id, all_ids, issues);
    if unit.kind.is_empty() {
        issues.push(format!("{path}.kind must not be empty"));
    }

    if let Some(id) = &unit.activate_capability {
        validate_capability_id_str(
            &format!("{path}.activate_capability"),
            id,
            all_capability_ids,
            issues,
        );
    }
    if let Some(id) = &unit.primary_capability {
        validate_capability_id_str(
            &format!("{path}.primary_capability"),
            id,
            all_capability_ids,
            issues,
        );
    }
    if let Some(id) = &unit.fallback {
        validate_unit_ref(&format!("{path}.fallback"), id, section_unit_ids, issues);
    }

    validate_select_references(unit, path, section_unit_ids, issues);
    validate_extra_value(
        path,
        &Value::Object(unit.extra.clone()),
        Some(all_capability_ids),
        issues,
    );
}

fn validate_select_references(
    unit: &Unit,
    path: &str,
    section_unit_ids: &HashSet<String>,
    issues: &mut Vec<String>,
) {
    if let Some(default) = unit.extra.get("default") {
        match default.as_str() {
            Some(id) => validate_unit_ref(&format!("{path}.default"), id, section_unit_ids, issues),
            None => issues.push(format!("{path}.default must be a unit id string")),
        }
    }
    if let Some(cases) = unit.extra.get("cases") {
        let Some(map) = cases.as_object() else {
            issues.push(format!(
                "{path}.cases must be an object mapping values to unit ids"
            ));
            return;
        };
        for (key, value) in map {
            match value.as_str() {
                Some(id) => {
                    validate_unit_ref(&format!("{path}.cases.{key}"), id, section_unit_ids, issues)
                }
                None => issues.push(format!("{path}.cases.{key} must be a unit id string")),
            }
        }
    }
}

fn validate_layouts(
    layouts: &[Layout],
    root_layout_value: Option<&Value>,
    section_unit_ids: &HashSet<String>,
    section_layout_ids: &HashSet<String>,
    path: &str,
    all_ids: &mut HashSet<String>,
    issues: &mut Vec<String>,
) {
    for (i, layout) in layouts.iter().enumerate() {
        let layout_path = format!("{path}[{i}]");
        validate_id_field(&format!("{layout_path}.id"), &layout.id, all_ids, issues);
        validate_extra_value(
            &layout_path,
            &Value::Object(layout.extra.clone()),
            None,
            issues,
        );
        for (j, member) in layout.members.iter().enumerate() {
            if !section_unit_ids.contains(member) && !section_layout_ids.contains(member) {
                issues.push(format!(
                    "{layout_path}.members[{j}] references undeclared unit or layout `{member}`"
                ));
            }
        }
    }

    match root_layout_value {
        Some(Value::String(root_layout)) => {
            if !section_layout_ids.contains(root_layout) {
                issues.push(format!(
                    "{path}.root_layout references undeclared layout `{root_layout}`"
                ));
            }
        }
        Some(_) => issues.push(format!("{path}.root_layout must be a layout id string")),
        None if !layouts.is_empty() => issues.push(format!(
            "{path}.root_layout is required when a layouts pool is present"
        )),
        None => {}
    }

    validate_layout_dag(layouts, path, issues);
}

fn validate_layout_dag(layouts: &[Layout], path: &str, issues: &mut Vec<String>) {
    let by_id: HashMap<&str, &Layout> = layouts
        .iter()
        .map(|layout| (layout.id.as_str(), layout))
        .collect();
    let mut visiting = HashSet::new();
    let mut visited = HashSet::new();
    for layout in layouts {
        visit_layout(
            layout.id.as_str(),
            &by_id,
            &mut visiting,
            &mut visited,
            path,
            issues,
        );
    }
}

fn visit_layout<'a>(
    id: &'a str,
    by_id: &HashMap<&'a str, &'a Layout>,
    visiting: &mut HashSet<&'a str>,
    visited: &mut HashSet<&'a str>,
    path: &str,
    issues: &mut Vec<String>,
) {
    if visited.contains(id) {
        return;
    }
    if !visiting.insert(id) {
        issues.push(format!("{path} has a cycle involving layout `{id}`"));
        return;
    }
    if let Some(layout) = by_id.get(id) {
        for member in &layout.members {
            if by_id.contains_key(member.as_str()) {
                visit_layout(member, by_id, visiting, visited, path, issues);
            }
        }
    }
    visiting.remove(id);
    visited.insert(id);
}

fn validate_id_field(
    path: &str,
    id: &str,
    all_ids: &mut HashSet<String>,
    issues: &mut Vec<String>,
) {
    if !is_valid_id(id) {
        issues.push(format!("{path} has invalid id `{id}`"));
    }
    if !all_ids.insert(id.to_string()) {
        issues.push(format!("{path} duplicates id `{id}`"));
    }
}

fn validate_unit_ref(
    path: &str,
    id: &str,
    section_unit_ids: &HashSet<String>,
    issues: &mut Vec<String>,
) {
    if !section_unit_ids.contains(id) {
        issues.push(format!("{path} references undeclared unit `{id}`"));
    }
}

fn validate_extra_value(
    path: &str,
    value: &Value,
    all_capability_ids: Option<&HashSet<String>>,
    issues: &mut Vec<String>,
) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                let child_path = format!("{path}.{key}");
                if key == FIELD_INTERACTIONS {
                    validate_interactions(&child_path, child, all_capability_ids, issues);
                    continue;
                }
                if key.ends_with("_binding") {
                    validate_binding_field(&child_path, child, issues);
                }
                if key == FIELD_INPUT_BINDINGS {
                    validate_input_bindings(&child_path, child, issues);
                    continue;
                }
                if key.ends_with("_uri") && !child.is_string() {
                    issues.push(format!("{child_path} must be a string URI"));
                }
                if let Some(ids) = all_capability_ids {
                    if key == "capability" || key.ends_with("_capability") {
                        validate_capability_ref(&child_path, child, ids, issues);
                    }
                    if key == "capabilities" || key.ends_with("_capabilities") {
                        validate_capability_ref_array(&child_path, child, ids, issues);
                    }
                }
                validate_extra_value(&child_path, child, all_capability_ids, issues);
            }
        }
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                validate_extra_value(
                    &format!("{path}[{index}]"),
                    item,
                    all_capability_ids,
                    issues,
                );
            }
        }
        _ => {}
    }
}

fn validate_interactions(
    path: &str,
    value: &Value,
    all_capability_ids: Option<&HashSet<String>>,
    issues: &mut Vec<String>,
) {
    let Some(events) = value.as_object() else {
        issues.push(format!(
            "{path} must be an object mapping events to effects"
        ));
        return;
    };
    for (event, effects) in events {
        let event_path = format!("{path}.{event}");
        if !is_valid_id(event) {
            issues.push(format!("{event_path} has invalid interaction event name"));
        }
        let Some(items) = effects.as_array() else {
            issues.push(format!(
                "{event_path} must be an array of interaction effects"
            ));
            continue;
        };
        for (index, effect) in items.iter().enumerate() {
            validate_interaction_effect(
                &format!("{event_path}[{index}]"),
                effect,
                all_capability_ids,
                issues,
            );
        }
    }
}

fn validate_interaction_effect(
    path: &str,
    value: &Value,
    all_capability_ids: Option<&HashSet<String>>,
    issues: &mut Vec<String>,
) {
    let Some(effect) = value.as_object() else {
        issues.push(format!("{path} must be an interaction effect object"));
        return;
    };
    let Some(effect_type) = effect.get("type").and_then(Value::as_str) else {
        issues.push(format!(
            "{path}.type must be an interaction effect type string"
        ));
        return;
    };
    match effect_type {
        "set_actor_state" => validate_set_actor_state_effect(path, effect, issues),
        "request" => validate_request_effect(path, effect, all_capability_ids, issues),
        other => issues.push(format!(
            "{path}.type has unknown interaction effect type `{other}`"
        )),
    }
}

fn validate_set_actor_state_effect(
    path: &str,
    effect: &serde_json::Map<String, Value>,
    issues: &mut Vec<String>,
) {
    match effect.get("path").and_then(Value::as_str) {
        Some(state_path) if is_valid_actor_state_path(state_path) => {}
        Some(_) => issues.push(format!(
            "{path}.path must be a relative actor-state dot path"
        )),
        None => issues.push(format!("{path}.path must be an actor-state path string")),
    }

    let has_value = effect.contains_key("value");
    let has_value_binding = effect.contains_key("value_binding");
    match (has_value, has_value_binding) {
        (true, false) | (false, true) => {}
        (false, false) => issues.push(format!("{path} must provide either value or value_binding")),
        (true, true) => issues.push(format!(
            "{path} must not provide both value and value_binding"
        )),
    }
    if let Some(value_binding) = effect.get("value_binding") {
        validate_binding_field(&format!("{path}.value_binding"), value_binding, issues);
    }
}

fn validate_request_effect(
    path: &str,
    effect: &serde_json::Map<String, Value>,
    all_capability_ids: Option<&HashSet<String>>,
    issues: &mut Vec<String>,
) {
    if let Some(ids) = all_capability_ids {
        match effect.get("capability") {
            Some(capability) => {
                validate_capability_ref(&format!("{path}.capability"), capability, ids, issues);
            }
            None => issues.push(format!("{path}.capability must be a capability id string")),
        }
    } else if !effect.get("capability").is_some_and(Value::is_string) {
        issues.push(format!("{path}.capability must be a capability id string"));
    }
    if let Some(input_bindings) = effect.get(FIELD_INPUT_BINDINGS) {
        validate_input_bindings(
            &format!("{path}.{FIELD_INPUT_BINDINGS}"),
            input_bindings,
            issues,
        );
    }
}

fn validate_binding_field(path: &str, value: &Value, issues: &mut Vec<String>) {
    let Some(binding_str) = value.as_str() else {
        issues.push(format!("{path} must be a string binding"));
        return;
    };
    validate_binding_str(path, binding_str, issues);
}

fn validate_binding_str(path: &str, binding_str: &str, issues: &mut Vec<String>) {
    if let Err(e) = Binding::parse(binding_str) {
        issues.push(format!("{path} {e}"));
    }
}

fn validate_input_bindings(path: &str, value: &Value, issues: &mut Vec<String>) {
    let Some(bindings) = value.as_object() else {
        issues.push(format!(
            "{path} must be an object mapping input field names to string bindings"
        ));
        return;
    };
    for (name, binding_value) in bindings {
        if name.is_empty() {
            issues.push(format!("{path} has an empty input field name"));
        }
        validate_binding_field(&format!("{path}.{name}"), binding_value, issues);
    }
}

fn validate_capability_ref(
    path: &str,
    value: &Value,
    all_capability_ids: &HashSet<String>,
    issues: &mut Vec<String>,
) {
    let Some(cap_id) = value.as_str() else {
        issues.push(format!("{path} must be a capability id string"));
        return;
    };
    validate_capability_id_str(path, cap_id, all_capability_ids, issues);
}

fn validate_capability_id_str(
    path: &str,
    cap_id: &str,
    all_capability_ids: &HashSet<String>,
    issues: &mut Vec<String>,
) {
    if !all_capability_ids.contains(cap_id) {
        issues.push(format!(
            "{path} references undeclared capability `{cap_id}`"
        ));
    }
}

fn validate_capability_ref_array(
    path: &str,
    value: &Value,
    all_capability_ids: &HashSet<String>,
    issues: &mut Vec<String>,
) {
    let Some(items) = value.as_array() else {
        issues.push(format!("{path} must be an array of capability id strings"));
        return;
    };
    for (i, item) in items.iter().enumerate() {
        validate_capability_ref(&format!("{path}[{i}]"), item, all_capability_ids, issues);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn space_from(value: serde_json::Value) -> Space {
        Space::try_from_value(value).unwrap_or_else(|_| Space::empty())
    }

    fn issues_of(space: &Space) -> Vec<String> {
        match validate_space(space) {
            Ok(()) => Vec::new(),
            Err(AgentFirstUiError::WellFormedness { issues }) => issues,
            Err(other) => vec![other.to_string()],
        }
    }

    #[test]
    fn accepts_minimal_valid_space() {
        let space = space_from(json!({
            "afmail": {
                "capabilities": [
                    { "id": "open_message", "label": "Open", "risk": "read_only" }
                ],
                "units": [
                    { "id": "inbox", "kind": "table" }
                ],
                "layouts": [
                    { "id": "root", "members": ["inbox"] }
                ],
                "root_layout": "root"
            }
        }));
        assert!(
            issues_of(&space).is_empty(),
            "expected valid, got {:?}",
            issues_of(&space)
        );
    }

    #[test]
    fn accepts_domain_facts_named_like_code_or_appearance() {
        let space = space_from(json!({
            "operations": {
                "html": "<b>x</b>",
                "theme": { "color_primary": "red" },
                "shell_command": "echo logged command text",
                "capabilities": [
                    { "id": "do_thing", "label": "Do", "risk": "destructive", "requires_confirmation": true }
                ],
                "units": [
                    { "id": "term1", "kind": "terminal", "command": "example command text" }
                ]
            }
        }));
        let issues = issues_of(&space);
        assert!(issues.is_empty(), "got {issues:?}");
    }

    #[test]
    fn binding_resolution_ignores_domain_fact_binding_names() {
        let space = space_from(json!({
            "operations": {
                "protein_binding": "space:missing.domain.fact",
                "units": [
                    { "id": "main_unit", "kind": "facts", "source_binding": "space:operations.protein_binding" }
                ]
            }
        }));
        assert!(
            validate_space_bindings(&space).is_ok(),
            "domain fact names ending in _binding must not be treated as hint bindings"
        );
    }

    #[test]
    fn binding_resolution_checks_declared_hint_bindings() {
        let space = space_from(json!({
            "operations": {
                "units": [
                    { "id": "main_unit", "kind": "facts", "source_binding": "space:missing.hint.binding" }
                ]
            }
        }));
        let result = validate_space_bindings(&space);
        assert!(result.is_err(), "expected unresolved hint binding");
    }

    #[test]
    fn binding_resolution_ignores_literal_interaction_values() {
        let space = space_from(json!({
            "operations": {
                "units": [
                    {
                        "id": "main_unit",
                        "kind": "list",
                        "interactions": {
                            "activate": [
                                {
                                    "type": "set_actor_state",
                                    "path": "selection.record",
                                    "value": {
                                        "protein_binding": "space:missing.domain.fact"
                                    }
                                }
                            ]
                        }
                    }
                ]
            }
        }));
        assert!(
            validate_space_bindings(&space).is_ok(),
            "literal actor-state values must not be scanned as hint bindings"
        );
    }

    #[test]
    fn rejects_invalid_applicable_binding() {
        let space = space_from(json!({
            "operations": {
                "capabilities": [
                    { "id": "do_thing", "label": "Do", "risk": "read_only", "applicable_binding": "bad:scope" }
                ]
            }
        }));
        let issues = issues_of(&space);
        assert!(
            issues.iter().any(|i| i.contains("applicable_binding")),
            "got {issues:?}"
        );
    }

    #[test]
    fn rejects_duplicate_ids() {
        let space = space_from(json!({
            "operations": {
                "capabilities": [
                    { "id": "do_thing", "label": "Do", "risk": "read_only" },
                    { "id": "do_thing", "label": "Do2", "risk": "read_only" }
                ]
            }
        }));
        let issues = issues_of(&space);
        assert!(
            issues.iter().any(|i| i.contains("duplicates")),
            "got {issues:?}"
        );
    }

    #[test]
    fn rejects_malformed_input_bindings() {
        let space = space_from(json!({
            "operations": {
                "units": [
                    { "id": "main_unit", "kind": "table", "input_bindings": { "target": 42 } }
                ]
            }
        }));
        let issues = issues_of(&space);
        assert!(
            issues.iter().any(|i| i.contains("input_bindings.target")),
            "got {issues:?}"
        );
    }

    #[test]
    fn rejects_layout_member_ref_to_undeclared_id() {
        let space = space_from(json!({
            "operations": {
                "units": [{ "id": "main_unit", "kind": "table" }],
                "layouts": [
                    { "id": "root", "members": ["missing_unit"] }
                ],
                "root_layout": "root"
            }
        }));
        let issues = issues_of(&space);
        assert!(
            issues.iter().any(|i| i.contains("missing_unit")),
            "got {issues:?}"
        );
    }

    #[test]
    fn rejects_layout_cycles() {
        let space = space_from(json!({
            "operations": {
                "units": [{ "id": "main_unit", "kind": "table" }],
                "layouts": [
                    { "id": "a", "members": ["b"] },
                    { "id": "b", "members": ["a"] }
                ],
                "root_layout": "a"
            }
        }));
        let issues = issues_of(&space);
        assert!(issues.iter().any(|i| i.contains("cycle")), "got {issues:?}");
    }
}

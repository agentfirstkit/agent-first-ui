use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::types::Space;
use crate::unit_profile::{
    FIELD_CAPABILITIES, FIELD_DEFAULT_PRESENTATION, FIELD_FIELDS, FIELD_GENRE, FIELD_INTERACTIONS,
    FIELD_REFERENCE_KIND, FIELD_RELATION, FIELD_ROLE, FIELD_SUBMIT_CAPABILITY, FIELD_SURFACE,
    FIELD_VISIBLE_WHEN, FormFieldFormat, FormFieldType, InteractionEvent, KIND_CAPABILITIES,
    KIND_FACTS, KIND_FORM, KIND_LIST, KIND_LOG, KIND_MEDIA, KIND_SELECT, KIND_TABLE, KIND_TERMINAL,
    KIND_TEXT, LayoutRelation, LayoutRole, PresentationLevel, SurfaceGenre,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationProfile {
    ReferenceHost,
}

impl ValidationProfile {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReferenceHost => "reference_host",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileIssueLevel {
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileIssue {
    pub level: ProfileIssueLevel,
    pub path: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileValidationReport {
    pub profile: ValidationProfile,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub issues: Vec<ProfileIssue>,
}

impl ProfileValidationReport {
    pub fn valid(&self) -> bool {
        !self
            .issues
            .iter()
            .any(|issue| issue.level == ProfileIssueLevel::Error)
    }

    pub fn error_count(&self) -> usize {
        self.issues
            .iter()
            .filter(|issue| issue.level == ProfileIssueLevel::Error)
            .count()
    }
}

pub fn validate_space_profile(
    space: &Space,
    profile: ValidationProfile,
) -> ProfileValidationReport {
    let mut report = ProfileValidationReport {
        profile,
        issues: Vec::new(),
    };
    match profile {
        ValidationProfile::ReferenceHost => {
            inspect_reference_host_space(space.as_value(), &mut report.issues);
        }
    }
    report
}

fn inspect_reference_host_space(value: &Value, issues: &mut Vec<ProfileIssue>) {
    let Some(sections) = value.as_object() else {
        return;
    };
    for (owner, section_value) in sections {
        let Some(section) = section_value.as_object() else {
            continue;
        };
        if let Some(surface) = section.get(FIELD_SURFACE) {
            inspect_reference_host_surface(
                surface,
                &format!("space.{owner}.{FIELD_SURFACE}"),
                issues,
            );
        }
        if let Some(units) = section.get("units").and_then(Value::as_array) {
            for (index, unit) in units.iter().enumerate() {
                inspect_reference_host_unit(unit, &format!("space.{owner}.units[{index}]"), issues);
            }
        }
        if let Some(layouts) = section.get("layouts").and_then(Value::as_array) {
            for (index, layout) in layouts.iter().enumerate() {
                inspect_reference_host_layout(
                    layout,
                    &format!("space.{owner}.layouts[{index}]"),
                    issues,
                );
            }
        }
    }
}

fn inspect_reference_host_unit(value: &Value, path: &str, issues: &mut Vec<ProfileIssue>) {
    if let Value::Object(map) = value {
        if let Some(Value::String(kind)) = map.get("kind") {
            inspect_reference_host_kind(kind, &format!("{path}.kind"), issues);
        }
        if let Some(interactions) = map.get(FIELD_INTERACTIONS) {
            inspect_reference_host_interactions(
                interactions,
                &format!("{path}.{FIELD_INTERACTIONS}"),
                issues,
            );
        }
        if let Some(default_presentation) = map.get(FIELD_DEFAULT_PRESENTATION) {
            validate_enum_value(
                default_presentation,
                &format!("{path}.{FIELD_DEFAULT_PRESENTATION}"),
                PresentationLevel::VALUES,
                "default presentation",
                issues,
            );
        }
        if let Some(reference_kind) = map.get(FIELD_REFERENCE_KIND)
            && reference_kind.as_str().is_none_or(|value| value.is_empty())
        {
            push_error(
                issues,
                &format!("{path}.{FIELD_REFERENCE_KIND}"),
                "reference_kind must be a non-empty domain kind",
            );
        }
        if map.get("kind").and_then(Value::as_str) != Some(KIND_FORM) {
            for member in [FIELD_FIELDS, "items", "columns"] {
                inspect_field_visibility_descriptors(
                    map.get(member),
                    &format!("{path}.{member}"),
                    issues,
                );
            }
        }
        match map.get("kind").and_then(Value::as_str) {
            Some(KIND_FORM) => inspect_reference_host_form(map, path, issues),
            Some(KIND_CAPABILITIES) => {
                inspect_reference_host_capabilities_unit(map, path, issues);
                if map.contains_key(FIELD_SUBMIT_CAPABILITY) {
                    push_error(
                        issues,
                        &format!("{path}.{FIELD_SUBMIT_CAPABILITY}"),
                        "submit_capability is only valid on a canonical form unit",
                    );
                }
            }
            Some(_) if map.contains_key(FIELD_SUBMIT_CAPABILITY) => push_error(
                issues,
                &format!("{path}.{FIELD_SUBMIT_CAPABILITY}"),
                "submit_capability is only valid on a canonical form unit",
            ),
            _ => {}
        }
    }
    inspect_reference_host_hint_value(value, path, issues);
}

fn inspect_field_visibility_descriptors(
    value: Option<&Value>,
    path: &str,
    issues: &mut Vec<ProfileIssue>,
) {
    let Some(descriptors) = value.and_then(Value::as_array) else {
        return;
    };
    for (index, descriptor) in descriptors.iter().enumerate() {
        let Some(predicate) = descriptor
            .as_object()
            .and_then(|field| field.get(FIELD_VISIBLE_WHEN))
        else {
            continue;
        };
        inspect_value_predicate(
            predicate,
            &format!("{path}[{index}].{FIELD_VISIBLE_WHEN}"),
            issues,
        );
    }
}

fn inspect_value_predicate(value: &Value, path: &str, issues: &mut Vec<ProfileIssue>) {
    if let Some(predicates) = value.as_array() {
        if predicates.is_empty() {
            push_error(
                issues,
                path,
                "visible_when predicate array must not be empty",
            );
        }
        for (index, predicate) in predicates.iter().enumerate() {
            inspect_atomic_value_predicate(predicate, &format!("{path}[{index}]"), issues);
        }
        return;
    }
    inspect_atomic_value_predicate(value, path, issues);
}

fn inspect_atomic_value_predicate(value: &Value, path: &str, issues: &mut Vec<ProfileIssue>) {
    let Some(predicate) = value.as_object() else {
        push_error(issues, path, "visible_when predicate must be an object");
        return;
    };
    if predicate.len() != 1 {
        push_error(
            issues,
            path,
            "visible_when predicate must contain exactly one comparison",
        );
        return;
    }
    let Some((comparison, expected)) = predicate.iter().next() else {
        return;
    };
    match comparison.as_str() {
        "lt" | "gt" if !expected.is_number() => push_error(
            issues,
            &format!("{path}.{comparison}"),
            "visible_when numeric comparison must use a number",
        ),
        "empty" if !expected.is_boolean() => push_error(
            issues,
            &format!("{path}.empty"),
            "visible_when empty comparison must use a boolean",
        ),
        "lt" | "gt" | "eq" | "contains" | "empty" => {}
        _ => push_error(
            issues,
            &format!("{path}.{comparison}"),
            "visible_when comparison is not a canonical View Hint Profile value",
        ),
    }
}

fn inspect_reference_host_capabilities_unit(
    unit: &serde_json::Map<String, Value>,
    path: &str,
    issues: &mut Vec<ProfileIssue>,
) {
    if unit
        .get(FIELD_CAPABILITIES)
        .and_then(Value::as_array)
        .is_none_or(|capabilities| capabilities.is_empty())
    {
        push_error(
            issues,
            &format!("{path}.{FIELD_CAPABILITIES}"),
            "canonical capabilities unit requires a non-empty capabilities array",
        );
    }
}

fn inspect_reference_host_form(
    form: &serde_json::Map<String, Value>,
    path: &str,
    issues: &mut Vec<ProfileIssue>,
) {
    match form.get(FIELD_SUBMIT_CAPABILITY) {
        Some(Value::String(value)) if !value.is_empty() => {}
        Some(_) => push_error(
            issues,
            &format!("{path}.{FIELD_SUBMIT_CAPABILITY}"),
            "form submit_capability must be a non-empty capability id",
        ),
        None => push_error(
            issues,
            path,
            "canonical form unit requires submit_capability",
        ),
    }

    let Some(fields) = form.get(FIELD_FIELDS).and_then(Value::as_array) else {
        push_error(issues, path, "canonical form unit requires a fields array");
        return;
    };
    if fields.is_empty() {
        push_error(
            issues,
            &format!("{path}.{FIELD_FIELDS}"),
            "canonical form fields must not be empty",
        );
    }

    let mut names = HashSet::new();
    for (index, field) in fields.iter().enumerate() {
        let field_path = format!("{path}.{FIELD_FIELDS}[{index}]");
        let Some(field) = field.as_object() else {
            push_error(issues, &field_path, "form field must be an object");
            continue;
        };
        for key in field.keys() {
            if ![
                "name",
                "label",
                "type",
                "format",
                "value_binding",
                "required",
                "options",
            ]
            .contains(&key.as_str())
            {
                push_error(
                    issues,
                    &format!("{field_path}.{key}"),
                    "unknown canonical form field member is invalid",
                );
            }
        }

        match field.get("name") {
            Some(Value::String(name)) if !name.is_empty() => {
                if !names.insert(name) {
                    push_error(
                        issues,
                        &format!("{field_path}.name"),
                        "form field name must be unique within the form",
                    );
                }
            }
            _ => push_error(
                issues,
                &format!("{field_path}.name"),
                "form field name must be a non-empty string",
            ),
        }
        match field.get("label") {
            Some(Value::String(label)) if !label.is_empty() => {}
            _ => push_error(
                issues,
                &format!("{field_path}.label"),
                "form field label must be a non-empty string",
            ),
        }

        let field_type = field.get("type").and_then(Value::as_str);
        if let Some(value) = field.get("type") {
            validate_enum_value(
                value,
                &format!("{field_path}.type"),
                FormFieldType::VALUES,
                "form field type",
                issues,
            );
        } else {
            push_error(
                issues,
                &format!("{field_path}.type"),
                "form field requires a type",
            );
        }

        let format = field.get("format").and_then(Value::as_str);
        if let Some(value) = field.get("format") {
            validate_enum_value(
                value,
                &format!("{field_path}.format"),
                FormFieldFormat::VALUES,
                "form field format",
                issues,
            );
        }
        let compatible_format = matches!(
            (field_type, format),
            (_, None)
                | (
                    Some("string"),
                    Some("text" | "textarea" | "markdown" | "email" | "enum")
                )
                | (Some("array"), Some("email_list" | "tags"))
        );
        if !compatible_format {
            push_error(
                issues,
                &format!("{field_path}.format"),
                "form field format is incompatible with its value type",
            );
        }

        let options = field.get("options").and_then(Value::as_array);
        if format == Some("enum") {
            let Some(options) = options else {
                push_error(
                    issues,
                    &format!("{field_path}.options"),
                    "enum form field requires a non-empty options array",
                );
                continue;
            };
            if options.is_empty() {
                push_error(
                    issues,
                    &format!("{field_path}.options"),
                    "enum form field requires a non-empty options array",
                );
            }
            let mut values = HashSet::new();
            for (option_index, option) in options.iter().enumerate() {
                let option_path = format!("{field_path}.options[{option_index}]");
                let Some(option) = option.as_object() else {
                    push_error(issues, &option_path, "form field option must be an object");
                    continue;
                };
                for key in option.keys() {
                    if key != "value" && key != "label" {
                        push_error(
                            issues,
                            &format!("{option_path}.{key}"),
                            "unknown canonical form field option member is invalid",
                        );
                    }
                }
                match option.get("value") {
                    Some(Value::String(value)) if !value.is_empty() => {
                        if !values.insert(value) {
                            push_error(
                                issues,
                                &format!("{option_path}.value"),
                                "form field option value must be unique",
                            );
                        }
                    }
                    _ => push_error(
                        issues,
                        &format!("{option_path}.value"),
                        "form field option value must be a non-empty string",
                    ),
                }
                match option.get("label") {
                    Some(Value::String(label)) if !label.is_empty() => {}
                    _ => push_error(
                        issues,
                        &format!("{option_path}.label"),
                        "form field option label must be a non-empty string",
                    ),
                }
            }
        } else if field.contains_key("options") {
            push_error(
                issues,
                &format!("{field_path}.options"),
                "only enum-formatted fields may declare options",
            );
        }

        if field
            .get("required")
            .is_some_and(|required| !required.is_boolean())
        {
            push_error(
                issues,
                &format!("{field_path}.required"),
                "form field required must be a boolean",
            );
        }
        if field
            .get("value_binding")
            .is_some_and(|binding| !binding.is_string())
        {
            push_error(
                issues,
                &format!("{field_path}.value_binding"),
                "form field value_binding must be a scoped binding string",
            );
        }
    }
}

fn inspect_reference_host_surface(value: &Value, path: &str, issues: &mut Vec<ProfileIssue>) {
    let Some(surface) = value.as_object() else {
        push_error(issues, path, "surface must be an object");
        return;
    };
    if let Some(genre) = surface.get(FIELD_GENRE) {
        validate_enum_value(
            genre,
            &format!("{path}.{FIELD_GENRE}"),
            SurfaceGenre::VALUES,
            "surface genre",
            issues,
        );
    }
}

fn inspect_reference_host_layout(value: &Value, path: &str, issues: &mut Vec<ProfileIssue>) {
    if let Value::Object(map) = value {
        if let Some(relation) = map.get(FIELD_RELATION) {
            validate_enum_value(
                relation,
                &format!("{path}.{FIELD_RELATION}"),
                LayoutRelation::VALUES,
                "layout relation",
                issues,
            );
        }
        if let Some(role) = map.get(FIELD_ROLE) {
            validate_enum_value(
                role,
                &format!("{path}.{FIELD_ROLE}"),
                LayoutRole::VALUES,
                "layout role",
                issues,
            );
        }
    }
    inspect_reference_host_hint_value(value, path, issues);
}

fn inspect_reference_host_hint_value(value: &Value, path: &str, issues: &mut Vec<ProfileIssue>) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                let child_path = if path.is_empty() {
                    key.to_string()
                } else {
                    format!("{path}.{key}")
                };
                inspect_reference_host_key(key, child, &child_path, issues);
                inspect_reference_host_binding(key, child, &child_path, issues);
                inspect_reference_host_hint_value(child, &child_path, issues);
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                inspect_reference_host_hint_value(child, &format!("{path}[{index}]"), issues);
            }
        }
        _ => {}
    }
}

fn inspect_reference_host_interactions(value: &Value, path: &str, issues: &mut Vec<ProfileIssue>) {
    let Some(events) = value.as_object() else {
        return;
    };
    for (event, effects) in events {
        if !InteractionEvent::VALUES.contains(&event.as_str()) {
            push_error(
                issues,
                &format!("{path}.{event}"),
                "interaction event is not a canonical View Hint Profile event",
            );
        }
        let Some(items) = effects.as_array() else {
            continue;
        };
        for (index, effect) in items.iter().enumerate() {
            let Some(effect_type) = effect.get("type").and_then(Value::as_str) else {
                continue;
            };
            if effect_type != "set_actor_state" && effect_type != "request" {
                push_error(
                    issues,
                    &format!("{path}.{event}[{index}].type"),
                    "interaction effect type is not a canonical View Hint Profile value",
                );
            }
        }
    }
}

fn inspect_reference_host_binding(
    key: &str,
    value: &Value,
    path: &str,
    issues: &mut Vec<ProfileIssue>,
) {
    if key == "input_bindings" {
        if let Some(bindings) = value.as_object() {
            for (name, binding_value) in bindings {
                inspect_reference_host_binding(
                    "input_binding",
                    binding_value,
                    &format!("{path}.{name}"),
                    issues,
                );
            }
        }
        return;
    }
    if !key.ends_with("_binding") && key != "input_binding" {
        return;
    }
    if let Some(text) = value.as_str()
        && text.starts_with("room:")
    {
        push_error(
            issues,
            path,
            "legacy binding scope `room:` is invalid; use `space:`",
        );
    }
}

fn inspect_reference_host_key(
    key: &str,
    value: &Value,
    path: &str,
    issues: &mut Vec<ProfileIssue>,
) {
    if key == "dockview" {
        push_error(
            issues,
            path,
            "host-specific Dockview hints are invalid; use owner layouts instead",
        );
    }
    if key == "layout"
        || key == "groups"
        || key == "unit_ids"
        || key == "size"
        || key == "priority"
        || key == "default_open"
    {
        push_error(
            issues,
            path,
            "legacy layout field is invalid; use layouts, members, relation, role, overlay, salience, and root_layout",
        );
    }
    if key == "header" || key == "footer" || key == "empty_unit" {
        push_error(
            issues,
            path,
            "legacy inline unit slot is invalid; declare a unit and reference it from a layout or select case",
        );
    }
    if key == "reference_label" {
        push_error(
            issues,
            path,
            "retired reference_label is invalid; label the reference envelope or let the viewer derive a label",
        );
    }
    if key.ends_with("_bind") {
        push_error(
            issues,
            path,
            "legacy binding field uses invalid `_bind`; use the canonical `_binding` suffix",
        );
    }
    if key == "action" || key.ends_with("_action") {
        push_error(
            issues,
            path,
            "legacy action field is invalid; use `capability` or a `*_capability` field",
        );
    }
    if key == "actions" || key.ends_with("_actions") {
        push_error(
            issues,
            path,
            "legacy actions field is invalid; use `capabilities` or a `*_capabilities` field",
        );
    }
    if (key == "default" || key == "cases")
        && value
            .as_object()
            .is_some_and(|map| map.values().any(Value::is_object))
    {
        push_error(
            issues,
            path,
            "legacy inline select case is invalid; `default` and `cases` values must be unit ids",
        );
    }
}

fn validate_enum_value(
    value: &Value,
    path: &str,
    allowed: &[&str],
    label: &str,
    issues: &mut Vec<ProfileIssue>,
) {
    let Some(text) = value.as_str() else {
        push_error(issues, path, &format!("{label} must be a string"));
        return;
    };
    if !allowed.contains(&text) {
        push_error(
            issues,
            path,
            &format!("{label} `{text}` is not a canonical View Hint Profile value"),
        );
    }
}

fn inspect_reference_host_kind(kind: &str, path: &str, issues: &mut Vec<ProfileIssue>) {
    match kind {
        KIND_FACTS | KIND_TABLE | KIND_LIST | KIND_TEXT | KIND_MEDIA | KIND_LOG | KIND_TERMINAL
        | KIND_SELECT | KIND_FORM | KIND_CAPABILITIES => {}
        "input" => push_error(
            issues,
            path,
            "retired unit kind `input` is invalid; use the canonical multi-field `form`",
        ),
        "record_list" => push_error(
            issues,
            path,
            "retired unit kind `record_list` is invalid; use the canonical `list`",
        ),
        "action_bar" => push_error(
            issues,
            path,
            "retired unit kind `action_bar` is invalid; use the canonical `capabilities`",
        ),
        "stats" | "detail" => push_error(
            issues,
            path,
            "retired unit kind is invalid; use the canonical `facts`",
        ),
        "tabs" | "switch" => push_error(
            issues,
            path,
            "retired unit kind is invalid; use the canonical `select`",
        ),
        "chart" | "progress" | "tree" => push_error(
            issues,
            path,
            "extension unit kind is outside the strict ReferenceHost profile; use a canonical kind",
        ),
        _ => push_error(
            issues,
            path,
            "non-canonical unit kind is outside the strict ReferenceHost profile",
        ),
    }
}

fn push_error(issues: &mut Vec<ProfileIssue>, path: &str, message: &str) {
    issues.push(ProfileIssue {
        level: ProfileIssueLevel::Error,
        path: path.to_string(),
        message: message.to_string(),
    });
}

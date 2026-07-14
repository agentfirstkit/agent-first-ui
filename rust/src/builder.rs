use serde_json::{Value, json};

use crate::error::AgentFirstUiError;
use crate::types::{
    Capability, Id, JsonMap, Layout, Prominence, Risk, Salience, SalienceState, Space, Unit,
};
use crate::unit_profile::{
    CanonicalUnitKind, FIELD_CAPABILITIES, FIELD_COUNT_BINDING, FIELD_DEFAULT_PRESENTATION,
    FIELD_FIELDS, FIELD_INPUT_BINDINGS, FIELD_INTERACTIONS, FIELD_REFERENCE_BINDING,
    FIELD_REFERENCE_KIND, FIELD_RELATION, FIELD_ROLE, FIELD_SESSION_ID_BINDING,
    FIELD_SOURCE_BINDING, FIELD_STATUS_BINDING, FIELD_SUBMIT_CAPABILITY, FIELD_TITLE_BINDING,
    FIELD_VALUE_BINDING, FormField, PresentationLevel,
};
use crate::unit_profile::{InteractionEffect, InteractionEvent, LayoutRelation, LayoutRole};
use crate::validation::validate_space;

#[derive(Debug, Clone, Default)]
pub struct InputSchemaBuilder {
    properties: JsonMap,
    required: Vec<String>,
}

impl InputSchemaBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn property(mut self, name: impl Into<String>, schema: Value, required: bool) -> Self {
        let name = name.into();
        self.properties.insert(name.clone(), schema);
        if required && !self.required.iter().any(|item| item == &name) {
            self.required.push(name);
        }
        self
    }

    pub fn string(self, name: impl Into<String>, required: bool) -> Self {
        self.property(name, json!({ "type": "string" }), required)
    }

    pub fn markdown(self, name: impl Into<String>, required: bool) -> Self {
        self.property(
            name,
            json!({ "type": "string", "format": "markdown" }),
            required,
        )
    }

    pub fn number(self, name: impl Into<String>, required: bool) -> Self {
        self.property(name, json!({ "type": "number" }), required)
    }

    pub fn boolean(self, name: impl Into<String>, required: bool) -> Self {
        self.property(name, json!({ "type": "boolean" }), required)
    }

    pub fn array(self, name: impl Into<String>, required: bool) -> Self {
        self.property(name, json!({ "type": "array" }), required)
    }

    pub fn build(self) -> Value {
        let mut schema = JsonMap::new();
        schema.insert("type".to_string(), json!("object"));
        schema.insert("properties".to_string(), Value::Object(self.properties));
        if !self.required.is_empty() {
            schema.insert("required".to_string(), json!(self.required));
        }
        Value::Object(schema)
    }
}

#[derive(Debug, Clone)]
pub struct CapabilityBuilder {
    capability: Capability,
}

impl CapabilityBuilder {
    pub fn new(id: impl Into<Id>, label: impl Into<String>, risk: Risk) -> Self {
        Self {
            capability: Capability::new(id, label, risk),
        }
    }

    pub fn read_only(id: impl Into<Id>, label: impl Into<String>) -> Self {
        Self::new(id, label, Risk::ReadOnly)
    }

    pub fn local_mutation(id: impl Into<Id>, label: impl Into<String>) -> Self {
        Self::new(id, label, Risk::LocalMutation)
    }

    pub fn external_effect(id: impl Into<Id>, label: impl Into<String>) -> Self {
        Self::new(id, label, Risk::ExternalEffect)
    }

    pub fn destructive(id: impl Into<Id>, label: impl Into<String>) -> Self {
        Self::new(id, label, Risk::Destructive)
    }

    pub fn description(mut self, value: impl Into<String>) -> Self {
        self.capability.description = Some(value.into());
        self
    }

    pub fn applicable_binding(mut self, value: impl Into<String>) -> Self {
        self.capability.applicable_binding = Some(value.into());
        self
    }

    pub fn input_schema(mut self, value: Value) -> Self {
        self.capability.input_schema = Some(value);
        self
    }

    pub fn field(mut self, key: impl Into<String>, value: Value) -> Self {
        self.capability.extra.insert(key.into(), value);
        self
    }

    pub fn build(self) -> Capability {
        self.capability
    }
}

#[derive(Debug, Clone)]
pub struct UnitBuilder {
    unit: Unit,
}

impl UnitBuilder {
    pub fn new(id: impl Into<Id>, kind: impl Into<String>) -> Self {
        Self {
            unit: Unit::new(id, kind),
        }
    }

    pub fn canonical(id: impl Into<Id>, kind: CanonicalUnitKind) -> Self {
        Self::new(id, kind.as_str())
    }

    pub fn facts(id: impl Into<Id>) -> Self {
        Self::canonical(id, CanonicalUnitKind::Facts)
    }

    pub fn table(id: impl Into<Id>) -> Self {
        Self::canonical(id, CanonicalUnitKind::Table)
    }

    pub fn list(id: impl Into<Id>) -> Self {
        Self::canonical(id, CanonicalUnitKind::List)
    }

    pub fn text(id: impl Into<Id>) -> Self {
        Self::canonical(id, CanonicalUnitKind::Text)
    }

    pub fn media(id: impl Into<Id>) -> Self {
        Self::canonical(id, CanonicalUnitKind::Media)
    }

    pub fn log(id: impl Into<Id>) -> Self {
        Self::canonical(id, CanonicalUnitKind::Log)
    }

    pub fn terminal(id: impl Into<Id>) -> Self {
        Self::canonical(id, CanonicalUnitKind::Terminal)
    }

    pub fn select(id: impl Into<Id>) -> Self {
        Self::canonical(id, CanonicalUnitKind::Select)
    }

    pub fn form(id: impl Into<Id>) -> Self {
        Self::canonical(id, CanonicalUnitKind::Form)
    }

    pub fn capabilities_unit(id: impl Into<Id>) -> Self {
        Self::canonical(id, CanonicalUnitKind::Capabilities)
    }

    pub fn description(mut self, value: impl Into<String>) -> Self {
        self.unit.description = Some(value.into());
        self
    }

    pub fn title(self, value: impl Into<String>) -> Self {
        self.field("title", json!(value.into()))
    }

    pub fn source_binding(self, value: impl Into<String>) -> Self {
        self.field(FIELD_SOURCE_BINDING, json!(value.into()))
    }

    pub fn count_binding(self, value: impl Into<String>) -> Self {
        self.field(FIELD_COUNT_BINDING, json!(value.into()))
    }

    pub fn value_binding(self, value: impl Into<String>) -> Self {
        self.field(FIELD_VALUE_BINDING, json!(value.into()))
    }

    pub fn session_id_binding(self, value: impl Into<String>) -> Self {
        self.field(FIELD_SESSION_ID_BINDING, json!(value.into()))
    }

    pub fn title_binding(self, value: impl Into<String>) -> Self {
        self.field(FIELD_TITLE_BINDING, json!(value.into()))
    }

    pub fn status_binding(self, value: impl Into<String>) -> Self {
        self.field(FIELD_STATUS_BINDING, json!(value.into()))
    }

    pub fn reference_binding(self, value: impl Into<String>) -> Self {
        self.field(FIELD_REFERENCE_BINDING, json!(value.into()))
    }

    pub fn reference_kind(self, value: impl Into<String>) -> Self {
        self.field(FIELD_REFERENCE_KIND, json!(value.into()))
    }

    pub fn default_presentation(self, value: PresentationLevel) -> Self {
        self.field(FIELD_DEFAULT_PRESENTATION, json!(value.as_str()))
    }

    pub fn form_field(mut self, field: FormField) -> Self {
        let entry = self
            .unit
            .extra
            .entry(FIELD_FIELDS.to_string())
            .or_insert_with(|| Value::Array(Vec::new()));
        if !entry.is_array() {
            *entry = Value::Array(Vec::new());
        }
        if let Value::Array(fields) = entry {
            fields.push(json!(field));
        }
        self
    }

    pub fn submit_capability(self, id: impl Into<Id>) -> Self {
        self.field(FIELD_SUBMIT_CAPABILITY, json!(id.into()))
    }

    pub fn capabilities<I, S>(self, ids: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let ids: Vec<String> = ids.into_iter().map(Into::into).collect();
        self.field(FIELD_CAPABILITIES, json!(ids))
    }

    pub fn row_capabilities<I, S>(self, ids: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let ids: Vec<String> = ids.into_iter().map(Into::into).collect();
        self.field("row_capabilities", json!(ids))
    }

    pub fn input_binding(mut self, name: impl Into<String>, binding: impl Into<String>) -> Self {
        let entry = self
            .unit
            .extra
            .entry(FIELD_INPUT_BINDINGS.to_string())
            .or_insert_with(|| Value::Object(JsonMap::new()));
        if !entry.is_object() {
            *entry = Value::Object(JsonMap::new());
        }
        if let Value::Object(bindings) = entry {
            bindings.insert(name.into(), json!(binding.into()));
        }
        self
    }

    pub fn interaction(mut self, event: InteractionEvent, effect: InteractionEffect) -> Self {
        let entry = self
            .unit
            .extra
            .entry(FIELD_INTERACTIONS.to_string())
            .or_insert_with(|| Value::Object(JsonMap::new()));
        if !entry.is_object() {
            *entry = Value::Object(JsonMap::new());
        }
        if let Value::Object(events) = entry {
            let effects = events
                .entry(event.as_str().to_string())
                .or_insert_with(|| Value::Array(Vec::new()));
            if !effects.is_array() {
                *effects = Value::Array(Vec::new());
            }
            if let Value::Array(items) = effects {
                items.push(effect.into_value());
            }
        }
        self
    }

    pub fn on_activate(self, effect: InteractionEffect) -> Self {
        self.interaction(InteractionEvent::Activate, effect)
    }

    pub fn on_select(self, effect: InteractionEffect) -> Self {
        self.interaction(InteractionEvent::Select, effect)
    }

    pub fn on_submit(self, effect: InteractionEffect) -> Self {
        self.interaction(InteractionEvent::Submit, effect)
    }

    pub fn activate_capability(mut self, id: impl Into<Id>) -> Self {
        self.unit.activate_capability = Some(id.into());
        self
    }

    pub fn primary_capability(mut self, id: impl Into<Id>) -> Self {
        self.unit.primary_capability = Some(id.into());
        self
    }

    pub fn fallback(mut self, id: impl Into<Id>) -> Self {
        self.unit.fallback = Some(id.into());
        self
    }

    pub fn salience(mut self, salience: Salience) -> Self {
        self.unit.salience = Some(salience);
        self
    }

    pub fn field(mut self, key: impl Into<String>, value: Value) -> Self {
        self.unit.extra.insert(key.into(), value);
        self
    }

    pub fn build(self) -> Unit {
        self.unit
    }
}

#[derive(Debug, Clone)]
pub struct LayoutBuilder {
    layout: Layout,
}

impl LayoutBuilder {
    pub fn new<I, S>(id: impl Into<Id>, members: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<Id>,
    {
        Self {
            layout: Layout::new(id, members.into_iter().map(Into::into).collect()),
        }
    }

    pub fn description(mut self, value: impl Into<String>) -> Self {
        self.layout.description = Some(value.into());
        self
    }

    pub fn overlay(mut self, value: bool) -> Self {
        self.layout.overlay = value;
        self
    }

    pub fn relation(self, value: LayoutRelation) -> Self {
        self.field(FIELD_RELATION, json!(value.as_str()))
    }

    pub fn role(self, value: LayoutRole) -> Self {
        self.field(FIELD_ROLE, json!(value.as_str()))
    }

    pub fn salience(mut self, salience: Salience) -> Self {
        self.layout.salience = Some(salience);
        self
    }

    pub fn primary(self) -> Self {
        self.salience(Salience::new().prominence(Prominence::Primary))
    }

    pub fn subtle(self) -> Self {
        self.salience(Salience::new().prominence(Prominence::Subtle))
    }

    pub fn state(self, state: SalienceState) -> Self {
        self.salience(Salience::new().state(state))
    }

    pub fn field(mut self, key: impl Into<String>, value: Value) -> Self {
        self.layout.extra.insert(key.into(), value);
        self
    }

    pub fn build(self) -> Layout {
        self.layout
    }
}

#[derive(Debug, Clone, Default)]
pub struct SectionBuilder {
    facts: JsonMap,
    capabilities: Vec<Capability>,
    units: Vec<Unit>,
    layouts: Vec<Layout>,
    root_layout: Option<Id>,
}

impl SectionBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn title(self, value: impl Into<String>) -> Self {
        self.fact("title", json!(value.into()))
    }

    pub fn description(self, value: impl Into<String>) -> Self {
        self.fact("description", json!(value.into()))
    }

    pub fn fact(mut self, key: impl Into<String>, value: Value) -> Self {
        self.facts.insert(key.into(), value);
        self
    }

    pub fn capability(mut self, capability: Capability) -> Self {
        self.capabilities.push(capability);
        self
    }

    pub fn unit(mut self, unit: Unit) -> Self {
        self.units.push(unit);
        self
    }

    pub fn layout(mut self, layout: Layout) -> Self {
        self.layouts.push(layout);
        self
    }

    pub fn layouts<I>(mut self, layouts: I) -> Self
    where
        I: IntoIterator<Item = Layout>,
    {
        self.layouts.extend(layouts);
        self
    }

    pub fn root_layout(mut self, id: impl Into<Id>) -> Self {
        self.root_layout = Some(id.into());
        self
    }

    fn into_value(self) -> Result<Value, AgentFirstUiError> {
        let mut section = self.facts;
        if !self.capabilities.is_empty() {
            section.insert(
                "capabilities".to_string(),
                serde_json::to_value(self.capabilities)?,
            );
        }
        if !self.units.is_empty() {
            section.insert("units".to_string(), serde_json::to_value(self.units)?);
        }
        if !self.layouts.is_empty() {
            section.insert("layouts".to_string(), serde_json::to_value(self.layouts)?);
        }
        if let Some(root_layout) = self.root_layout {
            section.insert("root_layout".to_string(), json!(root_layout));
        }
        Ok(Value::Object(section))
    }
}

#[derive(Debug, Clone, Default)]
pub struct SpaceBuilder {
    sections: JsonMap,
}

impl SpaceBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn section(
        mut self,
        owner: impl Into<String>,
        section: SectionBuilder,
    ) -> Result<Self, AgentFirstUiError> {
        let owner = owner.into();
        if self.sections.contains_key(&owner) {
            return Err(AgentFirstUiError::DuplicateIdentifier { identifier: owner });
        }
        self.sections.insert(owner, section.into_value()?);
        Ok(self)
    }

    pub fn raw_section(
        mut self,
        owner: impl Into<String>,
        section: Value,
    ) -> Result<Self, AgentFirstUiError> {
        let owner = owner.into();
        if self.sections.contains_key(&owner) {
            return Err(AgentFirstUiError::DuplicateIdentifier { identifier: owner });
        }
        self.sections.insert(owner, section);
        Ok(self)
    }

    pub fn build_unchecked(self) -> Result<Space, AgentFirstUiError> {
        Space::try_from_value(Value::Object(self.sections))
    }

    pub fn build(self) -> Result<Space, AgentFirstUiError> {
        let space = self.build_unchecked()?;
        validate_space(&space)?;
        Ok(space)
    }
}

pub fn layout<I, S>(id: impl Into<Id>, members: I) -> Layout
where
    I: IntoIterator<Item = S>,
    S: Into<Id>,
{
    Layout::new(id, members.into_iter().map(Into::into).collect())
}

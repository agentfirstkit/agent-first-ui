# agent-first-ui — Rust library

The Rust library for the Agent-First User Interface protocol — a way for a tool to
publish what is true and what can be done as data into a shared **space**, so any
viewer you trust renders the interface and nothing the tool publishes can run on
your machine.

This crate is protocol logic only: the space and message types, well-formedness
validation, binding resolution, JSON Patch (RFC 6902) helpers, request preparation,
a space store, projection materialization helpers, reports, and replay utilities.
It contains no rendering code — no viewer, web server, terminal, or browser assets
— so a tool can depend on it without pulling any of that in.

## SDK helpers

- `SpaceBuilder`, `SectionBuilder`, `UnitBuilder`, `CapabilityBuilder`, and
  `InputSchemaBuilder` for producer code that should not hand-assemble every
  `serde_json::Map`.
- `record_path_for_unit_row` and `space_binding_to_pointer` to bridge canonical
  unit `source_binding` values to request `record_path` JSON Pointers.
- `OwnerRequest`, `OwnerRequestResult`, `RequestExecutionStatus`, and
  `OwnerRequestHandler` for the owner-side request execution handoff.
- `validate_space_profile(space, ValidationProfile::ReferenceHost)` for optional
  reference-host warnings while base protocol validation stays open-ended.
- `compose_spaces`, `ensure_owner_available`, and `insert_owner_section` for
  collision-checked owner-section composition without rewriting owner-local ids.
- `compose::compose_messages` and `compose::ComposeSource` for namespaced
  composition of complete snapshot messages while preserving owner authority.
- `project_space_for`, `project_snapshot_for`, `ProjectionPatch`, and
  `ProjectionSet` for owner/transport code that materializes one delivered
  projection from a base space plus out-of-band audience overlays.
- `CanonicalUnitKind`, `FormField`, `FormFieldType`, `FormFieldFormat`,
  `ReferenceEnvelope`, `PresentationLevel`, interaction/layout enums, and field
  constants for the common view-hint profile used by reference viewers. Canonical
  `form` values become direct provided request input; `ReferenceEnvelope` is the
  viewer-synthesized inert context-sink output, not a source-record requirement.
- `actor_state_pointer` and `set_actor_state_operation` for translating
  viewer-owned interaction state into safe emits under the current actor section.

```bash
cargo add agent-first-ui
```

The model — Space, Facts, Emit, Request, Capability, Hint, and risk — and the
normative specification and conformance fixtures live in the repository:
<https://github.com/agentfirstkit/agent-first-ui>

## License

MIT

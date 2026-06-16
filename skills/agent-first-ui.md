---
name: agent-first-ui
description: Apply Agent-First UI conventions when designing, reviewing, producing, validating, composing, or implementing AFUI operation-surface documents, patches, user_action events, trusted host renderers, or Rust SDK support code.
---

# Agent-First UI Skill

AFUI is safe operation surfaces as data. A producer sends structured, non-executable UI data; a trusted host renderer turns it into a human interface; user intent returns as structured events.

Use this skill whenever working with AFUI documents, fixtures, host renderers, schema, SDK helpers, examples, or composition tooling.

## Core Boundary

Ordinary AFUI is data-only. AFUI documents, state, theme, and patches MUST NOT carry JavaScript, CSS, HTML fragments, WASM, package manifests, dependency graphs, build scripts, shell commands, callback/function bodies, expressions, or remote component/plugin URLs.

Review rule: if a proposed AFUI flow requires installing packages, compiling a frontend, downloading a component, running a script, or trusting document-supplied code, it is not ordinary AFUI. It belongs to a separate signed/sandboxed product or extension system.

## Roles

| Role | Responsibility |
| --- | --- |
| Producer | Emits `ui_snapshot`/patch data and handles `user_action` events. |
| Host | Renders AFUI, enforces safety policy, resolves bindings, redacts secrets, and captures user intent. |
| Composer | Namespaces and combines complete producer snapshots into one ordinary `ui_snapshot`. |

Product model:

- A: single producer surface is first-class and required now.
- B: multi-producer composition is compatible with A and uses ordinary snapshots.
- C: an agent dynamically inventing a full UI/workflow is not a first-stage goal.

## Two Layers

| Layer | Fields | Rule |
| --- | --- | --- |
| Programmatic control | `id`, `_bind`, action refs, `switch`, patches, `_secret`, `_uri`, `risk`, `undoable`, `input_schema` | Deterministic and host-enforced |
| Natural-language description | `description` | Advisory only |
| Theme | `theme` flat token map | Advisory appearance only |
| Presentation hints | `preview_len`, `labels`, unit/timestamp formatting | Advisory; host applies or may override |

Programmatic control always wins. `description`, `theme`, and presentation hints must never trigger, relax, waive, or override safety, binding, redaction, URI, action, or patch behavior.

`state` holds domain **facts**, not presentation. Truncated previews, pluralized count labels, formatted timestamps, and detail text stitched from several fields are render guidance — expressed as advisory view hints, applied by the host. One fact (`message_count`) must not be shipped as a baked spelling (`"3 messages"`). Data is referenced; presentation is applied by the host. A host MAY use `validate_state_bindings` to check that every `state:` binding resolves against the shipped state.

## Identity and Document Shape

Every `Surface`, `Screen`, `View` (including nested sub-views), and `Action` has a stable document-unique id. References use ids, not positions.

Minimal snapshot:

```json
{
  "afui": "0.1",
  "type": "ui_snapshot",
  "document": {
    "surface": { "id": "order_ops", "title": "Order Operations" },
    "screens": [ { "id": "main", "views": [] } ],
    "actions": []
  },
  "state": {}
}
```

`surface` is the human-facing operation surface metadata. Do not use the old `app` field.

## Messages

| Type | Direction | Payload |
| --- | --- | --- |
| `ui_snapshot` | producer -> host | `document`, `state`, optional `theme` |
| `state_patch` | producer -> host | JSON Patch over `state` |
| `document_patch` | producer -> host | JSON Patch over `document` |
| `theme_patch` | producer -> host | JSON Patch over `theme` |
| `user_action` | host -> handler | `action_id`, optional `input` |

## Bindings

A key ending `_bind` is a scoped binding string: `<scope>:<path>`. Scope is `state`, `record`, or `input`; omitted scope means `state`.

Examples:

```json
{
  "source_bind": "state:orders",
  "order_id_bind": "record:id",
  "prompt_bind": "input:."
}
```

Bindings are read-only. User edits emit events; handlers decide state changes. Never invent extra expression languages.

## Markers

| Marker | Meaning |
| --- | --- |
| `_bind` | Scoped read indirection |
| `_secret` | Redact by default |
| `_uri` | Untrusted locator resolved only by host policy |

Unknown suffixes are inert literals unless introduced by a new AFUI version or explicit negotiated extension. `_uri` is never a code entrypoint.

## Actions and Risk

Actions are semantic affordances. Views reference them by id with `action`, `*_action`, `actions`, or `*_actions`.

| Risk | Meaning |
| --- | --- |
| `read_only` | Observes or navigates only |
| `local_mutation` | Changes host-local, draft, UI, or file state |
| `external_effect` | Reaches a service, network, payment rail, or other actor |
| `destructive` | Can cause irreversible or user-harming effects |

Rules:

- `risk` is required and drives host safety gates.
- Missing/unknown risk is treated as `destructive`.
- There is no `requires_confirmation`; confirmation is host runtime policy.
- `description` can explain the risk but cannot lower it.
- Unresolved action references are integrity errors and fail safe.

## Action Input Assembly

For each property `P` in `Action.input_schema.properties`, the host resolves a source-view field named `P_bind` if present. Missing required fields are collected by host-native UI before emitting `user_action`.

Local preparation statuses:

- `ready`: host may emit `user_action`.
- `needs_input`: collect named fields, then retry preparation.
- `error`: unresolved/invalid action; emit nothing.

## Patches

Use RFC 6902 JSON Patch.

- `state_patch.patch` targets opaque state.
- `document_patch.patch` targets the canonical document encoding.
- `theme_patch.patch` targets advisory theme tokens.
- Patch failure keeps prior data.
- Document patches that edit existing nodes or ordered sibling arrays should guard positional paths with `test` operations asserting relevant ids.

## Producer Checklist

1. Support `ui snapshot` as the minimum integration shape.
2. Emit complete snapshots before optimizing with patches.
3. Keep AFUI data-only: no commands/code/builds/components in documents.
4. Give every document node a stable unique id.
5. Put safety and wiring in programmatic fields, not prose.
6. Use `_bind` for reads, selection, and action arguments.
7. Declare every action with truthful `risk`.
8. Put required user-provided fields in `input_schema.required`.
9. Suffix secrets with `_secret` and locators with `_uri`.
10. Use JSON Patch for updates and `test` guards for document identity edits.

## Trusted Host Renderer Checklist

When building or reviewing a host renderer:

1. Render a usable surface from `document + state` without generated code.
2. Resolve `_bind` deterministically and never execute expressions.
3. Redact `_secret` by default.
4. Enforce URI trust policy before resolving `_uri`.
5. Resolve action ids and fail safe on unresolved refs.
6. Assemble action input and collect missing required fields.
7. Present distinct risks distinctly and route effects through host gates.
8. Apply JSON Patch atomically.
9. Render unknown `kind` through fallback.
10. Never install, download, compile, or execute because a document asked.

Host appearance adapters may choose shells, classes, wrappers, and native widgets. They must not own binding, patching, redaction, URI policy, action input, risk gates, or fallback. Domain-specific mappings are host capabilities installed outside ordinary AFUI.

## Composition Guidance

Composition is a tooling/host step that outputs an ordinary `ui_snapshot`; it is not a new wire type.

When combining producers:

- Give each source a namespace such as `ops`, `crm`, or `billing`.
- Prefix surface/screen/view/action ids with that namespace.
- Rewrite action references to the prefixed action ids.
- Mount state under `state.<namespace>`.
- Rewrite `state:` bindings into the namespace.
- Leave `record:` and `input:` bindings local to the rendered view/action flow.
- Emit `user_action.action_id` with the prefix so a router can dispatch to the owning source.

Do not introduce an AFUI `fragment` message type in v1.

## Terminal Capability

A `terminal` view embeds a live shell. Treat it as a host capability, not a document feature:

- Use it for an interactive shell the operator (or agent) drives; use read-only `log` when you only display output.
- A terminal view references a host session and presentation only. Never put a command, args, or script field on it.
- Sessions are created by the host, not declared by the document. In v1 the host opens one terminal session per terminal view id at startup; PTY is only an internal backend term.
- Scrollback never enters AFUI state or patches; only session metadata does.
- The capability is opt-in and gated by the host (operator flag, loopback, token). When off or unsupported, the terminal renders as an inert placeholder.

## Record Reference

A `record_list` or `table` view MAY set `ref_bind` (a `record:`-scoped binding to each row's agent-facing locator, e.g. `record:message_id`) and an optional `ref_label`. A host MAY surface a per-row copy-to-clipboard affordance so a person can paste the locator when directing an agent. It falls back to the row id, hosts may ignore it, and it is presentation only — copy, never execute or send. Use the same locator token your CLI consumes.

## Markdown Body and Input

When a view's body text is markdown (case notes, summaries), set `body_format: "markdown"` on the view; a host MAY render a safe markdown subset and falls back to raw text otherwise. When an action input is markdown, give its `input_schema` property `"format": "markdown"`; a host MAY collect it with a markdown editor. Both are advisory presentation hints — the stored value is always a plain string, and the producer never sends rendered HTML. Set `body_format` only on views whose body is genuinely markdown; leave plain bodies (raw email text) unmarked so a host does not misrender them.

## Rust Packages

The `agent-first-ui` crate (`rust/`) is the lightweight Rust SDK for protocol, producer, and handler logic. It exports:

| Module | Exports |
| --- | --- |
| `types` | `AfuiMessage`, `Snapshot`, `Document`, `Surface`, `Screen`, `View`, `Action`, `Risk`, `State`, `Theme` |
| `binding` | `Binding`, `BindingScope`, `BindingContext`, `resolve_binding`, `get_dot_path` |
| `action` | `prepare_user_action`, `assemble_action_input`, `action_ids_from_view`, `redacted_input_for_display` |
| `patch` | `PatchOperation`, `apply_patch`, `parse_pointer` |
| `runtime` | `apply_afui_message` |
| `host` | `HostStore`, `PrepareActionRequest`, `prepare_action_against_snapshot` |
| `report` | `inspect_snapshot`, `replay_jsonl`, `replay_messages`, `AfuiReport`, `ReplayReport` |
| `validation` | `validate_document`, `validate_state_bindings`, `is_valid_id` |

A producer depends only on this SDK to emit and handle AFUI data — never on a host. A standalone convenience mode (`mytool ui`) may bundle or locate a trusted host, but that is tool-side wiring, outside the protocol.

## Anti-Patterns

| Bad | Good | Why |
| --- | --- | --- |
| `document.app` | `document.surface` | AFUI describes a surface, not a product bundle |
| Generated frontend project | AFUI JSON snapshot | Ordinary AFUI is data-only |
| Remote component URL | Preinstalled host capability or fallback | Unknown kind cannot execute code |
| `requires_confirmation: true` | `risk: "destructive"` plus host gate | Confirmation is runtime policy |
| Secret in prose | `_secret` suffixed state field | Hosts redact by suffix |
| URI followed directly | `_uri` plus host policy | Producers are untrusted |
| Behavior hidden in `description` | Programmatic field or extension | Prose cannot control safety |
| Baked display string in `state` | Fact in `state` + advisory view hint | Presentation is host-applied guidance, not data |

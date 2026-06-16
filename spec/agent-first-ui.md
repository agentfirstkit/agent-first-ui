# Agent-First UI v0.1

**Safe operation surfaces as data.** A trusted host renderer turns an AFUI
`document` plus `state` into a human-facing operation surface. User actions are
returned as structured events. AFUI is a semantic model with canonical JSON
encoding; it is not a frontend project, remote component system, website bundle,
or code transport.

## Status

This is the v0.1 specification. The core is intentionally small: identity,
scoped bindings, action references, risk labels, redaction/trust markers, and
patches. View `kind` is open and semantic; AFUI does not define a closed widget
catalog or per-kind component schema.

## Trust Boundary

Roles:

- **Producer** creates `ui_snapshot`, `state_patch`, `document_patch`, and
  `theme_patch` messages.
- **Host renderer** is trusted local software controlled by the user or
  operator. It renders AFUI, enforces host policy, and emits `user_action`.
- **Handler** receives `user_action` and returns updated messages.
- **Agent** may be a producer, assistant, or handler, but is never trusted to
  bypass host policy.

The producer describes workflow data. The host controls rendering, URI policy,
secret redaction, action gates, installed capabilities, and execution. A
producer-supplied AFUI document is untrusted data.

### No Embedded Code

AFUI values are data only. A conforming AFUI document, state, theme, or patch
MUST NOT carry JavaScript, CSS, HTML fragments, WASM, package manifests,
dependency graphs, build scripts, shell commands, callback/function bodies,
expressions, or remote component/plugin URLs.

Unknown `kind` values MUST render through deterministic fallback. They MUST NOT
cause a host to install, download, compile, or execute code. `_uri` values are
untrusted locators and MUST be resolved only through host URI policy; they are
not script, component, or extension entrypoints.

If a product needs pixel-perfect branding, arbitrary interaction code, or a
remote component model, it has left ordinary AFUI. Such code belongs to a
separate signed/sandboxed product or extension system. AFUI may be embedded in
that system, but ordinary AFUI does not define it.

### User-Owned Host Customization

The no-code rule applies to AFUI values supplied by producers. It does not
forbid a user, operator, or their local agent from editing, replacing, or
extending the trusted host renderer outside the AFUI document. Those changes are
host/product code, not AFUI protocol data.

A host MAY provide local asset overrides, an ejectable host directory,
preferences, plugins, or full renderer replacement. Those mechanisms may use any
implementation technology the user trusts. A conforming AFUI document MUST NOT
request, select, download, or configure those code paths.

AFUI therefore does not try to enumerate every possible visual or interaction
preference. Common preferences MAY have host-specific config, but arbitrary
changes belong in user-owned host files or extensions. If a customized host
still claims AFUI conformance, it remains responsible for binding resolution,
JSON Patch semantics, redaction, URI policy, action preparation, risk gates, and
fallback rendering.

## Two Layers

Every AFUI document has exactly two layers:

1. **Programmatic control** - deterministic, host-enforced structure with
   safety consequences: ids, bindings, action references, `switch`, patch
   targets, `_secret`, `_uri`, `risk`, `undoable`, `input_schema`.
2. **Natural-language description** - optional `description` text for humans
   and host renderers. It can explain intent, but it cannot trigger, relax, or
   override any programmatic control.

Classification test: if ignoring or lying about a value can break wiring, leak
data, perform the wrong effect, or hurt a user, it belongs in programmatic
control. Otherwise it belongs in `description`.

Precedence: programmatic control always wins. A host that lets `description`
change binding, redaction, URI, risk, action, or patch behavior is
non-conformant.

## Value Space and Encodings

AFUI is defined over this value space:

```ts
type Value = null | boolean | number | string | Value[] | { [key: string]: Value };
```

No functions, host objects, inline binary blobs, `NaN`, `Infinity`, comments, or
cycles are allowed. JSON is the mandatory canonical encoding. Other encodings
are conformant only if they round-trip losslessly to canonical JSON and remain
schema-free/self-describing enough for a generic host renderer to recover keys,
markers, ids, scopes, and values.

Self-describing binary encodings such as CBOR or MessagePack are compatible with
that rule. A typed envelope around a self-describing payload is compatible. A
per-view typed component vocabulary as the only representation is not
compatible, because it turns AFUI into a closed component catalog.

## Envelope

Every message has `afui` and `type`.

```json
{ "afui": "0.1", "type": "ui_snapshot", "document": {}, "state": {} }
```

| Type | Direction | Payload |
| --- | --- | --- |
| `ui_snapshot` | producer -> host | `document`, `state`, optional `theme` |
| `state_patch` | producer -> host | `patch` over `state` |
| `document_patch` | producer -> host | `patch` over `document` |
| `theme_patch` | producer -> host | `patch` over `theme` |
| `user_action` | host -> handler | `action_id`, optional `input` |

A `ui_snapshot` must be enough to render a safe usable surface. Patches keep it
fresh without resending everything.

## Document and State

```ts
type Document = { surface: Surface, screens: Screen[], actions?: Action[] };
type Surface = { id: Id, title: string, description?: string, version?: string };
type Screen = { id: Id, title?: string, description?: string, views: View[] };
type View = {
  id: Id,
  kind: string,
  description?: string,
  header?: View,
  footer?: View,
  empty_view?: View,
  default?: View,
  cases?: { [discriminant: string]: View },
  [key: string]: Value
};
```

The document is an identity graph. Every `Surface`, `Screen`, `View` (including
composed child views), and `Action` has a stable document-unique `id`. Action
references address ids, never array positions.

`state` is opaque task-shaped data owned by the producer/handler. State is not
an identity graph and may use positional paths naturally.

State carries domain **facts**, not presentation. A producer must not pre-render
display strings into state — truncated previews, pluralized count labels,
locale-formatted timestamps, or text stitched together from several fields are
not facts. Those are *render guidance*: the producer expresses them as advisory
view hints (see Presentation Hints) and the trusted host applies them and MAY
override them. Keeping one fact (`message_count`) out of many baked spellings
("3 messages") is what stops the data and its rendering from drifting apart, and
lets a different host present the same facts its own way. The split is the rule
the host relies on: data is referenced, never rendered into; presentation is
applied by the host, never shipped as data.

`kind` is an open semantic word, not a component name and not an enum. A host may
map known kinds to native controls, but unknown kinds must still render through
safe fallback.

## Markers

AFUI uses key suffixes for programmatic handling:

- `_bind` - a scoped binding string.
- `_secret` - sensitive value; host renderers MUST redact by default.
- `_uri` - untrusted host-resolved locator; host URI policy MUST allow it before
  it is resolved.

Unknown suffixes are inert literal data. New suffixes that change safety, trust,
or indirection semantics require a new AFUI version or explicit negotiated
extension so older hosts cannot silently miss them.

AFDATA-style unit suffixes may be used on scalar keys, such as `_ms`, `_s`,
`_epoch_ms`, `_bytes`, `_percent`, `_usd_cents`, `_sats`, and `_px`.

## Binding

A binding is the only indirection primitive. The key carries the marker: a key
ending `_bind` has a string value `<scope>:<path>`. If the scope is omitted, it
is `state`.

| Scope | Resolves against |
| --- | --- |
| `state` | the global `state` object |
| `record` | the contextual record for a row/item/point/entity |
| `input` | the host's live input buffer for the enclosing view |

Examples:

```json
{ "source_bind": "state:orders", "order_id_bind": "record:id", "text_bind": "input:." }
```

Paths are dot paths with no escape rule and no `:`. Bindings are read-only for
the host renderer. User edits and intent emit events; handlers decide whether
state changes.

## Actions and Risk

```ts
type Action = {
  id: Id,
  label: string,
  risk: "read_only" | "local_mutation" | "external_effect" | "destructive",
  description?: string,
  undoable?: boolean,
  input_schema?: object
};
```

Actions are semantic affordances, not buttons. A view references actions by id
using marker-named fields: `action`, `*_action`, `actions`, or `*_actions`.
Every reference must resolve to a declared action. If a reference is unresolved,
the host fails safe, surfaces the integrity error, and treats the control as
`destructive`.

Risk meanings:

| Risk | Meaning |
| --- | --- |
| `read_only` | observes or navigates only |
| `local_mutation` | changes host-local, UI, draft, or file state |
| `external_effect` | reaches a network, service, payment rail, or other actor |
| `destructive` | can cause irreversible or user-harming effects |

`risk` is required on every action. Missing or unrecognized risk is treated as
`destructive`. Confirmation style is runtime host policy derived from risk plus
user/operator preference. There is no `requires_confirmation` field.

When a user triggers action `A`, the host prepares `user_action.input` from
bindings on the source view. For each property `P` in `A.input_schema.properties`,
a source field named `P_bind` is resolved if present. Missing required fields
are collected by host-native UI before `user_action` is emitted.

An input property MAY carry an advisory `format` (for example
`{ "type": "string", "format": "markdown" }`). It is presentation guidance for
how the host collects that field — a host MAY offer a richer editor such as a
markdown editor — and never changes the value, which is still a plain string.
Hosts MAY ignore it and collect the field with a basic control.

## Patches

`state_patch`, `document_patch`, and `theme_patch` use JSON Patch (RFC 6902).
Patch application is atomic from the host's perspective: if any operation fails,
the prior snapshot is retained.

`state_patch.patch` targets opaque `state` and may use positional paths.
`document_patch.patch` targets the canonical document encoding. Because document
identity is semantic, a producer that edits existing document nodes or ordered
sibling arrays MUST guard positional paths with `test` operations that assert the
relevant `id` before mutation.

Examples:

```json
{ "op": "test", "path": "/screens/0/id", "value": "main" }
{ "op": "replace", "path": "/screens/0/views/0/kind", "value": "table" }
```

A patch that orphans an action reference or creates duplicate ids is invalid
because the resulting document is not well-formed.

## Theme

`theme` is an optional flat advisory token map. Values are strings or numbers.
Theme can guide colors, spacing, radii, typography, and host appearance, but it
is not programmatic control. A wrong theme can make the surface ugly; it cannot
bypass redaction, URI policy, risk gates, or action integrity.

Hosts may honor common tokens such as `color_primary`, `color_surface`,
`color_surface_raised`, `color_on_surface`, `color_border`, `color_risk_*`,
`space_*_px`, `radius_*_px`, `font_family`, and `font_size_base_px`. Unknown
tokens are ignored silently.

## Presentation Hints

A view MAY carry advisory presentation hints that tell a host how the producer
recommends rendering a fact. Hints are the render-guidance counterpart to theme:
the producer owns its domain and gives the best guidance, but a host MAY honor,
adjust, or ignore any hint. Like theme and `description`, hints are **not**
programmatic control — a wrong or ignored hint can only change how the surface
looks, never binding, redaction, URI, risk, action, or patch behavior. They are
why presentation does not belong baked into state: the producer recommends, the
host decides.

Hints describe presentation declaratively; they never carry expressions, code,
format callbacks, or markup. Hosts SHOULD recognize:

- `preview_len` (integer): when a view shows a shortened preview of a longer text
  field, truncate to about this many characters.
- `labels` (object): map a numeric field name to a label template, for example
  `{ "message_count": { "one": "{n} message", "other": "{n} messages" } }`. The
  host substitutes `{n}` with the field value and picks `one`/`other` by whether
  the value is 1. The producer supplies templates in the user's language; the host
  only fills in the number.
- `ref_bind` (binding) and `ref_label` (string): on a `record_list` or `table`
  view, `ref_bind` names each row's agent-facing locator (for example
  `record:message_id`). A host MAY expose a per-row copy-to-clipboard affordance
  labeled by `ref_label` (default "Copy id"), falling back to the row id when
  `ref_bind` is absent. This is presentation only: it places a locator on the
  clipboard so a person can paste it when directing an agent; it never executes,
  sends, or counts as an action.
- `body_format` (string): on a view with a body text field (such as
  `record_list`'s `body_key`), declares how the producer authored that text.
  `"markdown"` invites a host to render a safe markdown subset; any other value
  or its absence means plain text. A host that does not render markdown falls
  back to showing the raw text, which stays readable. The host renders the body
  it was given; this never changes the underlying fact.

A host MAY also format AFDATA-style unit-suffixed scalars and ISO-8601/RFC 3339
timestamp fields for display. The producer sends the raw fact (a count, an
`*_rfc3339` string); the host chooses the displayed form. Unknown hint keys are
inert, exactly like unknown suffixes.

## Reference Host Renderer Protocol

A reference host renderer MUST:

- Accept canonical JSON `ui_snapshot`, `state_patch`, `document_patch`, and
  `theme_patch` messages.
- Keep current `document`, `state`, and optional `theme` in memory.
- Apply JSON Patch atomically and validate the resulting document.
- Resolve `_bind` values against `state`, `record`, or `input`.
- Redact `_secret` values and enforce `_uri` policy before resolving locators.
- Resolve only declared actions; unresolved references fail safe.
- Present risk levels distinctly and route effects through host safety gates.
- Assemble action input deterministically and collect missing required fields.
- Render every screen and recursively composed view with stable ids.
- Render unknown kinds through deterministic fallback.
- Never execute code, follow blocked locators, hide integrity errors, or install
  capabilities because a document requested them.

A host SHOULD provide baseline mappings for common semantic kinds used by the
fixtures: `stats`, `table`, `detail`, `list`, `tree`, `tabs`, `input`, `log`,
`action_bar`, `switch`, `text`, `progress`, and `canvas` placeholder/reference
surfaces. These mappings are not a closed vocabulary.

Unknown-kind fallback:

1. Show `id`, `kind`, and `description` if present.
2. Resolve and safely display `_bind` fields.
3. Display arrays of objects as tables/lists, objects as details, and scalars as
   text.
4. Render action references with risk-aware controls.
5. Redact secrets, block disallowed locators, and surface integrity errors.

Host appearance adapters may choose shells, wrappers, native widgets, spacing,
classes, and risk colors. They are local trusted host code and must not own
binding resolution, JSON Patch, redaction, URI policy, action input assembly,
risk gates, or fallback.

A host SHOULD apply advisory presentation hints (`preview_len`, `labels`), MAY
format unit-suffixed scalars and timestamp facts for display, MAY surface a
per-row copy-to-clipboard affordance from `ref_bind`/`ref_label`, and MAY render
a `body_format: "markdown"` body as a safe markdown subset or collect a
`format: "markdown"` input with a markdown editor; these affect appearance only
and never override programmatic control. A host MAY check that
every `state:` binding in the document resolves against the snapshot's `state`
and surface unresolved bindings as integrity warnings; `record:` and `input:`
bindings resolve at render time and are not checked statically.

Preinstalled host capabilities may provide richer deterministic mappings for
open domain kinds such as a board, map, image surface, or terminal panel. An
AFUI document can use only capabilities already available in the host; it cannot
carry or fetch capability code.

## Safe Extension

AFUI grows by explicit versions or negotiated extensions, not by hidden behavior:

- Unknown `kind` -> safe fallback.
- Unknown suffix -> inert literal data.
- Unknown or absent `risk` -> `destructive`.
- Unresolved action -> integrity error and fail-safe presentation.
- Unsupported host capability -> fallback; never dynamic install.
- Unclear prose -> ignore for behavior; programmatic control wins.

## Terminal Capability (host-owned)

A `terminal` view is the canonical example of a preinstalled host capability. It
embeds an interactive shell pane while keeping the data-only document rule
intact:

- The view carries only a reference to a host session plus presentation:
  `session_bind` (defaults to `state:terminals.<view_id>.session_id`),
  `title_bind`, `status_bind`, and advisory `rows_bind`/`cols_bind`. It MUST NOT
  carry a command, args, or any code/script field; hosts reject `command`,
  `cmd`, `exec`, `run`, `program`, `entrypoint`, `args`, `argv` on a terminal
  view (in addition to the global code-transport blacklist).
- The terminal process lives in the trusted host (PTY is an internal backend detail), never in the
  document. Commands reach it only as host-channel input: a human's keystrokes,
  an agent's writes, or host-side code — never as an AFUI field.
- Terminal bytes (scrollback) travel on the host's own channel (e.g. SSE),
  **not** in AFUI state or patches. State holds only session metadata
  (`status`, `rows`, `cols`, `title`), and a host MAY expose that as a local
  overlay on its snapshot response without mutating producer-owned state.
- The capability is off by default and host-gated (operator opt-in, loopback,
  token). An unsupported or disabled terminal renders as an inert placeholder,
  per the fallback rule — never a dynamic install.

Multiple shells compose with existing kinds: a `tabs` view (tab bar) over
`terminal` children. The host owns the channel that carries terminal bytes; the
transport contract is a host implementation concern, not an AFUI field.

## Conformance Fixtures

`spec/fixtures/` contains worked examples for operation surfaces, not a widget
checklist:

- `tendril.afui.json` - CMN browser/control surface with input, switch, list,
  detail, tree, and record/input action args.
- `admin.afui.json` - operator dashboard with table, detail, log, secret, URI,
  and risk handling.
- `hud.afui.json` - game HUD-like operation surface with canvas reference,
  progress, log, switch, and risks.
- `image_editor.afui.json` - image editing operation surface with layer tree,
  canvas reference, destructive and undoable actions.
- `tendril.afui.jsonl` - lifecycle stream: snapshot, state patch, user action,
  and document patch.
- `*.patch.json` - standalone JSON Patch payloads.

The conformance question is: given a document the host has never seen, can it
produce a safe usable fallback while preserving every programmatic-layer rule?

## Deferred

Not part of v0:

- Computed expressions or predicate language.
- Multi-source bindings and advanced query selectors.
- Standard focus, selection-change, input-change, hover, drag, or ready events.
- Standard confirmation request/response wire flow.
- Full error object model.
- Permission/capability model beyond `risk`.
- Asset streaming/blob transport beyond `_uri` locators.
- Patch stream sequencing, acknowledgements, recovery, QoS, or channels.
- Concrete binary media-type specifications.
- Layout constraints beyond advisory theme tokens.
- Full JSON Schema constraints for `input_schema`.
- Complete branded product UI, arbitrary remote components, or code execution in
  producer-supplied AFUI data.

Explicitly not deferred but out of scope by principle: a closed widget catalog,
per-kind typed component schemas, and any AFUI mechanism that turns untrusted
surface data into executable code.

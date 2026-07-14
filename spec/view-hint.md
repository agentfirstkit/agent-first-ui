# Agent-First UI — the View Hint Profile

**An optional profile of AFUI [`0.2`](protocol.md).** The protocol carries advisory
hints and says only that a viewer *may use, adapt, or ignore* them. This profile is
the recommended, concrete way to interpret those hints so that independent viewers —
a web workbench, a terminal interface, a native app — converge on the same strong
default **without any AI**. It is the executable reference's contract, written down.

It is **not** part of the load-bearing protocol. Nothing here can override a fact, a
binding, a risk, or a capability, and a non-conforming viewer is still a conforming
*protocol* viewer. The normative protocol is [`protocol.md`](protocol.md); read it
first.

## Conformance

This profile is **opt-in** and uses conditional obligations:

- A viewer or owner **MAY** ignore this profile entirely. If it does, the protocol's
  own rules still hold: unknown kinds and unknown fields are inert and render through
  the safe deterministic fallback.
- **If** a viewer declares conformance to the View Hint Profile, it **MUST**
  interpret the canonical kinds and fields below as specified — so an owner that
  publishes to this profile gets the same surface from any conforming viewer.
- **If** an owner declares conformance, it **MUST** use the canonical field names and
  shapes below for the behavior it wants, rather than inventing parallel names. An
  owner may still add its own fields; conforming viewers treat unknown fields as
  inert.

Profile conformance adds rendering obligations only. It does not change the
protocol's ownership, request, risk, binding, redaction, locator, or fallback rules.
**A hint never widens authority.**

This profile shares the protocol's version string (`0.2`); it is not versioned
independently.

## Owner surface

An owner section may publish an advisory `surface` object:

```json
"surface": { "genre": "triage" }
```

`surface.genre` lets a viewer choose a shell before it inspects individual units.
It is purely advisory: a viewer may ignore it, and it cannot grant capabilities,
run code, change risk, choose URLs, or override any load-bearing protocol rule.
Conforming owners use one of these values:

| `genre` | Meaning |
| --- | --- |
| `triage` | A queue that feeds a focused reading or handling surface |
| `collection_admin` | Managing records in a collection |
| `monitor` | Status, health, alerts, and operational streams |
| `reader` | Reading-first content with supporting actions |
| `conversation` | Message or chat-like turns |
| `board` | Cards or work items moving through states |
| `console` | Operator command and diagnostics surface |

Unknown genres are inert to the protocol. The strict View Hint Profile validator
rejects them so producers that claim this profile converge on the same vocabulary;
non-conforming viewers remain valid protocol viewers by ignoring the field.

## Units

A **unit** is a recommended coherent slice of facts to present. Every unit is a
member of one flat pool, with a stable `id` and an open semantic `kind`. It may also
carry a `title`, a `description`, a `summary`, the kind-specific fields below, the
`capabilities` on it, an `activate_capability` (the capability triggered when the
unit — or one of its rows — is activated), a `primary_capability` (the suggested one
to emphasize), `interactions` (viewer intents for activate/select/submit), a
`salience` (how much it matters — see [Salience](#salience)), and, for a
non-canonical `kind`, a `fallback` (the id of a canonical unit to render instead).
`kind` is a semantic word,
**never a widget name**: the protocol defines no closed catalog, and the set below is
this *profile's* recommendation, not a protocol restriction.

Units do **not** nest inside one another. Composition is by **id reference**, two
ways only: a [layout](#layout) arranges units (and other layouts) by id, and the
`select` kind chooses one unit by id. There are no `header`, `footer`, `empty_unit`,
`default`, or inline `cases` unit slots — express a header as another unit in a
layout, an empty state as a `select` case.

The canonical kinds and the fields each one reads:

| `kind` | Reads | A conforming viewer renders |
| --- | --- | --- |
| `facts` | `source_binding` → one object, **else** `items: [{ label, value_binding }]` (scattered facts); `fields: [{ key, label }]` (else all keys); `capabilities` | a key/value detail of one record, or a row of label/value glances; capability references remain discoverable context, not automatic controls |
| `table` | `source_binding` → records; `columns: [{ key, label }]`; `reference_binding` / `reference_kind`; `row_capabilities`; `activate_capability` | a table; cell = `record[col.key]` (dot path); each row carries transferable context, discoverable contextual capabilities, and an optional non-button activate intent |
| `list` | `source_binding` → records; per row `title_key` / `subtitle_key` / `body_key`, `reference_binding` / `reference_kind`, body (`body_format: "markdown"`), `fields`, `row_capabilities`, `activate_capability` | a vertically scannable list of records with the same inert reference and contextual-capability semantics as `table` |
| `text` | `text` or `text_binding`; `body_format: "markdown"` | a block of text, markdown-rendered when asked |
| `media` | `source_binding` → a `_uri`; `media_type` (`image` / `video` / `audio`); `alt` / `alt_binding`; `activate_capability` | a standalone image, video, or audio resource; the `_uri` is resolved only through the viewer's locator policy and **not auto-loaded by default**; `alt` is shown when it is not loaded and read aloud by a voice viewer |
| `log` | `source_binding` → lines; `level_key`, `time_key`, `message_key` | a log stream; each line's level maps to a `state` (`info` / `warning` / `danger` …) — see [Salience](#salience) |
| `terminal` | `session_id_binding`, `title_binding`, `status_binding` | a reference to a **host-owned** terminal session — data only; the PTY runs in the trusted host, never from space data |
| `select` | `selector_binding` → discriminant; `cases: { value: unit-id }`; `default` (unit-id); `labels: { value: label }` | the unit whose case matches the discriminant, else `default`; when `labels` are present and the discriminant is a user-writable fact, a switchable picker (tabs/menu) that sets it |
| `form` | `fields: [{ name, label, type, format?, value_binding?, required?, options? }]`; `input_bindings`; `submit_capability` | a general multi-field form/editor whose current values occupy the `input` scope and submit as directly provided capability input |
| `capabilities` | `capabilities: [id, …]` | a standalone bar of risk-aware controls for the named capabilities |

Any other `kind` is a **non-canonical extension**: a conforming viewer renders its
`fallback` unit if one is given, otherwise the protocol's deterministic fallback.
Extension kinds include `chart` (`source_binding` → rows, `x_key` / `y_key`),
`progress` (`current` / `max`), and `tree` (recursive `children`) — none is
canonical, so give each a canonical `fallback` and every viewer shows something
useful. The retired names are invalid in the View Hint Profile: use `form` instead
of `input`, `list` instead of `record_list`, `capabilities` instead of `action_bar`,
`facts` instead of `stats` / `detail`, and `select` instead of `tabs` / `switch`;
a conforming validator rejects the retired names. There is no compatibility alias.

### Summary

A `summary` powers a glance without opening the unit:

```json
"summary": {
  "count_binding": "space:owner_afmail_7Qk2x9fL.pending_count",
  "attention_binding": "space:owner_afmail_7Qk2x9fL.pending_count",
  "fields": [
    { "label": "Pending", "value_binding": "space:owner_afmail_7Qk2x9fL.pending_count" }
  ]
}
```

A conforming viewer renders it as a count plus an attention signal — a web viewer as
a badge that lights up when `attention_binding` is non-zero, a terminal viewer as the
number — both reading the same fact. A summary holds only facts and bindings, never a
pre-rendered string.

### Form/editor

`form` is the canonical general editor. It is deliberately not tied to one widget
or domain: the same shape covers an email draft, CMS document, settings object, or
operator configuration.

```json
{
  "id": "draft_form",
  "kind": "form",
  "fields": [
    { "name": "to", "label": "To", "type": "array", "format": "email_list", "required": true },
    { "name": "cc", "label": "Cc", "type": "array", "format": "email_list" },
    { "name": "subject", "label": "Subject", "type": "string", "format": "text", "value_binding": "actor:draft.subject", "required": true },
    { "name": "body_markdown", "label": "Message", "type": "string", "format": "markdown", "value_binding": "actor:draft.body_markdown", "required": true },
    { "name": "send_receipt", "label": "Request receipt", "type": "boolean" }
  ],
  "input_bindings": { "draft_id": "actor:draft.id" },
  "submit_capability": "save_draft"
}
```

Every field has a non-empty, form-local unique `name`, a human `label`, and a JSON
value `type`: `string`, `boolean`, `number`, or `array`. `format` is advisory editor
treatment:

- string — omitted/`text`, `textarea`, `markdown`, `email`, or `enum`; an `enum`
  field has non-empty `{ value, label }` options;
- array — `email_list` or `tags`; a viewer may accept lines, comma-separated text,
  chips, or another native editor, but it **normalizes the result to a JSON array**
  before request preparation;
- boolean/number — no format; the viewer chooses a native control.

`value_binding` seeds the editor; it does not continuously overwrite user edits.
The viewer assembles the current values as one object, making each available at
`input:<name>`. On submit that object is **directly provided request input** to
`submit_capability`. The ordinary request algorithm first resolves `input_bindings`
(usually hidden context such as `draft_id`), then the form's directly provided values
override or supplement same-named entries. The owner still validates the declared
capability `input_schema` and risk. The form never mutates owner facts directly.

### Transferable context references

`reference_binding` no longer means “copy this locator”. It resolves a row or record
to one stable inert, self-describing string id. The viewer combines that value with
the owning section id, the source unit id, the unit's `reference_kind`, and an
optional row label to synthesize context for a viewer-owned **context sink** (for
example an agent context tray, a draft's related-items area, or a local inspection
shelf):

```json
{
  "owner": "owner_afmail_7Qk2x9fL",
  "unit": "message_list",
  "kind": "message",
  "id": "msg_123",
  "label": "Quarterly update"
}
```

This `ReferenceEnvelope` is the viewer's context-sink output, **not a shape that a
producer repeats in every source record**. `owner` and `unit` preserve provenance;
`kind` is the **domain kind** (`message`, `draft`, `case`, `document`), never the
canonical unit kind; `id` is the string resolved from `reference_binding`; `label`
is optional display text. A reference-bearing unit should publish `reference_kind`;
only when it is absent does the viewer fall back to `unit.kind`. A viewer may derive
the label from the current record's `title_key`, the unit title, or another safe
record value.

The viewer decides whether and how to expose transfer and owns the sink. A reference
is not a capability, URI, command, link, request, or instruction. Receiving one never
executes data, invokes a capability, navigates, opens content, or wraps it into a link
automatically. `reference_label` is retired.

### Capability visibility

Capability declarations and unit references remain fully discoverable to agents.
For an ordinary human viewer, however, a declaration or contextual reference is not
permission to manufacture a button. By default a capability becomes a visible human
control only through:

- an explicit `kind: "capabilities"` unit;
- a `form.submit_capability` submit control; or
- viewer/user policy, such as an explicitly enabled agent surface.

`capabilities` on another unit and `row_capabilities` describe available context for
discovery and policy; they do not become buttons by default. `primary_capability` may
rank a capability **inside an already surfaced action context**, but cannot surface it
by itself. `activate_capability` is the non-button primary intent of activating a unit
or row; it does not authorize a separate visible button. Every invocation still uses
the normal owner-local capability lookup, applicability check, risk gate, and Request.

### Canonical fields

These field names carry the meaning above across viewers. Names ending in `_binding`
are load-bearing (the binding grammar is normative); the rest are advisory shape.

- **Sources & values** — `source_binding`, `selector_binding`, `count_binding`,
  `value_binding`, `label_binding`, `title_binding`, `status_binding`,
  `text_binding`, `alt_binding`, `session_id_binding`, `attention_binding`.
- **References** — `reference_binding` (stable inert string row/record id) and
  `reference_kind` (the domain kind used when a viewer synthesizes its envelope).
- **Capabilities** — `capability`, `capabilities`, `row_capabilities`,
  `activate_capability`, `primary_capability`, `submit_capability` (each a
  declared-capability id, gated by `risk`; references are discoverable but visibility
  follows [Capability visibility](#capability-visibility)).
- **Interactions** — `interactions: { event: [effect, …] }`, where canonical
  events are `activate`, `select`, and `submit`; effects are `set_actor_state` or
  `request`.
- **Request wiring** — `input_bindings: { field: binding }`, the protocol-defined
  mapping from capability input fields to scoped bindings.
- **Composition** — `members` (a layout's ordered list of unit or layout ids),
  `root_layout`, `cases` / `default` (`select`), and `fallback` (a canonical unit
  id, for a non-canonical kind) are id references. `relation`, `role`, `overlay`
  (a summoned region), and `labels` (`select`) are advisory composition or
  selection hints; nothing nests inline.
- **Shapes** — `columns: [{ key, label, visible_when? }]`, fact/list
  `fields: [{ key, label, visible_when? }]`, form
  `fields: [{ name, label, type, format?, value_binding?, required?, options? }]`,
  `items: [{ label, value_binding, visible_when? }]`, and the per-kind keys
  `media_type`, `alt`, `level_key`, `time_key`, `message_key`.
- **Salience** — `salience: { prominence, state }` and `state_binding` (on a unit,
  field, item, or layout region; see [Salience](#salience)).
- **Presentation default** — `default_presentation: "glance" | "compact" | "full"`
  is an owner recommendation for initial viewer-owned state, never a safety field.

## Salience

Salience is **how much a thing matters**, as semantic intent — never as a visual
mechanism. A unit, a field, an item, or a layout region may carry a `salience`
object; a viewer realizes it in its own medium (a web viewer with size / weight /
position, a terminal with bold / dim / order, a voice viewer with reading order and
tone) and may ignore it entirely, degrading to a flat surface. It never carries a
colour, a size, or any appearance value — those are the viewer's.

- `prominence` — `primary` / `normal` / `subtle`: attention weight. `primary` is the
  headline or the central region; `subtle` is fine print or a peripheral region.
- `state` — `neutral` / `info` / `success` / `warning` / `danger`: the condition of
  the thing, from the owner's truth; use `state_binding` to read it from a fact the
  owner emits. A web viewer maps it to a colour or an icon, a terminal to a symbol, a
  voice viewer to a spoken cue ("warning: …").

`primary_capability` is the same idea for actions: the one capability a unit suggests
within an action context that is already visible. It ranks; it does not itself make a
button visible.

## Read-only field visibility

Read-only fact and record-detail fields omit values that carry no display information
by default: `null`, blank strings, empty arrays, and empty objects. Numeric zero and
`false` are values, not generic emptiness. This policy changes only presentation; the
owner's fact remains in the space and stays fully discoverable to agents.

A field, fact item, or table column may add `visible_when` when its domain has a more
specific display threshold. The predicate applies to that field's current resolved
value and supports one literal comparison: `lt`, `gt`, `eq`, `contains`, or `empty`.
An array is an AND of atomic predicates. A table column with `visible_when` remains
visible when at least one current row passes. Invalid predicates are ignored and the
default empty-value policy applies. Form fields are inputs rather than read-only facts
and do not use `visible_when`.

```json
"fields": [
  { "key": "cc", "label": "Cc", "visible_when": { "empty": false } },
  { "key": "attachment_count", "label": "Attachments", "visible_when": { "gt": 0 } }
]
```

Salience is **orthogonal to `risk`**: `state` is the condition of a *fact* (this build
failed → `danger`); `risk` is the consequence of invoking a *capability* (deploy is
`destructive`). A `success` row can carry a `destructive` action; a `danger` row a
`read_only` one — never conflate them. Every salience value must be renderable by both
a terminal and a voice viewer; any value that cannot (a pixel size, a font weight, a
colour) is appearance and belongs to the viewer, not here.

## Layout

A recommended **composition**, in neutral terms only. Layouts are a flat pool of
named regions; each has a stable `id` and `members` — an **ordered** list of ids
that reference units **or other layouts**. One layout is named by `root_layout`.
Composition is by id reference and may nest to any depth: the pool is a DAG, every
member id must resolve, and cycles are rejected.

The layout profile does not try to encode every visual layout. Instead it encodes
the cross-media **relationship** between members and the semantic **role** of a
region, so a web viewer, terminal viewer, voice viewer, native app, or agent can
choose its own concrete arrangement while preserving the same intent.

```json
"layouts": [
  { "id": "nav", "role": "navigation", "members": ["folders"], "salience": { "prominence": "subtle" } },
  { "id": "message_area", "role": "detail", "members": ["message_facts", "message_log", "message_actions"] },
  { "id": "compose", "role": "input", "members": ["compose_form"], "overlay": true },
  { "id": "root", "relation": "master_detail", "members": ["nav", "triage", "message_area", "compose"] }
],
"root_layout": "root"
```

Beyond its id, a region carries only neutral things:

- **order** — the position of an id within `members` (most important first).
- **relationship** — the region's `relation`, how its members work together.
- **role** — the region's `role`, what job it does in the surface.
- **emphasis** — the region's [`salience.prominence`](#salience): `primary` is
  central, `subtle` is peripheral (a navigation rail, an aside, a status line).
- **`overlay: true`** — the region is **summoned** (a modal, a sheet, a popover),
  not part of the resting arrangement.

The producer's member order is the initial recommendation. A viewer may let its user
reorder the resulting presentation and persist that override in the viewer/actor's
own section. This does not mutate the producer layout or change unit identity,
authority, risk, or agent discovery.

`relation` is a small semantic vocabulary:

| `relation` | Meaning | Typical viewer treatment |
| --- | --- | --- |
| `sequence` | Ordered regions; this is the default when omitted. | render/read in member order |
| `workspace` | Members are part of one working surface and should stay jointly reachable. | sections/buffers/landmarks |
| `master_detail` | Earlier members select, filter, or navigate later detail members. | list plus detail, menu plus content |
| `overview_detail` | Start with a glance, then expose deeper details. | summary first, detail after |
| `comparison` | Members are peer subjects to compare. | side-by-side when possible; paired sections otherwise |
| `flow` | Members form steps or a guided progression. | checklist/ordered narration |
| `supporting` | Members provide context, history, diagnostics, or help. | subdued supporting region |

`role` is also semantic, not positional:

| `role` | Meaning |
| --- | --- |
| `primary` | The main working area or answer surface |
| `navigation` | Selection, filtering, routing, or wayfinding |
| `detail` | Detailed view of the selected or primary subject |
| `inspector` | Metadata, properties, or contextual facts about a subject |
| `actions` | Risk-aware controls or forms that cause requests |
| `status` | Current state, health, progress, or alerts |
| `history` | Logs, audit trails, timelines, or prior events |
| `input` | Drafting, forms, composition, or editable local input |
| `summary` | Glanceable overview |
| `supporting` | Secondary context that should not dominate |

A conforming viewer composes the pool from `root_layout`, lays regions out in
`members` order, interprets `relation` and `role` as semantic guidance, gives
`role: "primary"` or `salience.prominence: "primary"` regions the central treatment
and `subtle` ones the periphery, shows `overlay` regions on demand, then lets the
user override any region per-`id` in the user's own section. *How* it arranges
(multiple regions, terminal buffers, reading order, voice landmarks) is the viewer's
own code. With no AI it produces a safe, usable
arrangement; a richer one is the viewer's own work, optionally informed by each
region's prose `description`.

The only load-bearing content is the id graph: ids are unique, every `members` entry
resolves to a declared unit or layout, and the graph has no cycles. A layout carries
**no** panel, window, split, column, dock, grid, width, pixel, or implementation
vocabulary — only `members`, `relation`, `role`, `overlay`, and optional `salience`
/ `description`.

Examples:

```json
{
  "id": "mail_root",
  "relation": "master_detail",
  "members": ["folders_region", "message_list_region", "message_detail_region", "compose_region"]
}
```

```json
{
  "id": "ops_workspace",
  "relation": "workspace",
  "role": "primary",
  "members": ["builds_table", "deployment_facts", "log_stream", "terminal_unit"]
}
```

```json
{
  "id": "review_comparison",
  "relation": "comparison",
  "members": ["before_facts", "after_facts", "diff_log"]
}
```

## Bindings

A **binding** references facts as `<scope>:<path>`, scope in `space` / `record` /
`input` / `actor` (the binding grammar is normative; see
[`protocol.md`](protocol.md)). The `record` scope resolves against the current row
of a `table` / `list`; the `input` scope against the request input a unit is
assembling; the `actor` scope against the current viewer/participant's own section.
Request-input wiring uses the protocol field `input_bindings`, not ad hoc
`<field>_binding` fields. In a `form`, the viewer maintains the current collected
object as input scope: field `name: "subject"` is available at `input:subject` and is
sent as direct provided input on submit.

## Interactions

`interactions` lets a unit describe what a viewer should do with a user intent
without adding code and without letting one owner write another owner's section.
It is viewer-executed advisory guidance: a web click, terminal Enter key, voice
command, or native gesture may all map to the same canonical event.

```json
{
  "id": "message_list",
  "kind": "list",
  "source_binding": "space:owner_afmail_7Qk2x9fL.messages",
  "reference_binding": "record:id",
  "reference_kind": "message",
  "interactions": {
    "activate": [
      {
        "type": "set_actor_state",
        "path": "selection.message",
        "value_binding": "record:."
      },
      {
        "type": "request",
        "capability": "mark_read",
        "input_bindings": { "message_id": "record:id" }
      }
    ]
  }
}
```

Canonical events:

| Event | Meaning |
| --- | --- |
| `activate` | The user invokes the primary intent for a unit or row |
| `select` | The user changes the current selection without implying owner action |
| `submit` | The user submits a form unit |

Canonical effects:

| Effect | Meaning |
| --- | --- |
| `set_actor_state` | Viewer emits into the current actor's own section at `path`, using exactly one of `value` or `value_binding` |
| `request` | Viewer sends a Request to the unit owner's declared `capability`, resolving optional effect-local `input_bindings` |

`set_actor_state.path` is a relative actor-state dot path such as
`selection.message_id` or `drafts.compose`. It never starts with an owner id, never
uses `space:`, and cannot target another section. The viewer translates it to an
emit authored by the current actor under that same actor's section. This is how
viewer-local selection, draft state, or presentation overrides influence other units:
other units read the actor state through `actor:` bindings and rerender.

`request` keeps business or consequential effects with the target owner. The
capability id is owner-local to the section that declared the unit; `risk` still
drives gating. `activate_capability` remains a compact shorthand for the common
case of `interactions.activate: [{ "type": "request", "capability": "..." }]`.

For a webmail shape, `message_list` can set `actor:selection.message` to the
activated record while also requesting `mark_read`; a detail text unit can render
`text_binding: "actor:selection.message.body_markdown"`. The list did not mutate
the detail unit. It changed actor-owned state, and the detail unit consumed that
state as data.

## Presentation

`glance`, `compact`, and `full` are viewer-owned presentation states, not owner
facts and not part of AFUI's safety layer. Every conforming viewer makes all three
states reachable for every canonical unit kind:

- `glance` — a badge or one line, using `summary` when present;
- `compact` — a row, small card, or equivalent short treatment;
- `full` — the unit's complete viewer-native surface.

An owner may publish `default_presentation` on a unit and an installed arrangement
pack may recommend its own default. Both are initial suggestions only. The trusted
viewer chooses the current state, stores any durable choice in viewer/user-owned
state, and lets the user change it. A useful precedence is owner recommendation <
genre shell < installed pack < viewer/user override. A presentation state cannot
grant or hide authority, change risk, alter binding resolution, remove agent
discovery, or enter the core data-only / owner / Request / emit safety contract.
The states carry no window, panel, screen position, dimensions, or pixel vocabulary;
each viewer realizes them in its own medium.

Advisory presentation hints a conforming viewer recognizes:

- `preview_length` — truncate a long bound value to this many characters for a
  preview.
- `labels` — count templates the viewer fills (for example `"{n} pending"`).
- `body_format: "markdown"` — render the bound body as markdown (still escaped).
  Every embedded `_uri` — in a **link or an inline image** — resolves only through
  the viewer's locator policy and is **not auto-loaded by default** (an inline image
  is a `_uri` like any other, and a known tracking / exfiltration vector). A `media`
  unit's `source_binding` follows the same rule.
- `reference_binding` / `reference_kind` — inputs from which the viewer synthesizes
  inert transferable context; they do not imply a copy button, link, or navigation.

### Time-based displays

A time-based display hint lets a viewer tick a value against its **own** clock with
no data round-trip — advisory display only; the authoritative value is always the
owner's, recomputed at request time.

- `show_as: "countdown"` — count a bound target time down to now.
- `show_as: "relative_time"` — render a bound timestamp as "3 minutes ago".
- `show_as: "accrual"` — fill from a bound base value at a bound `rate` toward a
  bound `maximum`, anchored at a bound base time.

There is no clock field in the wire: the owner emits its own time as an ordinary
fact, and the viewer uses it as it likes.

## Appearance is the viewer's

The profile defines **no appearance controls** — no colours, no pixels, no fonts. A
unit says `salience.state: "danger"`; the viewer decides danger is red, an icon, or a
spoken cue. Colour, spacing, type, and any restyling are entirely the viewer's own,
informed by the user — never commanded by another owner's data. Unknown fields are
still inert data; a conforming viewer may ignore styling-like extension fields
instead of treating their names as authority. This keeps every owner from smuggling
presentation across the trust boundary, and lets the same space look native in a web
workbench, a terminal, or a voice interface.

## The reference implementation

This profile is what the reference SDK and viewers implement; consult them as the
executable specification:

- **Canonical kinds & fields, typed** — `rust/src/unit_profile.rs`
  (`CanonicalUnitKind`, the `KIND_*` and `FIELD_*` constants).
- **Conformance validation** — `rust/src/profile.rs`
  (`validate_space_profile(space, ValidationProfile::ReferenceHost)`) rejects legacy and
  non-conforming fields; surfaced by `afui check`.
- **Reference viewers** — `agent-first-ui-host/src/assets/reference-host-renderer.js`
  (browser) and `agent-first-ui-host/src/tui/mod.rs` (terminal) render exactly the kinds
  and fields above.
- **Machine-checkable schema** — [`afui.view-hint.schema.json`](afui.view-hint.schema.json)
  validates a space that opts into this profile (canonical kinds, field shapes), and
  rejects legacy hint names such as `record_list` / `action_bar` / `*_bind` /
  `action`. The strict envelope schema is [`afui.schema.json`](afui.schema.json).

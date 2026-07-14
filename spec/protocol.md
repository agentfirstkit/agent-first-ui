# Agent-First User Interface — the Space model

**Safe operation as shared data.** A tool publishes what is true and what can be
done, as data, into a shared **space**. Participants — humans and agents, local or
remote — observe that data and propose changes, each from behind their own trusted
boundary. What is shown is rendered by the observer, never run by the publisher.

This is the **normative** document for the protocol: the reasoning, the model, the
wire format, and the safety boundary. It is a semantic model with a canonical JSON
encoding; it is not a frontend project, a remote component system, a website bundle,
or a code transport.

How to display a space is intentionally **not** in this document. The protocol says
only that hints are advisory and a viewer may ignore them. The recommended,
opt-in way to turn hints into a strong default surface — the canonical unit kinds,
their fields, layout, salience, and presentation — is a separate, **optional** profile:
[`view-hint.md`](view-hint.md). This document and that profile share one version.

## Status

This document defines the space model and supersedes the earlier drafts; there is
**no backward compatibility** with them. The protocol version string is `"0.2"`.
The core is intentionally tiny — six concepts plus one safety field — and every
heavier feature (notifications, search, live data, collaboration, distribution) is
a consequence of those, not a new mechanism.

The machine-checkable envelope schema is [`afui.schema.json`](afui.schema.json); it
validates the envelope and the load-bearing layer only. Everything advisory is left
to the optional [View Hint Profile](view-hint.md).

## Why this shape

Three forces produce it:

1. **Showing an interface usually means shipping code.** That code runs on your
   machine and an agent cannot reason about it. The answer here: an interface is
   *data only*; the viewer you trust renders it. Nothing the publisher sends runs
   on the viewer.
2. **The same live thing often must be operated by several parties at once** — you,
   your agents, a teammate, a few tools — on one shared state, safely, without one
   reaching into another. So the model is not one-way "render and click"; it is a
   shared, live, two-way space.
3. **A good default must not require AI.** Arranging an interface at runtime with a
   model is expensive and unreliable. The owner instead bakes deterministic
   recommendations, so a simple viewer is good out of the box; AI, if present, is
   for customization only.

## The model: a space

A **space** is an addressable tree of facts that participants join, each seeing it
from **their own perspective**. The whole protocol is six everyday words.

- **Space** — the one shared context all participants join; each observes only what
  owners have **projected to them**. An owner may
  publish a fact, capability, or hint to **everyone** (the shared **public space**),
  to a **named subset** (a private *whisper*), or as a **different value per
  participant**. There is no single global tree — there is what each participant
  receives. Everything published carries one **owner** (its sole writer) and an
  **audience** (its readers, defaulting to everyone); a participant is an owner of a
  section, and an interface is one viewer's rendering of its own perspective.
- **Facts** — an owner's concrete, typed truth. A fact is a typed, addressable
  value (agent-first-data-typed where types matter: `_secret`, `_uri`, `_epoch_ms`,
  `_usd_cents`, `*_rfc3339`, …). Facts carry domain truth, never pre-rendered
  presentation (no baked `"3 messages"`, no locale-formatted dates), and never code.
  On the wire a fact is always a literal value — never something "computed".
- **Emit** — the way an owner changes its **own** facts. An emit applies
  immediately and is delivered, in order, to every participant in its **audience**. The ordered,
  attributed stream of emits is the **emit log**: it is synchronization, history,
  audit, and how a joiner catches up — all at once.
- **Request** — the way a participant asks **another** owner to change that owner's
  facts. A request references a **capability**; the target owner validates it,
  gates it by the capability's **risk**, then either performs it (emitting the
  result) or rejects it.
- **Capability** — what an owner publishes saying "here is something you may
  request from me", with a truthful **risk**, the **input** it needs, and its
  applicability. A capability is itself a fact in the owner's section; it is not a
  separate primitive, just a fact of a known shape.
- **Hint** — an owner's advisory suggestion for *how to display* its facts:
  recommended units (a slice of facts plus an open `kind`), a recommended layout, and
  presentation hints. A viewer may use, adapt, or ignore any hint;
  getting a hint wrong can only make a surface less convenient, never unsafe. The
  recommended way to interpret hints is the optional [View Hint Profile](view-hint.md).

Two role words and one safety word complete the vocabulary:

- **Owner** — the participant authoritative for a section: its sole writer and the
  gate for requests aimed at it.
- **Participant** — a peer in the space: a tool, an agent, or a person's viewer.
  Peers are equal; the only structure is ownership.
- **Viewer** — trusted local software that renders facts for a person and proposes
  changes on their behalf. A viewer never runs anything it observes.
- **risk** — the one safety label, carried by every capability:
  `read_only` / `local_mutation` / `external_effect` / `destructive`. A missing or
  unrecognized risk is treated as `destructive`.

The closed loop: **Capability → Request → (owner validates and gates by risk) →
Emit.** An owner changing its own facts is just an Emit directly.

## The principle: the protocol never computes

The protocol carries exactly three things: **concrete Facts**, advisory **Hints**,
and the two operations (**Emit** and **Request**). It never carries or runs a
computation. All computation lives in one of two trusted places:

- **The owner** computes authoritative values and emits them as facts — anything
  involving business rules, remote data, or a decision with consequences. When a
  value depends on time (a regenerating resource, an offer that opens at a set
  moment), the owner recomputes it **at the moment it processes a request**, using
  its own clock, and gates accordingly.
- **The viewer** computes display values locally from the facts it observes — it is
  the viewer's own rendering code and needs no blessing from the protocol. For
  values that change with time on screen (a countdown, a relative time, a filling
  bar), the owner attaches a display **hint** ("show this timestamp as a countdown")
  and the viewer ticks it against its own clock.

Consequences of this principle:

- There is **no expression language** in the protocol. A display computation is the
  viewer's implementation; one viewer might use one language, another a built-in
  renderer — the protocol does not pick.
- **Merging a field two owners co-edit** is an implementation choice of whoever
  needs it; it is not a protocol concept and its mechanism is not named here.
- **Clock synchronization is an implementation concern, not part of this protocol.**
  If an owner wants viewers to align their displayed time, it simply emits its own
  time as an ordinary fact; how a viewer uses it is the viewer's code. Correctness
  never depends on it, because every consequence is computed by the owner at request
  time. There is no clock or timestamp field in the wire.

The iron rule that makes this safe: **control and safety are the owner's,
authoritatively; display is the viewer's, advisorily.** A wrong display value is a
wrong pixel, never a breach.

## Two kinds of fields

Every value in the protocol is one of two kinds:

1. **Load-bearing** — enforced structure with safety consequences: owner
   identifiers, capability references, `risk`, binding targets, emit/request paths,
   `_secret`, `_uri`. **Always literal — never computed, never guessed.**
2. **Advisory** — everything else: units, layout, salience, presentation hints,
   summaries. A viewer may honor, adapt, or ignore it; wrong or ignored advice can
   only make a surface less convenient, never unsafe or untrue.

Natural-language `description` text is allowed anywhere; it explains intent and
never changes behavior. When in doubt, a field is advisory.

## Value space and encoding

The protocol is defined over `null | boolean | number | string | array | object`.
No functions, host objects, inline binary, not-a-number, infinity, comments, or
cycles. JSON is the canonical encoding; another encoding conforms only if it
round-trips losslessly and stays self-describing.

AFUI has no executable field, callback slot, component reference, stylesheet hook,
package manifest, dependency graph, or plugin mechanism. A string may still be a
domain fact that contains code-like text — an email body, a log line, a source
snippet, a recorded shell command — but it remains inert data. A viewer must never
execute observed values, inject them as trusted markup or style, or use an unknown
`kind` to install, download, compile, or run code. `_uri` values are untrusted
locators, resolved only through the viewer's own policy.

## The space: facts and ownership

The space is one tree of facts, partitioned into **sections**. The top level is
keyed by the owner's self-generated opaque id. Owner ids are separator-safe
`[A-Za-z0-9_-]` strings: they deliberately avoid `.`, `:`, and `/` so bindings and
JSON Pointers remain unambiguous.

```
space = {
  "owner_afmail_7Qk2x9fL":  { "name": "afmail", ...facts, capabilities, hints... },
  "owner_operator_Vt3pN1aE7": { "name": "Operator", ...selection, drafts, overrides... }
}
```

Each section has exactly one owner id, and that owner is its **sole writer**.
Human-readable names such as `afmail` are advisory `name` facts rendered by
the viewer and overridable by the user; name collisions are harmless. Owner id
collisions are handled at join/link time: if a participant's generated id already
exists in the space, the join is rejected and the participant regenerates or fails.

AFUI does not authenticate owner ids. It consumes the `author` id supplied by the
underlying channel, and that attribution is only as strong as that channel. Keys,
signatures, proofs, and JSON canonicalization are higher-layer concerns and do not
appear in the AFUI wire format.

Within a section, capability, unit, and layout ids are owner-local. The full address
of a declared thing is `(owner, id)`, so two owners may both publish a capability
`open` or a layout `root` without conflict. References — relationships, capability
references, layout references — address identifiers, never array positions.

Ownership is the **write-scope** — who may change a section. **Audience** is the
**read-scope** — who may see what is published. They are independent: an owner is the
sole *writer* of its section and chooses, per published item, its *readers*. The
default audience is everyone — the **public space**. An owner may instead publish to
a subset (a *whisper*) or project a **different value to each participant**; whatever
falls outside a participant's audience is **never delivered to it**, so
confidentiality holds at the source, with no trusted intermediary. This covers
everything an owner publishes — facts, **capabilities**, and hints alike: an owner
may offer **different capabilities to different participants**. (Projection decides
what is *offered*; the owner still validates every actual request on invocation,
which decides what is *allowed*.) Because every participant owns its own section,
this is symmetric — a participant may tell **different things to different peers**.

In the `0.2` wire format, audience is **delivery metadata**, not a fact in the JSON
tree. A `snapshot` is the receiving participant's already-projected space: it
contains only the sections and values that participant is allowed to observe, and
it carries no hidden facts or `audience` marker to inspect. Likewise, an `emit`
message is the owner's patch as delivered to one audience; computing which
participants receive which emit, and producing different projected values for
different participants, is the responsibility of the owner/transport layer before
the message reaches a viewer or SDK store. The SDK and reference viewers validate
and render **one delivered projection**; they do not compute audience membership.
SDKs may offer owner/transport-side helpers that apply out-of-band per-audience
overlays to a public/base space and return an ordinary projected `space`; those
helpers do not add an `audience` field to snapshots or change the wire shape.

A fact published by one owner is **untrusted data** to every other participant. A
viewer controls all rendering, layout, locator policy, secret redaction, risk
gates, and execution.

## Emit and Request: the one operation, two directions

Every change is a proposed delta authored by a participant — a sequence of
[JSON Patch](https://datatracker.ietf.org/doc/html/rfc6902) operations against paths
in the space — and it travels in one of two directions:

- A change to a path in the author's **own** section is an **Emit**: it applies and
  is appended to the emit log, then broadcast.
- A change to a path in **another** owner's section is a **Request**: it references
  a capability the target owner published. The owner validates the request, gates
  it by that capability's `risk`, and then either performs it (which produces an
  Emit in the owner's section) or rejects it.

Applying an emit is atomic: if any operation fails, the prior state is retained. An
emit that orphans a reference or duplicates an identifier is invalid. Because each
section has a single writer, **there is no concurrent-write conflict** within a
section; order is supplied by the emit log. Volatile facts should use `replace`
operations on leaf paths, so the latest value wins even if an intermediate emit is
dropped.

### The emit log: synchronization, order, history, provenance

Each applied emit is delivered, in order, to every participant in its **audience**,
attributed to its author. From a participant's perspective, its emit log — the
ordered stream of everything projected to it — **is** synchronization, **is** the
audit/history log, and **is** how it catches up:

- **Join** — receive a snapshot of your perspective, then apply the tail of your emit log.
- **Resync** after a disconnect — re-fetch the snapshot, then resume the tail.
- **Order** — each participant sees a consistent order over the emits addressed to
  it; cross-section causality is rare because sections are single-writer.

How the emit log is carried between processes — and whether the space runs in one
process or is distributed — is an implementation concern outside this protocol. AFUI
defines the model, the wire format, and the safety boundary, never the transport.

## Capabilities and risk

A **capability** is an owner-declared affordance, published as a fact in the owner's
section:

```json
{
  "owner_afmail_7Qk2x9fL": {
    "name": "afmail",
    "capabilities": [
      { "id": "open_message", "label": "Open", "risk": "read_only",
        "input_schema": { "type": "object", "required": ["message_id"],
          "properties": { "message_id": { "type": "string" } } } },
      { "id": "send_draft", "label": "Send", "risk": "external_effect",
        "applicable_binding": "record:has_recipient",
        "input_schema": { "type": "object", "required": ["draft_id"],
          "properties": { "draft_id": { "type": "string" } } } },
      { "id": "delete_message", "label": "Delete", "risk": "destructive",
        "input_schema": { "type": "object", "required": ["message_id"],
          "properties": { "message_id": { "type": "string" } } } }
    ]
  }
}
```

| Risk | Meaning |
| --- | --- |
| `read_only` | observes or navigates only |
| `local_mutation` | changes viewer-local, draft, or file state |
| `external_effect` | reaches a network, service, payment rail, or other actor |
| `destructive` | can cause irreversible or user-harming effects |

`risk` is required and truthful; a missing or unrecognized risk is treated as
`destructive`. There is no confirmation field — confirmation is the viewer's policy,
derived from risk, provenance, and preference.

**Invoking** a capability is a Request: the viewer assembles input from the source
unit's explicit `input_bindings` plus any fields it collected from the user, then
sends a request that references the target owner id plus that owner's local
capability identifier. The owner validates and performs it (an emit, an effect,
whatever is the owner's business), and the result broadcasts as an emit. Invoking is
never running observed data. If opening a row requires owner authority (hydrating
content, marking read, checking access), make it a `read_only` or stronger
capability; if it is only viewer selection, write actor-owned state instead.

`input_bindings` is a unit field whose value is an object mapping capability input
field names to scoped bindings. It is the load-bearing wiring from displayed data to
request input:

```json
{
  "id": "builds_table",
  "kind": "table",
  "source_binding": "space:owner_operations_H8sJ3kL9mN.builds",
  "input_bindings": {
    "build_id": "record:id",
    "environment": "actor:selection.environment"
  },
  "row_capabilities": ["view_build_log", "deploy_build"]
}
```

When preparing a request for a capability with `input_schema.properties`, a viewer
resolves the `input_bindings` entries whose keys match those properties, using
`record:` against the selected row, `input:` against any request input already
being assembled, and `actor:` against the current viewer/participant's own section.
Directly provided input then overrides or supplements the resolved bindings.
Required schema fields still missing after that step are collected from the user
before the request is sent.

The optional View Hint Profile's canonical `form` uses this exact algorithm: its
named field values are the directly provided input, including arrays already
normalized by the viewer, while `input_bindings` supplies hidden or contextual
values. The form adds no write path; only the Request to the owner can act.

The owner-side handoff should preserve the same shape even when the transport is
local process calls rather than a network relay: a viewer gives the owner a
`request_id`, author, target owner, capability id, risk, and object input. The
owner returns accepted/rejected/failed, an optional error or output payload, and
any resulting `emit` messages. The request result is not a fourth space message; it
is a local/transport acknowledgement that ties the request to the emits the owner
chooses to publish.

**Applicability** (for example, "cannot send a draft with no recipient") is the
owner's authoritative fact via `applicable_binding`. A viewer may also compute a
display-only hint to grey out a control, but enforcement is always the owner
validating on invocation. An unresolved capability reference fails safe: the viewer
surfaces the integrity error and treats the control as `destructive`.

A declared or referenced capability is always discoverable data, especially to an
agent, but declaration alone does not require an ordinary viewer to manufacture a
human button. Human visibility is viewer policy or an explicit View Hint Profile
action surface (`capabilities` unit or `form.submit_capability`). Contextual
`capabilities` / `row_capabilities` and `activate_capability` remain meaningful for
discovery and primary activation without implying separate buttons.

## Hints: advisory display guidance

So a viewer with no AI renders a strong default, an owner may publish, as facts in
its own section, advisory **hints**. There are four families:

- **Units** — recommended coherent slices of facts to present, a flat pool each with
  a stable `id` and an open semantic `kind` (`facts`, `table`, `list`, `text`,
  `media`, `select`, `form`, …), the capabilities on it, and an optional `summary`. Units do
  not nest; they compose only by id reference.
- **Interactions** — optional unit-level viewer intents such as activate/select:
  set actor-owned state, or request a declared owner capability. Interactions never
  directly mutate another owner or another unit.
- **Layout** — a recommended composition, in neutral terms only: a pool of regions
  composing units (and other layouts) by id, in order, each with optional semantic
  `relation` / `role`, emphasis, and a summoned flag — never panels, windows,
  splits, columns, docks, grids, or pixels as hint vocabulary.
- **Presentation** — advisory formatting (preview length, count templates, markdown
  body, time-based displays). Appearance — colour, spacing, type — is entirely the
  viewer's; the profile defines no appearance controls.

All of them are render guidance; **a viewer may use, adapt, or ignore any of them**,
and getting one wrong can only make a surface less convenient, never unsafe. `kind`
is an open word, never a widget name; the protocol defines **no closed catalog** of
kinds. An unknown `kind` or unknown field is inert data and must render through a
safe deterministic fallback.

What stays load-bearing inside a hint — enforced because deviation breaks or
breaches:

- A **binding** is how a hint references facts. Its value is `<scope>:<path>`, where
  scope is one of `space` (the shared tree), `record` (the current row of a list or
  table), `input` (the request input being assembled), or `actor` (the current
  viewer/participant's own section). A binding field is named with the `_binding`
  suffix; `input_bindings` is the explicit object form for request-input wiring.
  Every binding target path is **load-bearing and always literal**.
- A `reference_binding` resolves only a stable inert, self-describing string id.
  Under the optional profile a viewer may combine it with owner/unit provenance and a domain
  `reference_kind` into transferable context. That context is never a locator,
  capability, command, link, request, or automatic navigation instruction.
- A **capability reference** in a unit (`*_capability` / `*_capabilities` /
  `row_capabilities`) is a load-bearing identifier that must resolve to a declared
  capability; an unresolved reference fails safe and is treated as `destructive`.
- An **actor-state interaction** may only name a relative actor path such as
  `selection.message_id`; the viewer translates it to an emit authored by that same
  actor and addressed under that actor's section. A hint never names another owner
  as a write target.
- The `_secret` and `_uri` suffixes apply to any bound value: viewers redact secrets
  and resolve locators only through their own policy.
- A hint can **never** override a fact, a binding, a risk, or a capability, and
  cannot grant executable authority (see *Value space and encoding*).

Everything else about a hint — which kinds exist, what fields each kind reads, how a
layout region maps to a surface, which presentation hints and salience a
viewer recognizes — is **advisory** and is specified, as a recommendation, in the
optional [**View Hint Profile**](view-hint.md). A viewer MAY ignore the profile; if
it declares conformance to it, it MUST interpret those fields as the profile states,
so independent viewers converge on the same strong default.

## Being an owner

A tool joins a space, owns the section under its identifier, and is that section's
sole writer. Everything it publishes — facts, capabilities, hints — lives there;
everything else it only observes.

- Keep facts as domain **truth**, typed where types matter; never bake presentation
  or a computed display value into a fact.
- Use stable identifiers for every entity, unit (including composed children),
  capability, and layout region. References address identifiers, never positions.
- Declare every capability with a truthful `risk` and the required input; mark
  availability with `applicable_binding`.
- Make acting go through a declared capability, never observed data. If opening a
  row's detail needs owner work, model it as a `read_only` capability; if it is
  only selection, let the viewer set actor-owned state.
- Recommend a neutral, overridable layout so a viewer with no AI renders a strong
  default. Use semantic `relation` / `role` where useful; the canonical units,
  layout, and presentation that reference viewers understand are the
  [View Hint Profile](view-hint.md).
- Suffix secrets with `_secret` (viewers redact) and locators with `_uri` (viewers
  apply policy).
- Push live updates by emitting into your own section; use `replace` on leaf paths
  for volatile facts. A **notification** is an emit (an attention fact changes);
  **search** is a capability you declare or a local filter a viewer runs over facts
  it already observes; **live data** is rapid emits. None is a new mechanism.
- Keep implementation vocabulary (panel, window, widget, tiling, split, column,
  dock, page, pixel, grid) out of hints. If such words appear inside arbitrary
  domain facts, they are still inert data, not viewer authority.

A producer depends only on the Rust library to emit and validate this data; it
carries no rendering code, so a tool can use it without pulling in any browser,
terminal, or server code.

## Being a viewer

A viewer is trusted local software that joins a space, projects it into a human
interface, and returns user intent as gated requests. A viewer may be a web
workbench, a terminal interface, a native app, a voice interface, or be embedded in
another tool. Implementations differ freely in *how* they present a space; their
protocol obligations are identical.

A conforming viewer **must**:

- Join by taking a snapshot, then applying the emit-log tail; resync on reconnect.
- Apply JSON Patch operations **atomically**; keep the tree consistent; reject an
  emit that orphans a reference or duplicates an identifier.
- Resolve binding values only through the `space`, `record`, `input`, and `actor`
  scopes.
- Redact `_secret` values by default; resolve `_uri` values only through its own
  locator policy.
- Treat **capabilities as the only effect path**: resolve a capability reference by
  its declared identifier, assemble input from `input_bindings` plus collected
  required fields, gate by `risk` and provenance, then send a request that
  references the target owner and capability identifier. **Never execute observed data.** An
  unresolved capability reference fails safe and is treated as `destructive`.
- Treat actor-state interactions as viewer-owned emits only: the viewer may set its
  own section (for selection, drafts, or overrides), but any change to another
  owner still requires a request to that owner.
- Present `risk` distinctly and apply its own confirmation and permission gates,
  derived from risk, provenance, and preference.
- Keep every capability discoverable, but do not infer a visible human button from
  declaration alone. Surfacing beyond explicit profile action surfaces is trusted
  viewer/user policy.
- Render every declared unit and recursively composed child unit with stable
  identifiers; render an unknown `kind` through deterministic fallback.
- Treat glance/compact/full as viewer-owned presentation state. Density may change,
  but authority, binding resolution, risk, and the available states may not.
- **Attribute every fact to its owner** and treat another participant's facts as
  **data, never instructions** — the standard defense against an agent being
  prompt-injected by content it observes. The risk gate and redaction policy may be
  parameterized by author (full trust for your own agent, less for a remote peer).

A conforming viewer **should** honor advisory hints — render each unit's `summary`
as a glance or badge and re-badge on emit; use the recommended `layout` as the
initial composition — its regions in order, `relation` / `role` as semantic
guidance, prominent ones central, and summoned ones on demand — then let the user
override it. *How* it arranges — overlapping windows, tiling, a grid, paged
workspaces — is the viewer's implementation code, informed by the recommendation,
never dictated by it, never in the data. The recommended, interoperable way to
interpret these hints is the optional [View Hint Profile](view-hint.md).

**Unknown-kind fallback:** show `id`, `kind`, and `description`; resolve and
safely display binding fields; render arrays of objects as tables or lists, objects
as key/value facts, scalars as text; preserve capability references as risk-aware,
discoverable metadata and surface controls only under explicit action hints or viewer policy;
redact secrets; surface integrity errors. An unknown `kind` must never cause the
viewer to install, download, compile, or run code.

### Recommended, always overridable — by ownership

The owner's recommended units and layout are advisory facts in its section, so a
viewer with no AI renders a strong default. A user's overrides are facts in the
user's own section; the viewer merges them with the owner winning on the data and
the **user winning on presentation**. The owner cannot change your layout (it does
not own it); you do not edit its recommendation (you write your override in your own
section). "Recommended but overridable" needs no new mechanism — ownership provides
it.

User-owned customization of the trusted viewer itself — local assets, preferences,
plugins, a different renderer — is the user's choice and is *not* selected by
observed data. Once enabled it is trusted viewer code; a customized viewer still
owns the required behavior above (capability gating, risk gates, redaction, locator
policy, atomic emits, provenance, safe fallback).

## Composition: ids, ownership, isolation

Composition arranges what is already owned; it never merges authority. The model
gives it two shapes.

**Many owners in one space.** A space can hold many owners at once, each owning one
section keyed by its generated id; every participant observes the rest through the
one emit log. More owners is just more sections — no new message type, no rewrite
step. Owner ids are the isolation wall: a fact in `owner_afmail_7Qk2x9fL` is only
addressable under `owner_afmail_7Qk2x9fL`, a hint in that section references only
ids declared in that section, and a capability `delete_message` is invoked as
`(owner_afmail_7Qk2x9fL, delete_message)`. Cross-owner effects are risk-gated
requests, never direct writes. Composing spaces means placing owner sections under
their existing ids and rejecting owner-id collisions; AFUI never prefixes or
rewrites local ids.

**One participant in many spaces.** A single participant joins several spaces and
composes them **locally** — a cockpit might join afmail's space and afpay's space,
observe both, and project them side by side. Composition happens in the participant,
not by merging spaces: each space keeps its own emit log, ordering, and ownership.
Separate spaces are mutually invisible — the strongest isolation — and only a
participant that deliberately joins both can bridge them, locally.

In both shapes the invariants hold: a participant is the sole writer of what it
owns, a cross-owner change is a risk-gated request, and a fact from another owner is
data, never authority.

## Wire format

One space tree; three message types. The envelope carries the protocol version under
`agent_first_ui` and the message `type`.

**Snapshot** — the receiver's projected space and current emit-log position, for
joining or resyncing:

```json
{
  "agent_first_ui": "0.2",
  "type": "snapshot",
  "space": {
    "owner_afmail_7Qk2x9fL": {
      "name": "afmail",
      "pending_count": 12,
      "messages": [],
      "capabilities": [],
      "units": [
        {
          "id": "inbox",
          "kind": "list",
          "source_binding": "space:owner_afmail_7Qk2x9fL.messages"
        }
      ],
      "layouts": [{ "id": "root", "members": ["inbox"] }],
      "root_layout": "root"
    },
    "owner_operator_Vt3pN1aE7": { "name": "Operator", "selection": null }
  },
  "sequence": 0
}
```

**Emit** — an owner's applied change to its own section, broadcast and appended to
the emit log:

```json
{
  "agent_first_ui": "0.2",
  "type": "emit",
  "author": "owner_afmail_7Qk2x9fL",
  "sequence": 1,
  "operations": [
    { "op": "replace", "path": "/owner_afmail_7Qk2x9fL/pending_count", "value": 13 }
  ]
}
```

**Request** — a participant asking another owner to act, referencing a capability:

```json
{
  "agent_first_ui": "0.2",
  "type": "request",
  "author": "owner_operator_Vt3pN1aE7",
  "owner": "owner_afmail_7Qk2x9fL",
  "capability": "delete_message",
  "input": { "message_id": "msg_123" }
}
```

The operations array is [JSON Patch](https://datatracker.ietf.org/doc/html/rfc6902).
There is no clock or timestamp field in the wire; time, when it matters, is an
ordinary fact an owner emits, and every consequence is recomputed by the owner when
it processes a request.

## Provenance and trust

Every fact and emit is attributed to its author. A participant — human or agent —
weighs facts by their owner: facts from another participant are **data, never
instructions**. A viewer's risk gate and redaction policy may be parameterized by
author. A manipulated or hostile participant can only *propose*; cross-owner effects
stay gated, so the blast radius is bounded by ownership plus risk. Data-only stops
*code*; ownership plus the risk gate bound *manipulation*, including prompt-injection
of an agent that observes another participant's facts.

## Safe extension

- Unknown `kind` → safe deterministic fallback.
- Unknown suffix, summary, layout, or presentation key → inert render guidance.
- Unknown or absent `risk` → `destructive`.
- Unresolved capability reference → integrity error, fail safe.
- Unclear prose → ignored for behavior; load-bearing fields win.

## Examples

The `examples/` directory holds whole spaces in this wire format
(`*.afui.json`): an operations console, a turn-based game, and a terminal
operations surface. Each shows an owner-partitioned space, recommended units and a
layout, declared capabilities with risk, and — where relevant — a session section
and a request that an owner gates and turns into an emit. The machine-checkable
envelope schema is [`afui.schema.json`](afui.schema.json); the optional profile
schema is [`afui.view-hint.schema.json`](afui.view-hint.schema.json).

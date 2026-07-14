# Agent-First User Interface

A protocol that lets a tool publish **what is true** and **what can be done** as plain
**data**, into a shared live **space** — so any viewer you trust, human or agent,
renders the interface, and nothing the tool publishes can run on your machine.
**One tool, any viewer:** the same space drives a web workbench, a terminal
interface, a native app, a voice assistant, or an agent operating it directly.

> **Ask your agent:** "Publish my tool's data and capabilities into an Agent-First
> User Interface space — typed facts, declared capabilities with a risk on each, and
> a recommended layout — without shipping any interface code."

## The problem: showing an interface usually means running someone else's code

A command-line tool can print text. The moment it needs something richer — a table
you can sort, a form to fill in, a button that does something — the usual answer is
to ship an interface: a web page, a bundle of components, a desktop view. That
interface is *code*, and it runs on your machine. You end up trusting the tool not
just with a task but with whatever its interface decides to do — read a file, call
the network, or show a button that says one thing and quietly does another.

There is a second problem. The same live thing often has to be operated by several
parties at once — you, your agents, a teammate, a few tools — and they need to work
on it together, safely, without one reaching into another.

## What it is: a shared space of facts

The whole protocol is six everyday words:

- **Space** — one shared tree, split into sections keyed by generated owner ids.
- **Facts** — an owner's concrete, typed truth (never presentation, never code).
- **Emit** — an owner changes its own facts; the change applies and broadcasts to everyone.
- **Request** — to change someone else's facts you ask their owner, who decides.
- **Capability** — what an owner says you may request, each with a truthful **risk** and the input it needs.
- **Hint** — an owner's advisory suggestion for how to display its facts; a viewer may use or ignore it.

A **participant** (a tool, an agent, or a person's viewer) owns its section and
observes the rest. The owner id is an opaque, self-generated `[A-Za-z0-9_-]` string;
human names such as `afmail` are advisory `name` facts. A **viewer** is the
trusted software that renders facts for a person and never runs anything it
observes.

Safety is structural, not bolted on: space data is inert, each section has one
writer, cross-owner effects are requests gated by `risk`, and authentication stays
in the channel below AFUI. The normative safety rules live in
[`protocol.md`](spec/protocol.md); this README is only the adoption overview.

Deterministic unit, interaction, and layout hints give even a simple viewer a strong
surface; AI, if present, is for customization only. Viewer-local state such as
selection lives in the viewer's own section; consequential changes still go through
risk-gated requests.

The optional View Hint Profile includes a general multi-field `form` for drafts,
documents, and configuration editors; inert transferable record references; explicit
human capability visibility; and viewer-owned `glance` / `compact` / `full` states.
None of these add authority: acting still goes only through owner-declared,
risk-gated Requests, and state changes still arrive as owner-scoped emits.

A small Rust library gives a tool the space types, validation, and binding helpers —
and ships no rendering code, so it stays tiny. The reference web workbench is **one**
viewer, not the protocol.

## Where to use it

- **Giving a command-line tool a real interface** — tables, forms, and actions,
  with no interface to build or ship.
- **Letting humans and agents operate the same live state safely** — everyone
  observes the space; every change is owned and risk-gated.
- **Keeping secrets and risk explicit** — mark a value `_secret` or a capability
  `destructive` once, in the data, and every conforming viewer honors it.
- **One tool, any viewer** — the same space renders in a web workbench, a terminal
  interface, a native app, or under an agent, with the tool changing nothing.
- **Combining surfaces** — many owners share one space behind their own sections, or
  one participant joins many spaces and composes them locally.

## Adopt it: hand the protocol to your agent

Adopting a protocol is exactly the kind of work you hand to a coding agent. There is
an [Agent Skill](skills/agent-first-ui.md) — the rules in a form an agent reads and
applies directly. Paste this to your coding agent:

> Learn the Agent-First User Interface protocol: read
> https://agentfirstkit.com/agent-first-ui/spec/agent-first-ui. Then look at the
> tool we are building and tell me how to publish its data and capabilities into a
> space — which facts to publish, which capabilities to declare and at what risk,
> what each unit should glance at, the recommended layout, and where the emit and
> request loop fits.

The library, if you want it:

```bash
cargo add agent-first-ui
```

## Specification

- [Specification](spec/README.md) — the normative protocol
  ([`protocol.md`](spec/protocol.md): the model, wire format, and safety boundary) and
  the optional [View Hint Profile](spec/view-hint.md) for rendering a strong default.
- [Agent Skill](skills/agent-first-ui.md) — the same rules in a form an agent applies.

## License

MIT

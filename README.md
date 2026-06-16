# Agent-First UI

A UI protocol that lets a tool describe its interface as data instead of code — so a trusted host you own renders the screens and actions, and nothing the tool sends can run on your machine.

> **Ask your agent:** "Add an Agent-First UI surface to my tool so it can show tables, forms, and actions — without shipping any UI code."

## The problem: showing a UI usually means running someone else's code

A command-line tool can print text. The moment it needs something richer — a table you can sort, a form to fill in, a button that does something — the usual answer is to ship a user interface: a web page, a bundle of components, a desktop view. That interface is *code*, and it runs on your machine.

So you end up trusting the tool not just with a task but with whatever its interface decides to do — read a file, call the network, or show a button that says one thing and quietly does another. The interface belongs to the tool, not to you. Every tool builds its own, and none are accountable to you.

An agent driving that tool inherits the problem: it can't tell what a control really does, because the meaning is buried in UI code instead of stated in the open.

## What it does: the tool describes, your host renders

Agent-First UI pulls those two halves apart. The tool only *describes* a surface as plain data — screens, views such as tables and forms, and the actions available on them, each with a stated risk. A **trusted host that you own** reads that data and decides how to draw it, what to allow, and what to hide. When you act, the host sends a structured event back; the tool replies with more data, never code.

- **UI as data, never code.** A surface is JSON. It may not carry JavaScript, CSS, HTML, WASM, build scripts, shell commands, callbacks, or remote URLs — there is nothing in it to execute.
- **Your host is in charge.** It renders the surface, hides anything marked secret, and gates risky actions before they run — under your policy, not the tool's.
- **Safety travels with the data.** Actions declare a `risk`; fields carry `_secret`, `_uri`, and required-input markers — so the host can be honest about what a button will do before you press it.
- **Stable surfaces.** Ids stay stable across runs, so the host can keep focus, local state, and an audit trail.
- **A small SDK, no renderer.** The `agent-first-ui` crate gives a tool the wire types, validation, binding resolution, and JSON-Patch helpers — and ships no rendering code, so it stays tiny.

## Where to use it

- **Giving a command-line tool a real interface** — tables, forms, and actions, with no UI to build or ship.
- **Letting someone act on results safely** — present rows and the operations allowed on them, with risky ones clearly gated.
- **Keeping secrets and risk explicit** — mark a value secret or an action destructive once, in the data, and every conforming host honors it.
- **Combining surfaces** — namespaced ids let independent surfaces merge into one without a new format.

## Adopt it: hand Agent-First UI to your agent

Adopting a protocol is exactly the kind of work you hand to a coding agent. There's an [Agent Skill](skills/agent-first-ui.md) — the rules in a form an agent reads and applies directly. Paste this to your coding agent:

> Learn the Agent-First UI protocol: read https://agentfirstkit.com/agent-first-ui/docs/specification and https://agentfirstkit.com/agent-first-ui/docs/producer-guide. Then look at the tool we're building and tell me how to expose its state and actions as a surface — which views and actions to declare, and where the snapshot and the action handler fit.

The SDK, if you want it:

```bash
cargo add agent-first-ui
```

## Docs

- [Overview](docs/overview.md) — what the protocol is and how the pieces fit
- [Specification](spec/agent-first-ui.md) — the normative model and safety boundary
- [Producer guide](docs/producer-guide.md) — how a tool exposes a surface
- [Host renderer contract](docs/host-renderer-contract.md) — what a conforming host must do
- [Composition](docs/composition.md) — merging surfaces with namespaced ids
- [Agent Skill](skills/agent-first-ui.md) — for AI-assisted adoption

## License

MIT

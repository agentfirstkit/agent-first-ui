# agent-first-ui Rust SDK

The Rust SDK for the Agent-First UI (AFUI) data protocol — a way for a tool to describe its interface as data instead of code, so a trusted host you own renders the screens and actions and nothing the tool sends can run on your machine.

This crate is protocol logic only: wire types, validation, binding resolution, JSON Patch helpers, action preparation, host-store helpers, reports, and replay utilities. It intentionally contains no rendering code — no host, web server, terminal, or browser assets — so a producer can depend on it without pulling any of that in.

```bash
cargo add agent-first-ui
```

The protocol overview, the normative specification, and conformance fixtures live in the repository:
<https://github.com/agentfirstkit/agent-first-ui>

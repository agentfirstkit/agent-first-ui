# AFUI Producer Guide

A producer is a tool that owns domain state and effects. It emits AFUI data so a trusted host can show a human an operation surface.

## Minimum Integration

```bash
mytool ui snapshot > surface.afui.json   # emit a surface a trusted host can render
```

A fuller integration adds action handling:

```bash
mytool ui action < user_action.afui.json > patch.afui.jsonl
```

## Snapshot Checklist

- Emit complete snapshots before optimizing with patches.
- Use stable ids for `surface`, `screen`, `view`, and `action` nodes.
- Declare every action with truthful `risk`.
- Use `_bind` fields for state, record, and input reads; do not invent expression languages.
- Keep `state` as domain **facts**. Express presentation — truncated previews, pluralized count labels, formatted timestamps, detail text stitched from several fields — as advisory view hints (`preview_len`, `labels`), never baked into state. The host applies hints and may override them.
- On `record_list`/`table` views, set `ref_bind` (a `record:`-scoped locator such as `record:message_id`) and optional `ref_label` so a host can offer a per-row "copy locator" affordance for directing an agent. Hosts fall back to the row id and may ignore it; copy is presentation only, never an action.
- When a body text field holds markdown (notes, summaries), set `body_format: "markdown"` so a host renders it instead of showing raw source. Mark a markdown action input with `{ "type": "string", "format": "markdown" }` so a host MAY collect it with a markdown editor. Both are presentation hints; the value stays a plain string and hosts may ignore them.
- Put required human-provided fields in `input_schema.required`.
- Suffix secrets with `_secret` so hosts redact them by default.
- Suffix locators with `_uri`; hosts decide whether and how to open them.
- Keep AFUI data free of commands, scripts, styles, HTML, component URLs, package manifests, and build steps.

## A standalone convenience command (optional)

A tool may offer one command that builds a snapshot and hands it to a host it bundles or finds locally, so a user does not have to wire the two steps together:

```bash
mytool ui            # build a snapshot and open it in a trusted host
```

This is a tool-side convenience, not part of the wire protocol. A future `mytool ui action` can read `user_action` events, apply local domain changes, and return AFUI patch messages. The producer depends only on the protocol SDK:

```bash
cargo add agent-first-ui
```

It should not depend on any host, web server, or terminal backend.

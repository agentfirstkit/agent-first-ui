# AFUI Host Renderer Contract

A host is trusted local software that renders AFUI documents. Hosts may be web, TUI, native desktop, or embedded into another tool, but their protocol obligations are the same.

## Required Behavior

- Parse `afui` version and `type` before applying messages.
- Treat `ui_snapshot` as the authoritative full document and state.
- Apply RFC 6902 patches atomically; failure keeps the prior snapshot.
- Validate document well-formedness after snapshots and document patches.
- Resolve `_bind` values only through `state`, `record`, and `input` scopes.
- Redact `_secret` values by default.
- Resolve `_uri` values only through host policy.
- Resolve action references by declared `action.id`; unresolved actions fail safe.
- Present `risk` distinctly and apply host-owned confirmation/permission gates.
- Collect missing `input_schema.required` fields before emitting `user_action`.
- Render unknown `kind` values through deterministic fallback.

## Out Of Scope For AFUI Data

AFUI documents cannot request package installation, frontend builds, JavaScript execution, shell commands, component downloads, browser permissions, or host capability installation. Those are host or product distribution concerns.

## User-Owned Host Customization

The restriction above is about untrusted AFUI data. It does not prevent the user
or a user-approved local agent from changing the trusted host itself. A host may
support local CSS/JS assets, an ejectable host directory, preferences, plugins,
or a completely different renderer implementation.

Those customization mechanisms are not selected by AFUI documents and are not
part of the wire protocol. Once enabled by the user, they are trusted host code.
A customized host that claims AFUI conformance still owns the required behavior
above: action preparation, risk gates, redaction, URI policy, patch semantics,
and safe fallback cannot be delegated to producer data.

Hosts should not try to model every possible visual or interaction change as a
protocol field. Common knobs can be host-specific preferences; arbitrary changes
belong in user-owned host files or extensions.

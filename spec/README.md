# Agent-First UI — Specification

The specification is split into a small **normative** core and an **optional**
display profile. Both share one version string: **`0.2`**.

| File | Status | What it defines |
| --- | --- | --- |
| [`protocol.md`](protocol.md) | **Normative** | The space model: ownership, facts, emit/request, capabilities and risk, the binding grammar, the wire format, composition, and the safety boundary. This is the protocol. |
| [`afui.schema.json`](afui.schema.json) | **Normative** | Machine-checkable validation of the wire envelope and the load-bearing layer only — strict where deviation breaks or breaches, permissive where viewer presentation is in charge. |
| [`view-hint.md`](view-hint.md) | **Optional** | The View Hint Profile: the recommended, opt-in way to interpret advisory hints — canonical unit kinds, their fields, layout, salience, and presentation — so independent viewers converge on a strong default with no AI. |
| [`afui.view-hint.schema.json`](afui.view-hint.schema.json) | **Optional** | Machine-checkable validation of a space that opts into the View Hint Profile (canonical kinds, field shapes, salience/layout hints; rejects legacy hint names). |
| [`fixtures/`](fixtures) | — | Wire-format fixtures used by the SDK tests. |

## How to read it

- **Adopting the protocol, or building any viewer:** read [`protocol.md`](protocol.md).
  It is the whole contract for being a safe owner or viewer. An agent applying the
  protocol can stop here — the [Agent Skill](../skills/agent-first-ui.md) distills it.
- **Building a viewer that renders a strong default, or an owner that wants reference
  viewers to render it well:** also read [`view-hint.md`](view-hint.md). It is
  optional — ignoring it costs convenience, never safety.

The protocol is data-only: observed values are inert, and nothing in a space runs on
a viewer. That guarantee lives entirely in the normative core — the optional profile
only ever makes a surface nicer, never wider in authority.

---
name: agent-first-ui
description: "Ask a person questions with suggested answers and room for their own words, or deliver a custom interface through a scoped window or attention-bounded remote page. Use when a person should answer through AFUI, a step needs a human-facing page, embedding a UI session in a Provider, or managing `afui frontend` overrides."
allowed-tools: Bash, Read
---

# Agent-First UI

Use AFUI for questions with suggested answers and supporting evidence, or for a
custom page whose structure helps a person decide or work. AFUI delivers one
interface to one person and returns one terminal outcome. It transports typed
values without interpreting the Provider's domain payloads.

Keep ordinary chat questions in chat unless the person requests AFUI, needs to
answer from another device, or needs context a separate page presents better.
Use built-in `afui ask` for questions, including image/table/diff context and
several related questions on one page. Use a Provider for domain operations,
dynamic forms, and sustained interaction.

## Read only the relevant route

| Task | Read |
| --- | --- |
| Ask questions, supply context, detach, or collect answers | [ask.md](references/ask.md) |
| Build a Provider page, choose runtime primitives, or test an integration | [providers.md](references/providers.md) |
| Find, open, close, or remotely view existing sessions; recover unattended cards | [sessions.md](references/sessions.md) |
| Edit, check, preview, or enable a frontend replacement | [frontends.md](references/frontends.md) |

For exact flags and legal invocation combinations, run the relevant command's
`--help`, such as `afui ask --help` or `afui session serve --help`. Installed
references work offline; checkout API examples live in `docs/providers.md` and
`docs/ask.md`.

## Core Rules

- Parse Agent-First Data events on the resolved output destinations. By default,
  finite commands send results to stdout and progress and errors to stderr;
  capture both streams. `--output-to` can override this routing. Success is a
  `kind:"result"` event whose business `code` sits inside `result`; failure is
  `kind:"error"` with `error.code`, `error.message`, and often `error.hint`.
- Exit 2 means the invocation was rejected before anything ran, and `error.code`
  is a `cli_*` code — retrying the same call unchanged cannot help. Exit 1 means
  the command ran and failed, carrying its own domain `error.code`.
- Exit 4 means output could not be written. The action may already have run;
  inspect its state before retrying, especially when collecting an answer.
- A session is one person, one interface, and one bounded lifetime with one
  terminal outcome. `provider_id`, `ui_kind`, and `session_id` route it back to
  its owner. A Provider may attach the AFUI runtime for retained state, typed
  calls, progress, ordered events, and managed blobs while it remains open.
- AFUI owns the credential, the origin checks, the isolated window, and the
  cleanup. The Provider owns the Router, the HTML, the typed API, and what
  completion *means*. Do not ask AFUI to render or validate domain data — it has
  no opinion about cases, invoices, or queries.
- If the page already exists on another listener, `UiUpstream` makes it a
  source for the same `UiDeliveryPlan`; it does not create a second meaning for
  `link`. AFUI still owns the local window or outer remote page. The source
  owner must keep its private credential live while the delivery exists and
  revoke it afterwards.
- An HTTPS upstream is Window-only. Link and Session require a private HTTP
  source because the AFUI streaming proxy does not terminate upstream TLS.

## Terminal outcomes stay separate from runtime traffic

- `Completed(T)` — a decision session completed. `T` is the person's decision,
  not a suggestion to weigh against your own plan.
- `Closed` — the session ended: a closed window, "End" pressed on a served card,
  or `afui session close`. For a decision session this means no decision; for a
  watched/live surface it is the ordinary terminal outcome. It is not a
  transient runtime reconnect and does not imply consent.
- `Expired` — the credential ran out before they acted. Reopening is legitimate
  here, but say plainly that the first one timed out. This can only happen to a
  managed URL; a window never expires, because the window is what bounds it.

Any amount of runtime traffic may precede these, and none of it completes the
session. Never read `Closed` or `Expired` as consent. If the step needed a
human decision before, it still does.

## Delivery follows the environment, not each command

A session reaches a person as a `window`, a `link`, or a `session` — chosen
once per process by `AFUI_DELIVERY`, overridable per invocation by a
Provider's own `--mode` flag if it has one. Do not assume a step will pop a
window: a terminal that is itself being shown to someone away from the
machine sets `AFUI_DELIVERY=session` for whatever it spawns, so a step that
would otherwise open a window quietly registers instead and waits. If a step
you expected to open a window seems to hang, check that process's environment
for `AFUI_DELIVERY` before assuming something broke, and use `afui session
list` to see what is actually waiting.

`link` binds an AFUI page on the LAN and advertises an unencrypted HTTP URL.
The other device must be able to reach that LAN address; it is not a public
relay or TLS tunnel. Choose it only when that exposure suits the task, and hand
the bearer URL only to the intended person. For an existing protected remote
connection, use `session` with `session serve`; see
[sessions.md](references/sessions.md). A Provider that does not offer `link`
omits it from its `--mode` values.

## Recovery

- `cli_unregistered_combination` means every flag was spelled correctly but the
  mixture is not a registered shape. Read the shapes in `--help` and pick one;
  adding more flags makes it less likely to match, not more.
- `cli_unknown_argument` and `cli_unknown_command` are spelling errors against a
  closed registry — the flag does not exist, so no amount of retrying invents it.
- `ask_window_wait_failed` means a window process exited unsuccessfully;
  `ask_window_unavailable` means it could not launch or exited successfully
  without reaching its page. Neither is a human close or a decision.
- A window that never opens is an environment problem (no supported desktop
  runtime or no display), not an argument problem. Report it rather than
  falling back to printing a URL the person never asked for.

## Setup

Check availability with `afui --version`. A nonzero exit can also mean an output
failure; it is not by itself a reason to reinstall. When setup is requested,
the package is `agentfirstkit/tap/afui` on Homebrew or `agent-first-ui` on Cargo.
`afui skill install` installs the embedded skill and references for Codex,
Claude Code, opencode, and Hermes; choose `--agent` and `--scope` for a narrower
target.

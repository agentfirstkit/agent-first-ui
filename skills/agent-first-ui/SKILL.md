---
name: agent-first-ui
description: "Hand a person a real interface mid-task — a scoped window, a short-lived credential, and one typed answer back — instead of flattening a decision into chat or standing up an ad-hoc web server. Trigger when a step needs human judgement on something page-shaped, when embedding a UI session in a Provider, or when managing `afui frontend` overrides."
allowed-tools: Bash, Read
---

# Agent-First UI

Use this skill when a task reaches a point only a person can settle and the
thing they must judge is shaped like a page rather than a sentence — a set of
drafts to approve, a diff to accept, a form whose fields validate each other.
AFUI delivers one interface to one person and returns one typed answer. It is
delivery infrastructure: it never interprets what is on the page.

For flag-level detail, ask the command itself: `afui frontend --help` returns
every legal shape of the call at once. This skill covers behavior, decisions,
and recovery only.

## Core Rules

- Treat stdout as the protocol: parse Agent-First Data events. Success is a
  `kind:"result"` event whose business `code` sits inside `result`; failure is
  `kind:"error"` with `error.code`, `error.message`, and often `error.hint`.
- Exit 2 means the invocation was rejected before anything ran, and `error.code`
  is a `cli_*` code — retrying the same call unchanged cannot help. Exit 1 means
  the command ran and failed, carrying its own domain `error.code`.
- A session is one person, one interface, one answer. `provider_id`, `ui_kind`,
  and `session_id` are what route that answer back; never reuse a session to ask
  a second question.
- AFUI owns the credential, the origin checks, the isolated window, and the
  cleanup. The Provider owns the Router, the HTML, the typed API, and what
  completion *means*. Do not ask AFUI to render or validate domain data — it has
  no opinion about cases, invoices, or queries.

## Open a window only when the decision is page-shaped

Chat already handles a yes/no. A window earns its interruption when the person
needs to see structure — compare items, scan a diff, fill a form that checks
itself. Reaching for one to ask a question you could have asked in text costs
the user a context switch and buys nothing.

## All three outcomes are answers

- `Completed(T)` — the person decided. `T` is their decision, not a suggestion
  to weigh against your own plan.
- `Closed` — they closed the window without deciding. That is a refusal to
  answer right now, not a transient error: reopening the same window is nagging.
  Report it and stop.
- `Expired` — the credential ran out before they acted. Reopening is legitimate
  here, but say plainly that the first one timed out. This can only happen to a
  managed URL; a window never expires, because the window is what bounds it.

Never read `Closed` or `Expired` as consent. If the step needed a human before,
it still does.

## Finding sessions other processes are running

`afui session list` reports every UI session live on this machine, whichever
Provider owns it. Use it to answer "what is waiting for me?" before assuming a
person has nothing open, and to tell two sessions of the same kind apart by
their `subject`.

`afui session open <id>` puts a window onto one. It does not take the session
over: whoever created it still decides what completing it means, and closing
this window ends this view the same way closing the Provider's own would.

The listing never contains the credential. Do not go looking for a session URL
to hand out — `open` is the sanctioned way to reach one.

## Serving sessions to a person who is not at this machine

`afui session serve` is `open` for someone holding a phone: one page, every live
session on it, each reverse-proxied so that loopback stops being the obstacle.
It is a third delivery of the sessions that already exist, not a place to put
new ones — nothing registers with it and nothing is asked of a Provider.

It is not a remote-access transport and must not be described as one. It binds a
port; the person supplies WireGuard, Tailscale, an SSH tunnel, or a TLS reverse
proxy. Never widen `--listen` on their behalf, and never suggest exposing that
port directly.

Framing is isolation, and there are two ways to get it. By default every framed
UI runs on an opaque origin, which costs it browser storage: an interface that
reads `localStorage` shows its own error there instead of running — afhttp's
KasmVNC takeover panel is the known one. `--session-origin-host <HOST>` moves
each session onto an origin of its own, `s-<credential>.<HOST>`, which restores
storage without loosening anything: the isolation becomes the browser's origin
boundary instead of an attribute on a frame.

It is opt-in because it asks for names. Suggest `--session-origin-host localhost`
freely — browsers resolve `*.localhost` to the machine itself, so it needs no
setup. Suggest any other host only once the person has said that `*.<HOST>`
resolves to that machine and, behind TLS, that their certificate covers it;
without both, every frame fails. The page is then reachable only at `<HOST>`.
Never loosen the frame itself instead — there is no flag for that.

The URL it prints carries the credential for everything running on that machine.
Give it to the person and nowhere else: not into a file, not into a commit
message, not into a chat transcript you do not control.

## Frontend overrides are trusted by content, not by path

`afui frontend` manages Provider-owned interface replacements. Trust is pinned
to an exact content fingerprint: workspace code stays ignored until someone
enables that fingerprint, and any later edit revokes it until it is enabled
again.

That revocation is the guardrail, not an obstacle. After you edit an override,
do not re-enable it to make a check pass — surface the change and let the person
decide, exactly as they did the first time. `AFUI_SAFE_MODE=1` ignores overrides
entirely and is the right setting when you only need the Provider's default.

`dev` and `preview` locate and validate an implementation for the Provider's own
typed server. Neither renders templates nor starts a generic domain server; if
you expected a running app, you wanted the Provider's binary.

An override is a page, never behaviour. AFUI refuses a frontend file whose name
says it is a script and refuses a script hidden inside a template, so the only
code a page runs is the Provider's own, admitted by a per-session nonce. Write
what a control is *for* using the Provider's declared hook and let the Provider
bind what it does; a control the Provider does not recognise binds to nothing,
and a page missing a control the Provider requires is reported as a broken
override rather than opened.

Files are overlaid one at a time. Supply only the ones you are changing: a
directory is not replaced, so a Provider's remaining templates, partials and
language packs keep serving. Replacing a whole tree instead is how a one-page
customisation silently deletes localisation.

If a Provider says a frontend will not load, it did not fall back — nothing you
are looking at is the override, and nothing is the Provider's page either. Fix
the frontend, or set `AFUI_SAFE_MODE=1` to get the Provider's own interface
back. A frontend that is merely untrusted is silent by design; read the
Provider's readiness event for a frontend id to tell "mine is running" from
"mine is inert".

## Recovery

- `cli_unregistered_combination` means every flag was spelled correctly but the
  mixture is not a registered shape. Read the shapes in `--help` and pick one;
  adding more flags makes it less likely to match, not more.
- `cli_unknown_argument` and `cli_unknown_command` are spelling errors against a
  closed registry — the flag does not exist, so no amount of retrying invents it.
- A window that never opens is an environment problem (no Chromium-family
  browser, no display), not an argument problem. Report it rather than falling
  back to printing a URL the person never asked for.

## Setup Checklist

```bash
afui --version || brew install agentfirstkit/tap/afui
afui skill install            # installs this skill for codex, claude-code, opencode, hermes
```

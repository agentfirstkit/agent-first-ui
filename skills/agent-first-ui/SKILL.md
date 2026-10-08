---
name: agent-first-ui
description: "Hand a person a real interface mid-task — a scoped window, an attention-bounded remote page, one typed live runtime, and one terminal outcome — instead of flattening a page-shaped interaction into chat or standing up an ad-hoc web server. Trigger when a step needs a human-facing page, when embedding a UI session in a Provider, or when managing `afui frontend` overrides."
allowed-tools: Bash, Read
---

# Agent-First UI

Use this skill when a task reaches a point whose human interaction is shaped
like a page rather than a sentence — a set of drafts to approve, a diff to
accept, a form whose fields validate each other, or live state with commands
that act on it. AFUI delivers one interface to one person, may carry live typed
values in both directions, and returns one terminal outcome. It is delivery
infrastructure: it never interprets the Provider's domain payloads.

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

## Open a window only when the interaction is page-shaped

Chat already handles a yes/no. A window earns its interruption when the person
needs to see structure — compare items, scan a diff, fill a form that checks
itself, or watch changing state while issuing commands against it. Reaching for
one to ask a question you could have asked in text costs the user a context
switch and buys nothing.

## Use the AFUI runtime; do not define another UI transport

Use `UiSession::with_runtime` when a page and its controlling process exchange
anything before the terminal outcome. The Provider defines typed action, reply,
and state payloads. AFUI owns the explicit envelope, call identity, progress,
result/error completion, cancellation, reconnect, deduplication, bounds, and
session shutdown. The page connects only through `afui.connect()`.

Choose the runtime primitive by meaning:

- `publish_state` for a current snapshot. It is latest-value, never waits for a
  viewer, and is delivered before a new or reconnecting page becomes live.
- `UiCall` for a surface action. Reply through `progress`, `finish`, or `fail`;
  never add a transport request id to the action or reply type.
- `publish_event` only when each occurrence matters. AFUI assigns the sequence
  and keeps a bounded replay window.
- `with_runtime_and_blobs` for bytes too large or unsuitable for JSON. Declare
  the size and media-type policy; AFUI owns URLs, safe headers, release,
  eviction, and cleanup.

For a long-lived listener using `UiMountAccess`, mint the same capability with
`issue_with_runtime` or `issue_with_runtime_and_blobs`. The returned runtime is
owned by that one credential and closes when the credential ends. Use the
capacity variants only when measured bounds require them.

Three delivery facts decide how the page must be written, and none of them is
visible from a page that happens to work:

- Republishing an equal state still reaches the page, so a unit-like state
  used purely as an invalidation is a supported shape.
- Only the newest state is guaranteed to arrive; faster publication coalesces.
  Publish what is true, never a delta.
- State, events, and call results are not ordered against each other. Either
  the state carries the data and the reply only ends the wait, or the reply
  carries the data and the state only says that something changed. Never split
  one change across both, and make every refresh idempotent so a later one
  wins.

An opening state is optional. A page whose first screen is server-rendered
receives no state frame and takes `live` as its only opening signal.

Connection state is AFUI's word, not a domain state: bind it with
`afui.connection(element, words)` so the baseline dresses it and the page
keeps the wording. The handler receives `live`, `reconnecting`, and `closed`;
`connecting` is the state a page starts in and is never delivered.

Read the kernel's primitive table before hand-writing what is in it; three
lines look cheaper than a lookup right up until the copy stops tracking what it
copied. `afui.status(element)` is the one this keeps happening to — a status
line is `textContent` plus `data-state`, and writing those two by hand is how a
page ends up with its own slightly different idea of what clearing means. Use
`afui.inFlight(elements)` for a control that is not a native form element,
where `disabled` alone does nothing and `aria-disabled` is the whole of it.

Do not draw a header inside a frame. Every page here grew the same one — the
program's name, what the session is on, a word about how it is going — and a
framed session shows the first two twice. Ask with `afui.frame`: `onHead(true)`
means hide yours and send up `title` and `status` instead. Never send identity;
that is the listener's word, and a page putting its own name in a head it does
not own is the one thing this channel refuses.

A second screen inside a page — a record opened over a list, a file over a
tree — is a place, not a panel. Declare it with `afui.place` so the system back
gesture, Escape, the control you drew and the navigation control of a frame
around your page are one way out with one result. A detail view drawn without
one is a page where back leaves the page from under the person, which on the
width that covers the list completely is the first gesture they reach for. Keep
drawing your own control: `onFrame` is for dropping the second one when a frame
already offers navigation, not for having none in a window or a direct link.

Do not add a Provider WebSocket, SSE endpoint, sequence field, reconnection
flag, overflow fallback, or fetch-based RPC beside this runtime. If durable
domain state must survive the session itself, persist it through an explicit
typed action; retained UI state is not a database.

Provider integration tests use the `test-support` feature's `RuntimeClient`.
Give it the issued access URL and typed domain values; it owns the real
WebSocket, AFUI envelope, request identity, and acknowledgements. Do not write
a second protocol client in a test. A synchronous process test uses
`BlockingRuntimeClient`; both client forms expose `call_with_progress`.

## Build the Provider page for the person, not the event

AFUI deliberately has no shared domain renderer, so the Provider must do the
human translation before it opens a session. Do not hand a template the raw
union, row set, protocol envelope, or generic field map merely because that is
what the agent consumed. Build a typed document around the thing the person is
looking at: one object with its attached facts, a scannable group of related
objects, or one decision with everything needed to refuse it safely. Preserve
the untouched source behind an optional details section when verification
matters; summarising is not permission to hide evidence.

Use the smallest page shape that matches the task:

- Put the fact or action the session exists for first. Context follows it;
  implementation identifiers and raw records do not compete with it.
- Collapse repeated history and technical detail by default, while keeping the
  newest or currently actionable item open.
- Render safe visual material as visual material. A filename is not a preview,
  and an image someone must judge should not require a second tool to see.
- Put the safe refusal before a consequential approval in source and focus
  order. Closing or expiry still refuses at the typed Provider boundary.
- Say what is saving, stale, disconnected, expired, or complete next to the
  thing affected. Never make color the only carrier of that state.

The built-in frontend is real product UI, not a debugging fallback. Link the
baseline stylesheet AFUI serves (`{{ afui.base_style_path }}`) before your own
and build on top of it: both themes, the focus ring, a working `hidden`, the
in-flight look of a disabled control and the inset that clears a phone's edges
are already there, and a page that hand-writes its own palette instead will
drift from every other interface on the machine without anything noticing.
Redeclare an `--afui-*` property to disagree with a value; never fork the
floor. Beyond it the page must still work at a coarse-pointer touch size,
reflow without horizontal page scrolling, and keep status changes available to
assistive technology without announcing every keystroke. Use native HTML controls and disclosure elements before inventing
custom behavior. A replacement may change presentation; the Provider's trusted
runtime remains the only owner of behavior and decision routing.

These are Provider responsibilities, not a reason to grow AFUI into a component
library. Similar-looking pages may share principles and still keep separate CSS,
typed documents, templates, and runtimes when their human tasks differ.

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

## Finding sessions other processes are running

`afui session list` reports every UI session live on this machine, whichever
Provider owns it. Use it to answer "what is waiting for me?" before assuming a
person has nothing open, and to tell two sessions of the same kind apart by
their `subject`. Each entry also reports `waits_for_decision` — a stopped
agent versus an open monitor someone merely has up — and
`announced_at_epoch_ms`, so several waiting entries can be read oldest-first
rather than by whichever happens to sort first alphabetically.

`afui session open <id>` puts a window onto one. It does not take the session
over: whoever created it still decides what completing it means, and closing
this window ends this view the same way closing the Provider's own would.

`afui session close <id>` ends the session itself, not merely a view of it —
whoever created it gets `Outcome::Closed`, indistinguishable from a person
closing it themselves. Use it to cancel a session you started that no longer
needs an answer, never one someone else is using: ending is available to
anyone who can already reach or list the session, the same access `session
open` already assumes, not a reason to reach for it more freely.

The listing never contains the credential. Do not go looking for a session URL
to hand out — `open` is the sanctioned way to reach one.

## Serving sessions to a person who is not at this machine

`afui session serve` is `open` for someone holding a phone: one AFUI-owned
remote page, every live session on it, each reverse-proxied so that loopback
stops being the obstacle. It is a view of sessions that already exist, not a
place to put new ones — nothing registers with it and nothing is asked of a
Provider.

It is not a remote-access transport and must not be described as one. It binds a
port; the person supplies WireGuard, Tailscale, an SSH tunnel, or a TLS reverse
proxy. Never widen `--listen` on their behalf, and never suggest exposing that
port directly.

Framing is isolation, and there are two ways to get it. By default every framed
UI runs on an opaque origin, which costs it browser storage: an interface that
reads `localStorage` shows its own error there instead of running, which
third-party remote-framebuffer clients commonly do. `--session-origin-host <HOST>` moves
each session onto an origin of its own, `s-<credential>.<HOST>`, which restores
storage without loosening anything: the isolation becomes the browser's origin
boundary instead of an attribute on a frame.

One more thing it buys: with an origin per session, each card can be opened as a
page of its own. Without it that control is absent — not broken — because a
top-level navigation would put the Provider UI on the page's own origin and end
the isolation the frames rely on.

It is opt-in because it asks for names. Suggest `--session-origin-host localhost`
when the browser and service run on the same machine, or the browser's device
already forwards its local port to the service — browsers resolve `*.localhost`
to their own machine. For a phone over a VPN or another remote connection,
suggest a host only once the person has said that `<HOST>` and `*.<HOST>` resolve
from that device to the service machine and, behind TLS, their certificate covers
the wildcard; without both, every frame fails. The page is then reachable only
at `<HOST>`.
Never loosen the frame itself instead — there is no flag for that.

The URL it prints carries the credential for everything running on that machine.
Give it to the person and nowhere else: not into a file, not into a commit
message, not into a chat transcript you do not control.

That URL warns after fifteen minutes without attention and stops working five
minutes later. Direct `link` delivery uses this same AFUI-owned page and the
same policy; it is not a second kind of expiry chosen by the Provider. The
defaults come from the global AFUI `config.json` under
`attention.idle_timeout_s` and `attention.grace_period_s`
(`~/Library/Application Support/afui/config.json` on macOS,
`$XDG_CONFIG_HOME/afui/config.json` on Linux (or
`~/.config/afui/config.json` when `XDG_CONFIG_HOME` is unset), or
`%APPDATA%\afui\config.json` on Windows). `AFUI_CONFIG_DIR` replaces that
file's parent directory. `--idle-minutes` and `--grace-minutes` override them
only for one `session serve` process. So a person reporting
"the page went blank" or "the terminal card is gone" has usually walked away, not
hit a bug: a lapsed card comes back with *Reopen*, and a lapsed page needs a new
remote URL (rerun the direct delivery or `session serve`). Do not disable
attention expiry merely to make either complaint go away. It is the right
answer only when the person says they want a view that outlives them leaving,
and it is theirs to ask for, because what it turns off is the thing that stops
their terminal being readable by whoever picks the device up.

A person who says a card wastes the screen wants the size control in its head —
Short / Tall / Full, remembered per Provider and UI kind. Do not suggest they
reload, and do not go looking for a way to make a card fit its contents: what is
inside a frame is on an origin the page cannot measure, and that is deliberate.

## Frontend overrides are trusted by content, not by path

`afui frontend` manages interface replacements — Provider pages, and AFUI's own
session list under `afui frontend init afui sessions`. That page is held to the
same contract as any other, with one thing worth saying out loud when a person
asks for it: its URL carries the credential for every session on the machine, so
an override is markup and CSS only and the trust gate is the whole guardrail.
Never enable one on somebody's behalf. Trust is pinned
to an exact content fingerprint: workspace code stays ignored until someone
enables that fingerprint, and any later edit revokes it until it is enabled
again. A session that is already running keeps serving the bytes that were
trusted when it started — editing a file does not change what is on screen, and
it does not sneak past the gate either.

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

`assets/app-icon.svg` is part of that same overlay. It identifies the Provider
frontend being shown, so it follows the page into its browser favicon and Dock
icon. Do not replace it with an AFUI-wide icon: only AFUI's own `afui sessions`
frontend uses the AFUI icon. If you customise the asset, its bytes participate
in the same trusted fingerprint as every other frontend file.

If a Provider says a frontend will not load, it did not fall back — nothing you
are looking at is the override, and nothing is the Provider's page either. Fix
the frontend, or set `AFUI_SAFE_MODE=1` to get the Provider's own interface
back. A frontend that is merely untrusted is silent by design; read the
Provider's readiness event for a frontend id to tell "mine is running" from
"mine is inert".

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

A `link` URL is the one delivery that reaches another device with no tunnel in
the way — treat it exactly like the `session serve` URL above and hand it only
to the person it is for. Not every Provider offers `link`; one that does not
simply does not list it as a `--mode` value, which is a capability being
absent rather than broken.

## Recovery

- `cli_unregistered_combination` means every flag was spelled correctly but the
  mixture is not a registered shape. Read the shapes in `--help` and pick one;
  adding more flags makes it less likely to match, not more.
- `cli_unknown_argument` and `cli_unknown_command` are spelling errors against a
  closed registry — the flag does not exist, so no amount of retrying invents it.
- A window that never opens is an environment problem (no supported desktop
  runtime or no display), not an argument problem. Report it rather than
  falling back to printing a URL the person never asked for.

## Setup Checklist

```bash
afui --version || brew install agentfirstkit/tap/afui
afui skill install            # installs this skill for codex, claude-code, opencode, hermes
```

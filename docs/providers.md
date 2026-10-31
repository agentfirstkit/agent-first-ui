# Building a Provider UI on AFUI

The README covers what AFUI hands you. This covers the decisions you make before
using it: whether the thing you have is a session at all, what an
override is allowed to do to it, and — if your page returns an answer that
matters — how to keep a replaced template from redefining what that answer is.

## Is it a session?

A UI that fails any one of these tests is not a session, and forcing it
into one produces something that looks alive and answers nobody.

1. **There is a web UI.** Either you serve it, or you point at one that already
   exists somewhere else.
2. **The ending has a name.** A session that waits for a decision ends when the
   person submits something; a session someone just watches ends when they close
   it. There is no third kind that stays open forever.
3. **A process's lifetime bounds it.** Whoever created the session owns it: when
   that process exits, the session ends and its credential is revoked. Nothing
   outlives its owner.

A notification with no UI fails the first. A page that keeps serving after the
command that made it has exited fails the third — that is not a session, it is a
server you forgot about.

## What a session's shape decides

A session that **waits for a decision** returns a typed value: `Outcome<T>` where
`Completed(T)` is the person's answer. A session someone **watches** has `T = ()`
and never completes — it ends `Closed`.

That difference is not cosmetic. It decides the terminal outcome, but it does
not give a Provider a second remote-lifetime policy:

- **A window bounds itself.** The person can see it; leaving it open all
  afternoon is still using it. Window delivery never expires, and
  `Outcome::Expired` cannot occur.
- **An AFUI-owned remote page has no physical bound.** Direct `link` delivery
  and `afui session serve` therefore share AFUI's attention heartbeat, warning,
  grace period, and global configuration. A Provider supplies no timeout.
- **A registered session is not a remote page.** Its owner process bounds it;
  each later AFUI view applies the remote policy without expiring the session
  underneath.

Registry entries are published as complete files by an atomic replacement
from a private temporary file in their directory; on Unix each entry is `0600`
under a `0700` directory. Listing skips unreadable or malformed records and
leaves them in place rather than treating them as ended sessions; it prunes
only a successfully parsed entry whose session is no longer live — its host
stopped answering, or on Unix its owner process is gone.
`AFUI_NO_REGISTRY=1` keeps a process's sessions out of the registry entirely.

On Unix, registry liveness requires both an existing owner process and a host
that accepts a TCP connection. On Windows, the registry checks host
reachability only. An AFUI-served session has its host and owner in the same
process, so that process exiting also stops the host. An announced upstream
can have a separate host: if its announcing process crashes on Windows while
the host continues to answer, its listing can remain until that host stops.
The upstream owner remains responsible for revoking its credential when the
delivery ends.

Getting this backwards is the common failure: putting a timeout on the session
itself kills work a person may still be using through another view, while
putting separate clocks on direct and shared pages gives the same AFUI URL two
different promises.

### Say so when a `T = ()` session still waits on a person

`afui session list` reports `waits_for_decision` for every entry, so a person
or an agent looking at the list can tell a stopped agent from an open monitor
before assuming nothing is blocked. It is `true` automatically for any session
whose `T` is not `()` — handing a value back is, by definition, waiting on
whoever supplies it. It has no way to see the other shape on its own: a
session with `T = ()` that a person is nonetheless expected to act on — several
unrelated items settled one at a time, say, with nothing ever handed back.
Call `UiSession::awaiting_person` on that session to say so.

The rule this implies: **a session that does not wait for anyone should use
`T = ()`.** Reaching for a unit struct instead of `()` just to look distinct
gets automatically counted as waiting — harmlessly, since that direction is
always safe, but not what you meant.

## The live runtime is orthogonal to the terminal outcome

Some interfaces are not one request followed by one answer. A surface may show
live measurements while accepting inspection commands, or receive progress
while a person refines work in progress. That still has one bounded session
lifetime and one terminal `Outcome<T>`; intermediate traffic uses
`UiSessionRuntime<Action, Reply, SurfaceState>`.

Attach it with `UiSession::with_runtime`. The returned runtime is the Provider
side; the trusted page calls `afui.connect()`. AFUI owns the only transport
protocol between them: the versioned envelope, request identity, latest-state
replacement, event sequence and replay, reconnect, call deduplication,
cancellation, bounds, and shutdown. Provider payload types contain none of
those fields.

### Current state: replace, never queue

Use `publish_state` for the complete value a newly connected page should see.
AFUI retains exactly the newest state, assigns its revision, and sends it before
the connection becomes live. Publishing never waits for a viewer, so a timer
continues running when the page is unopened or temporarily disconnected. A
reconnect needs no Provider-owned `resumed` flag and no overflow fallback.
Concurrent publications are linearized with their revisions, so an older
revision cannot race back into the retained slot after a newer one.

These properties of that retention decide how a page has to be written, and a
page that guesses wrongly at any of them fails in a way nothing here would
catch:

- **Republishing an equal value still reaches the page.** Nothing compares one
  state to the last; the revision advances and the frame is sent. So a
  Provider whose page only needs to know *that* something changed may publish
  one unit-like state as an invalidation and let the page pull what it needs.
  That is a supported shape, not an abuse of one.
- **Only the newest publication is guaranteed to arrive.** Publishing faster
  than a socket drains coalesces, exactly as it does for a page that has not
  connected yet. A state carrying a delta — "three rows were appended" — is
  therefore wrong, and wrong only under load, which is the worst schedule to
  find out on. Publish what is true, not what changed.
- **An opening state is optional.** A runtime nobody has published to sends no
  state frame at all, and the page still becomes live. If your first screen is
  rendered by the server, that is already your shape, and `live` is your only
  opening signal.

### Nothing is ordered against anything else

Retained state, ordered events and a call's result are three separate channels
sharing one socket, and AFUI orders none of them against the others. A call
that changes what the state shows may have its result arrive before the new
state or after it. An event's sequence orders it among *events*, and says
nothing about anything else.

This is not a gap waiting to be closed. Ordering them means holding a
publication behind a reply, and `publish_state` never waiting for a viewer is
worth more than an ordering that a reconnect would break anyway.

So *the page did something, and now what it shows is different* has these
working shapes:

- **The state carries the data.** The reply says only that the work finished,
  or why it did not — enough to stop the spinner. What the person sees comes
  from the next state.
- **The state says only that something changed.** The data arrives in the
  reply, or in a call the page makes when it hears a state.

The shape that does not work — half the new data in the reply, half in the
state — is the one that needs the ordering that does not exist. It works until the two frames
arrive the other way round, and then shows a mixture of two moments with
nothing logged. Pick one channel for the data and let the other one only say
*when*, and make the refresh idempotent so that a later one simply wins.

### Surface actions: one typed call handle

`recv` yields a `UiCall<Action, Reply>`. Read the action with `call.action()` and
answer only through the handle:

- `progress(&reply)` sends a non-terminal update;
- `finish(&reply)` completes successfully;
- `fail(UiCallError)` completes with a caller-safe code, message, optional
  hint, and retryability;
- `cancelled()` lets long work stop when the page cancels it or the session ends.

The page's `runtime.call(payload, {onProgress, signal})` returns one promise.
AFUI assigns the request id, resends an unacknowledged call after a wire drop,
and replays an already completed result rather than handing the action to the
Provider twice. A call the page cancelled while the wire was down is not
resent; only its cancel goes out, which ends it on a runtime that did receive
it and is nothing to one that did not. The action is captured as one JSON value when `call` begins;
later page mutation cannot change what a reconnect resends. Never add
`request_id`, reconnect tokens, or acknowledgements to `Action` or `Reply`.

### Ordered events: only when every occurrence matters

Use `publish_event` for an occurrence rather than a snapshot. AFUI assigns the
sequence and retains a bounded replay window. A reconnect receives missed
events or an `onEventReset` range when the cursor fell outside that window.
An old cursor presented to a newly empty runtime resets to `0/0`, so the next
event starts at one instead of being mistaken for an already-seen event.
Most screens want `publish_state`; choosing events merely because the source is
a stream recreates a backlog where only the newest screen matters.

### Managed byte bodies

Use `with_runtime_and_blobs` when a reply needs bytes that do not belong in
JSON. Declare a `UiBlobPolicy` with the maximum body size and admitted media
types. `store_blob` returns `UiBlobRef`; the page uses `blobUrl`, `fetchBlob`,
and `releaseBlob`. AFUI owns the unguessable session-relative URL, `no-store`,
`nosniff`, inline disposition, bounded retention/eviction, and cleanup when the
session ends. Do not invent a private download route for per-call UI bytes.

The default runtime admits 32 unacknowledged calls in total, ordered-event
replay holds 128 events, and every encoded frame is limited to 1 MiB. A full
call bound produces a caller-safe, retryable `ui_runtime_busy` result instead
of blocking the session transport. Use the explicit-capacity methods when
those bounds are part of the interface's measured shape. Build the page policy
with `allow_runtime()`; a runtime attached to an AFUI-assembled policy that
forbids same-origin connections is rejected before the page opens.

## Delivery: `window`, `link`, or `session`

A session reaches a person one of three ways. `UiDeliveryPlan` resolves all
three through one plan:

| Mode | Delivered as | Expires |
| --- | --- | --- |
| `window` | an isolated window this machine opens | never — the window is the bound |
| `link` | an AFUI page that proxies exactly this loopback session | the global AFUI attention policy: 15 minutes idle plus 5 minutes warning by default |
| `session` | registered only, reached later through `afui session open` or `afui session serve` | the session never expires; each remote view uses the same AFUI attention policy |

```rust
// `resolve` for a Provider that stays local; `resolve_allowing_link` for one
// that offers the LAN page as well.
let outcome = UiDeliveryPlan::resolve_allowing_link(explicit_mode)?
    .deliver(session, router)
    .await?;
```

When the caller must announce the AFUI-owned Link URL before it blocks, split
the same plan at its handoff point:

```rust
let active = UiDeliveryPlan::resolve_allowing_link(explicit_mode)?
    .start(session, router)
    .await?;

if let Some(url) = active.link_url_secret() {
    publish_secret_link(url)?;
}

let outcome = active.wait().await?;
```

`link_url_secret()` is available only for `link`. It is named as a secret on
purpose: the credential is part of the URL, so publish it only through the
intended private handoff. `deliver` remains the shorter `start` + `wait` form
for callers that do not need the URL in between.

`resolve` reads one thing besides its argument: `AFUI_DELIVERY`, at the fixed
priority `explicit --mode > AFUI_DELIVERY > window`. Wire your own `--mode`
flag with these same three words and pass `None` for `explicit` when the
person did not give it. **Do not give that flag a default value of its own** —
a default there is indistinguishable from someone having typed `--mode
window`, so `AFUI_DELIVERY` would never be reached and an unattended run would
always pop a window nobody is there to see.

`link` is opt-in per Provider: build the plan with `resolve` and a resolved
`link` — from either source — is refused **there**, naming the modes that are
on offer, and your own `--mode` flag should not list `link` as a legal value
either. The refusal lands at `resolve` rather than at `deliver` on purpose: a
Provider normally announces its resolved delivery before it blocks, and a
refusal that arrives after that announcement produces a readiness report that
was false when it was written.

The opt-in is only permission to bind a LAN listener. Lifetime is not part of
it: `attention_policy()` is report-only, and the intervals come from AFUI's
global `config.json` under `attention.idle_timeout_s` and
`attention.grace_period_s`.

### Reporting a ready delivery

Everything a readiness report needs is one call, on either kind of active
delivery:

```rust
let facts = active.facts();
```

| Field | Meaning |
| --- | --- |
| `mode` | the resolved delivery, spelled the way a person types it |
| `session_id` | stable identity, shared with this delivery's registry entry |
| `link_url_secret` | the bearer URL; present for `link` and nothing else |
| `idle_timeout_s` | how long this delivery may sit unattended; absent when nothing lapses |
| `grace_period_s` | how long the warning runs; present exactly when there is an attention clock |

`UiDeliveryFacts` is facts and nothing else — no sentence to print, no notion
of which field matters more, no opinion on whether a field belongs in your
output at all. Those are yours. The names follow the conventions a structured
output layer reads (`_s` for seconds, `_secret` for a bearer capability), so
splicing the whole structure into an event gets correct handling by default;
a Provider that deliberately publishes the URL renames that one field itself.
`Debug` prints `***` for `link_url_secret`, and `UiSessionEntry`'s `Debug`
does the same for both of its URLs, so `{:?}` in a log line never carries a
credential.

`UiDeliveryPlan::mode()` reads the same resolved word before anything is
served, for a Provider that has to name the delivery earlier still. Do not
resolve `AFUI_DELIVERY` a second time to learn it: two reads of one decision
are two chances to disagree. `UiDeliveryMode::description()` is one factual
sentence per delivery, for a Provider that wants one and does not want to
write its own three-armed match over the same three facts.

`Outcome::ending()` is the counterpart at the other end: `completed`,
`closed`, or `expired`. The completed case usually deserves a richer word of
your own — what the person actually decided — but the other two carry no
domain meaning, and every Provider spelling them for itself is how one tool's
`closed` becomes another's `cancelled`.

### `link` puts a person's data on the network unencrypted

`link` is the one delivery that reaches another device with no tunnel in the
way: plain HTTP on this machine's own LAN-reachable address, with no
authentication beyond the credential already in the URL. Use it only on a
network you trust, and treat that URL the way you would treat a password —
give it to the person it is for, and nowhere else: not into a file, not into a
commit message, not into a chat transcript you do not control.

### When the page is not this crate's to serve

A Provider that already answers on a listener it bound itself hands AFUI no
`Router`. Name that private source with `UiUpstream` and give it to the same
plan:

```rust
let active = UiDeliveryPlan::resolve_allowing_link(explicit_mode)?
    .start_upstream(
        UiUpstream::new("inspector", "console", upstream_url_secret)?
            .with_subject("running service"),
    )
    .await?;

if let Some(url) = active.link_url_secret() {
    publish_secret_link(url)?;
}

let outcome = active.wait().await?;
```

The meanings do not change for an upstream. `window` opens its private URL
locally. `link` starts an AFUI-owned LAN listener and fixed-session shell; the
upstream URL remains private behind the proxy, so its own listener may stay on
loopback. `session` only registers it and has no AFUI expiry. The Provider may
race `active.wait()` against its own shutdown signal; cancelling the wait drops
the announcement.

AFUI cannot mint or revoke a credential on a listener it does not own. The
upstream contract is therefore narrow: its owner keeps that private URL usable
for as long as the `ActiveUiUpstreamDelivery` exists and revokes it afterwards.
An internal crash-cleanup lease is fine; exposing that lease as the Link or
Session lifetime is not. The person-facing remote lifetime remains AFUI's
attention policy alone.

`window` may open an HTTP or HTTPS upstream directly. `link` and `session`
require a private HTTP upstream: both may pass the page through AFUI's
streaming proxy, which intentionally does not act as a TLS client or trust
arbitrary upstream certificates.

## Ending a session from outside it

Every session answers `POST <access_url>` + `agent_first_ui::SESSION_END_PATH`
under its own credential, on every delivery: `afui session close <id>` sends
it, and so does the "End" control on a served session's card. Whoever created
the session gets `Outcome::Closed` back — the same outcome as a person closing
its window — never a manufactured `Completed(T)`. However a session ends, its
credential stops working, and a Provider response still streaming under it —
an event stream, a long poll — ends with it. A WebSocket the Provider upgraded
itself is not ended this way.

The threshold for ending a session is the same as the threshold for seeing
it: whoever holds the URL already holds the whole credential, and can already
do worse from inside a Provider's own page (a terminal's `exit`, for one), so
refusing to let them end it from outside would be a boundary that only looks
like one.

**The whole `__afui/` prefix is reserved.** `SESSION_END_PATH` is the one public
external-control path; live-runtime paths are deliberately not public API.
Do not route on any name under the prefix, proxy through it, or otherwise give
it a second meaning in your own Router — this crate's dispatch answers for the
whole prefix before your router is ever reached, so a route of your own under
it is simply unreachable, not a conflict you would notice at runtime.
Reserving the prefix rather than only today's paths lets AFUI add another later
without silently colliding with something you already routed there.

## What an override may and may not do

An override supplies **presentation**. It may restructure freely — reorder,
regroup, drop, rename, restyle — and that freedom is the point: an agent
customises a Provider's UI by writing files, not by editing the Provider.

These limits hold regardless of what a template contains.

**Runtime data may not choose or inject the UI code that runs.** Data flows
*into* a template. Data must never select *which* template executes. If a domain
value can reach the page as markup rather than as text, that boundary is already
broken — escape everything, including the values a provider returned to you.

**The server stays authoritative.** An override can be wrong, ugly, or send a
malformed request. It cannot bypass typed validation, a lock, a base hash, an
idempotency key, or a plan/confirm boundary. A page that says "saved" has not
changed any domain state by saying so.

**A frontend supplies no behaviour.** AFUI refuses an override file whose name
says it is a script, and `reject_frontend_script` refuses one hidden inside a
template it does supply. The only JavaScript your page loads is yours.

The guard also refuses markup that runs no script but still sends the person, a
request, or the page's credentialed address somewhere: `<meta http-equiv>` (a
refresh navigates without a click), `<meta name=referrer>` and `referrerpolicy`
attributes (either would undo the session's `no-referrer`), the form request
attributes `action`, `formaction`, `method` and `formmethod`, and the `frame`,
`frameset`, `applet` and `base` elements beside `script`, `iframe`, `object`
and `embed`. A form your runtime reads is fine; a form that posts by itself
goes to whatever route it names, past the decision its button is labelled
with. In a rendered page `javascript:`, `vbscript:` and `data:` count only in
an attribute a browser loads or navigates to — `href`, `src`, `srcset`,
`poster`, `ping`, SVG animation values and the like — so a `title` or
`aria-label` that mentions one is text.

`UiPage::builder` (the `page` feature) assembles these limits for you: the
escaping policy, the render, the rendered-markup guard, and a check that every
element id your runtime binds to actually exists in the output — a page
missing one is reported by that id rather than opened as a window that does
nothing. It knows nothing about what your elements or your document mean; it
only checks that what you declared is there. A page missing several is
reported once, naming all of them: somebody writing a replacement cannot read
your contract from where they stand, and finding it one render at a time is
the same fix spread across as many sittings as there are ids.

The builder also covers template trees without weakening that boundary.
`entry_at` and `template_at` separate a stable logical template name from its
frontend-relative file path. `UiFrontend::files_under` plus
`override_template` registers open-ended partial directories owned by an
override. `requires_element_map` derives the runtime's required ids from the
same serializable map exposed to templates, so a Provider need not maintain a
second list by hand.

Trusted Provider code may use `extend_environment` to add plain-value filters
and globals. The extension receives only `UiPageEnvironment`: it cannot add a
loader, change escaping or undefined-value behaviour, restore the reserved
`safe` filter, or replace the `document` global. Filter/global values also
cannot be raw MiniJinja `Value`s, so an extension cannot smuggle the engine's
“already safe” bit back to a frontend under another name. Those invariants
stay owned by `UiPage`, including when a page needs locale-aware formatting or
another domain-specific projection. A filter that needs to return a sequence
or map selected from its input can use `UiPageSerializedValue`; it rebuilds the
data through JSON and deliberately drops engine-only safety provenance.

## Errors: what happened, and what to do about it

`Error`'s `Display` says what happened. It does not say what to do — that is
`hint()`, and keeping the two apart is what stops a caller with a hint field of
its own from printing the same sentence twice, once inside the message and once
beside it.

```rust
Err(error) => fail(
    &format!("myprovider_{}", error.kind()),
    &error.to_string(),
    error.hint(),
),
```

A coarser code than that is yours to build, but build it out of named arms
rather than a default. A fallback that names one cause — "the frontend could
not be read" — reports a closed session runtime and a machine with no browser
as a broken frontend, and sends somebody to look at a file that is fine. If
you want a fallback at all, let it carry AFUI's own word through.

`kind()` returns `UiErrorKind`, a closed enum with no domain in it, so your own
error code is a prefix rather than a match over every AFUI variant. It is an
enum rather than a string on purpose: translating a dependency's failures into
your own vocabulary is your job, but you can only do it *completely* if
something tells you when you have missed one, and nothing exhaustively matches
a string. A `&'static str` would force every caller into a wildcard arm, which
is where a classification added later is mis-reported.

```rust
match error.kind() {
    UiErrorKind::WindowUnavailable | UiErrorKind::WindowWaitFailed => ...,
    // no wildcard: a classification added to AFUI stops this compiling
}
```

It is deliberately not `#[non_exhaustive]`, for that reason. `Display` and
`as_str()` give the stable `snake_case` word when a prefix is all you want.

| `kind()` | `as_str()` | Raised by |
| --- | --- | --- |
| `InvalidArgument` | `invalid_argument` | an identifier, URL, TTL, or icon AFUI cannot accept |
| `DeliveryModeInvalid` | `delivery_mode_invalid` | `--mode` or `AFUI_DELIVERY` named something other than the three words |
| `DeliveryModeNotOffered` | `delivery_mode_not_offered` | the resolved delivery is one this Provider did not offer |
| `UpstreamNotProxyable` | `upstream_not_proxyable` | an `https://` upstream behind a delivery that proxies plain HTTP |
| `LinkAddressUnavailable` | `link_address_unavailable` | no LAN-reachable address to advertise a `link` on |
| `RuntimeMisconfigured` | `runtime_misconfigured` | a second runtime, a zero capacity, or a page that cannot reach its runtime |
| `RuntimeClosed` | `runtime_closed` | the session or one of its calls has ended |
| `RuntimeBusy` | `runtime_busy` | a bounded in-process runtime state lock was temporarily unavailable |
| `RuntimeMessageTooLarge` | `runtime_message_too_large` | one encoded runtime frame exceeded the fixed wire bound |
| `RuntimeBlob` | `runtime_blob` | managed blobs were not enabled, or bytes violated their declared policy |
| `RuntimePayload` | `runtime_payload` | a runtime payload could not be encoded or decoded |
| `FrontendUnreadable` | `frontend_unreadable` | a frontend that exists and is trusted but could not be read |
| `FrontendIncompatible` | `frontend_incompatible` | a frontend written against another `ui_api_version` |
| `FrontendUnsafe` | `frontend_unsafe` | markup or a file name an override may not supply |
| `PageRender` | `page_render` | a template failed to compile or render |
| `PageIncomplete` | `page_incomplete` | a rendered page is missing an element id or its runtime marker |
| `ConfigUnreadable` | `config_unreadable` | AFUI's own configuration directory or attention config |
| `WindowUnavailable` | `window_unavailable` | no browser found, or one that would not launch |
| `WindowWaitFailed` | `window_wait_failed` | waiting on an open window failed |
| `Io` | `io` | anything else AFUI could not read, write, or generate |

The match behind `kind()` has no wildcard arm, so a new AFUI variant does not
compile until someone classifies it; `tests/error_kinds.rs` fails if that
classification invents a word that is not in this table, and again if a
classification in the table is one no error can actually raise.

`hint()` returns a recovery action or `None`. Safe mode is suggested only for a
failure an override could have caused — a built-in page that will not render is
the Provider's own bug, and sending a person to disable something they never
installed helps nobody.

## AFUI's own routes, from inside your page

Every session reserves the whole `__afui/` prefix under its own credential.
AFUI uses it for the resolved application icon, ending, the typed runtime, and
managed blobs. Provider routers must not claim anything below that prefix.

A `UiPage` render carries the paths a template may render directly as a
reserved `afui` global:

```jinja
<link rel="icon" href="{{ afui.app_icon_path }}" type="image/svg+xml">
```

`app_icon_path` and `end_path` are relative to the session root, which makes
them resolve under the session's own credential in every delivery. `afui` is
reserved exactly the way `document` is: an extension that registered that name
is refused, because a replaceable layer may not redirect AFUI-owned behavior.
From Rust they are `APP_ICON_PATH` and `SESSION_END_PATH`, available without the
`session` feature.

The runtime and blob paths are deliberately absent from the template global.
Trusted JavaScript uses `afui.connect()` and its blob methods; Provider code
uses `UiBlobRef`. Neither side names transport paths or builds their URLs.

## When your own listener hands out access

The usual shape is one credential per session, minted when you hand AFUI a
session and dropped when that session ends. Some Providers have the other
shape: a long-lived listener with a domain API of its own that grants access to
the same UI on request, and takes it away again.

Do not reimplement credential routing for that. It is the same dispatch, driven
from outside:

```rust
let mount = UiMount::new("http://127.0.0.1:8080/ui/")?;
let app = provider_routes.nest_service("/ui", mount.router());
let access = mount.external_access(ui_routes, policy, Some(app_icon));

// From your own endpoint, when you decide a request deserves access:
let credential = access.issue(UiExpiry::Idle(Duration::from_secs(1800)))?;
credential.access_url_secret();  // the URL to hand back
access.revoke(credential.secret());
```

If that mounted page is live, mint the credential and its runtime together:

```rust
let (credential, runtime) = access
    .issue_with_runtime::<Action, Reply, State>(UiExpiry::Idle(Duration::from_secs(1800)))?;
```

Use `issue_with_runtime_and_blobs` when calls also return managed bytes, or the
`*_capacities*` variants when the default unacknowledged-call and event replay bounds
are not the right ones. The runtime is scoped to exactly that credential;
revoking, ending, or reaping it closes the runtime and wakes `recv`, and also
ends any Provider response still streaming under it — an event stream, a long
poll — because removing the credential only stops new requests. A Provider
WebSocket upgrade is not covered by that, so do not combine `issue` with a
Provider WebSocket route; carry live traffic on the runtime instead.

Everything a session gets comes with it: the 404 that does not confirm a UI is
here, the `Host` check that stops a rebound name reaching the same socket, the
response headers, the Provider icon route, the reserved `__afui/` prefix — and
`__afui/end`, which for these credentials revokes the one it arrived on, since
there is no session waiting on an outcome to wake. What stays yours is the part
that is genuinely yours: which request deserves a credential, what your endpoint
for asking looks like, and what you do with the URL afterwards.

**The idle expiry is crash cleanup, not a lifetime a person sees.** A process
that revokes on its way out never reaches it; one that dies without revoking
leaves a credential that stops working on its own. Do not present it to anyone
as how long their UI lasts — the delivery decides that.

### Holding a credential open from the other side

A process that was handed one of these URLs renews it while a person is still
using it:

```rust
let lease = UiCredentialLease::new(idle_timeout);
tokio::select! {
    outcome = delivery.wait() => { /* the person finished */ }
    failure = lease.keep_alive(async || my_api.renew(&credential).await) => { /* it stopped */ }
}
```

`keep_alive` renews three times per idle window, so one lost renewal is not a
lapsed credential, and resolves only when a renewal fails. The call itself is
yours: the endpoint that renews a credential is your domain API, and AFUI
inventing a wire protocol for your service is not a trade worth making.
`is_ui_credential` answers whether a string is the shape AFUI issues, for a
caller reading one back out of a URL — that fact belongs here rather than as
"sixty-four hex digits" written down somewhere else.

## The floor your page stands on

Every session serves one AFUI-owned stylesheet at `BASE_STYLE_PATH`. Link it
before your own, from the template:

```jinja
<link rel="stylesheet" href="{{ afui.base_style_path }}">
<link rel="stylesheet" href="style.css">
```

In that order, because yours loads second and wins at equal specificity; most
selectors in the baseline are wrapped in `:where()` so overriding one is never
a specificity fight.

It carries only rules of these kinds: what AFUI's own runtime decides —
what a control looks like while a call is in flight, what `hidden` does — and
what is *wrong* rather than plain when a page forgets it: a focus ring, a
`[hidden]` that beats an author `display`, a page inset that clears the edges
of the phone a `link` session was opened on, `prefers-reduced-motion`. There
are no components: no button variants, no grid, no utility classes.

Its only vocabulary sits on the same line:
`afui.connection(el)` writes AFUI's own connection word to
`data-afui-connection`, and `reconnecting` and `closed` are dressed here
because *this page may be showing something that is already wrong* is AFUI's
fact and no page should have to answer it for itself. `afui.status(el).set(text,
state)` is the other direction — that word is yours, it lands on `data-state`,
and what "busy" looks like is not AFUI's to say.

Everything tunable is a custom property on `:root`, so a different palette is
a declaration in your own stylesheet rather than a fork:

| | |
|---|---|
| Surfaces | `--afui-page` `--afui-panel` `--afui-panel-strong` |
| Text | `--afui-ink` `--afui-muted` |
| Lines | `--afui-line` |
| Emphasis | `--afui-accent` `--afui-accent-ink` `--afui-focus` |
| Meaning | `--afui-warn-ink/-bg` `--afui-danger-ink/-bg` `--afui-ok-ink/-bg` |
| Shape | `--afui-radius` `--afui-shadow` `--afui-font` `--afui-mono` |
| Layout | `--afui-page-padding` |

Every colour has a light and a dark value already; redeclaring one outside a
`prefers-color-scheme` block sets it for both.

`--afui-page-padding` is how far your content sits from the edge of the
viewport: set that one value and each edge takes the larger of it and the area
the hardware actually leaves reachable. A page that is edge-to-edge instead —
a sticky bar, a full-bleed deck — says so with `body { padding: 0 }` and
places its own insets, because a sticky bar pushed down by a body inset leaves
content scrolling past above it.

A route rather than bytes handed to you to serve, for one reason that decides
it: an override replaces your stylesheet in full, and a baseline concatenated
into that file would go with it. Answered by the session, the floor survives
the replacement — which is what lets an override be one template and no CSS at
all and still open as an AFUI page. `page_base_style_source()` is
there for a build that assembles a page outside a session, such as a preview.

## The routes a page serves

A page is at `/`, its stylesheet is a route rather than an inline block, and
the override's `assets/` tree is nested under a fixed name so a template can
refer to it relatively. Nothing about that shape varies between pages:

```rust
page_routes(page, stylesheet, &frontend).merge(my_domain_routes)
```

For a page rebuilt per request — a live panel, or one reread because the
workspace behind it can change while a person is looking at it — route `/`
yourself and merge `page_asset_routes(stylesheet, &frontend)` instead.

### Bytes your page serves itself

A preview, a generated code, an attachment. `UiBlobPolicy` is a media-type
allowlist and a size bound; you supply both, because only you know what your
page is for.

```rust
let policy = UiBlobPolicy::new(max_bytes, ["image/png", "image/jpeg"]);
policy.respond(media_type, bytes)                     // bytes you already have
policy.respond_with_file(&root, relative, media_type) // one file under a root
policy.read(&root, relative)                          // just the bounded read
```

Every refusal is a `404`, whatever the reason — too large, a type you never
listed, a symlink, a path that tried to leave the root. A page asking for
something it may not have learns only that it is not there.

`respond_with_file` opens the supplied root as a directory handle. `relative`
must contain only plain names; each component is opened without following a
symlink relative to the preceding handle, so swapping a path component cannot
redirect the walk. Type and length checks inspect the opened file, and the
bound is applied again to what was actually read. `read` is the same walk without the
response, for a page that serves one part of a container it read off disk.

There is no caching, no `Range`, and no upload here. A page's own bytes are
small and generated per session, and each of those is a second thing to get
right in a response a session hands to a browser under a bearer credential.

## What your page may load

A Provider page denies everything and then names a small set of exceptions.
Say which with switches rather than a string:

```rust
let policy = UiPagePolicy::new(UiPageScript::Nonce(nonce.clone()))
    .allow_images()
    .allow_form_submission()
    .into_security_policy();
```

The floor `UiPagePolicy::new` starts from is `default-src 'none'` plus a
stylesheet from this session's own origin — no base URI, no framing, no plugin,
no form target, and no network destination. Everything else is an addition you
name:

| Switch | Adds |
| --- | --- |
| `UiPageScript::None` | nothing; the page has no behaviour and cannot be given any |
| `UiPageScript::Nonce(nonce)` | exactly one script, carrying this session's nonce |
| `UiPageScript::SameOrigin` | script files served from this session's own origin |
| `allow_images()` | images from this session's own origin |
| `allow_fonts()` | fonts from this session's own origin |
| `allow_runtime()` | the same-origin AFUI runtime connection |
| `allow_form_submission()` | form posts back to this session |
| `allow_inline_styles()` | inline `style` attributes and `<style>` blocks |

Assembling rather than formatting is the point. A hand-written policy is a
`format!` over nine directives, where a typo produces a policy that is merely
*different* rather than invalid, and where the `HeaderValue::from_str` at the
end has a failure arm that is very tempting to answer by quietly serving some
other policy. That is the one outcome a security header must never have. Here
every part is fixed ASCII or a hex nonce, so there is no parse step and nothing
to fall back from.

Attaching a `UiSessionRuntime` to a page built without `allow_runtime()` is
refused when the session is mounted. It has to be caught there: the browser
refuses that connection, not AFUI, so nothing on this side would otherwise hear
about it and the page would simply sit there connected to nothing.

`UiSecurityPolicy::with_content_security_policy` remains the escape hatch for a
page these switches do not cover. Taking it has costs. The string is
yours, so a mistake in it is your error and must be reported as one — never
answered by falling back to a different policy, which turns a broken security
header into a page that looks like it worked. And AFUI can no longer tell
whether your page may reach its own session, so the runtime check above stops
applying to it.

## What your page runtime is written in

Every page that talks to a live session has the same work to do: deriving the
session root, reconnecting live state, keeping call identity straight,
releasing temporary byte bodies, and small pieces of care that are only
interesting when they are missing.

`page_kernel_source()` is that, as source to concatenate ahead of your own
runtime inside one function scope:

```rust
format!(
    "<script nonce=\"{}\">(function(){{\n{}\n{}\n}})();</script>",
    nonce.as_str(),
    page_kernel_source(),
    MY_RUNTIME,
)
```

It defines one lexical `afui` binding and writes nothing to `window`.

A live page is built out of these:

| Primitive | What it is |
| --- | --- |
| `afui.sessionRoot`, `afui.url` | a Provider-owned static or navigation URL kept under this session's credential |
| `afui.connect({onState, onEvent, onEventReset, onConnectionState})` | the complete typed session runtime; `call` sends actions and receives progress/result/error |
| `runtime.blobUrl`, `runtime.fetchBlob`, `runtime.releaseBlob` | AFUI-managed byte bodies without a Provider download protocol |
| `runtime.openExternal(url)` | a URL opened *outside* the page — the system browser for the local window, a new tab for a remote view — instead of navigating the surface (see below) |
| `afui.status(element)` | an aria-live region written as text, with `data-state` for the stylesheet |
| `afui.inFlight(elements)` | controls disabled while a call is pending: `begin()` sets `disabled` and `aria-disabled`, `end()` restores their original values. For a non-native control your handler must still reject activation while pending; ARIA alone does not suppress events |
| `afui.connection(element, words)` | the runtime's own connection state, marked for the baseline and worded by you |
| `afui.guardUnload(isDirty)` | warn before leaving, asked at the moment of leaving |
| `afui.place({at, show, hide, onFrame})` | a second screen inside one page: one history entry per place, `popstate` in both directions, Escape, and focus returned to whatever opened it |
| `afui.frame({onHead})` | what the window frame around your page should say about it — `title`, `status` — and whether it draws a head at all, so a framed page stops drawing a second one |

`runtime.openExternal` acknowledges handoff of an opening request. A remote
browser's `window.open` with `noopener,noreferrer` returns no window handle even
when it succeeds, so a null return is not evidence of popup blocking. AFUI keeps
those isolation flags; remote handoff cannot confirm that a tab is visible. A
framed session's sandbox allows popups and lets them escape it, so the tab
opens as an ordinary page of the person's browser rather than failing inside
the frame.
See the [HTML window-open steps](https://html.spec.whatwg.org/multipage/nav-history-apis.html#window-open-steps).

These fit narrower shapes. Reach for one only when your page has that shape;
each has a more common alternative that is usually the better fit.

| Primitive | What it is | When it applies |
| --- | --- | --- |
| `afui.autosave({save, baseHash})` | debounce, one save on the wire at a time, base-hash writeback, and a 409 reported as a conflict rather than an error. A value typed while a save is in flight waits — only the newest waiting value is kept — and goes out against the hash that save's reply returned, so the server never sees a second save made against a hash it has already moved past. On a 409 the waiting value is dropped and `onConflict` decides what happens to it. `pending()` is true while anything is unsaved: debounced, on the wire, or queued | a field that saves as somebody types. An explicit save control needs none of it |
| `afui.whileVisible(run, ms)` | poll only while somebody is looking, and refresh at once on return; `stop()` is final, and a later tab switch does not undo it | a page that has to poll something AFUI does not carry. A page whose Provider can publish retained state should receive that instead |
| `afui.end()` | end this session from inside its own page — the browser side of `SESSION_END_PATH`, so whoever created it sees the same `Closed` as a shut window | a page with its own "done" control. Otherwise the window, the card and `afui session close` are the ways out |

There is deliberately no `location.hash` helper. Hash state differs from page
to page — repeated keys or not, which entry to evict — and a primitive that has
to be worked around is worse than none: the next person tries it first and
then writes theirs anyway.

`afui.connection` is the one primitive with a vocabulary of its own, so it is
worth a paragraph. The runtime's connection state is `connecting`, `live`,
`reconnecting` or `closed`. Only three of those are ever handed to
`onConnectionState`: `connecting` is where every page starts, so there is
nothing to announce — `afui.connection` paints it when you call it, and
`runtime.state` reports it. A handler with an `else if (state ===
'connecting')` arm has an arm that never runs. The word lands on `data-afui-connection`, which
the baseline stylesheet dresses for the two values that mean *what you are
looking at may already be wrong*; the wording stays yours, and a line that
also carries your own messages passes no wording at all and keeps its text.

**This is not a framework and will not become one.** There is no template here,
no DOM diff and no state store; every text write is `textContent` and every
element comes from you. A primitive that needed a third-party library would not
be collected — it would be declined.

The boundary is unchanged: a frontend still cannot supply JavaScript. This
source reaches a browser only because you spliced it into your own trusted
runtime. `socketUrl`, generic request wrappers, raw channels, and SSE helpers
are intentionally not exposed: they would recreate a second UI protocol beside
`afui.connect()`.

## Decide panels: declaration is theirs, binding is yours

If your page returns an answer that moves money, runs a statement, or otherwise
cannot be taken back, a replaced template controls the markup — and therefore
controls which button is *labelled* "approve". Do not solve that by restricting
what a template may replace. Solve it by keeping the decision out of the
template:

- The **template declares what a control is for** — a semantic attribute you
  define, for example `data-yourprovider-decision="approve"`.
- **You decide what it does.** The map from that declaration to the real route
  lives in your own runtime, injected under a per-session `UiCspNonce` with
  `script-src 'nonce-…'` in the policy. A frontend cannot supply that script,
  cannot hide it in a template, and could not run it if it did — it does not
  know the nonce.

These rules make that hold up:

- **Match the declaration exactly, not by prefix.** `approve-all` starting with
  `approve` is how a page passes a completeness check and then opens with a
  control bound to nothing.
- **An unrecognised declaration binds to nothing** — it does not fall back to
  something plausible.
- **A page that declares no decision control refuses to open.** Erroring is
  correct; a window whose buttons do nothing is worse than no window.
- **Absence is refusal.** Make the closed window, the lapsed credential and the
  refuse button all mean *no*, in a match with no wildcard arm — so that adding
  an outcome later cannot compile until someone decides what it means.

What remains is that a trusted template can mislabel a control it declared. That
is the accepted model: the person enabled that exact fingerprint, and any later
byte revokes it. Encryption of intent is not on offer; what is on offer is that
the *binding* is never up for grabs.

### You do not write those rules again

`UiDecisionRuntime` generates the script; you own the tag it goes in, because
only you know this session's nonce.

```rust
let runtime = UiDecisionRuntime::new("data-myprovider-decision")?
    .with_status_attribute("data-myprovider-decision-status")?
    .with_decision("approve", "approve")?
    .with_pending_text("approve", "Approving…")?
    .with_decision("refuse", "refuse")?
    .with_pending_text("refuse", "Refusing…")?;

let page = UiPage::builder(&frontend)
    .entry("templates/page.html.j2")
    .fallback(BUILTIN_PAGE)
    // The same list, read back as the page's contract.
    .requires_decisions(&runtime)
    .runtime_marker("<!-- myprovider:trusted-runtime -->")
    .runtime(Some(format!(
        "<script nonce=\"{}\">{}</script>",
        nonce.as_str(),
        runtime.source()
    )))
    .render(&document)?;
```

The declaration vocabulary, the routes, and every word a person reads are
yours. AFUI carries no default text and never will: "Approving…" and "Sending
payment…" are the same moment in two domains, and a library that guessed
between them would be writing your product copy.

`requires_decisions` is the reason the runtime and the page cannot drift: add a
decision to one and the other stops rendering until it catches up. Underneath
it are checks you can also use directly —
`requires_attribute_values(attribute, values)` for one attribute's vocabulary,
and `requires_attributes(names)` for a contract whose vocabulary is the
attribute names themselves. Both match the same way `requires_elements` does:
the whole value, and only on an element a browser would actually render, so a
declaration surviving in a comment or inside `<template>` does not count as a
control anyone can press.

At least one carrier per value, not exactly one — a runtime binds every control
it finds, so an approve button at the top of a long page and another at the
bottom is a page that works.

### Partials the Provider has never heard of

A frontend that restructures a page needs partials of its own, and every
template has to be registered before an `{% include %}` resolves:

```rust
UiPage::builder(&frontend)
    .entry("templates/page.html.j2")
    .fallback(BUILTIN_PAGE)
    .template("templates/layout.html.j2", BUILTIN_LAYOUT)
    // Everything else the frontend put under `templates/`.
    .override_templates_under("templates", ".j2")?
```

Templates already named keep their own fallbacks; the rest are registered
override-only, so a partial that disappears between discovery and render fails
the page rather than silently resolving to a built-in of the same name.

## A link inside a window opens outside it

A `window` is a bare web view: no address bar, no tabs, nothing to navigate
away to. That is what makes it a window rather than a browser, and it is also
why a page must never let a link navigate *inside* it. A person who followed
one would be on a site whose address they cannot see, in a surface that has
just lost the session it existed for — and the content that put the link
there was usually not the Provider's own. A page that shows remote content
therefore renders links as text with a visible destination, and when a person
asks for the page itself, calls `runtime.openExternal(url)`.

What happens then depends on where the person is. In the local window the
host opens the URL in the system default browser, where the address bar and
the browser's own protections apply. In a remote view — the person's own
browser, reached through `afui session serve` or a `link` — a new tab is
opened right there; the host never opens anything on the machine the runtime
runs on for someone who is not sitting at it. The runtime tells the page which
case it is (`remote_view` on the `ready` frame), and the host enforces the
distinction again on its side, refusing `open_external` from a remote view
with `ui_open_external_remote`.

A surface is remote unless it proves it is local. Arriving without AFUI's
proxy proves nothing: your own listener behind a `UiMount`, a tunnel, or a
mapped container port reaches the runtime straight from another machine. So
when AFUI opens its own window — through `UiDeliveryPlan`, `open_window`, or
`afui session open` — it puts a per-session key in the URL's fragment, which
never reaches a server or a log, and the page kernel hands it back in the
runtime hello. Only that key makes a surface local, and a connection through
AFUI's proxy, which marks every request it forwards, is remote even with it.
A window you launch yourself with `UiWindow::launch` on a bare access URL has
no key, so its links open as a remote view's would.

The host, not the page, decides what is eligible: a plain `http` or `https`
URL of readable length, nothing else. `javascript:`, `data:`, `file:` and
`mailto:` are refused as `ui_open_external_url_invalid` before any opener
runs, and the URL is handed to the platform opener as one argument, never
through a shell. A request is not a call: nothing is resent after a wire drop,
because a link a person clicked once must not open twice. The promise resolves
to `{opened, via}` or rejects with a `code`.

## What is shown must be what happens

If the answer commits to something resolved earlier — a payment plan, a quoted
fee, a statement — carry that identity in your own code from the thing you
stored, never from anything the page could influence. The template renders the
plan; it does not get to say which plan.

## If your UI reads browser storage

`afui session serve` either puts every session on a path of its own origin or
gives each session an origin of its own, and only the second lets a framed UI
touch `localStorage`.

By default every session is a path on the listener's own origin and the frames
are sandboxed without `allow-same-origin`. That gives each one an opaque origin,
which is what keeps them from reading each other — and what leaves them with no
browser storage at all. A third-party client that reaches for storage unguarded
refuses to load there.

`afui session serve --session-origin-host <HOST>` gives every session an origin
of its own, `s-<credential>.<HOST>`. Isolation is then the browser's own origin
boundary, the frame is same-origin with itself, and storage works. It asks
something of whoever runs the listener rather than of you: `*.<HOST>` has to
resolve to that machine, and behind TLS the certificate has to cover it.
(`localhost` needs neither — browsers resolve `*.localhost` themselves.)

So: your UI is not required to avoid browser storage, but a UI that depends on
it is one that only works when the person serving it arranged that. Say so in
your own documentation, and remember the deliveries that never had the problem —
its own window and `afui session open`. A direct `link` frames the session on a
path, like the shared page's default, so it has no storage either.

## Your page must rebuild itself from its own URL

`afui session serve` frames every session in an iframe, and that frame gets
torn down and rebuilt whenever its address changes — which happens once, when
a lapsed view is reopened: the credential the old address carried was
revoked, so the new one needs a new frame, not the old one reused. Whatever
your page was showing at that moment — an expanded tree, a half-filled
form — is gone, and AFUI cannot preserve it: the frame is on an origin AFUI's
own page cannot read by design (see above), and the shell does not consume
Provider payloads. The runtime restores the Provider's retained current state,
but it cannot infer that an expanded disclosure or half-filled local form is
domain state. Persist anything that must survive through an explicit typed
action; DOM view state is not automatic persistence.

Dropping `allow-same-origin` from the sandbox to let the shell reach in would
remove the isolation that keeps one session's frame from reading another's.
AFUI will not do that. If view state must survive, make it Provider-owned state
with an explicit typed operation and the same validation as any other write.

So the requirement is on you: **your page has to be able to reconstruct what
it is showing from its own URL alone**, the same way a browser tab already
has to survive a reload. AFUI's own contract is the address does not change
underneath a session that is still being watched — a session's frame path is
stable across every poll while its view is live or merely warning, and
`afui session close`/window-close remain the only ways a session ends outside
your control. The one moment your page has to answer for is being loaded
fresh at a new address after its own view lapsed and someone asked for it
back.

### A second screen inside your page is a place, not a panel

The moment your page opens one record over its list, it has made a second
screen — and at the width these sessions are opened at most often, that record
covers the list completely. A person can then ask to leave it with the
system's own back gesture, the key that closes things, a control you drew, or
the navigation control of a window frame your page may be running inside. They
have to be one gesture with one result. A page that pushes no
history entry loses the first outright: back leaves the whole page from under
the record, which on a phone is the gesture people reach for first. A page that
pushes one per disclosure loses it the other way, and leaving takes a dozen
taps.

`afui.place` is that one entry and every one of those ways in and out of it. It renders
nothing: `show` and `hide` are yours, because whether a place is a pane beside
the list or a screen over it is a question about your width and your content.
Enter it after the place is on screen and *before* you write it into the
address — what is pushed is a duplicate of where the person already was, so the
entry below keeps your page as they left it and your own `replaceState` writes
the place into the new one.

`at` and `show` are a pair rather than one "you left" callback because a place
is somewhere a person can arrive: going forward again, or opening an address
that already names it. `at` is asked what the current address names; whatever
it returns is handed to `show`, and arriving that way pushes nothing.

Read what `at` returns off what your page has already been given, not off the
address as a name to fetch. An address is a person's to edit, and a place
restored by handing a raw identifier to a typed call is a page asking for
something nobody listed.

### Do not draw a second head inside a frame

A live page tends to draw a header: the program's name, what the session is
on, and a word about how it is going. A framed session then shows it
twice — once in the frame's own head, which already has the first two from the
listener, and once again underneath in the page. On the width these sessions
are opened at, the second one is a row of the screen spent on nothing.

`afui.frame` is the ask. `onHead(true)` means there is a head out there; a page
that hears it hides its own and sends up what a frame cannot know:

- `frame.title(text)` — where the page is now. `null` says nothing and leaves
  the frame showing whatever the session itself is on. A place is the usual
  reason it changes: set it as you enter and clear it as you leave.
- `frame.status(text, state)` — one short line about what is being shown. The
  optional `state` is one of AFUI's own connection words, and a frame that
  knows the vocabulary dresses the line exactly as your baseline would.

Identity is never sent. Whose program this is and what the session is on are the
listener's word, and a page repeating either into a head it does not own would
be a page naming itself in somebody else's chrome. That is also what makes the
rest safe to hand over: a card cannot be made to introduce itself as something
else.

`afui.place` uses the same channel for the one navigational fact — that your
page is somewhere with a way back — and a frame with navigation to offer draws
the control the person's window already keeps in that corner, reaching you as
`onFrame({navigation: true})`.

Keep drawing your own head and your own way back. Your page is delivered in its
own window and as a direct link as often as it is framed, and in those there is
no head but yours. What these callbacks are for is dropping the *second* one,
not having none.

Nothing else crosses. The frame cannot read your page and does not consume what
it is showing; it takes a boolean and two lines of text, and the only thing it
ever sends back is a request to leave the place you said you were in. Both lines
are flattened and capped before they are shown, so they are words and never a
lever on the frame's own layout.

## One page, handed over, waited on

Everything above composes, and for a page that is handed over and waited on the
composition is always the same sequence. `UiSurface` is that sequence:

```rust
let active = UiSurface::<Decision>::new("myprovider", "confirm")?
    .with_subject(subject)
    .with_app_icon(app_icon)
    .with_page_policy(policy)
    .on("approve", |_fields| Decision::Approve)
    .on("refuse", |_fields| Decision::Refuse)
    .with_answer_page(|decision, recorded| decided_page(decision, recorded))
    .merge(my_domain_routes)
    .start(plan, page, stylesheet, &frontend)
    .await?;

let facts = active.facts();
let outcome = active.wait().await?;
```

A submission arrives as `BTreeMap<String, String>` and is parsed by a function
you wrote. **AFUI does not learn what a field means, and this layer will never
gain validation, field types, conditional display, layout, or grouping.**
Anyone who needs those needs a Router, which they have always had. `with_answer_page`
is called with what was answered and whether that answer was the one that
counted — `false` for a click that arrived after the decision, which is the
only honest thing to show the person who made it.

This is deliberately the last layer, not the first. Everything it does is
reachable directly, and a page that does not fit this shape should use the
pieces rather than bend to it.

## Testing your UI through AFUI

Add AFUI to your `[dev-dependencies]` with `features = ["test-support"]`. It is
not in the default set and never reaches a release binary.

```rust
use agent_first_ui::test_support::{InstalledFrontend, stub_browser, write_attention_config};

InstalledFrontend::new(&workspace, "myprovider", "console", "1")
    .with_frontend_id("custom")
    .with_file("templates/page.html.j2", MY_TEST_PAGE)
    .install()?
    .trust()?;
```

`trust()` computes the fingerprint with AFUI's own code, through the same
function AFUI trusts against. That is the whole point: a fingerprint
reimplemented in a Provider's tests agrees with AFUI today and goes on agreeing
right up until AFUI's algorithm changes, at which point that suite fails for a
reason it is not about. Keeping AFUI honest about not drifting
is AFUI's job; yours is to prove your UI works.

Drive a live session through AFUI's real wire with `RuntimeClient`:

```rust
use agent_first_ui::test_support::RuntimeClient;

let mut page = RuntimeClient::connect(&access_url, "integration-page").await?;
let state: Option<MyState> = page.opening_state().await?;
let reply: MyReply = page.call(&MyAction::Refresh).await?;
page.close().await?;
```

The test supplies only domain payloads. `RuntimeClient` owns the reserved path,
protocol hello, request identity, acknowledgements, JSON envelope, and
WebSocket framing. Do not reproduce those details in a Provider test any more
than you would reproduce the fingerprint algorithm. Both clients expose
`call_with_progress`; a synchronous process test uses `BlockingRuntimeClient`
so that callback can drive child stdin/stdout between typed progress and the
final reply without exposing transport identity.

The rest is arranging any suite would otherwise write for itself:
`stub_browser` records the URL instead of opening one, so window delivery is
testable on a machine with no screen. AFUI does not pass a Chromium binary the
session URL: it launches `--app=` on a private `launch.html` (`0600`) in the
window's temporary directory, which moves on to the session URL with
`location.replace`, so the credential never appears in the browser's process
arguments. A browser confined to a snap or Flatpak cannot read this machine's
temporary directory, so it alone is still given the URL directly. `stub_browser` reads that page and records `--app=<session URL>` in
its place; a stub browser of your own behind `AFUI_BROWSER_BINARY` has to read
the launch page the same way to learn the URL; `write_attention_config` writes the idle
and grace periods you pass to where AFUI reads them, so a test can let a Link
lapse in a second instead of waiting fifteen minutes for the default; and
`wait_for_session` / `wait_for_no_session` poll the registry for a session this
process is not the one announcing.

Those last two are for a test that drives a Provider **as a child process**. In
a test that holds the session itself they buy nothing: announcing happens
inside `attach`, before the active session is handed back, so the entry is
already there on the line after `start`. Withdrawal is the direction that is
genuinely late — the entry goes when the handle drops — and both helpers block
the thread they are called on, so an async test reaches for them through
`spawn_blocking` rather than from inside a single-threaded runtime that the
session it is waiting for also needs.

Keep your own end-to-end tests driving the real library. `RuntimeClient` also
drives a real socket; what this removes is the arranging, not the proving.

### Window exit evidence

For `UiDeliveryPlan` window delivery and `afui ask`, each launch adds a fresh
query ID to the window URL. The host records a successful request for that
page carrying the launch ID; loading it without JavaScript is sufficient.
Nonzero exits and signals are `WindowWaitFailed`; successful exits without
page arrival are `WindowUnavailable`; successful exits after arrival are
`Closed`, which supplies no decision. `afui ask` preserves these kinds as
`ask_window_wait_failed` and `ask_window_unavailable`.

`afui session open` and upstream windows have no host request evidence for
that launch. They check exit status only; a successful exit remains `Closed`.

## AFUI's own test gate

This is for changing AFUI itself rather than testing a Provider through it.

`scripts/test.sh all` runs the portable checks and binary smokes.
`scripts/test.sh kernel` runs only the JavaScript kernel regression checks,
which are also included in `unit` and `all`. `scripts/test.sh browser` runs one
optional real Chrome/Chromium smoke at a 360 × 640 viewport, checking iframe removal after attention expiry and
enforced CSP. The fixture uses the stock polling cadence with a 1-second idle
window and 3-second warning period. If no browser is available it reports a
visible skip; `AFUI_TEST_BROWSER_BINARY` selects an explicit executable.
`scripts/test.sh release` adds that browser check to the portable gate, and the
release workflow runs it on its Linux x86_64 leg.

`scripts/test.sh window-shortcuts` checks native window closure: Command+W on
macOS, Ctrl+W and Alt+F4 on Windows/Linux, including process exit after closure.
It requires a desktop; Linux needs an X11 display, a window manager and
`xdotool`, and Windows needs an interactive desktop session rather than SSH
Session 0. Windows/Linux use Chrome or Chromium, selected with
`AFUI_TEST_BROWSER_BINARY` when needed. The macOS fixture queues AppKit events
inside its own applet and does not require Accessibility permission.

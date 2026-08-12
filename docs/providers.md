# Building a Provider UI on AFUI

The README covers what AFUI hands you. This covers the three decisions you make
before using it: whether the thing you have is a session at all, what an
override is allowed to do to it, and — if your page returns an answer that
matters — how to keep a replaced template from redefining what that answer is.

## Is it a session?

Three tests. A UI that fails any one of them is not a session, and forcing it
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

That difference is not cosmetic. It decides your expiry:

- **A window bounds itself.** The person can see it; leaving it open all
  afternoon is still using it. Window delivery never expires, and
  `Outcome::Expired` cannot occur.
- **A URL has nobody watching it**, so it must say when it stops working.
  `UiExpiry::After` for a fixed lifetime, `UiExpiry::Idle` for one that slides
  forward while the page is actually being used.

Getting this backwards is the most common mistake: a fixed session-level timeout
kills a terminal someone is reading and leaves a shared link alive longer than
you meant.

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

## A live channel is orthogonal to the terminal outcome

Some interfaces are not one request followed by one answer. A surface may show
live measurements while accepting inspection commands, or receive progress
while letting a person refine the work in progress. That still has one bounded
session lifetime and one terminal `Outcome<T>`; the intermediate traffic is a
different concern.

Attach it with `UiSession::with_channel::<FromSurface, ToSurface>()`. The
returned `UiSessionChannel` is the controller side. The surface opens a
WebSocket at `agent_first_ui::SESSION_CHANNEL_PATH` relative to its session URL.
Every frame is directly one JSON-encoded value of the Provider's type:

- AFUI does not wrap messages, assign request ids, or infer request/response
  pairs;
- AFUI does not retain, replay, or present history;
- either direction may send independently, so streaming and commands do not
  have to pretend to be a conversation;
- each direction is bounded (32 queued values by default), and senders wait for
  space rather than growing memory without limit;
- one encoded value may be at most
  `MAX_UI_SESSION_CHANNEL_MESSAGE_BYTES` (1 MiB); serve larger artifacts from a
  Provider route and use the channel to name or invalidate them;
- only one surface connection is active at a time. A connection may be replaced
  after it closes, but AFUI never fans one controller stream out to several
  surfaces.

The endpoint is in AFUI's reserved namespace and inherits the session
credential, exact-host/origin checks, revocation, reverse-proxy support, and
shutdown. If you replace `UiSecurityPolicy`'s default CSP, allow the surface to
connect back to itself, normally with `connect-src 'self'`.

### Backpressure is the whole flow-control story

The bound is not a hint, and AFUI never drops a value to stay under it. Two
consequences are worth designing for before they surprise you:

- **A send with nobody listening eventually waits.** A session can be delivered
  and stay unopened — a `link` nobody clicked, a card nobody expanded — while
  the Provider keeps pushing. After the queue fills, `send` waits until a
  surface drains it or the session ends. A loop that pushes state on a timer
  therefore stops on its own timer, and the surface that finally connects
  receives the backlog, oldest first, rather than the current state. If the
  newest value is the only one that matters, keep it in Provider-owned state
  and send from there.
- **Not reading blocks writing.** One connection carries both directions, so a
  surface-to-Provider queue left full stops that connection reading *and*
  writing. Call `recv` for as long as the session is open, even in a Provider
  that only pushes and treats surface values as noise.

Neither is a bug to route around with a bigger capacity: they are what a
bounded queue means. `with_channel_capacity` sets how much slack a burst gets,
not whether the slow side is allowed to be slow.

Presentation remains the frontend's decision. One frontend may display every
value received during this page load; another may replace one status block in
place. Neither choice changes the transport and neither history belongs to
AFUI. Persist or replay anything that must survive a reload in Provider-owned
state and reconstruct the page from its own URL.

## Delivery: `window`, `link`, or `session`

A session reaches a person one of three ways. `UiDeliveryPlan` resolves all
three through one plan:

| Mode | Delivered as | Expires |
| --- | --- | --- |
| `window` | an isolated window this machine opens | never — the window is the bound |
| `link` | a URL another device can open directly, with no `afui` binary involved in getting it there | must be declared with `with_link_expiry`; there is no default |
| `session` | registered only, reached later through `afui session open` or `afui session serve` | never — the view is bounded by the outer shell's idle policy instead |

```rust
let outcome = UiDeliveryPlan::resolve(explicit_mode)?
    // Omit this call entirely to not offer `link` at all.
    .with_link_expiry(UiExpiry::After(Duration::from_secs(900)))
    .deliver(session, router)
    .await?;
```

When the caller must announce a direct URL before it blocks, split the same
plan at its handoff point:

```rust
let active = UiDeliveryPlan::resolve(explicit_mode)?
    .with_link_expiry(UiExpiry::After(Duration::from_secs(900)))
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

`link` is opt-in per Provider, not per call: if you never call
`with_link_expiry`, `deliver` refuses a resolved `link` mode with a clear
reason instead of inventing an expiry for it, and your own `--mode` flag
should not list `link` as a legal value either — an absent capability is
better than one that fails every time it is reached. Do not give it a default
expiry if you do offer it: different Providers have needed no TTL, a fixed
timeout, and a sliding idle window for the same delivery, and one session-level
default cannot express more than one of those.

### `link` puts a person's data on the network unencrypted

`link` is the one delivery that reaches another device with no tunnel in the
way: plain HTTP on this machine's own LAN-reachable address, with no
authentication beyond the credential already in the URL. Use it only on a
network you trust, and treat that URL the way you would treat a password —
give it to the person it is for, and nowhere else: not into a file, not into a
commit message, not into a chat transcript you do not control.

## Ending a session from outside it

Every session answers `POST <access_url>` + `agent_first_ui::SESSION_END_PATH`
under its own credential, on every delivery: `afui session close <id>` sends
it, and so does the "End" control on a served session's card. Whoever created
the session gets `Outcome::Closed` back — the same outcome as a person closing
its window — never a manufactured `Completed(T)`.

The threshold for ending a session is the same as the threshold for seeing
it: whoever holds the URL already holds the whole credential, and can already
do worse from inside a Provider's own page (a terminal's `exit`, for one), so
refusing to let them end it from outside would be a boundary that only looks
like one.

**The whole `__afui/` prefix is reserved**, including
`agent_first_ui::SESSION_END_PATH` and
`agent_first_ui::SESSION_CHANNEL_PATH`. Do not route on any name under it,
proxy through it, or otherwise give it a second meaning in your own Router —
this crate's dispatch answers for the whole prefix before your router is ever
reached, so a route of your own under it is simply unreachable, not a conflict
you would notice at runtime. Reserving the prefix rather than only today's
routes lets AFUI add another later without silently colliding with something
you already routed there.

## What an override may and may not do

An override supplies **presentation**. It may restructure freely — reorder,
regroup, drop, rename, restyle — and that freedom is the point: an agent
customises a Provider's UI by writing files, not by editing the Provider.

Three limits hold regardless of what a template contains.

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

`UiPage::builder` (the `page` feature) assembles all three limits for you: the
escaping policy, the render, the rendered-markup guard, and a check that every
element id your runtime binds to actually exists in the output — a page
missing one is reported by that id rather than opened as a window that does
nothing. It knows nothing about what your elements or your document mean; it
only checks that what you declared is there.

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

Four rules make that hold up:

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

## What is shown must be what happens

If the answer commits to something resolved earlier — a payment plan, a quoted
fee, a statement — carry that identity in your own code from the thing you
stored, never from anything the page could influence. The template renders the
plan; it does not get to say which plan.

## If your UI reads browser storage

`afui session serve` frames sessions two ways, and only one of them lets a
framed UI touch `localStorage`.

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
its own window, a direct link, and `afui session open`.

## Your page must rebuild itself from its own URL

`afui session serve` frames every session in an iframe, and that frame gets
torn down and rebuilt whenever its address changes — which happens once, when
a lapsed view is reopened: the credential the old address carried was
revoked, so the new one needs a new frame, not the old one reused. Whatever
your page was showing at that moment — an expanded tree, a half-filled
form — is gone, and AFUI cannot preserve it: the frame is on an origin AFUI's
own page cannot read by design (see above), and the shell does not consume or
store a Provider's channel values. An optional session channel can let the
surface ask its own Provider to persist something, but AFUI assigns that no
meaning and replays nothing. It is not automatic view-state preservation.

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

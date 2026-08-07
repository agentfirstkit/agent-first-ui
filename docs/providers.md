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

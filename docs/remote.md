# The remote view

The README covers why a remote page exists and what it bounds. This covers how
it behaves once it is running: the attention clock and where its defaults live,
what a lapsed view looks like, what the proxy forwards and holds, and how to put
each session on an origin of its own.

## When you walk away

A remote page is the one delivery nothing physical bounds. A window ends when
you close it; a phone goes into a pocket with an interface still on it. So every
AFUI-owned remote page — a direct `link` or `afui session serve` — runs the same
clock: fifteen minutes with nobody attending, then a five-minute warning during
which everything still works and the page offers to keep itself open, and then
it stops.

The defaults are global AFUI settings. Put this in the platform AFUI config
file: `~/Library/Application Support/afui/config.json` on macOS,
`$XDG_CONFIG_HOME/afui/config.json` on Linux (or
`~/.config/afui/config.json` when `XDG_CONFIG_HOME` is unset), or
`%APPDATA%\afui\config.json` on Windows. `AFUI_CONFIG_DIR` replaces the parent
`afui` directory for an isolated runtime:

```json
{
  "attention": {
    "idle_timeout_s": 900,
    "grace_period_s": 300
  }
}
```

`idle_timeout_s: 0` disables attention expiry and `grace_period_s: 0` removes
the warning interval. `afui session serve --idle-minutes` and
`--grace-minutes` override the global values for that one server process; a
direct link always uses the global AFUI policy.

Attending is stated rather than guessed. The page says so while it is visible,
naming the sessions it is showing, and traffic through a frame says so too —
that is the only evidence there is that someone is using a Provider UI, because
what happens inside one is on an origin this listener cannot read. Polling for
the session list is not attending; a poll that renewed what it reported would be
a clock that never ran down.

When a session's view lapses, its proxy credential is revoked and the frame
comes off the page — an expiry that left the last screen painted on a phone
somebody else picks up would be a warning, not an expiry. On the shared page the
session itself is untouched, still running and still listed, and one press of
*Reopen* mounts it again at an address it has never had before; a direct `link`
ends instead, as below.

The shared page moves from Live through Warning to Unattended. Becoming
unattended revokes every mount and disconnects established SSE and WebSocket
streams. New sessions remain listed with no proxy address. The original page
URL stays valid until `session serve` exits: reload and polling can show the
current list, including questions created while you were away. A visible-page
heartbeat starts a new attendance clock, but old cards still need *Reopen*;
only sessions arriving after attendance resumes are automatically mounted.
Each mount also retains its own attention clock, so attending another card
does not extend an unattended mount's streams.

Direct `link` delivery is single-use: its attention deadline ends the delivery
with `Expired`. Run the delivery again to obtain another link. It shares mount
attendance rules with the shared page, but has a terminal entry lifetime.

What this bounds is how long an unattended *view* lasts, not who may open one.
Whoever holds the shared page's URL can return and explicitly reopen sessions.
AFUI-owned controls follow the browser language (English or Simplified Chinese,
with English fallback); Provider content keeps its original language.

How tall each card is, is yours: one control cycles **Short / Tall / Full**, and
the choice is remembered per Provider and UI kind, so a new session of the same
kind starts with the previous choice. It has to be a choice — what is inside a
frame is on an origin this page cannot read, so the page cannot measure it and
the frame cannot tell it without a protocol this listener refuses to have.
Rather than guess a height for a program it has never heard of, it lets the
person choose.
That choice is the only thing this page stores; folding and removing still come
back on reload, because a view you only rearranged is expected to.

## What the proxy forwards and holds

It is **not a transport**: it binds a port, and
crossing anything to reach that port is WireGuard, Tailscale, an SSH tunnel, or
a TLS reverse proxy, which already exist and are better at it. And it does not
hand its own credential to anything it frames: each Provider UI is reached by a
separate mount credential governed by the remote attention policy, on an origin
that is not the page's, and requests are forwarded upstream through a header
allowlist that has no line for a `Cookie`, an `Authorization`, an `Origin`, or a
`Referer`.

A TLS terminator in front has to pass the original `Host` through (nginx needs an
explicit `proxy_set_header Host $host`). The page's own POSTs are checked by
comparing their `Origin` with `Host`, so a rewritten `Host` gets its heartbeat,
*Reopen* and *End* answered `403`.

Cookies still work, because a Provider that identifies a visitor by one — a VNC
takeover panel, say — would otherwise be unproxyable. They are simply held here
rather than by the browser: each proxied session gets a jar of its own, and what
is in it goes to that session and nowhere else. The same goes for an upgraded
connection: a WebSocket is relayed once both ends have agreed to switch, which is
what a remote framebuffer needs and what nothing above it can see anyway.

Each jar holds at most 256 cookie names. Updating or deleting an existing name
still works at capacity, and a cookie lives at most 400 days whatever its
`Max-Age` asks.
Proxy connection, handshake, and response-header waits have finite budgets and
return the existing 502 response on timeout. Streaming bodies have no total
duration limit; the remote view's attention and revocation rules still apply.

Every proxied response carries `Referrer-Policy: no-referrer`, so a Provider
page that loads something from elsewhere does not tell that site which session
address it came from. Every Content Security Policy the Provider sent, report-only
ones included, keeps its directives except `frame-ancestors`, which is rewritten
— or added — to name this page; a Provider that limited framing only with
`X-Frame-Options`, which cannot name a page, gets that same `frame-ancestors`
in its place. On a session's own origin the value includes `'self'` too, so a
Provider UI that frames its own pages still can. A Provider that limited framing
in neither way gets no `frame-ancestors` added, for the same reason: frames it
nests inside itself keep loading.
`Service-Worker-Allowed` is dropped, because widening a worker's scope past its
own path would, on a shared origin, reach every session. A request path with a
`.` or `..` segment, raw or percent-encoded, is answered `404` rather than
forwarded: a browser resolves those before it asks, and an upstream resolving
one after the proxy joined it onto its base would answer outside that base. A
redirect that stays on the session drops the query the upstream was announced
with — a handoff credential, say — so it never lands in the address bar or
history; the proxy adds it back on the next request anyway.

Each card also offers to open its session as a page of its own — but only under
`--session-origin-host`. In the default shape a session is a path on the
listener's own origin, and the sandbox is the only thing keeping the Provider UI
off that origin. The proxied response carries `sandbox allow-scripts allow-forms
allow-popups allow-popups-to-escape-sandbox` as a policy header of its own,
beside any policy the Provider sent, so a frame's address opened by hand stays
isolated. A `sandbox` the Provider wrote can narrow it but not widen it: a
browser enforces every policy it is given. As a page of its own the session
would gain nothing a frame lacks, still an opaque origin with no storage, so the
control is absent there. Where it does appear, the new page gets no handle on
the one that opened it.

Popups are the one thing the sandbox lets out, and they leave it: a link a
framed page opens with `runtime.openExternal` becomes an ordinary tab of the
person's browser rather than a copy of the frame, which is what makes
`openExternal` work inside a framed session at all. Top navigation, pointer
lock and modal dialogs stay denied.

## One origin per session, when a framed UI needs storage

By default every session is a path on the one origin this listener owns, and the
frames are sandboxed into an *opaque* origin — which is what isolates them from
each other and from the page, and also what denies them `localStorage`. A
third-party client that reads storage without guarding it therefore refuses to
load, and a remote-framebuffer client is quite likely to do exactly that in
several places at once.

`--session-origin-host <HOST>` moves the isolation onto a boundary the browser
draws anyway. Each session becomes `s-<credential>.<HOST>`, an origin of its own,
so the frame can be same-origin with itself and keeps its storage while still
being unable to reach the page or any other session. The page itself is served on
`<HOST>` and nothing else answers there.

```bash
afui session serve --listen 127.0.0.1:8787 --session-origin-host localhost
```

`localhost` costs nothing: Chrome, Firefox and Safari all resolve `*.localhost`
to this machine with no DNS entry at all. Any other host is a real deployment and
needs what one always does: a wildcard record for `*.<HOST>` pointing here and,
behind TLS, a certificate that covers `*.<HOST>`. The page
must then be reached at exactly `<HOST>`; a request arriving under any other name
is refused rather than served a page whose every frame the browser would blank.

The shell and its proxy belong to the default `session` feature and are also
used by direct link delivery.

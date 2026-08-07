# Agent-First UI

Let your AI agent hand you a real interface when a step needs a person — a window opens, you decide, and the agent gets a typed answer back.

> **Ask your agent:** "Show me that in a window and wait for my answer before you continue."

## The problem: an agent that needs you has nowhere to put you

An agent can ask a question in chat. What it cannot do in chat is show you
twelve drafts and let you approve four, or put a diff in front of you, or hand
you a form that validates as you type. So the moment a task needs human
judgement on something shaped like a page, the agent either flattens it into
text or invents a web server — a port, a token, a browser tab left open, and no
reliable way to know whether you ever answered.

AFUI is that missing step, and only that step: one person, one interface, one
answer, then gone.

## What AFUI owns, and what stays yours

You bring an Axum `Router` and decide what a finished interaction means. AFUI
brings the mechanics nobody wants to write twice:

- a revocable access credential, and the decision of when it lapses;
- secure response headers, exact-origin and exact-host checks;
- an isolated Chromium-family window on a disposable profile — never your
  browser, never your cookies;
- cleanup, and a typed outcome: `Completed(T)`, `Closed`, or `Expired`.

A session carries stable `provider_id`, `ui_kind`, and `session_id`, so several
interfaces can be open at once and each answer returns to the caller waiting
for it.

AFUI never interprets what you put on the page. It does not know what a case,
an invoice, or a query is.

## Three ways a session reaches a person

A **window** is the common one: AFUI opens it, the person acts, the call
returns their answer. A window bounds its own session — someone who leaves it
open all afternoon is still using it — so a window credential does not expire.
It ends when a handler completes it or the person closes the window.

A **managed direct URL** is for when the person is not at this machine: call
`serve`, hand out `access_url()`, and await `wait()`. Nobody is watching that
credential, so this is the delivery that needs an expiry, and AFUI makes you
name one — `UiExpiry::After` for a fixed lifetime, or `UiExpiry::Idle` for one
that slides forward while the page is actually being used.

A **served page** is for when the person is somewhere else entirely — the agent
is working on your laptop and you are holding a phone. `afui session serve`
puts every session running on this machine onto one page you can open from
another device, reverse-proxying each so that `127.0.0.1` — which on a phone is
the phone — stops being the problem.

Expiry belongs to the delivery rather than the session, because the same
interface is worth fifteen minutes as a link and worth as long as you like as a
window.

## Several at once, on one port

Sessions and ports are orthogonal. Bind a `UiHost` once and `attach` as many
sessions as you like: each is reached by its own credential, each answer returns
to the call waiting for it, and ending one leaves its neighbours serving. That
also survives the places a session has to live — a container with one mapped
port, a machine behind a firewall, a single tunnel.

A Provider that already runs a server does not have to give the port up. A
`UiMount` is the same routing without a listener of its own: nest its `router()`
under a prefix on the Axum router you already have, and `attach` sessions to it.
They are ordinary sessions — same credentials, same expiry and revocation, same
security policy, same typed outcome, and still listed by `afui session list`.

## Finding what is already open

A session lives inside whichever process created it, so nothing could see across
them. Every active session now announces itself, and `afui session list` shows
what is waiting; `afui session open <id>` puts a window onto one.

The credential never appears in that listing — a session URL carries one, and
stdout is where an agent reads. `open` hands the URL straight to a window
instead of printing it. Entries are written `0600` under a `0700` directory, and
a session is pruned rather than offered once either its host stops answering or
the process that announced it is gone. `AFUI_NO_REGISTRY=1` keeps a session out
entirely.

Not every interface worth listing is one this crate serves. `UiUpstream` names
one that already exists somewhere else — afhttp's takeover panel is a KasmVNC
web client behind afhttp's own listener and its own credential — and announces
it as an ordinary session, so nothing reading the list can tell which kind it is
holding. What AFUI takes on there is the listing and only the listing: a
credential it did not mint is not one it can revoke, and ending an announcement
withdraws the entry rather than the URL.

## Reaching them from your phone

`afui session serve --listen <ADDRESS>` puts that same list on a page, with each
session proxied onto an address a phone can reach. On a narrow screen the
terminal sits on top and everything else stacks below it; on a wide one the
stack moves left and the terminal is pinned right. Arrangement is the page's and
yours — no Provider is asked where it goes.

Two things it will not do. It is **not a transport**: it binds a port, and
crossing anything to reach that port is WireGuard, Tailscale, an SSH tunnel, or
a TLS reverse proxy, which already exist and are better at it. And it does not
hand its own credential to anything it frames: each Provider UI is reached by a
separate short-lived credential of its own, on an origin that is not the page's,
and requests are forwarded upstream through a header allowlist that has no line
for a `Cookie`, an `Authorization`, an `Origin`, or a `Referer`.

Cookies still work, because a Provider that identifies a visitor by one — a VNC
takeover panel, say — would otherwise be unproxyable. They are simply held here
rather than by the browser: each proxied session gets a jar of its own, and what
is in it goes to that session and nowhere else. The same goes for an upgraded
connection: a WebSocket is relayed once both ends have agreed to switch, which is
what a remote framebuffer needs and what nothing above it can see anyway.

### One origin per session, when a framed UI needs storage

By default every session is a path on the one origin this listener owns, and the
frames are sandboxed into an *opaque* origin — which is what isolates them from
each other and from the page, and also what denies them `localStorage`. A
third-party client that reads storage without guarding it therefore refuses to
load; KasmVNC does exactly that, in three places.

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
needs the two things a real deployment needs — a wildcard record for `*.<HOST>`
pointing here, and, behind TLS, a certificate that covers `*.<HOST>`. The page
must then be reached at exactly `<HOST>`; a request arriving under any other name
is refused rather than served a page whose every frame the browser would blank.

The shell and its proxy are behind the `serve` feature, off by default, so a
Provider consuming this crate as a library gets none of it.

## What you depend on

Three tiers, so a Provider pays for what it uses and nothing else:

| Feature | What it adds | What it costs |
| --- | --- | --- |
| *(none)* | the frontend script guard, frontend discovery, the trust gate, the per-file overlay | file reading and hashing — no server |
| `session` *(default)* | `UiSession`, `UiHost`, `UiMount`, `UiWindow`, credentials, security policy, the registry, `UiUpstream`, `UiFrontend::assets_router` | axum, tokio, tower |
| `serve` | the reverse proxy and the shell above it | a streaming HTTP client |

The featureless tier exists because keeping a frontend honest is not the same
job as hosting one. A Provider whose human UI is itself an optional feature
still has to refuse a smuggled `<script>` in the build where that UI is
compiled out — so `reject_frontend_script` has to be reachable without dragging
a web server into a build that has no port to bind. Take it with
`default-features = false`, and add `agent-first-ui/session` to whichever of
your own features opens a window.

## Bring your own frontend

The interface a Provider ships is a default, not a decision. `afui frontend`
lets you replace it with your own — scaffold one, preview it, and enable it
when it is ready. Run `afui frontend --help` for the full set.

Trust is content-addressed rather than path-based: workspace code stays ignored
until you enable its exact fingerprint, and any later edit revokes that trust
until you look again. `AFUI_SAFE_MODE=1` ignores overrides entirely.

A Provider reaches all of that through `UiFrontend`, which is the delivery half
and only that: it resolves whether a trusted override exists for a
`provider_id` + `ui_kind`, hands back a named file when the override supplies
one, serves the override's `assets/`, and turns "unloadable" or "wrong
`ui_api_version`" into an error that names safe mode rather than a quiet
built-in page. What a file *means* stays the Provider's — AFUI never templates,
never renders, and never sees a domain value.

```rust,no_run
# fn example() -> agent_first_ui::Result<()> {
use agent_first_ui::UiFrontend;

let frontend = UiFrontend::resolve(std::path::Path::new("."), "afpsql", "table_inspect", "1")?;
// `None` means "this override did not supply that file", not "there is no
// override": the overlay is per file, so replacing one page keeps every other
// file the Provider ships — including its language packs.
let page = frontend.text("templates/page.html.j2")?;
let assets = frontend.assets_router();
# let _ = (page, assets);
# Ok(())
# }
```

Two rules AFUI enforces rather than describes, because a Provider that binds its
own behaviour under a nonce depends on them: an override may not supply a file
whose name says it is a script, and `reject_frontend_script` refuses one hiding
inside a template it does supply.

## From Rust

```rust,no_run
use agent_first_ui::{Outcome, UiSession, UiWindowConfig};
use axum::{Router, routing::get};

# async fn example() -> agent_first_ui::Result<()> {
let session = UiSession::<String>::new("afmail", "case_review")?;
let completion = session.completion();
let router = Router::new().route(
    "/",
    get(move || {
        let completion = completion.clone();
        async move {
            completion.complete("approved".to_owned()).await;
            "saved"
        }
    }),
);

let outcome = session.window(router, &UiWindowConfig::default()).await?;
assert!(matches!(outcome, Outcome::Completed(_)));
# Ok(())
# }
```

## What AFUI is not

Delivery infrastructure, and nothing else: not a domain UI protocol, not a
generic renderer, not a remote-access transport, not a frontend package format.
It proxies a Provider's UI without understanding one byte of it.

## Install

```bash
# prebuilt binary
brew install agentfirstkit/tap/afui   # macOS / Linux
scoop bucket add agentfirstkit https://github.com/agentfirstkit/scoop-bucket && scoop install afui   # Windows

# or from source, after reviewing the repo
cargo install agent-first-ui --features serve
```

`--features serve` is deliberate, and only the binary needs it. The published
packages above are built with it, so `afui session serve` works out of the box;
a `cargo install` without it gives you every other verb and answers that one
with `feature_unavailable` and this same line. A Provider depending on this
crate as a library wants the opposite: `default-features = false` and only the
tier it uses.

## Docs

- [Building a Provider UI](docs/providers.md) — is your UI a session, what an
  override may do to it, and how a decide panel stays safe when its template is
  replaced
- [CLI](docs/cli.md) — generated command and flag reference

## License

MIT

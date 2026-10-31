# Agent-First UI

Let your AI agent hand you a real interface when a step needs a person — a window opens, values can stay live in both directions, and the session ends with one typed outcome.

> **Ask your agent:** "Show me that in a window and wait for my answer before you continue."

## Ask a question without building a page

`afui ask` puts a few questions with suggested answers in front of a person and
returns one structured result, with room for their own words — no Provider or
HTML needed. See [Standard questions](docs/ask.md).

## The problem: an agent that needs you has nowhere to put you

An agent can ask a question in chat. What it cannot do in chat is show you
twelve drafts and let you approve four, put a diff in front of you, stream live
service state while you issue an inspection command, or hand you a form that
validates as you type. So the moment a task needs something shaped like a page,
the agent either flattens it into text or invents a web server — a port, a token,
a browser tab left open, and no reliable way to know when that interaction
ended.

AFUI is that missing step: one person, one interface, one bounded lifetime. A
Provider may use only the terminal outcome, or attach the typed session runtime
for retained state, calls, progress, ordered events, and managed byte bodies.

## What AFUI owns, and what stays yours

You bring an Axum `Router` and decide what a finished interaction means. AFUI
brings the mechanics nobody wants to write twice:

- a revocable access credential, and the decision of when it lapses;
- secure response headers, exact-origin and exact-host checks;
- an isolated desktop window with non-persistent web data — never your browser,
  never your cookies;
- one versioned live runtime with AFUI-owned correlation, reconnect, replay,
  cancellation, bounds, and retained latest state;
- cleanup, and a typed outcome: `Completed(T)`, `Closed`, or `Expired`.

A session carries stable `provider_id`, `ui_kind`, and `session_id`, so several
interfaces can be open at once and each remains bound to the process that owns
it.

AFUI never interprets what you put on the page. It does not know what a case,
an invoice, or a query is.

## How a session reaches a person

A **window** is the common one: AFUI opens it, the person acts, the call
returns their answer. A window bounds its own session — someone who leaves it
open all afternoon is still using it — so a window credential does not expire.
It ends when a handler completes it or the person closes the window.

A **link** is for when the person is not at this machine. AFUI keeps the
Provider session on loopback and hands out a second, AFUI-owned page that
frames exactly that session. That page supplies the attention heartbeat,
visible warning, grace period, proxy isolation, and public credential; the
Provider never chooses or implements its lifetime.

A **served page** is for when the person is somewhere else entirely — the agent
is working on your laptop and you are holding a phone. `afui session serve`
puts every session running on this machine onto one page you can open from
another device, reverse-proxying each so that `127.0.0.1` — which on a phone is
the phone — stops being the problem.

Remote expiry belongs to the AFUI-owned view. A window is bounded by the
window. A direct link and a shared served page use the same attention policy;
what a lapse ends differs between them, as [When you walk away](#when-you-walk-away)
says.

`UiDeliveryPlan` collapses those ways into one plan instead of a Provider
writing its own `match` over `window`/`link`/`session`. Use
`resolve_allowing_link(explicit_mode)?.deliver(session, router)` when one call
is enough and the Provider offers LAN exposure. If
the caller must publish a link URL before waiting for the person, call
`start(session, router)`, read `link_url_secret()`, then call `wait()`. `resolve`
also reads `AFUI_DELIVERY`, so an outer unattended interface can pass
`AFUI_DELIVERY=session` to a UI it starts without every downstream call
inventing its own flag for it.

## Several at once, on one port

Sessions and ports are orthogonal. Bind a `UiHost` once and `attach` as many
sessions as you like: each is reached by its own credential, each outcome and
live runtime stays with its owner, and ending one leaves its neighbours serving. That
also survives the places a session has to live — a container with one mapped
port, a machine behind a firewall, a single tunnel.

A Provider that already runs a server does not have to give the port up. A
`UiMount` is the same routing without a listener of its own: nest its `router()`
under a prefix on the Axum router you already have, and `attach` sessions to it.
They are ordinary sessions — same credentials, same expiry and revocation, same
security policy, same typed outcome, and still listed by `afui session list`.

## Finding what is already open

A session lives inside whichever process created it, so nothing could see across
them. Every active session announces itself, and `afui session list` shows
what is waiting — including whether it is waiting on a *decision* or merely
being watched, and how long it has been open — `afui session open <id>` puts a
window onto one, and `afui session close <id>` asks it to end: whoever
created it gets `Outcome::Closed`, on any delivery, the same as if they had
closed a window themselves. The command reports that the request was accepted;
the owning process notices a moment later.

The credential never appears in that listing — a session URL carries one, and
stdout is where an agent reads. `open` hands the URL straight to a window
instead of printing it. How entries are written, protected, checked for
liveness and pruned is in [Building a Provider UI](docs/providers.md#what-a-sessions-shape-decides).

A `session` delivery has nothing but its entry, so it fails when the entry
cannot be written. Opting out with `AFUI_NO_REGISTRY=1` is a decision rather
than a failure: a library `session` delivery then succeeds unlisted and
reachable by nobody, which is why `afui ask --mode session` refuses it.

Not every interface worth listing is one this crate serves. `UiUpstream` names
one that already exists somewhere else — a third-party web client behind its
own listener and its own credential, say. Hand it to
`UiDeliveryPlan::start_upstream` and the same deliveries apply: AFUI opens
the local window, wraps exactly that upstream in its own Link page, or only
registers it as a Session. The source owner still has one obligation AFUI
cannot perform remotely: keep its private credential live while the delivery
exists and revoke it when the delivery ends.

An HTTPS upstream can be opened directly as `window`. `link` and `session`
require a private HTTP upstream because AFUI's streaming proxy deliberately
does not terminate TLS.

## Reaching them from your phone

`afui session serve --listen <ADDRESS>` puts that same list on a page, with each
session proxied onto an address a phone can reach. Every session is a card in
one column, **in the order they were announced**, each with the same slice of
the viewport, and you fold or drop the ones you are not using. Open a terminal,
work in it, ask for a second interface, and that one arrives below the
terminal — where what you just did put it, rather than wherever its Provider's
name falls in an alphabet. No kind of
interface has a reserved position: the page has no way to know which kinds
exist, and reserving one would be this page ranking a program it has never
heard of above its neighbour.

### When you walk away

A remote page is the one delivery nothing physical bounds. A window ends when
you close it; a phone goes into a pocket with an interface still on it. So every
AFUI-owned remote page — a direct `link` or `afui session serve` — runs the same
attention clock: a stretch with nobody attending, then a warning during which
everything still works and the page offers to keep itself open, and then the
view stops. What that ends depends on the page. A direct `link` is single-use:
its delivery ends with `Expired`, and getting another link means delivering
again. On the shared page only the view lapses: the session is untouched, still
running and still listed, and one press of *Reopen* mounts it again at an
address it has never had before.

The page is **not a transport**. Reaching its port is WireGuard, Tailscale, an
SSH tunnel, or a TLS reverse proxy. It never hands its own credential to
anything it frames: each Provider UI sits behind a separate mount credential,
on an origin that is not the page's, and a framed UI that needs browser storage
can have an origin of its own.

The attention defaults and where they are configured, cookie and proxy limits,
reverse-proxy setup, and per-session origins are in
[The remote view](docs/remote.md).

## What you depend on

Choose the features for the capabilities your program uses:

| Feature | What it adds | What it costs |
| --- | --- | --- |
| *(none)* | the frontend script guard, frontend discovery, the trust gate, the per-file overlay | file reading and hashing — no server |
| `cli` | the delivery flag declaration (`UiDeliveryOffer::arg`), and reading it back as a `UiDeliveryMode`; resolving that into a `UiDeliveryPlan` needs `session` | AFDATA CLI types |
| `page` | template rendering, markup guards, and page contract checks | minijinja |
| `session` *(default)* | `UiSession`, `UiSessionRuntime`, managed blobs, `UiHost`, `UiMount`, `UiWindow`, credentials, security policy, the registry, `UiUpstream`, `UiFrontend::assets_router`, remote-view shell and proxy | `cli`, axum with WebSockets, tokio, tower, hyper, minijinja |
| `test-support` | `InstalledFrontend`, `RuntimeClient`, `stub_browser`, and registry waits for testing a Provider through AFUI | `session` and a WebSocket client; take it as a dev-dependency only |

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

**This page is one of them.**
`afui frontend init afui sessions --frontend-id custom` scaffolds a replacement
for the session list itself, held to the same contract as any Provider's: it renders against a `document` that hands out every id, class and
control name the runtime binds to, a page that drops one is refused before the
port is bound, and a workspace file stays inert until somebody enables its exact
fingerprint. Worth saying plainly, because this page is not like the others —
its URL carries the credential for every session on the machine. A frontend
cannot supply behaviour, so an override is markup and CSS and cannot read that
URL; the page's own policy lets presentation load only from this listener and
has nowhere to send anything elsewhere.

Workspace trust is content-addressed: workspace code stays ignored
until you enable its exact fingerprint, and any later edit revokes that trust
until you look again. The files that were trusted are read once, when the
fingerprint is checked, and a running session serves exactly those bytes — an
edit made afterwards reaches nothing until the next start, where it faces the
gate again. `AFUI_SAFE_MODE=1` ignores overrides entirely.

Global frontends are trusted user configuration: installing or editing their
files affects the next session without a workspace enable step. `frontend
enable` and `disable` control workspace trust only; safe mode suppresses both
scopes.

**A replacement starts from a floor, not from nothing.** Every session serves
one AFUI-owned stylesheet that a page links and an override cannot remove: the
palette in both themes, the focus ring, a `hidden` that actually hides, what a
control looks like while a call is in flight, and an inset that clears the
edges of the phone a session was opened on. It is a floor and not a design
system — no components, no layout, no grid — so a replacement can be one
template and no CSS at all and still open as a page that belongs here, and
retuning it is one declaration rather than a fork.

A Provider reaches all of that through `UiFrontend`, which is the delivery half
and only that: it resolves whether a trusted override exists for a
`provider_id` + `ui_kind`, hands back a named file when the override supplies
one, serves the override's `assets/`, and turns "unloadable" or "wrong
`ui_api_version`" into an error that names safe mode rather than a quiet
built-in page. `UiFrontend` resolves files without rendering them. The optional
`UiPage` builder renders the Provider's typed document and checks its markup and
binding contract; the Provider still defines what the data means and how the
page presents it.

```rust,no_run
# fn example() -> agent_first_ui::Result<()> {
use agent_first_ui::UiFrontend;

let frontend = UiFrontend::resolve(std::path::Path::new("."), "inspector", "table_inspect", "1")?;
// `None` means "this override did not supply that file", not "there is no
// override": the overlay is per file, so replacing one page keeps every other
// file the Provider ships — including its language packs.
let page = frontend.text("templates/page.html.j2")?;
let assets = frontend.assets_router();
// Every Provider supplies its own built-in SVG. An override may replace only
// `assets/app-icon.svg`; the resolved icon then follows the frontend into its
// browser favicon and desktop window rather than being chosen by AFUI.
let app_icon = frontend.app_icon(include_str!("app-icon.svg"))?;
# let _ = (page, assets, app_icon);
# Ok(())
# }
```

AFUI enforces these rules rather than describing them, because a Provider that
binds its own behaviour under a nonce depends on them: an override may not supply a file
whose name says it is a script, and `reject_frontend_script` refuses one hiding
inside a template it does supply.

## Live values in both directions

Completion and live I/O are separate. `Outcome<T>` still says how the session
ended exactly once. `UiSession::with_runtime` adds everything that happens
before then without asking each Provider to design transport semantics:

- `publish_state` replaces the retained latest value without waiting for a
  browser; every initial connection and reconnect receives it before becoming
  live;
- typed calls carry actions in and return progress, result, or caller-safe
  error through `UiCall`; request identity never enters a Provider payload;
- `publish_event` assigns AFUI-owned sequence numbers and keeps a bounded replay
  window for facts where every occurrence matters;
- reconnect resends unacknowledged calls under the same identity, so the
  Provider sees one call rather than a retry protocol of its own;
- `with_runtime_and_blobs` lets the Provider hand over bounded bytes while AFUI
  owns unguessable URLs, media allowlists, response headers, release, eviction,
  and cleanup.

The wire is the explicit, versioned `afui-session-v1` envelope, implemented by
AFUI on both sides. Provider types contain only domain data. The page calls
`afui.connect()`; it never names a WebSocket path, injects a request id, or
implements SSE recovery.

```rust,no_run
use agent_first_ui::UiSession;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum SurfaceAction {
    Inspect { target: String },
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum SurfaceReply {
    Inspection { target: String, details: String },
}

#[derive(Serialize)]
struct SurfaceState {
    load_percent: u8,
}

# async fn example() -> agent_first_ui::Result<()> {
let (session, runtime) = UiSession::<()>::new("inspector", "live_status")?
    .with_runtime::<SurfaceAction, SurfaceReply, SurfaceState>()?;
runtime.publish_state(&SurfaceState { load_percent: 42 })?;

tokio::spawn(async move {
    while let Some(call) = runtime.recv().await? {
        match call.action() {
            SurfaceAction::Inspect { target } => {
                let target = target.clone();
                call.finish(&SurfaceReply::Inspection {
                    details: format!("inspected {target}"),
                    target,
                }).await?;
            }
        }
    }
    Ok::<(), agent_first_ui::Error>(())
});

// Deliver `session` with the Provider's Router in the ordinary way.
# let _ = session;
# Ok(())
# }
```

Build the page policy with `UiPagePolicy::allow_runtime()`. The runtime receives
the same credential, exact-host/origin checks, revocation, proxying, and
session shutdown as every AFUI-owned route. A hand-written CSP is an escape
hatch, but AFUI cannot prove that such a string permits the runtime.

## From Rust

This read-only surface stays open until the person closes its window. Opening
or loading the page does not complete a decision.

```rust,no_run
use agent_first_ui::{Outcome, UiAppIcon, UiSession, UiWindowConfig};
use axum::{Router, response::Html, routing::get};

# async fn example() -> agent_first_ui::Result<()> {
let app_icon = UiAppIcon::from_svg(include_str!("app-icon.svg"))?;
let session = UiSession::<()>::new("inspector", "status")?
    .with_app_icon(app_icon);
let router = Router::new().route(
    "/",
    get(|| async {
        Html(r#"<!doctype html>
<html lang="en">
<head>
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Status</title>
  <link rel="stylesheet" href="__afui/base.css">
</head>
<body><main>
  <h1>Inspection ready</h1>
  <p>Close this window when finished.</p>
</main></body>
</html>"#)
    }),
);

let outcome = session.window(router, &UiWindowConfig::default()).await?;
assert!(matches!(outcome, Outcome::Closed));
# Ok(())
# }
```

For decisions, collect an explicit action through the typed runtime or
`UiDecisionRuntime` with `UiSurface`; see
[decide panels](docs/providers.md#decide-panels-declaration-is-theirs-binding-is-yours).
Keep `Closed` and `Expired` as no decision, and never complete an approval from
a GET handler.

## What AFUI is not

Delivery infrastructure, and nothing else: not a domain UI protocol, not a
conversation store, not a generic renderer, not a remote-access transport, not
a frontend package format. It proxies a Provider's UI and transports its typed
values without understanding one byte of either.

## Install

```bash
# prebuilt binary
brew install agentfirstkit/tap/afui   # macOS / Linux
scoop bucket add agentfirstkit https://github.com/agentfirstkit/scoop-bucket && scoop install afui   # Windows

# or from source, after reviewing the repo
cargo install agent-first-ui
```

A source install builds the same feature set as the published packages. A
Provider depending on this crate as a library wants the opposite:
`default-features = false` and only the tier it uses.

Prebuilt archives are also available from
[GitHub Releases](https://github.com/agentfirstkit/agent-first-ui/releases).

## Agent Skill

Then install the embedded [Agent Skill](skills/agent-first-ui/SKILL.md) so the
agent follows AFUI's rules for handing a person an interface — when a step
needs a window at all, which delivery reaches them, and what an override may
and may not change. `skill install` targets Codex, Claude Code, opencode and
Hermes; `skill status` reports whether each install is present, valid and
current:

```bash
afui skill install --agent all --scope workspace
afui skill status
```

## Docs

- [Building a Provider UI](docs/providers.md) — is your UI a session, what an
  override may do to it, and how a decide panel stays safe when its template is
  replaced
- [The remote view](docs/remote.md) — the attention clock and its
  configuration, what the proxy forwards and holds, and per-session origins
- [Standard questions](docs/ask.md) — `afui ask`: evidence, choices, and
  written answers without building a page
- [CLI](docs/cli.md) — generated command and flag reference

## License

MIT

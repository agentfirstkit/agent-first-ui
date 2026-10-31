# Provider pages and live runtime

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

The page may pass an AbortSignal to `runtime.call(action, {signal})`. An already
aborted signal rejects before sending. Aborting a sent call stops the page's
wait and notifies the Provider; it does not roll back domain work. The Provider
checks `UiCall::is_cancelled` or waits on `cancelled()` where stopping is safe.
Use an explicit typed action for business undo or revocation.

The runtime bounds each complete JSON frame at 1 MiB of UTF-8, including its
envelope and JSON escaping. Oversized page actions reject with
`ui_runtime_message_too_large` before sending. Provider byte output belongs in
managed blobs rather than oversized JSON strings.

For a long-lived listener using `UiMountAccess`, mint the same capability with
`issue_with_runtime` or `issue_with_runtime_and_blobs`. The returned runtime is
owned by that one credential and closes when the credential ends; so does any
Provider response still streaming under it, such as an event stream or a long
poll. A Provider WebSocket upgrade is not ended that way, which is one more
reason to carry live traffic on the runtime. Use the capacity variants only
when measured bounds require them.

These delivery facts decide how the page must be written, and none of them is
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

Use the primitive table below before hand-writing what is in it; a few
lines look cheaper than a lookup right up until the copy stops tracking what it
copied. `afui.status(element)` is the one this keeps happening to — a status
line is `textContent` plus `data-state`, and writing those two by hand is how a
page ends up with its own slightly different idea of what clearing means.
Use `afui.inFlight(elements)` whenever controls must stay inactive while a call
is pending: it sets native `disabled` and `aria-disabled` while the operation
runs, then restores their previous values. For non-native controls,
`aria-disabled` only announces the state; the Provider's handler must also
reject activation while pending. Prefer native buttons for actions.

Do not draw a header inside a frame. A live page's usual header — the
program's name, what the session is on, a word about how it is going — repeats
what the frame already shows, so a framed session shows the first two twice. Ask with `afui.frame`: `onHead(true)`
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
For window delivery use `test_support::stub_browser`. A Chromium binary is
never given the session URL: it is launched on a private launch page that
moves on to it, so a hand-written stub behind `AFUI_BROWSER_BINARY` that
records its arguments records a `file://` page, not the URL. Use the provided
stub, or read the URL out of that page. (A snap or Flatpak browser, which
cannot see the launch page, is still given the URL itself.)

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

## Page kernel primitives

Concatenate `page_kernel_source()` before the Provider runtime in one function
scope, inside the session's nonce-bearing script. It defines one lexical `afui`
binding, with no globals. Link the baseline stylesheet and use a
`UiPagePolicy` with `allow_runtime()` when attaching a session runtime.

| API | Use |
| --- | --- |
| `afui.sessionRoot`, `afui.url(path)` | Keep static and navigation URLs inside the credential's mount |
| `afui.connect({onState, onEvent, onEventReset, onConnectionState})` | Connect the typed runtime; use its `call` for actions |
| `runtime.blobUrl`, `runtime.fetchBlob`, `runtime.releaseBlob` | Read and release managed bytes |
| `runtime.openExternal(url)` | Open outside the surface rather than navigating it |
| `afui.status(element)` | Set or clear text and status state in an aria-live region |
| `afui.connection(element, words)` | Bind AFUI connection state with Provider wording |
| `afui.inFlight(elements)` | Disable controls while a call is pending, then restore their previous `disabled` and `aria-disabled` |
| `afui.guardUnload(isDirty)` | Warn before leaving with unsaved changes |
| `afui.place({at, show, hide, onFrame})` | Coordinate a second screen, browser history, Escape, and focus return |
| `afui.frame({onHead})` | Negotiate the surrounding header; publish `title` and `status` |

`runtime.openExternal(url)` acknowledges that the opening request was handed to
the host or browser. In a remote browser, `noopener,noreferrer` prevents a tab
handle being returned even on success, so the result cannot prove a tab is
visible or detect popup blocking. Call from the person's activation and keep a
normal external link available when the task needs a browser-controlled fallback.
It works inside a framed session too: the frame's sandbox lets popups out and
lets them leave the sandbox, so do not replace it with in-frame navigation.

For full Rust signatures and integration examples in a checkout, read
`docs/providers.md` and the relevant module's rustdoc. For a Provider's tests,
use `test-support` and the real runtime clients described above.

# Standard questions: evidence, choices, and written answers

## Problem and boundary

An agent often needs a person's answer to a few questions, each with a short
list of suggestions, while leaving room for something the list did not
anticipate. Requiring a Provider, an HTML page, and button bindings for that
interaction makes a small question unnecessarily expensive. `afui ask` owns
this generic interaction: background context, one to eight questions, one
answer per question, and one terminal result.

Ask displays evidence for a judgment and collects choices and text. Domain
operations, dynamic forms, and sustained interaction belong in a Provider page
using the existing Router/session APIs. The page does not execute commands from answers, render Markdown or HTML, or offer a general
form schema.

## Invocation

One question:

```sh
afui ask --question 'What should happen next?' \
  --options '[{"id":"retry","label":"Retry","description":"Run the operation again","recommended":true},{"id":"details","label":"Inspect details"},{"id":"stop","label":"Stop"}]'
```

Several questions on one page, with background context:

```sh
afui ask \
  --context '[{"type":"paragraph","text":"The review found two issues."},{"type":"list","items":["README lists feature details","The session subject is the whole question"]}]' \
  --questions '[{"id":"scope","question":"Which should be fixed?","multiple":true,"options":[{"id":"readme","label":"README"},{"id":"subject","label":"Subject"}]},{"id":"when","question":"When to release?","allow_custom":false,"options":[{"id":"now","label":"Now"},{"id":"later","label":"Later"}]}]'
```

An option has a stable `id`, a human `label`, an optional `description`, and an
optional `recommended` flag. A question in `--questions` has an `id`, the
`question` text, its `options`, optional `multiple` (allow several choices) and
optional `allow_custom` (default `true`). The single-question form expresses
the last two as `--multiple` and `--no-custom`, and its question's ID is
`question`. Context blocks are `paragraph {text}`, `list {items}`, and
`code {text}`, `image {path,title}`, `table {columns,rows}`, and `diff {text}`.

A complete JSON request contains `context` and `questions`:

```sh
afui ask --request file:request.json
cat request.json | afui ask --request stdin --detach
```

`--request` accepts literal JSON, `file:PATH` (the complete UTF-8 file), `stdin`,
`fd:N`, and `env:NAME`; `file:PATH#DOT_PATH` reads an addressed string value,
and `file+FORMAT:PATH#DOT_PATH` reads it out of a document of that format —
`json`, `toml` or `yaml` — when the extension does not say which.
Use `literal:` to escape source-like literal text. It cannot be combined with
`--question`, `--options`, `--questions`, `--context`, `--multiple`, or
`--no-custom`: use the fields in the complete request instead. File and stream
sources are capped at 16 MiB. Relative image paths resolve from the process's
working directory, including when the request came from a file. Large validated
requests use a session-protected JSON reference so the runtime's existing
1 MiB frame limit does not lower the documented question/context limits.

```json
{"context":[
  {"type":"image","path":"preview-a.png","title":"Image A"},
  {"type":"table","columns":["ID","Status"],"rows":[["A","Ready"]]},
  {"type":"diff","text":"@@ -1 +1 @@\n-before\n+after"}
],"questions":[{"id":"assessment","question":"What should change?","options":[]}]}
```

Images admit local PNG, JPEG, and WebP files only, with matching file signatures;
URLs and SVG are refused. Titles identify the image and may correspond to an
ordinary option ID, such as `a` for Image A. Files are read into bounded runtime
blobs before readiness; the page receives a controlled reference, never a local
path. Tables contain plain text and rectangular rows, without sorting, editing,
or formulas. Diffs display text by line with addition/deletion colors; they do
not execute patches or provide per-hunk decisions.

`--mode window|link|session` uses the existing delivery resolver, including
`AFUI_DELIVERY`. Link delivery uses AFUI's existing attention policy. A
`session` question is reachable only through the session registry, so it is
refused with `ask_delivery_invalid` when `AFUI_NO_REGISTRY` is set, and it fails
if its registry entry cannot be written.

The request is validated before opening a window or issuing a credential:

| Item | Bound |
| --- | --- |
| questions | 1–8, unique IDs |
| question text | non-whitespace, at most 8 KiB |
| options per question | 0–32; zero requires `allow_custom=true`; unique IDs within the question, at most one `recommended` |
| IDs | 1–64 ASCII letters, digits, underscores, or hyphens |
| option label / description | non-whitespace label up to 1 KiB; description up to 4 KiB |
| context | at most 32 blocks; paragraph and code up to 8 KiB; lists of 1–32 items up to 1 KiB |
| images | up to 16; 8 MiB each, 32 MiB total; non-empty title up to 1 KiB |
| tables | 1–32 columns, up to 128 rectangular rows; headings non-empty; cells up to 1 KiB, 256 KiB total |
| diff | non-whitespace text up to 64 KiB |
| written answer or note | at most 16 KiB each |

Unknown fields and unknown block types are rejected. A malformed JSON argument
is reported with the flag, the line and column, and the field name when a field
is unknown, missing, or duplicated; values are never echoed.

## Interaction

The page shows the context, then each question with its options. Single-choice
questions use radio buttons; `multiple` questions use checkboxes. A recommended
option carries a badge but is never preselected: nothing is preselected and
there is no default answer.

When a question allows the person's own words, each kind of text sits inside
the card it belongs to:

- A **note** field opens inside a chosen option's card: one line that grows as
  the person writes. A single-choice question has one note, which moves with
  the selection, so switching options keeps the draft. In a multiple-choice
  question every checked option has its own note.
- A **written answer** is the last card, "Write your own answer". Like a note,
  its text box stays hidden until the card is chosen, then opens inside it with
  focus. In a single-choice question it replaces the options; in a
  multiple-choice question it can be checked beside them, as an answer the
  options did not cover. Unchoosing it hides the box but keeps the draft.

A question with zero options displays its text box immediately, without a
"Write your own answer" selection. It requires `allow_custom=true`; the
single-question equivalent is `--options '[]'`, which rejects `--no-custom`.
Other text boxes appear after choosing their card.

Submit stays disabled until every question has a choice or written answer, so
selection is reviewable before submission. Cancel precedes Submit in source and
focus order. The page uses native form controls, AFUI's baseline stylesheet and
theme variables, responsive layout, visible focus, and live connection/error
status. All request strings are rendered as text, never HTML. English and
Simplified Chinese page controls follow the browser language; supplied content
is unchanged. When framed, the page participates in `afui.frame` header
negotiation.

While disconnected or submitting, answering is disabled. Runtime validation
failures remain on the page and allow correction; a failure the page
recognises is worded in the page's own language rather than shown as the
server's English reason, and anything else reads as a plain failure to submit.
After Cancel the page says it was cancelled. The guidance line is rewritten
once a large request, fetched separately, arrives after the connection went
live, so it reflects whether that question takes a written answer. Reconnect
keeps the current draft. Reloading starts a new draft, never a fabricated
answer.

## Result contract

The readiness progress event includes `phase: "ask_ready"`, the session ID and
resolved delivery facts. It does not include the questions or a private access
URL. Only Link delivery intentionally publishes its handoff URL using the
existing readiness helper. Default finite output sends progress to stderr and
the one terminal result to stdout.

An answered request returns one answer per question, in question order:

```json
{"kind":"result","result":{"code":"ask_answered","answers":[
  {"question_id":"scope","selected":[
    {"option_id":"readme","label":"README","note":"Keep the link"},
    {"option_id":"subject","label":"Subject"}
  ],"text":"Also check the skill"},
  {"question_id":"when","selected":[],"text":"After the next gate run"}
]},"trace":{}}
```

`selected` lists the chosen options with server-resolved labels; `note` is the
person's note on that choice. `text` is a written answer of their own. A
single-choice question has exactly one of a selection or `text`; a
multiple-choice question may have both. Text is preserved exactly as submitted,
and absent when the person wrote nothing or only whitespace.
The 16 KiB limit applies before discarding whitespace-only text, including for
notes attached to a selected option.

Explicit Cancel returns `ask_cancelled`; closing a window or ending the session
returns `ask_closed`; attention expiry returns `ask_expired`. These have no
`answers` field. All are successful interactions (exit 0), but only
`ask_answered` supplies answers. Setup, validation, and transport failures
return structured errors and exit 1; CLI grammar errors retain exit 2.

Window failures keep AFUI's error kind with an `ask_` prefix. Nonzero or
signal termination yields `ask_window_wait_failed`. A successful exit without
a successful page request carrying this launch's query ID yields
`ask_window_unavailable`. Successful exit after page arrival yields
`ask_closed`, which still supplies no answer and implies no consent. This
arrival check does not depend on runtime JavaScript.

## Waiting without holding a command open

`afui ask` blocks until the question ends. A caller whose commands have a time
limit uses `--detach` instead: the question runs in a background process, and
the command returns as soon as it is ready. The parent reads and validates
the complete request and sends it to the worker over stdin, without forwarding
request argv or writing a request file. The worker reads all image files before
reporting readiness, so after `ask_started` the source request and image files
may be removed:

```json
{"kind":"result","result":{"code":"ask_started","mode":"session","session_id":"4181c256…"},"trace":{}}
```

`afui ask wait <session_id>` then returns the same terminal result `afui ask`
would have printed. With `--timeout-s N` it stops after N seconds and reports
that the question is still open:

```json
{"kind":"result","result":{"code":"ask_pending","session_id":"4181c256…","waiting_s":312},"trace":{}}
```

N must be a nonnegative integer. Omit the flag or use `0` to wait until the
question ends; use a positive value for bounded polling. Negative values are
CLI errors and do not collect a stored answer.

`waiting_s` is how long the question has been open, so the caller can decide
when to remind the person through its own channel. AFUI does not send
reminders itself: it does not know where the person is.

A result is collected once: the wait that returns it removes it. A later wait,
or a wait for a question whose process ended without recording a result,
returns `ask_not_found`. The background process records its ending in an
owner-only directory beside the session registry, so `--detach` is refused
with `ask_detach_unavailable` when `AFUI_NO_REGISTRY` keeps questions out of
that registry. The background
process leaves the caller's process group, so a caller that kills its command
on timeout does not take the question with it.

The registry subject of a question is the first line of its first question,
shortened to 120 characters, so a session list stays one line per session.

## Implementation

`ask` is a session-tier module with typed `AskRequest`, `AskQuestion`,
`AskOption`, `AskBlock`, `AskAction`, `AskReply`, `AskChoice`, `AskResponse`,
`AskAnswer`, and `AskSelection` values, plus a start/wait API. It composes `UiSession`,
`UiDeliveryPlan`, nonce CSP, and the existing runtime. The opening retained
state carries the validated request. The trusted page script uses
`afui.connect()`, `afui.connection()` and `afui.status()`; no second transport
or background server is introduced. The built-in page has its own small
stylesheet; the shared baseline remains domain-agnostic. The page is fixed
rather than a new frontend-override contract.

The wait loop handles runtime calls and delivery termination together, so there
is no detached worker whose failure leaves a window waiting indefinitely.
The server validates that every question is answered exactly once, each chosen
ID belongs to its question, a single-choice question has one choice or one
written answer, notes and written text are allowed and bounded, and that
cancellation is explicit.
Completion uses AFUI's existing single-winner completion state. Before ending
the session, the final call reply gets a bounded acknowledgement window
(250 ms); this prevents normal teardown from overtaking that reply, without
letting a disconnected surface hold the question open. Dropping or ending a
delivery revokes access and removes its registry entry.

The same standard page is available to a Rust caller without invoking a CLI:

```rust,no_run
use agent_first_ui::{UiDeliveryPlan, ask::{AskOption, AskQuestion, AskRequest}};

# async fn example() -> agent_first_ui::Result<()> {
let active = AskRequest::single(AskQuestion {
    id: "next".into(),
    question: "What should happen next?".into(),
    options: vec![AskOption {
        id: "retry".into(),
        label: "Retry".into(),
        description: None,
        recommended: true,
    }],
    multiple: false,
    allow_custom: true,
})
.start(UiDeliveryPlan::resolve(None)?)
.await?;
// Report readiness using active.facts() before waiting if needed.
let outcome = active.wait().await?;
# let _ = outcome;
# Ok(())
# }
```

`--detach` and `afui ask wait` are CLI behaviour: a Rust caller already owns
its process and can hold `ActiveAsk` for as long as it likes.

## Verification

Use `scripts/test.sh` as the fixed entrypoint. Cover request validation, single
and multiple choices, notes and written answers through the real runtime,
several questions on one page, invalid/forged submissions and retry,
cancellation, external close, detach/wait including a timeout and single
collection, CLI output/routing, and registry cleanup. The real-browser smoke
test exercises native selection, notes, written answers, context blocks,
several questions, disabled states and narrow-screen layout.

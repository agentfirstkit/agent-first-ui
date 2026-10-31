# Standard questions

## Standard questions: options plus the person's own words

When the interaction is a few questions with suggested answers, use `afui ask`.
Do not create a temporary program, server, or page for this shape. Put
everything the person needs to decide in the request: background goes in
context blocks (paragraphs, lists, code, local raster images, text tables,
diffs). Several related questions belong on one page with `--questions`, not
in a series of windows.

```bash
afui ask --question 'What should happen next?' \
  --options '[{"id":"retry","label":"Retry","recommended":true},{"id":"details","label":"Inspect details"},{"id":"stop","label":"Stop"}]'
```

For several questions and context, put the complete request in a UTF-8 JSON
file and pass `--request file:request.json`:

```json
{"context":[{"type":"paragraph","text":"Two proposed fixes are ready for review."}],"questions":[
  {"id":"scope","question":"Which fixes should be included?","multiple":true,"options":[{"id":"docs","label":"Documentation"},{"id":"validation","label":"Input validation"}]},
  {"id":"timing","question":"When should they be applied?","options":[]}
]}
```

Question IDs and option IDs are stable answer keys. `allow_custom` defaults to
true; a zero-option question is a written-answer question. Relative image paths
resolve from the command's working directory, not the request file's directory.

Give each option a stable ID and a human label; use descriptions for
tradeoffs. Mark your suggestion `recommended` rather than writing
"(recommended)" into a label; the page shows it as a badge and still
preselects nothing. Allow several choices only when the choices genuinely
combine. The person's own words are available by default: do not add an
"Other" option, and use `--no-custom` only when the task requires a closed
choice.

Only `result.code:"ask_answered"` carries `answers`, one per question in
question order. For each, `selected` is the chosen options (act on
`option_id`; `label` is for display), each with an optional `note`, and `text`
is a written answer of the person's own. Read a note as part of the answer: it
often qualifies or narrows the choice it sits on. In a multiple-choice question
`text` beside selections is an answer the options did not cover.
If written text does not settle what the question needed, ask again in fewer
words rather than mapping it onto an option yourself.
Written answers and notes each allow at most 16 KiB of submitted UTF-8 text,
including whitespace. Within that bound, whitespace-only text is omitted.

For example, read the answer keys and any qualifications from this terminal
event; keep the person's wording intact:

```json
{"kind":"result","result":{"code":"ask_answered","answers":[
  {"question_id":"scope","selected":[{"option_id":"docs","label":"Documentation","note":"Keep the public CLI examples"}]},
  {"question_id":"timing","selected":[],"text":"After the tests pass"}
]},"trace":{}}
```

Foreground readiness is a progress event with `progress.phase:"ask_ready"`
and `progress.session_id`, on stderr by default; the terminal result is on
stdout. A detached invocation instead returns `result.code:"ask_started"`.
That confirms readiness, not an answer: use `ask wait` to collect the ending.

`ask_cancelled`, `ask_closed`, and `ask_expired` have no answers. Do not pick
an option on the person's behalf, treat silence as approval, or immediately
reopen a cancelled question. A runtime validation error keeps the page open
for correction; an `ask_request_invalid` CLI error means fix the request first
— JSON parse errors name the flag and field; validation errors describe the
request constraint that failed.

### Waiting when your commands have a time limit

`afui ask` blocks until the question ends. If your command runner kills long
commands, start the question with `--detach`, keep the `session_id` from
`ask_started`, and collect the answer with `afui ask wait <session_id>
--timeout-s N`, repeating while it returns `ask_pending`. A result is collected
once; `ask_not_found` after a successful wait means it was already returned.
Use a positive timeout for bounded polling; omitting it or supplying `0` waits
until the question ends. `--detach` needs the session registry: under
`AFUI_NO_REGISTRY` it is refused with `ask_detach_unavailable`, so either
unset that or run the question in the foreground.

`ask_pending` reports `waiting_s`. AFUI never reminds the person; that is your
call, through the conversation or whatever channel already reaches them. A
reminder is worth sending when the answer blocks work they expect done, not on
every poll.

`--mode` follows `AFUI_DELIVERY` and then defaults to `window`. In `session`
mode, use the announced session ID with `afui session open` or the person's
existing remote session page; `session` needs the registry, so it is refused
with `ask_delivery_invalid` under `AFUI_NO_REGISTRY`. In `link` mode, hand the readiness event's
`link_url` (or `ask_started`'s, with `--detach`) only to the intended person;
the existing attention policy applies.

Ask displays evidence for a judgment and collects choices and text. Use its
image, table, and diff context blocks for previews and comparisons; use a
Provider for domain operations, dynamic forms, or sustained interaction.
`--request file:request.json` or `--request stdin` supplies the complete request
without argv size limits; do not combine it with question/context flags.
A zero-option question with `allow_custom=true` shows a text box immediately.
Images are local PNG/JPEG/WebP files, up to 8 MiB each, with stable titles such
as Image A; match ordinary option IDs to those titles. No arbitrary URLs, SVG,
HTML, or Markdown. The detached parent returns only after the worker has loaded
all images, so files can be removed after `ask_started`.
The standard question does not execute commands or interpret option meanings. A Rust program can use `ask::AskRequest`
with the same delivery plan and typed terminal outcome.

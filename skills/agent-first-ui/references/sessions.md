# Existing sessions and remote pages

## Finding sessions other processes are running

`afui session list` reports every UI session live on this machine, whichever
Provider owns it. Use it to answer "what is waiting for me?" before assuming a
person has nothing open, and to tell two sessions of the same kind apart by
their `subject`. Each entry also reports `waits_for_decision` — a stopped
agent versus an open monitor someone merely has up — and
`announced_at_epoch_ms`, so several waiting entries can be read oldest-first
rather than by whichever happens to sort first alphabetically.

`afui session open <id>` puts a window onto one. It does not take the session
over: whoever created it still decides what completing it means. Closing this
window returns `session_view_closed` and only ends this view; the underlying
session may remain open. Use `session close` when the session itself must end.

`afui session close <id>` ends the session itself, not merely a view of it —
whoever created it gets `Outcome::Closed`, indistinguishable from a person
closing it themselves. Use it to cancel a session you started that no longer
needs an answer, never one someone else is using: ending is available to
anyone who can already reach or list the session, the same access `session
open` already assumes, not a reason to reach for it more freely.

`session_close_requested` means the end request was accepted, not that the
owner has exited; the two are usually a moment apart, so a `session list` run
immediately afterwards may still show the entry. Do not close it again.
`session_not_found` from `open` or `close` means the session has already ended.
List again to see what is open rather than retrying the same id.

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

Framing is isolation, and where it comes from is a choice. By default every framed
UI runs on an opaque origin, which costs it browser storage: an interface that
reads `localStorage` shows its own error there instead of running, which
third-party remote-framebuffer clients commonly do. `--session-origin-host <HOST>` moves
each session onto an origin of its own, `s-<credential>.<HOST>`, which restores
storage without loosening anything: the isolation becomes the browser's origin
boundary instead of an attribute on a frame.

One more thing it buys: with an origin per session, each card can be opened as a
page of its own. Without it that control is absent — not broken — because
opened by hand the session is still sandboxed to an opaque origin with no
storage, so a page of its own gains nothing a frame lacks.

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

The shared page warns after fifteen minutes without attention and becomes
Unattended five minutes later. Its original URL remains valid for the lifetime
of `session serve`, including for questions created while the person was away.
Unattended revokes all mounts and closes their streams; new sessions are listed
without a proxy address. Heartbeat starts a new Live clock but does not revive
old cards: each needs *Reopen*. Sessions arriving after attendance resumes are
automatically mounted again. Reload and polling never revive old mounts.

Direct `link` delivery shares mount attendance rules but retains terminal
`Expired`: rerun that delivery for a new link. Do not disable attention expiry
just to recover a lapsed view; use *Reopen*. Disable it only when the person
requests views that remain attended indefinitely. Only a missing AFUI config
file selects the defaults; any other problem reading it is an error, so repair
the configured source instead of assuming AFUI ignored it. `afui session serve
--help` names the config keys and the per-process overrides.

A person who says a card wastes the screen wants the size control in its head —
Short / Tall / Full, remembered per Provider and UI kind. Do not suggest they
reload, and do not go looking for a way to make a card fit its contents: what is
inside a frame is on an origin the page cannot measure, and that is deliberate.

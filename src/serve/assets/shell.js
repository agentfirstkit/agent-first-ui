'use strict';

/* What is running, refreshed, framed.
 *
 * Two rules run through this file.
 *
 * A frame is never rebuilt for a reason other than its address changing. A
 * terminal that reloaded on every poll would be a terminal that lost its
 * scrollback every two seconds, so cards are added and removed and otherwise
 * left alone.
 *
 * Folding and removing are this page's, not the session's. Removing a card
 * removes it from this person's view; the session is still running, still
 * listed, and still whatever its Provider says it is. Nothing is reported
 * anywhere, and nothing is stored — reopening the page shows everything again,
 * which is the behaviour a person expects from a view they only rearranged. */

(() => {
  const REFRESH_MS = 2000;
  const base = location.pathname.endsWith('/') ? location.pathname : `${location.pathname}/`;

  const countNode = document.querySelector('#count');
  const stateNode = document.querySelector('#state');
  const stateLine = document.querySelector('.state');
  const terminalSlot = document.querySelector('#terminal-slot');
  const terminalEmpty = document.querySelector('#terminal-empty');
  const uiSlot = document.querySelector('#ui-slot');
  const uiEmpty = document.querySelector('#ui-empty');
  const template = document.querySelector('#card');

  /** session_id -> {card, frame, path, slot} */
  const shown = new Map();
  /** session_ids the person has taken off their own page. */
  const dropped = new Set();

  /* Which boundary is keeping the frames apart, as the listener reports it.
   *
   * 'opaque-origin' — every session is a path on this very origin, and what
   * isolates them is the sandbox refusing them an origin at all. That also
   * costs them browser storage, which some third-party clients cannot do
   * without.
   *
   * 'own-origin' — every session has an origin of its own, so the browser's own
   * origin boundary does the isolating and the frame may keep `localStorage`.
   *
   * Reported rather than inferred: the addresses below are the listener's to
   * write and this page's only to use. */
  let isolation = 'opaque-origin';

  function isTerminal(session) {
    return session.provider_id === 'afterminal' && session.ui_kind === 'terminal';
  }

  function frameFor(src, label) {
    const frame = document.createElement('iframe');
    // `allow-same-origin` is added only when the session has an origin of its
    // own. It is not a loosening there: the frame is a different origin from
    // this page either way, so it still cannot reach this page, this page's
    // URL, or another framed UI — it can only reach itself, which is what
    // browser storage requires. Without one origin per session the same flag
    // would put the framed UI on *this* origin, so it stays off.
    //
    // Everything else the sandbox denies stays denied in both shapes: no top
    // navigation, no popups, no pointer lock, no modal dialogs.
    const sandbox = isolation === 'own-origin'
      ? 'allow-scripts allow-forms allow-same-origin'
      : 'allow-scripts allow-forms';
    frame.setAttribute('sandbox', sandbox);
    frame.setAttribute('referrerpolicy', 'no-referrer');
    frame.title = label;
    frame.src = src;
    return frame;
  }

  function build(session) {
    const card = template.content.firstElementChild.cloneNode(true);
    const label = `${session.provider_id} ${session.ui_kind}`;
    card.dataset.session = session.session_id;
    card.querySelector('.provider').textContent = session.provider_id;
    card.querySelector('.kind').textContent = session.ui_kind;
    card.querySelector('.subject').textContent = session.subject || '';
    const holder = card.querySelector('.frame');
    const frame = frameFor(session.proxy_url_secret, label);
    holder.append(frame);
    card.addEventListener('click', event => {
      const act = event.target instanceof HTMLElement ? event.target.dataset.act : null;
      if (act === 'fold') fold(card, event.target);
      if (act === 'drop') drop(session.session_id);
    });
    return {card, frame, path: session.proxy_url_secret};
  }

  function fold(card, button) {
    const folded = card.dataset.folded === 'true';
    card.dataset.folded = folded ? 'false' : 'true';
    if (button) button.textContent = folded ? 'Fold' : 'Unfold';
  }

  function drop(sessionId) {
    dropped.add(sessionId);
    remove(sessionId);
    settle();
  }

  function remove(sessionId) {
    const entry = shown.get(sessionId);
    if (!entry) return;
    entry.card.remove();
    shown.delete(sessionId);
  }

  function place(session, slot) {
    const existing = shown.get(session.session_id);
    if (existing && existing.path === session.proxy_url_secret && existing.card.parentElement === slot) {
      return;
    }
    // Either the address moved — a lapsed proxy credential was replaced — or
    // the session changed slots. Both mean this frame is pointed at the wrong
    // place, and a wrong address is the one thing worth a reload.
    remove(session.session_id);
    const built = build(session);
    slot.append(built.card);
    shown.set(session.session_id, built);
  }

  function settle() {
    terminalEmpty.hidden = terminalSlot.childElementCount > 0;
    uiEmpty.hidden = uiSlot.childElementCount > 0;
    countNode.textContent = String(shown.size);
  }

  function render(sessions) {
    const live = new Set();
    let terminalTaken = false;
    for (const session of sessions) {
      // Live before dismissed: a session the person took off their page is
      // still running, and forgetting that would put it back two seconds later.
      live.add(session.session_id);
      if (dropped.has(session.session_id)) continue;
      // One terminal has the pinned area. A second one is an interface like any
      // other rather than a second pinned area — which area a session lands in
      // is this page's decision and is not asked of anybody.
      const pinned = isTerminal(session) && !terminalTaken;
      if (pinned) terminalTaken = true;
      place(session, pinned ? terminalSlot : uiSlot);
    }
    for (const sessionId of [...shown.keys()]) {
      if (!live.has(sessionId)) remove(sessionId);
    }
    // A session the person removed and whose Provider then ended can be
    // forgotten: if it ever comes back it is a new session with a new id.
    for (const sessionId of [...dropped]) {
      if (!live.has(sessionId)) dropped.delete(sessionId);
    }
    settle();
  }

  async function poll() {
    try {
      const response = await fetch(`${base}sessions`, {headers: {Accept: 'application/json'}});
      if (!response.ok) throw new Error(String(response.status));
      const body = await response.json();
      if (body.isolation === 'own-origin' || body.isolation === 'opaque-origin') {
        isolation = body.isolation;
      }
      render(Array.isArray(body.sessions) ? body.sessions : []);
      stateNode.textContent = 'live';
      stateLine.classList.remove('stale');
    } catch (error) {
      // Say so rather than showing a list that stopped being true. The page
      // keeps its frames: whatever they are talking to is not this listener.
      stateNode.textContent = 'not answering';
      stateLine.classList.add('stale');
    }
  }

  poll();
  setInterval(poll, REFRESH_MS);
})();

'use strict';

/* What is running, refreshed, framed.
 *
 * Four rules run through this file.
 *
 * Every session is placed the same way. This page does not rank them, does not
 * reserve a position for one, and has no notion of which kinds of interface
 * exist — the only ordering is the one the listener reported, which is the
 * order they were announced. A layout that treated some session specially would
 * be this page deciding, on behalf of a program it has never heard of, that its
 * UI matters more than its neighbour's; a layout that sorted them by name would
 * be filing what a person just asked for wherever its Provider's name happens
 * to fall in an alphabet.
 *
 * A frame is never rebuilt for a reason other than its address changing. An
 * interface that reloaded on every poll would lose whatever state it was
 * holding every two seconds, so cards are added and removed and otherwise
 * left alone.
 *
 * How tall a card is, is the person's. What is inside a frame is on an origin
 * this page cannot read, so the page cannot measure it and the frame cannot
 * tell it — not without a protocol this listener would have to trust, which is
 * the one thing it refuses to have. Rather than guess a height for a program it
 * has never heard of, it offers three and remembers which was picked. That is
 * the only thing here worth remembering, so it is the only thing stored.
 *
 * Folding and removing are this page's, not the session's. Removing a card
 * removes it from this person's view; the session is still running, still
 * listed, and still whatever its Provider says it is. Nothing is reported
 * anywhere, and nothing is stored — reopening the page shows everything again,
 * which is the behaviour a person expects from a view they only rearranged.
 *
 * Attention is stated, never inferred from polling. This file asks the listener
 * for the list on a timer whether or not anybody is here; if that renewed what
 * it asked about, nothing would ever lapse. So there is a second, separate
 * request that says a person is present, it is sent only while this page is
 * visible, and it names the sessions actually on the page. Everything else —
 * the countdown, the warning, the end — is the listener's answer, not this
 * page's arithmetic. */

(() => {
  const REFRESH_MS = 2000;
  /* Far below any sane idle window, and far above what the listener needs to
   * hear from a page that is simply open. */
  const ATTENTION_MS = 30000;
  const base = location.pathname.endsWith('/') ? location.pathname : `${location.pathname}/`;

  const countNode = document.querySelector('#count');
  const stateNode = document.querySelector('#state');
  const stateLine = document.querySelector('#state-line');
  const uiSlot = document.querySelector('#ui-slot');
  const uiEmpty = document.querySelector('#ui-empty');
  const template = document.querySelector('#card');
  const pageNotice = document.querySelector('#page-notice');
  const pageNoticeText = document.querySelector('#page-notice-text');
  const pageRenew = document.querySelector('#page-renew');

  /** session_id -> {card, holder, frame, path} */
  const shown = new Map();
  /** session_ids the person has taken off their own page. */
  const dropped = new Set();

  /* The heights on offer, in the order the control cycles them. Kept in step
   * with the stylesheet by a test, because a size nothing styles is a button
   * that does nothing. */
  const SIZES = ['short', 'tall', 'full'];
  const SIZE_LABELS = {short: 'Short', tall: 'Tall', full: 'Full'};
  const SIZE_STORE = 'afui.card-size';

  /* Which size the last card of a given shape was given.
   *
   * Keyed by `(provider_id, ui_kind)`, not by `session_id`: a session ends
   * with its process and a new one gets a new id, so keying on the id would
   * forget the choice the moment the session it was made for did. Keying on
   * the shape instead means the *next* terminal opens at the size the last
   * one was left at — the one piece of arrangement worth outliving both a
   * reload and the session itself. Everything else here is still
   * deliberately unstored — folding and removing come back, because a view
   * somebody only rearranged is expected to. */
  function sizeKeyFor(session) {
    return `${session.provider_id} ${session.ui_kind}`;
  }

  function storedSizes() {
    try {
      const raw = JSON.parse(localStorage.getItem(SIZE_STORE) || '{}');
      return raw && typeof raw === 'object' ? raw : {};
    } catch (error) {
      return {};
    }
  }

  function rememberSize(key, size) {
    try {
      localStorage.setItem(SIZE_STORE, JSON.stringify({...storedSizes(), [key]: size}));
    } catch (error) {
      // A browser refusing storage costs the choice its memory and nothing
      // else; the card is already the size it was asked to be.
    }
  }

  let refreshTimer = null;
  let attendedAt = 0;
  let over = false;

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

  function labelOf(session) {
    return `${session.provider_id} ${session.ui_kind}`;
  }

  function build(session) {
    const card = template.content.firstElementChild.cloneNode(true);
    card.dataset.session = session.session_id;
    card.querySelector('.provider').textContent = session.provider_id;
    card.querySelector('.kind').textContent = session.ui_kind;
    card.querySelector('.subject').textContent = session.subject || '';
    card.addEventListener('click', event => {
      const control = event.target instanceof HTMLElement
        ? event.target.closest('[data-act]')
        : null;
      if (!(control instanceof HTMLButtonElement)) return;
      const act = control.dataset.act;
      const entry = shown.get(session.session_id);
      if (!entry || entry.busy) return;
      if (act === 'fold') fold(card, control);
      if (act === 'drop') drop(session.session_id);
      if (act === 'renew') renewCard(entry);
      if (act === 'reopen') reopen(entry, session.session_id);
      if (act === 'size') resize(card, session);
      if (act === 'end') end(entry, session.session_id);
      closeActionMenu(control);
    });
    setSize(card, storedSizes()[sizeKeyFor(session)] || 'tall');
    return {
      card,
      holder: card.querySelector('.frame'),
      notice: card.querySelector('.card-notice'),
      actionStatus:
        card.querySelector('.card-action-status') || card.querySelector('.card-notice'),
      open: card.querySelector('.act-open'),
      frame: null,
      path: null,
      busy: false,
    };
  }

  function closeActionMenu(control) {
    const menu = control.closest('details');
    if (menu) menu.open = false;
  }

  function setCardAction(entry, busy, message, state = '') {
    entry.busy = busy;
    entry.card.dataset.actionBusy = busy ? 'true' : 'false';
    entry.card.setAttribute('aria-busy', busy ? 'true' : 'false');
    for (const button of entry.card.querySelectorAll('button')) {
      button.disabled = busy;
    }
    entry.actionStatus.textContent = message;
    if (state) entry.actionStatus.dataset.state = state;
    else delete entry.actionStatus.dataset.state;
    if (entry.actionStatus === entry.notice) entry.actionStatus.hidden = !message;
  }

  /* One step through the sizes. A single control rather than three, because a
   * card head on a phone has room for a choice and not for a menu. */
  function resize(card, session) {
    const next = SIZES[(SIZES.indexOf(card.dataset.size) + 1) % SIZES.length];
    setSize(card, next);
    rememberSize(sizeKeyFor(session), next);
  }

  function setSize(card, size) {
    const chosen = SIZES.includes(size) ? size : 'tall';
    card.dataset.size = chosen;
    const button = card.querySelector('[data-act="size"]');
    // The control is labelled with the size the card is, not with the one
    // pressing it would produce: a person reads it far more often than they
    // press it.
    if (button) button.textContent = SIZE_LABELS[chosen];
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

  /* Seconds, as a person reads a countdown they are being asked to act on. */
  function countdown(milliseconds) {
    const total = Math.max(0, Math.round(Number(milliseconds) / 1000));
    return `${Math.floor(total / 60)}:${String(total % 60).padStart(2, '0')}`;
  }

  /* Where a session can be opened as a page of its own, when it can.
   *
   * Only when every session has an origin of its own. In the other shape a
   * session is a path on *this* origin, and opening it top-level would take the
   * Provider UI out of the sandbox and put it on the page's own origin —
   * where it would share storage with every other Provider UI opened the same
   * way. The isolation that makes the frames safe is exactly what a top-level
   * navigation would discard, so the control is absent rather than dangerous.
   *
   * `noopener` on top of that: the new page gets no handle on this one. */
  function opensInAPage(entry, session) {
    const address = session.proxy_url_secret;
    if (isolation !== 'own-origin' || !address) {
      entry.open.hidden = true;
      entry.open.removeAttribute('href');
      return;
    }
    entry.open.hidden = false;
    entry.open.href = address;
    entry.open.title = `Open ${labelOf(session)} in its own page`;
  }

  /* What the listener says about this card, said to the person.
   *
   * The state and the countdown are both the listener's — this page renders the
   * number it was given rather than running a clock of its own, so a phone that
   * was asleep cannot come back believing it has time it does not have. */
  function annotate(entry, session) {
    const state = session.state || 'live';
    entry.card.dataset.state = state;
    entry.notice.dataset.state = state;
    opensInAPage(entry, session);
    const renew = entry.card.querySelector('[data-act="renew"]');
    const reopen = entry.card.querySelector('[data-act="reopen"]');
    const endButton = entry.card.querySelector('[data-act="end"]');
    renew.hidden = state !== 'expiring';
    reopen.hidden = state !== 'dead';
    // Dead means no live proxy path, and `end()` has nowhere to send its
    // request without one. The session may still be running — it is only
    // this *view* that lapsed — but ending it from here needs `reopen` first.
    endButton.hidden = state === 'dead';
    if (state === 'expiring') {
      entry.notice.hidden = false;
      entry.notice.textContent =
        `This view stops in ${countdown(session.expires_in_ms)} unless you keep it open.`;
    } else if (state === 'dead') {
      entry.notice.hidden = false;
      entry.notice.textContent =
        'This view expired and its address stopped working. The session itself is still running.';
    } else {
      entry.notice.hidden = true;
      entry.notice.textContent = '';
    }
  }

  function place(session) {
    // Absent while a session is lapsed, which is what takes its frame off the
    // page rather than leaving an iframe holding the last thing it painted.
    const address = session.proxy_url_secret || null;
    let entry = shown.get(session.session_id);
    if (!entry) {
      entry = build(session);
      uiSlot.append(entry.card);
      shown.set(session.session_id, entry);
    }
    if (entry.path !== address) {
      // The address moved: a lapsed view has none, and a reopened one has a
      // credential it did not have before. Either way this frame is pointed at
      // the wrong place, and a wrong address is the one thing worth a reload.
      if (entry.frame) entry.frame.remove();
      entry.frame = address ? frameFor(address, labelOf(session)) : null;
      if (entry.frame) entry.holder.append(entry.frame);
      entry.path = address;
    }
    annotate(entry, session);
  }

  function settle() {
    uiEmpty.hidden = uiSlot.childElementCount > 0;
    countNode.textContent = String(shown.size);
  }

  function render(sessions) {
    const live = new Set();
    for (const session of sessions) {
      // Live before dismissed: a session the person took off their page is
      // still running, and forgetting that would put it back two seconds later.
      live.add(session.session_id);
      if (dropped.has(session.session_id)) continue;
      place(session);
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

  function renderPage(page) {
    const state = (page && page.state) || 'live';
    pageNotice.dataset.state = state;
    if (state === 'expiring') {
      pageNotice.hidden = false;
      pageNoticeText.textContent =
        `This page stops working in ${countdown(page.expires_in_ms)} unless you keep it open.`;
    } else {
      pageNotice.hidden = true;
      pageNoticeText.textContent = '';
    }
  }

  /* Somebody is here, and these are the sessions they can see.
   *
   * Sent on a timer of its own rather than with every poll, because attention
   * does not need two-second resolution and a phone should not spend a request
   * on it that often. `force` is a person pressing a button, which is worth
   * saying immediately whatever the timer thinks.
   *
   * Every card on the page counts, folded ones included: folding is setting
   * something aside for a moment, not leaving. */
  async function attend(force) {
    if (over) return false;
    if (!force) {
      if (document.visibilityState !== 'visible') return true;
      if (Date.now() - attendedAt < ATTENTION_MS) return true;
    }
    attendedAt = Date.now();
    try {
      const response = await fetch(`${base}attention`, {
        method: 'POST',
        headers: {'Content-Type': 'application/json'},
        body: JSON.stringify({sessions: [...shown.keys()]}),
      });
      if (!response.ok) throw new Error(String(response.status));
    } catch (error) {
      // Say nothing and let the next tick try: the poll is what reports whether
      // this listener is answering, and two voices saying it would disagree.
      attendedAt = 0;
      return false;
    }
    if (force) await poll();
    return true;
  }

  async function renewCard(entry) {
    setCardAction(entry, true, 'Keeping open…', 'pending');
    const kept = await attend(true);
    setCardAction(
      entry,
      false,
      kept ? 'Kept open' : 'Couldn’t keep this open. Try again.',
      kept ? '' : 'error',
    );
  }

  /* Put one lapsed session back, with a credential it did not have before.
   *
   * A person acting on one card, which is why it is not something the heartbeat
   * above can do: a page that revived everything it could see would undo the
   * lapse two seconds after it happened. */
  async function reopen(entry, sessionId) {
    setCardAction(entry, true, 'Reopening…', 'pending');
    try {
      const response = await fetch(
        `${base}sessions/${encodeURIComponent(sessionId)}/reopen`,
        {method: 'POST'},
      );
      if (!response.ok) throw new Error(String(response.status));
    } catch (error) {
      setCardAction(entry, false, 'Couldn’t reopen. Try again.', 'error');
      return;
    }
    await poll();
    const current = shown.get(sessionId);
    if (current) setCardAction(current, false, 'Reopened');
  }

  /* End the session itself, not merely this card's view of it.
   *
   * Reached through the same proxied path the frame is loaded at, one level
   * deeper: `<proxy_url_secret>__afui/end` is the reserved route every
   * session answers under its own credential, regardless of delivery. This
   * page never talks to the session's real URL — only this listener does —
   * so ending it has to go through the same forwarded path everything else
   * about that session goes through. The process waiting on it sees exactly
   * the outcome it would see if a person had closed its own window. */
  async function end(entry, sessionId) {
    if (!entry || !entry.path) return;
    setCardAction(entry, true, 'Ending session…', 'pending');
    try {
      const response = await fetch(`${entry.path}__afui/end`, {method: 'POST'});
      if (!response.ok) throw new Error(String(response.status));
    } catch (error) {
      setCardAction(entry, false, 'Couldn’t end the session. Try again.', 'error');
      return;
    }
    await poll();
    const current = shown.get(sessionId);
    if (current) setCardAction(current, false, 'End requested');
  }

  /* The page's own ending, which is final.
   *
   * Every frame goes with it. The person's terminal is not left painted on a
   * phone that has stopped being allowed to talk to it, and the only way back
   * is a new URL from the machine doing the work — which is the whole of what
   * this expiry is for. */
  function expire() {
    over = true;
    if (refreshTimer) clearInterval(refreshTimer);
    for (const sessionId of [...shown.keys()]) remove(sessionId);
    settle();
    uiEmpty.hidden = true;
    pageNotice.hidden = false;
    pageNotice.dataset.state = 'dead';
    pageNoticeText.textContent =
      'This page expired. Ask for a new session serve URL on the machine doing the work.';
    pageRenew.hidden = true;
    stateNode.textContent = 'expired';
    stateLine.classList.add('stale');
  }

  async function poll() {
    if (over) return;
    try {
      const response = await fetch(`${base}sessions`, {headers: {Accept: 'application/json'}});
      // Not "not answering": this listener answered, and what it said is that
      // this page is over.
      if (response.status === 404) {
        expire();
        return;
      }
      if (!response.ok) throw new Error(String(response.status));
      const body = await response.json();
      if (body.isolation === 'own-origin' || body.isolation === 'opaque-origin') {
        isolation = body.isolation;
      }
      render(Array.isArray(body.sessions) ? body.sessions : []);
      renderPage(body.page);
      stateNode.textContent = 'live';
      stateLine.classList.remove('stale');
    } catch (error) {
      // Say so rather than showing a list that stopped being true. The page
      // keeps its frames: whatever they are talking to is not this listener.
      stateNode.textContent = 'not answering';
      stateLine.classList.add('stale');
    }
    attend(false);
  }

  pageRenew.addEventListener('click', async () => {
    pageRenew.disabled = true;
    pageNoticeText.textContent = 'Keeping this page open…';
    const kept = await attend(true);
    pageRenew.disabled = false;
    if (!kept) pageNoticeText.textContent = 'Couldn’t keep this page open. Try again.';
  });
  // Coming back to a page is the clearest statement there is that somebody is
  // here, and it is the moment a phone has just been unlocked.
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState === 'visible') attend(true);
  });

  poll();
  refreshTimer = setInterval(poll, REFRESH_MS);
})();

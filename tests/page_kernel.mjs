/* The page kernel's behaviour, run in a sandbox with the browser it needs
 * standing in for the browser it does not have.
 *
 * The Rust tests beside this one check what the kernel *is* — one lexical
 * binding, no markup, the reserved routes this crate answers. What only
 * running it can check is what it *does*, and every case below is a mistake a
 * hand-written copy of one of these primitives actually made.
 */
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';

/* Values that crossed out of the sandbox carry that realm's prototypes, so a
 * strict deep-equal on them compares realms rather than content. Comparing the
 * serialisation is what this file actually means every time it says "deep". */
const same = (actual, expected, message) =>
  assert.equal(JSON.stringify(actual), JSON.stringify(expected), message);

const source = fs.readFileSync(new URL('../src/page_runtime/kernel.js', import.meta.url), 'utf8');

/* Enough browser for the primitive under test, and no more. A fuller fake
 * would be a second implementation of the thing being tested. */
function sandbox(overrides = {}) {
  const listeners = new Map();
  /* A history with a cursor and a `popstate` on every move, rather than three
   * counters. What the place primitive is for is exactly what a browser does
   * with the entries it pushes, and a stub that never fires the event would
   * let a primitive that pushes and never comes back pass. */
  const entries = ['/abc123/'];
  let cursor = 0;
  const applyUrl = url => {
    const cut = String(url).indexOf('#');
    context.location.hash = cut === -1 ? '' : String(url).slice(cut);
  };
  const context = {
    location: {
      pathname: '/abc123/',
      protocol: 'http:',
      host: 'localhost:8080',
      hash: '',
      get href() {
        return this.pathname + this.hash;
      },
    },
    history: {
      pushed: 0,
      /** Set when a back gesture had no entry of this page's left to spend. */
      leftPage: false,
      replaceState(_state, _title, url) {
        context.__replaced = url;
        entries[cursor] = url;
        applyUrl(url);
      },
      pushState(_state, _title, url) {
        entries.splice(cursor + 1);
        entries.push(url);
        cursor += 1;
        context.history.pushed += 1;
        applyUrl(url);
      },
      back() {
        if (cursor === 0) {
          context.history.leftPage = true;
          return;
        }
        cursor -= 1;
        applyUrl(entries[cursor]);
        listeners.get('popstate')?.();
      },
      forward() {
        if (cursor + 1 >= entries.length) return;
        cursor += 1;
        applyUrl(entries[cursor]);
        listeners.get('popstate')?.();
      },
    },
    document: {
      visibilityState: 'visible',
      activeElement: null,
      addEventListener(name, handler) {
        listeners.set(name, handler);
      },
    },
    /* A frame around the page, so a primitive that talks to one has somewhere
     * to talk to and the test can read what was said. */
    window: {
      addEventListener(name, handler) {
        listeners.set(name, handler);
      },
      parent: {
        postMessage(message) {
          context.__posted.push(message);
        },
      },
    },
    HTMLElement: class {
      constructor() {
        this.isConnected = true;
        this.focusCount = 0;
      }

      focus() {
        this.focusCount += 1;
      }
    },
    __posted: [],
    setTimeout,
    clearTimeout,
    setInterval,
    clearInterval,
    TextEncoder,
    console,
    __fire: (name, event) => listeners.get(name)?.(event),
    ...overrides,
  };
  vm.createContext(context);
  vm.runInContext(source + '\nglobalThis.__afui = afui;', context, {filename: 'kernel.js'});
  return {afui: context.__afui, context};
}

// ── the session root ─────────────────────────────────────────────────────
{
  const requests = [];
  let status = 204;
  const {afui} = sandbox({fetch: async (url, options) => {
    requests.push({url, options});
    return {ok: status >= 200 && status < 300, status};
  }});
  await afui.end();
  assert.equal(requests[0].url, '/abc123/__afui/end');
  assert.equal(requests[0].options.method, 'POST');
  for (status of [403, 404, 500]) {
    await assert.rejects(afui.end(), failure =>
      failure.code === 'ui_session_end_failed' && failure.status === status);
  }
}

{
  const {afui} = sandbox();
  assert.equal(afui.sessionRoot, '/abc123/');
  assert.equal(afui.url('browse'), '/abc123/browse');
  assert.equal(
    afui.url('/browse'),
    '/abc123/browse',
    'a leading slash must not escape the session root — that is the whole reason the root is derived rather than assumed',
  );
  for (const obsolete of ['request', 'json', 'socketUrl', 'channel', 'stream']) {
    assert.equal(
      afui[obsolete],
      undefined,
      `${obsolete} would let a Provider build a second UI transport beside afui.connect`,
    );
  }
}

/* A path a browser gave without a trailing slash still has to resolve inside
 * the session; a URL built on the wrong root reaches the listener root, where
 * the credential is not. */
{
  const {afui} = sandbox({
    location: {pathname: '/abc123', protocol: 'https:', host: 'lan:9', hash: ''},
  });
  assert.equal(afui.sessionRoot, '/abc123/');
}

// ── the runtime: AFUI owns state, calls, identity, and reconnect ──────────
{
  const sockets = [];
  const reconnects = [];
  class FakeSocket {
    static OPEN = 1;
    constructor(url) {
      this.url = url;
      this.readyState = 0;
      this.handlers = {};
      this.sent = [];
      sockets.push(this);
    }
    addEventListener(name, handler) {
      this.handlers[name] = handler;
    }
    send(data) {
      this.sent.push(JSON.parse(data));
    }
    close() {
      this.readyState = 3;
    }
    open() {
      this.readyState = 1;
      this.handlers.open();
    }
    receive(envelope) {
      this.handlers.message({data: JSON.stringify(envelope)});
    }
    drop() {
      this.readyState = 3;
      this.handlers.close();
    }
  }
  const states = [];
  const connections = [];
  const progress = [];
  const events = [];
  const eventResets = [];
  const {afui, context} = sandbox({
    WebSocket: FakeSocket,
    AbortController,
    setTimeout(handler) {
      reconnects.push(handler);
      return reconnects.length;
    },
    clearTimeout() {},
  });
  const ui = afui.connect({
    onState: (state, meta) => states.push({state, meta}),
    onEvent: (event, meta) => events.push({event, meta}),
    onEventReset: range => eventResets.push(range),
    onConnectionState: state => connections.push(state),
  });
  assert.equal(sockets[0].url, 'ws://localhost:8080/abc123/__afui/runtime');
  sockets[0].open();
  assert.equal(sockets[0].sent[0].kind, 'hello');
  assert.equal(sockets[0].sent[0].hello.protocol, 'afui-session-v1');
  const clientId = sockets[0].sent[0].hello.client_id;

  /* Current state arrives before ready. The page renders it but has no
   * recovery callback to invent: AFUI is not live until synchronization is
   * already complete. */
  sockets[0].receive({
    kind: 'state',
    state: {revision: 4, payload: {screen: 'current'}},
  });
  sockets[0].receive({kind: 'ready', ready: {protocol: 'afui-session-v1'}});
  same(states, [{state: {screen: 'current'}, meta: {revision: 4}}]);
  same(connections, ['live']);

  const blob = {
    blob_url: '__afui/blobs/b0123456789abcdef0123456789abcdef',
    media_type: 'image/png',
    size_bytes: 4,
  };
  assert.equal(
    ui.blobUrl(blob),
    '/abc123/__afui/blobs/b0123456789abcdef0123456789abcdef',
  );
  assert.equal(ui.releaseBlob(blob), true);
  same(sockets[0].sent.at(-1), {
    kind: 'release_blob',
    release_blob: {blob_url: blob.blob_url},
  });
  assert.throws(
    () => ui.blobUrl({blob_url: '../outside'}),
    error => error.code === 'ui_blob_reference_invalid',
  );

  const result = ui.call(
    {type: 'inspect', value: 3},
    {onProgress: value => progress.push(value)},
  );
  const sentCall = sockets[0].sent.at(-1);
  assert.equal(sentCall.kind, 'call');
  assert.equal(sentCall.call.request_id, 'r1');
  same(sentCall.call.payload, {type: 'inspect', value: 3});
  assert.equal(
    sentCall.call.payload.request_id,
    undefined,
    'transport identity must never be injected into a Provider payload',
  );
  sockets[0].receive({
    kind: 'call_progress',
    call_progress: {request_id: 'r1', payload: {phase: 'working'}},
  });
  sockets[0].receive({
    kind: 'event',
    event: {sequence: 9, payload: {name: 'changed'}},
  });
  same(progress, [{phase: 'working'}]);
  same(events, [{event: {name: 'changed'}, meta: {sequence: 9}}]);

  /* A second call is still pending when the wire drops. The new socket uses
   * the same client/request identity and resends only after AFUI has delivered
   * its state and ready marker, so the server can replay or deduplicate it. */
  const resumedPayload = {type: 'slow', value: 'original'};
  const resumed = ui.call(resumedPayload);
  assert.equal(sockets[0].sent.at(-1).call.request_id, 'r2');
  resumedPayload.value = 'mutated after call';
  const controller = new AbortController();
  const cancelled = ui.call({type: 'cancel_me'}, {signal: controller.signal});
  assert.equal(sockets[0].sent.at(-1).call.request_id, 'r3');
  controller.abort();
  await assert.rejects(cancelled, error => error.code === 'ui_call_cancelled');
  same(sockets[0].sent.at(-1), {
    kind: 'cancel',
    cancel: {request_id: 'r3'},
  });
  sockets[0].drop();
  same(connections, ['live', 'reconnecting']);
  reconnects.shift()();
  assert.equal(sockets.length, 2);
  sockets[1].open();
  assert.equal(sockets[1].sent[0].hello.client_id, clientId);
  assert.equal(sockets[1].sent[0].hello.last_event_sequence, 9);
  assert.equal(sockets[1].sent.length, 1, 'pending calls wait until synchronization completes');
  sockets[1].receive({kind: 'state', state: {revision: 5, payload: {screen: 'new'}}});
  sockets[1].receive({kind: 'ready', ready: {protocol: 'afui-session-v1'}});
  same(sockets[1].sent.slice(1).map(frame => ({
    kind: frame.kind,
    requestId: frame[frame.kind].request_id,
  })), [
    {kind: 'call', requestId: 'r1'},
    {kind: 'call', requestId: 'r2'},
    {kind: 'cancel', requestId: 'r3'},
  ], 'every unacknowledged call keeps its AFUI identity across reconnect, and a cancelled one is only cancelled');
  same(
    sockets[1].sent.find(frame => frame.kind === 'call' && frame.call.request_id === 'r2').call
      .payload,
    {type: 'slow', value: 'original'},
    'reconnect must replay the JSON value captured when call() began',
  );

  sockets[1].receive({
    kind: 'call_error',
    call_error: {request_id: 'r3', code: 'ui_call_cancelled', message: 'cancelled'},
  });
  same(sockets[1].sent.at(-1), {kind: 'ack', ack: {request_id: 'r3'}});

  sockets[1].receive({
    kind: 'call_result',
    call_result: {request_id: 'r1', payload: {ok: true}},
  });
  sockets[1].receive({
    kind: 'call_result',
    call_result: {request_id: 'r2', payload: {ok: 'resumed'}},
  });
  same(await result, {ok: true});
  same(await resumed, {ok: 'resumed'});
  assert.equal(sockets[1].sent.at(-1).kind, 'ack');
  const cyclic = {type: 'cyclic'};
  cyclic.self = cyclic;
  const sentBeforeInvalidAction = sockets[1].sent.length;
  await assert.rejects(ui.call(cyclic), error => error.code === 'ui_action_encode_failed');
  assert.equal(sockets[1].sent.length, sentBeforeInvalidAction);
  const alreadyCancelled = new AbortController();
  alreadyCancelled.abort();
  await assert.rejects(
    ui.call({type: 'must_not_run'}, {signal: alreadyCancelled.signal}),
    error => error.code === 'ui_call_cancelled',
  );
  assert.equal(
    sockets[1].sent.length,
    sentBeforeInvalidAction,
    'an already-cancelled action must never be delivered to the Provider',
  );
  for (const text of ['x'.repeat(1024 * 1024 - 16), '字'.repeat(350000), '\u0001'.repeat(180000)]) {
    await assert.rejects(ui.call({text}), error => error.code === 'ui_runtime_message_too_large');
    assert.equal(sockets[1].sent.length, sentBeforeInvalidAction, 'an oversized frame must not reach the socket');
  }
  const bounded = ui.call({text: '字'.repeat(300000)});
  const boundedId = sockets[1].sent.at(-1).call.request_id;
  sockets[1].receive({kind: 'call_result', call_result: {request_id: boundedId, payload: 'accepted'}});
  assert.equal(await bounded, 'accepted', 'a rejected frame must not poison later calls');
  sockets[1].receive({
    kind: 'event_reset',
    event_reset: {oldest_sequence: 0, latest_sequence: 0},
  });
  same(eventResets, [{oldestSequence: 0, latestSequence: 0}]);
  sockets[1].drop();
  reconnects.shift()();
  sockets[2].open();
  assert.equal(sockets[2].sent[0].hello.last_event_sequence, 0);
  sockets[2].receive({kind: 'ready', ready: {protocol: 'afui-session-v1', remote_view: true}});
  const browserRequests = [];
  context.window.open = (...args) => { browserRequests.push(args); return null; };
  same(await ui.openExternal('https://example.test/'), {opened: true, via: 'browser'});
  same(browserRequests, [['https://example.test/', '_blank', 'noopener,noreferrer']]);
  context.window.open = () => { throw new Error('private-browser-detail'); };
  await assert.rejects(ui.openExternal('https://example.test/'), failure =>
    failure.code === 'ui_open_external_failed' && !failure.message.includes('private-browser-detail'));
  ui.close();
  await assert.rejects(ui.call({type: 'too_late'}), error => error.code === 'ui_runtime_closed');
}

// ── autosave: revisions, base hash, conflict ─────────────────────────────
{
  const {afui} = sandbox();
  const saves = [];
  let resolveFirst;
  const saver = afui.autosave({
    baseHash: 'h0',
    delayMs: 1,
    save: (value, baseHash) => {
      saves.push({value, baseHash});
      return saves.length === 1
        ? new Promise(resolve => {
            resolveFirst = resolve;
          })
        : Promise.resolve('h2');
    },
  });

  /* One save on the wire at a time. A second sent while the first is in
   * flight would name the hash from before the first, and the server — which
   * has just moved past it — would answer a conflict that never happened. A
   * newer value waits and goes out against the hash the reply returned, and
   * only the newest of several waiting values is sent at all. */
  const first = saver.flush('first');
  const second = saver.flush('second');
  const third = saver.flush('third');
  same(saves, [{value: 'first', baseHash: 'h0'}], 'nothing else goes out while one is in flight');
  assert.equal(saver.pending(), true, 'a save on the wire, or queued behind one, is not saved yet');
  resolveFirst('h1');
  await Promise.all([first, second, third]);
  assert.equal(saver.pending(), false);
  same(saves, [
    {value: 'first', baseHash: 'h0'},
    {value: 'third', baseHash: 'h1'},
  ]);
  assert.equal(saver.baseHash, 'h2');
}

{
  const {afui} = sandbox();
  const conflicts = [];
  const errors = [];
  const saver = afui.autosave({
    baseHash: 'h0',
    save: async () => {
      const failure = new Error('stale');
      failure.status = 409;
      throw failure;
    },
    onConflict: failure => conflicts.push(failure.message),
    onError: failure => errors.push(failure.message),
  });
  await saver.flush('x');
  same(conflicts, ['stale'], 'a 409 is a conflict, not an error');
  same(errors, []);
  assert.equal(saver.baseHash, 'h0', 'a failed save leaves the base where it was');
}

// ── polling only while somebody is looking ───────────────────────────────
{
  let runs = 0;
  const {afui, context} = sandbox();
  const polling = afui.whileVisible(() => {
    runs += 1;
  }, 10_000);
  assert.equal(runs, 1, 'a visible page is current immediately, not one interval late');

  context.document.visibilityState = 'hidden';
  context.__fire('visibilitychange');
  context.document.visibilityState = 'visible';
  context.__fire('visibilitychange');
  assert.equal(runs, 2, 'coming back refreshes at once rather than showing stale state');
  polling.stop();

  /* Stopping is the caller's decision and outranks the tab's. A page that
   * stopped polling because it left the view that wanted it must not be
   * started again — immediately, at that — by somebody switching tabs. */
  context.document.visibilityState = 'hidden';
  context.__fire('visibilitychange');
  context.document.visibilityState = 'visible';
  context.__fire('visibilitychange');
  assert.equal(runs, 2, 'a stopped poller must stay stopped across a tab switch');
}

/* A page that starts in a background tab must not poll at all. */
{
  let runs = 0;
  const {afui} = sandbox({
    document: {visibilityState: 'hidden', addEventListener() {}},
  });
  afui.whileVisible(() => {
    runs += 1;
  }, 10_000).stop();
  assert.equal(runs, 0);
}

// ── the unload guard asks at the moment of leaving ───────────────────────
{
  let dirty = true;
  const {afui, context} = sandbox();
  afui.guardUnload(() => dirty);

  const warned = {prevented: false};
  context.__fire('beforeunload', {
    preventDefault() {
      warned.prevented = true;
    },
  });
  assert.equal(warned.prevented, true);

  /* Saved in between: warning about nothing is how a guard trains people to
   * click through it. */
  dirty = false;
  const quiet = {prevented: false};
  context.__fire('beforeunload', {
    preventDefault() {
      quiet.prevented = true;
    },
  });
  assert.equal(quiet.prevented, false);
}

// ── status and in-flight write text and disable, and nothing else ────────
{
  const {afui} = sandbox();
  const element = {textContent: 'x', dataset: {}};
  const line = afui.status(element);
  line.set('Saving…', 'pending');
  assert.equal(element.textContent, 'Saving…');
  assert.equal(element.dataset.state, 'pending');
  line.clear();
  assert.equal(element.textContent, '');
  assert.equal(element.dataset.state, undefined);

  const attributes = {};
  const link = {
    getAttribute: name => attributes[name] ?? null,
    setAttribute: (name, value) => {
      attributes[name] = value;
    },
    removeAttribute: name => {
      delete attributes[name];
    },
  };
  const button = {...link, disabled: false};
  const flight = afui.inFlight([link, button]);
  flight.begin();
  assert.equal(attributes['aria-disabled'], 'true');
  assert.equal(button.disabled, true);
  flight.end();
  assert.equal(attributes['aria-disabled'], undefined);
  assert.equal(button.disabled, false);
}

// An in-flight operation must not enable controls disabled for another reason.
{
  const {afui} = sandbox();
  const attributes = {'aria-disabled': 'false'};
  const button = {
    disabled: true,
    getAttribute: name => attributes[name] ?? null,
    setAttribute: (name, value) => { attributes[name] = value; },
    removeAttribute: name => { delete attributes[name]; },
  };
  const flight = afui.inFlight([button]);
  flight.end();
  assert.equal(button.disabled, true, 'ending an idle operation changes nothing');
  flight.begin();
  flight.begin();
  assert.equal(attributes['aria-disabled'], 'true');
  flight.end();
  assert.equal(button.disabled, true, 'the original disabled state survives');
  assert.equal(attributes['aria-disabled'], 'false', 'an explicit ARIA value survives');
  flight.begin();
  flight.end();
  assert.equal(button.disabled, true, 'a second operation captures its own original state');
}

/* ── the connection word is AFUI's, the wording is the page's ─────────────
 *
 * A page left to map these words to a look and a sentence on its own is free
 * to write a branch for the one word that is never delivered. */
{
  const {afui} = sandbox();
  const line = {textContent: 'x', dataset: {}};
  const show = afui.connection(line, {
    connecting: 'Connecting',
    live: 'Live',
    reconnecting: 'Reconnecting',
    closed: 'Disconnected',
  });
  assert.equal(
    line.dataset.afuiConnection,
    'connecting',
    'a page starts disconnected, and the one moment it has nothing else to show is exactly that one',
  );
  assert.equal(line.textContent, 'Connecting');
  show('live');
  assert.equal(line.dataset.afuiConnection, 'live');
  assert.equal(line.textContent, 'Live');
  show('reconnecting');
  assert.equal(line.dataset.afuiConnection, 'reconnecting');
  assert.equal(line.textContent, 'Reconnecting');

  /* A line that also carries page messages says so by supplying no wording:
   * it still gets the attribute, and its own text is left alone. */
  const shared = {textContent: '12 at the root', dataset: {}};
  afui.connection(shared)('closed');
  assert.equal(shared.dataset.afuiConnection, 'closed');
  assert.equal(shared.textContent, '12 at the root');
}

// ── a place inside one page ──────────────────────────────────────────────
/* Every way out of a place has to be the same way out. The cases below are the
 * ones a hand-written version gets wrong: an entry pushed per record instead of
 * per place, a close that tears the place down and leaves the spent entry above
 * the person, a forward gesture that arrives and pushes again, and a frame
 * whose navigation control drives some other page's history. */
{
  const {afui, context} = sandbox();
  const opener = new context.HTMLElement();
  context.document.activeElement = opener;
  const seen = [];
  const place = afui.place({
    at: () => (context.location.hash.includes('record=7') ? '7' : null),
    show: id => seen.push(`show:${id}`),
    hide: () => seen.push('hide'),
    onFrame: frame => seen.push(`frame:${frame.navigation}`),
  });

  same(
    context.__posted,
    [{afui: 1, type: 'head'}],
    'a page says hello, so a frame around it can answer with what it draws',
  );
  assert.equal(place.entered, false);

  place.enter();
  // The page writes the place into the address itself, after entering.
  context.history.replaceState(null, '', '/abc123/#record=7');
  assert.equal(context.history.pushed, 1, 'entering a place is one entry to come back from');
  assert.equal(place.entered, true);
  same(context.__posted.at(-1), {afui: 1, type: 'head', back: true});

  place.enter();
  assert.equal(
    context.history.pushed,
    1,
    'one record after another beside the same list is the same place showing something else',
  );

  place.leave();
  assert.deepEqual(seen, ['hide']);
  assert.equal(place.entered, false);
  assert.equal(context.history.pushed, 1, 'leaving spends the entry rather than pushing another');
  assert.equal(context.history.leftPage, false, 'the first back is the place, not the page');
  assert.equal(opener.focusCount, 1, 'the keyboard goes back to whatever opened the place');
  same(context.__posted.at(-1), {afui: 1, type: 'head', back: false});

  // Forward, into the place again: the entry is already the person's.
  context.history.forward();
  assert.deepEqual(seen, ['hide', 'show:7']);
  assert.equal(place.entered, true);
  assert.equal(context.history.pushed, 1, 'arriving by history pushes nothing of its own');

  // Escape is one of the four ways out, and takes the same one.
  context.__fire('keydown', {key: 'Escape', preventDefault() {}});
  assert.equal(place.entered, false);
  assert.deepEqual(seen, ['hide', 'show:7', 'hide']);

  // With no place open, back belongs to the browser.
  place.leave();
  assert.equal(context.history.leftPage, false, 'leaving nothing must not spend a gesture');
  context.history.back();
  assert.equal(context.history.leftPage, true);
}

{
  const {afui, context} = sandbox();
  const seen = [];
  const place = afui.place({
    at: () => null,
    hide: () => seen.push('hide'),
    onFrame: frame => seen.push(`frame:${frame.navigation}`),
  });
  place.enter();

  const fromFrame = data => context.__fire('message', {source: context.window.parent, data});
  fromFrame({afui: 1, type: 'frame', navigation: true});
  assert.deepEqual(seen, ['frame:true'], 'a frame says what it draws; the page decides what to do about it');

  fromFrame({type: 'back'});
  fromFrame('back');
  fromFrame(null);
  context.__fire('message', {source: {}, data: {afui: 1, type: 'back'}});
  assert.equal(place.entered, true, "only this page's own frame, and only AFUI's own message, may move it");

  fromFrame({afui: 1, type: 'back'});
  assert.equal(place.entered, false, "the frame's navigation control is one more way out of the same place");
  assert.deepEqual(seen, ['frame:true', 'hide']);
}

/* The head a framed page does not draw twice.
 *
 * A live page tends to draw the same one: the program's name, what the
 * session is on, and a word about how it is going — all three of which a frame
 * around it is already showing or can be told. What is checked here is that a
 * page only drops its own on being told there is another, that identity is
 * never what gets sent, and that a page with no frame is left drawing its own. */
{
  const {afui, context} = sandbox();
  const seen = [];
  const head = afui.frame({onHead: drawn => seen.push(drawn)});

  same(context.__posted, [{afui: 1, type: 'head'}], 'a hello carries nothing at all');
  assert.deepEqual(seen, [], 'a page hides nothing until a frame has answered');
  assert.equal(head.head, false);

  context.__fire('message', {
    source: context.window.parent,
    data: {afui: 1, type: 'frame', head: true, navigation: true},
  });
  assert.deepEqual(seen, [true]);
  assert.equal(head.head, true);

  head.title('Record 7');
  same(context.__posted.at(-1), {afui: 1, type: 'head', title: 'Record 7'});
  head.title(null);
  same(
    context.__posted.at(-1),
    {afui: 1, type: 'head', title: null},
    'saying nothing leaves the frame showing what the session itself is on',
  );

  head.status('Live', 'live');
  same(context.__posted.at(-1), {afui: 1, type: 'head', status: 'Live', state: 'live'});
  head.status('16 at the root');
  same(
    context.__posted.at(-1),
    {afui: 1, type: 'head', status: '16 at the root', state: ''},
    "a page's own word about what it is showing is not a connection state",
  );

  assert.doesNotMatch(
    JSON.stringify(context.__posted),
    /provider|kind|afui-sessions/,
    'identity is the listener\'s word, never something a page puts in a head it does not own',
  );
}

/* A page with no frame around it draws its own head and keeps it. A window is
 * its own parent when nothing is holding it, which is the check itself — not
 * the send failing. */
{
  const alone = {addEventListener() {}};
  alone.parent = alone;
  const {afui, context} = sandbox({window: alone});
  const seen = [];
  const head = afui.frame({onHead: drawn => seen.push(drawn)});
  head.title('Record 7');
  head.status('Live', 'live');
  assert.deepEqual(seen, [], 'nothing answered, so nothing is dropped');
  assert.equal(head.head, false);
  assert.deepEqual(context.__posted, [], 'and nothing was said to a frame that is not there');
}

/* The window key: what lets AFUI's own window, and only it, ask the host to
 * open a link. It arrives in the fragment, leaves the address before anything
 * can copy it, survives a reload through this tab's storage, and rides in the
 * hello. A page with no key says nothing, and the runtime treats it as remote. */
{
  const key = 'ab'.repeat(32);
  const helloOf = overrides => {
    const sockets = [];
    class FakeSocket {
      static OPEN = 1;
      constructor() {
        this.readyState = 1;
        this.handlers = {};
        this.sent = [];
        sockets.push(this);
      }
      addEventListener(name, handler) {
        this.handlers[name] = handler;
      }
      send(data) {
        this.sent.push(JSON.parse(data));
      }
    }
    const {afui, context} = sandbox({WebSocket: FakeSocket, ...overrides});
    afui.connect({});
    sockets[0].handlers.open();
    return {hello: sockets[0].sent[0].hello, context};
  };
  const storage = () => {
    const items = new Map();
    return {
      getItem: name => (items.has(name) ? items.get(name) : null),
      setItem: (name, value) => items.set(name, String(value)),
    };
  };
  const at = hash => ({pathname: '/abc123/', search: '', protocol: 'http:', host: 'localhost:8080', hash});

  const sessionStorage = storage();
  const opened = helloOf({location: at(`#afui-window=${key}&record=7`), sessionStorage});
  assert.equal(opened.hello.window_key, key);
  assert.equal(opened.context.__replaced, '/abc123/#record=7', 'the key leaves the address and the page\'s own fragment stays');
  assert.equal(sessionStorage.getItem('afui-window-key:/abc123/'), key);

  const reloaded = helloOf({location: at(''), sessionStorage});
  assert.equal(reloaded.hello.window_key, key, 'a reload of the same window is still the window');

  const linked = helloOf({location: at('#record=7'), sessionStorage: storage()});
  assert.equal('window_key' in linked.hello, false, 'a page reached any other way claims nothing');
  assert.equal(linked.context.__replaced, undefined, 'and its address is left alone');

  const malformed = helloOf({location: at('#afui-window=not-a-key'), sessionStorage: storage()});
  assert.equal('window_key' in malformed.hello, false);
}

console.log('page kernel: ok');

/* AFUI's page kernel: the building material a Provider's own runtime is made
 * of, not a third source of page behaviour.
 *
 * A frontend still cannot supply JavaScript. This source is spliced into the
 * Provider's trusted runtime — under a per-session nonce, or served as the
 * Provider's own file — which is why it may exist at all. What it removes is
 * the part every page in this kit had written for itself: deriving the session
 * root, connecting to the typed AFUI runtime, correlating and replaying calls,
 * resynchronizing retained state and ordered events after a disconnect, and
 * half a dozen small pieces of care that are only interesting when missing.
 *
 * `afui` is a lexical binding, not a global. Concatenate this ahead of your own
 * runtime inside one function scope and it is in scope; nothing here writes to
 * `window`, so two pages on one origin cannot see each other's kernels.
 *
 * Nothing here renders. Every text write is `textContent`, every element comes
 * from the caller, and there is no template, no diff and no state store —
 * because a page's shape is the Provider's, and a library that started deciding
 * it would be deciding what a person sees.
 */
const afui = (function () {
  'use strict';

  /* The path the browser was actually given, ending in a slash so a relative
   * reference resolves inside the session rather than above it. Every URL below
   * is built from this, which is what keeps a request under this session's own
   * credential without the page ever seeing that credential. */
  const sessionRoot = location.pathname.endsWith('/')
    ? location.pathname
    : location.pathname + '/';

  /* The key that marks this page as the local window AFUI itself opened.
   *
   * AFUI opens its own window on the session URL with the key in the
   * fragment, which no server, proxy or log ever sees. A page reached any
   * other way — a link, a remote view, a Provider's own listener reached from
   * another machine — has none, and the runtime treats it as remote. The key
   * moves into this tab's session storage and out of the address at once, so
   * a reload keeps it and nothing the page later copies from the address
   * carries it along. */
  const windowKey = (function () {
    const storageName = 'afui-window-key:' + sessionRoot;
    const pattern = /(^#|&)afui-window=([0-9a-f]{64})(?=&|$)/;
    const match = pattern.exec(location.hash);
    if (!match) {
      try { return sessionStorage.getItem(storageName); } catch (_failure) { return null; }
    }
    try { sessionStorage.setItem(storageName, match[2]); } catch (_failure) {}
    const rest = location.hash.replace(pattern, match[1] === '#' ? '#' : '').replace(/^#&/, '#');
    try {
      history.replaceState(history.state, '', location.pathname + location.search + (rest === '#' ? '' : rest));
    } catch (_failure) {}
    return match[2];
  })();

  function url(path) {
    return sessionRoot + String(path).replace(/^\//, '');
  }

  function socketUrl(path) {
    const scheme = location.protocol === 'https:' ? 'wss:' : 'ws:';
    return scheme + '//' + location.host + url(path);
  }

  function blobPath(reference) {
    const path = reference && reference.blob_url;
    if (typeof path !== 'string' || !/^__afui\/blobs\/b[0-9a-f]{32}$/.test(path)) {
      const failure = new Error('the AFUI blob reference is invalid');
      failure.code = 'ui_blob_reference_invalid';
      throw failure;
    }
    return path;
  }

  function clientId() {
    const browserCrypto = globalThis.crypto;
    if (browserCrypto && typeof browserCrypto.randomUUID === 'function') {
      return 'p' + browserCrypto.randomUUID();
    }
    return 'p' + Date.now().toString(36) + '-' + Math.random().toString(36).slice(2, 12);
  }

  /* An aria-live region a page already declared, written to as text.
   *
   * `state` lands on `data-state` so a stylesheet can say what a status looks
   * like without this deciding it. Clearing is a first-class call rather than
   * `set('')`, because "nothing is happening" and "this says nothing" are
   * different things to a screen reader. */
  function status(element) {
    return {
      set(text, state) {
        if (!element) return;
        element.textContent = text;
        if (state) element.dataset.state = state;
        else delete element.dataset.state;
      },
      clear() {
        if (!element) return;
        element.textContent = '';
        delete element.dataset.state;
      },
    };
  }

  /* Whether this page is still hearing from its own session.
   *
   * The other direction from `status`: that word is the Provider's and lands
   * on `data-state`, which is why nothing styles it. This word is AFUI's own —
   * `connecting`, `live`, `reconnecting`, `closed` — so it lands on
   * `data-afui-connection`, where the baseline stylesheet can say what "what
   * you are looking at may already be wrong" looks like without any page
   * deciding it again.
   *
   * The wording stays yours. `words` is optional and may cover only some of
   * the four, for a line that carries page messages as well and must not have
   * them overwritten; pass none and this only marks the element.
   *
   * `connecting` is painted here rather than delivered to `onConnectionState`.
   * It is true from this call until the first connection is established, and
   * announcing a state a page is already in is not information — which is why
   * a `connecting` branch in a connection handler is a branch that never runs. */
  function connection(element, words) {
    const wording = words || {};
    function show(state) {
      if (!element) return;
      element.dataset.afuiConnection = state;
      if (typeof wording[state] === 'string') element.textContent = wording[state];
    }
    show('connecting');
    return show;
  }

  /* Controls that must not be pressed twice while something is in flight.
   *
   * `aria-disabled` as well as `disabled`, because a control that is not a
   * `<button>` — a link a template chose to style as one — takes only the
   * first, and a person using a screen reader is told either way. */
  function inFlight(elements) {
    const controls = Array.from(elements || []);
    return {
      begin() {
        for (const control of controls) {
          control.setAttribute('aria-disabled', 'true');
          if ('disabled' in control) control.disabled = true;
        }
      },
      end() {
        for (const control of controls) {
          control.removeAttribute('aria-disabled');
          if ('disabled' in control) control.disabled = false;
        }
      },
    };
  }

  /* The complete AFUI session runtime.
   *
   * A Provider supplies only state/action/reply payloads. This client owns the
   * one versioned socket, reconnect, latest-state handoff, ordered-event
   * cursor, request ids, call replay and cancellation. A reconnect always
   * receives AFUI's current state before it becomes `live`, so `onState` never
   * needs an `onGap` or `resumed` recovery branch. */
  function connect(handlers) {
    const settings = handlers || {};
    const protocol = 'afui-session-v1';
    const browserClientId = clientId();
    const pending = new Map();
    // Open-outside requests are not calls: nothing is resent after a wire drop
    // — a link the person clicked once must not open twice — so they wait in
    // their own map and simply fail when the connection is lost.
    const openPending = new Map();
    let openSequence = 0;
    let remoteView = false;
    let requestSequence = 0;
    let lastEventSequence = 0;
    let socket = null;
    let ready = false;
    let stopped = false;
    let reconnectTimer = null;
    let reconnectDelayMs = 100;
    let connectionState = 'connecting';

    function setConnectionState(next) {
      if (connectionState === next) return;
      connectionState = next;
      if (settings.onConnectionState) settings.onConnectionState(next);
    }

    function send(kind, payload) {
      if (!ready || !socket || socket.readyState !== WebSocket.OPEN) return false;
      socket.send(JSON.stringify({kind, [kind]: payload}));
      return true;
    }

    function sendCall(requestId, waiting) {
      if (send('call', {request_id: requestId, payload: waiting.payload})) {
        waiting.sent = true;
      }
    }

    function detachAbort(waiting) {
      if (waiting.abortSignal && waiting.abortHandler) {
        waiting.abortSignal.removeEventListener('abort', waiting.abortHandler);
      }
      waiting.abortSignal = null;
      waiting.abortHandler = null;
    }

    function failPending(failure) {
      for (const [, waiting] of pending) {
        detachAbort(waiting);
        waiting.reject(failure);
      }
      pending.clear();
      failOpenPending(failure);
    }

    function failOpenPending(failure) {
      for (const [, waiting] of openPending) waiting.reject(failure);
      openPending.clear();
    }

    function openFailure(code, message) {
      const failure = new Error(message);
      failure.code = code;
      return failure;
    }

    function openInOwnBrowser(target) {
      // A remote view is a person's own browser: the address bar is right
      // there, and the machine the runtime is on is not where they are.
      const opened = window.open(target, '_blank', 'noopener,noreferrer');
      return opened === null
        ? Promise.reject(openFailure('ui_open_external_blocked', 'the browser blocked opening a new tab'))
        : Promise.resolve({opened: true, via: 'browser'});
    }

    function stopWith(failure) {
      stopped = true;
      ready = false;
      if (reconnectTimer !== null) clearTimeout(reconnectTimer);
      reconnectTimer = null;
      failPending(failure);
      setConnectionState('closed');
      if (socket && socket.readyState < 2) socket.close();
    }

    function callFailure(body) {
      const failure = new Error(body.message || 'the UI action failed');
      failure.code = body.code || 'ui_call_failed';
      failure.hint = body.hint || null;
      failure.retryable = body.retryable === true;
      return failure;
    }

    function actionPayload(payload) {
      try {
        const encoded = JSON.stringify(payload);
        if (typeof encoded !== 'string') throw new Error('not_json');
        return JSON.parse(encoded);
      } catch (_failure) {
        const failure = new Error('the UI action could not be encoded as JSON');
        failure.code = 'ui_action_encode_failed';
        throw failure;
      }
    }

    function receive(event) {
      let envelope;
      try {
        envelope = JSON.parse(event.data);
      } catch (_ignored) {
        return;
      }
      if (!envelope || typeof envelope.kind !== 'string') return;
      const body = envelope[envelope.kind];
      if (!body || typeof body !== 'object') return;

      if (envelope.kind === 'state') {
        if (settings.onState) settings.onState(body.payload, {revision: body.revision});
        return;
      }
      if (envelope.kind === 'event') {
        if (typeof body.sequence === 'number') lastEventSequence = body.sequence;
        if (settings.onEvent) settings.onEvent(body.payload, {sequence: body.sequence});
        return;
      }
      if (envelope.kind === 'event_reset') {
        if (typeof body.latest_sequence === 'number') lastEventSequence = body.latest_sequence;
        if (settings.onEventReset) settings.onEventReset({
          oldestSequence: body.oldest_sequence,
          latestSequence: body.latest_sequence,
        });
        return;
      }
      if (envelope.kind === 'ready') {
        if (body.protocol !== protocol) {
          stopWith(new Error('the AFUI runtime protocol is unsupported'));
          return;
        }
        ready = true;
        remoteView = body.remote_view === true;
        reconnectDelayMs = 100;
        // Open-outside requests that were in flight when the wire dropped are
        // over; the person clicks again if they still want the page.
        failOpenPending(openFailure('ui_open_external_interrupted', 'the connection was lost before the link was opened'));
        for (const [requestId, waiting] of pending) {
          sendCall(requestId, waiting);
          if (waiting.cancelled) send('cancel', {request_id: requestId});
        }
        setConnectionState('live');
        return;
      }
      if (envelope.kind === 'protocol_error') {
        stopWith(callFailure(body));
        return;
      }
      if (envelope.kind === 'closed') {
        stopWith(new Error('the UI session ended'));
        return;
      }

      const requestId = body.request_id;
      if (envelope.kind === 'open_external_result') {
        const opening = typeof requestId === 'string' ? openPending.get(requestId) : null;
        if (!opening) return;
        openPending.delete(requestId);
        if (body.opened === true) opening.resolve({opened: true, via: 'host'});
        else opening.reject(openFailure(body.code || 'ui_open_external_failed', body.message || 'the link was not opened'));
        return;
      }
      const waiting = typeof requestId === 'string' ? pending.get(requestId) : null;
      if (!waiting) return;
      if (envelope.kind === 'call_progress') {
        if (!waiting.cancelled && waiting.onProgress) waiting.onProgress(body.payload);
        return;
      }
      if (envelope.kind === 'call_result') {
        pending.delete(requestId);
        detachAbort(waiting);
        send('ack', {request_id: requestId});
        if (!waiting.cancelled) waiting.resolve(body.payload);
        return;
      }
      if (envelope.kind === 'call_error') {
        pending.delete(requestId);
        detachAbort(waiting);
        send('ack', {request_id: requestId});
        if (!waiting.cancelled) waiting.reject(callFailure(body));
      }
    }

    function reconnect() {
      if (stopped || reconnectTimer !== null) return;
      setConnectionState('reconnecting');
      const delayMs = reconnectDelayMs;
      reconnectDelayMs = Math.min(reconnectDelayMs * 2, 5000);
      reconnectTimer = setTimeout(() => {
        reconnectTimer = null;
        open();
      }, delayMs);
    }

    function open() {
      if (stopped) return;
      ready = false;
      try {
        socket = new WebSocket(socketUrl('__afui/runtime'));
      } catch (_failure) {
        reconnect();
        return;
      }
      socket.addEventListener('open', () => {
        socket.send(JSON.stringify({
          kind: 'hello',
          hello: Object.assign({
            protocol,
            client_id: browserClientId,
            last_event_sequence: lastEventSequence,
          }, windowKey ? {window_key: windowKey} : {}),
        }));
      });
      socket.addEventListener('message', receive);
      socket.addEventListener('close', () => {
        ready = false;
        if (stopped) setConnectionState('closed');
        else reconnect();
      });
      socket.addEventListener('error', () => {});
    }

    open();

    return {
      get state() {
        return connectionState;
      },
      call(payload, options) {
        if (stopped) {
          const failure = new Error('the UI runtime is closed');
          failure.code = 'ui_runtime_closed';
          return Promise.reject(failure);
        }
        let stablePayload;
        try {
          stablePayload = actionPayload(payload);
        } catch (failure) {
          return Promise.reject(failure);
        }
        const callOptions = options || {};
        requestSequence += 1;
        const requestId = 'r' + requestSequence;
        let waiting;
        const result = new Promise((resolve, reject) => {
          waiting = {
            payload: stablePayload,
            onProgress: callOptions.onProgress,
            resolve,
            reject,
            sent: false,
            cancelled: false,
            abortSignal: null,
            abortHandler: null,
          };
          pending.set(requestId, waiting);
          sendCall(requestId, waiting);
        });
        if (callOptions.signal) {
          const cancel = () => {
            const waiting = pending.get(requestId);
            if (!waiting || waiting.cancelled) return;
            waiting.cancelled = true;
            detachAbort(waiting);
            const failure = new Error('the UI action was cancelled');
            failure.code = 'ui_call_cancelled';
            if (!waiting.sent) {
              pending.delete(requestId);
              waiting.reject(failure);
              return;
            }
            send('cancel', {request_id: requestId});
            waiting.reject(failure);
          };
          waiting.abortSignal = callOptions.signal;
          waiting.abortHandler = cancel;
          if (callOptions.signal.aborted) cancel();
          else callOptions.signal.addEventListener('abort', cancel, {once: true});
        }
        return result;
      },
      /* Open a URL outside the page.
       *
       * A page never navigates its own surface to a remote site: a window
       * has no address bar, so the person could not see where they were. In
       * the local window the host opens the URL in the system default
       * browser; in a remote view — the person's own browser — a new tab is
       * opened right there. Only plain http/https URLs are eligible, and the
       * host checks that again. Resolves to `{opened, via}`; rejects with a
       * `code` when the URL was refused, the host could not start a browser,
       * or the connection was lost first. */
      openExternal(target) {
        const value = typeof target === 'string' ? target.trim() : '';
        if (!/^https?:\/\/[^\s/]/i.test(value)) {
          return Promise.reject(openFailure('ui_open_external_url_invalid', 'only a plain http or https URL can be opened outside the page'));
        }
        if (remoteView) return openInOwnBrowser(value);
        if (stopped || !ready) {
          return Promise.reject(openFailure('ui_runtime_closed', 'the UI runtime is not connected'));
        }
        openSequence += 1;
        const requestId = 'o' + openSequence;
        return new Promise((resolve, reject) => {
          openPending.set(requestId, {resolve, reject});
          if (!send('open_external', {request_id: requestId, url: value})) {
            openPending.delete(requestId);
            reject(openFailure('ui_runtime_closed', 'the UI runtime is not connected'));
          }
        });
      },
      blobUrl(reference) {
        return url(blobPath(reference));
      },
      async fetchBlob(reference) {
        const response = await fetch(url(blobPath(reference)), {cache: 'no-store'});
        if (response.ok) return response;
        const failure = new Error('the AFUI-managed bytes are no longer available');
        failure.code = 'ui_blob_unavailable';
        failure.status = response.status;
        throw failure;
      },
      releaseBlob(reference) {
        const blobUrl = blobPath(reference);
        return send('release_blob', {blob_url: blobUrl});
      },
      close() {
        stopWith(new Error('the UI runtime was closed by the page'));
      },
    };
  }

  /* Saving an edit while a person is still making it.
   *
   * Transport only. The debounce, the revision counter that discards a reply
   * to a save the person has already typed past, the `base_hash` written back
   * so the next save is against what the server actually holds, and the
   * separate `onConflict` for a 409 — all of that is the same everywhere. What
   * a conflict *looks like* is the Provider's: this says one happened and hands
   * back what the server has.
   *
   * `save` must resolve to the new base hash, or reject. */
  function autosave(options) {
    const settings = options || {};
    const delay = settings.delayMs || 600;
    let timer = null;
    let revision = 0;
    let baseHash = settings.baseHash || '';

    async function run(value) {
      const mine = (revision += 1);
      try {
        const next = await settings.save(value, baseHash);
        /* A reply to a save the person has already typed past is not the
         * current state, and writing it back would move the base hash
         * backwards. */
        if (mine !== revision) return;
        if (typeof next === 'string') baseHash = next;
        if (settings.onSaved) settings.onSaved(baseHash);
      } catch (failure) {
        if (mine !== revision) return;
        if (failure && failure.status === 409 && settings.onConflict) {
          settings.onConflict(failure);
          return;
        }
        if (settings.onError) settings.onError(failure);
      }
    }

    return {
      get baseHash() {
        return baseHash;
      },
      /* Call on every keystroke; the debounce is here so the caller's handler
       * stays one line. */
      changed(value) {
        if (timer !== null) clearTimeout(timer);
        timer = setTimeout(() => {
          timer = null;
          run(value);
        }, delay);
      },
      /* Save now — on blur, or before something that must not lose the edit. */
      flush(value) {
        if (timer !== null) {
          clearTimeout(timer);
          timer = null;
        }
        return run(value);
      },
      pending() {
        return timer !== null;
      },
    };
  }

  /* The one channel to the window frame this page may be running inside.
   *
   * Both primitives below talk through it, so the frame receives one kind of
   * message from one page and each field has exactly one writer. What is sent
   * is never identity: whose program this is and what the session is on are the
   * listener's word, and a page repeating either into a head it does not own
   * would be a page naming itself in somebody else's chrome.
   *
   * `'*'` because a frame that isolates by sandbox has no origin to name, and
   * nothing sent here is worth keeping from whoever is already holding the
   * page. The answer is remembered so a primitive created later still hears it. */
  const frameChannel = (function () {
    const answered = [];
    const departures = [];
    let answer = null;

    function tell(fields) {
      if (window.parent === window) return;
      try {
        window.parent.postMessage({afui: 1, type: 'head', ...fields}, '*');
      } catch (failure) {
        /* A parent that cannot be posted to is, as far as this page is
         * concerned, no frame at all. Everything still works. */
      }
    }

    if (window.parent !== window) {
      window.addEventListener('message', event => {
        if (event.source !== window.parent) return;
        const message = event.data;
        if (!message || message.afui !== 1) return;
        if (message.type === 'frame') {
          answer = {head: Boolean(message.head), navigation: Boolean(message.navigation)};
          for (const listener of answered) listener(answer);
          return;
        }
        if (message.type === 'back') {
          for (const listener of departures) listener();
        }
      });
    }

    return {
      tell,
      /* Says hello as much as it asks anything: a frame learns from an empty
       * head that the page it is holding speaks at all, and answers with what
       * it draws. */
      hello() {
        tell({});
      },
      onAnswer(listener) {
        answered.push(listener);
        if (answer) listener(answer);
      },
      onBack(listener) {
        departures.push(listener);
      },
    };
  })();

  /* What the window frame around this page should say about it.
   *
   * A framed page otherwise has two heads. The frame draws one — the name of
   * the program, what the session is on, the controls that act on the window —
   * and the page draws its own underneath saying those same things again. On
   * the width these sessions are opened at, the second one is a row of the
   * screen spent on nothing.
   *
   * So the page asks rather than assumes: `onHead` is called with `true` when
   * there is a head out there, and a page that hears it drops its own and
   * sends up the two things a frame cannot know — where the page is, and how
   * what it is showing is going. It is called with nothing at all when the page
   * is a window or a direct link, which is why the page's own head is what it
   * starts with and hiding it is what it does on being told.
   *
   * A frame may cap either, dress either, or ignore both. */
  function frame(options) {
    const settings = options || {};
    let head = false;

    frameChannel.onAnswer(state => {
      head = state.head;
      if (settings.onHead) settings.onHead(state.head);
    });
    frameChannel.hello();

    return {
      /* Where the page is now — `null` to say nothing and leave the frame
       * showing whatever the session itself is on. */
      title(text) {
        frameChannel.tell({title: text === null || text === undefined ? null : String(text)});
      },
      /* One short line about what is being shown, and optionally which of
       * AFUI's own connection words it is: a frame that knows the vocabulary
       * dresses the line the same way this page's baseline would. */
      status(text, state) {
        frameChannel.tell({
          status: text === null || text === undefined ? null : String(text),
          state: state || '',
        });
      },
      get head() {
        return head;
      },
    };
  }

  /* A second screen inside one page, and every way out of it agreeing.
   *
   * A page that opens one record over its list has made a place: on the width
   * this kind of session is opened at most often, that record covers the list
   * completely. Whatever the person then reaches for has to mean the same one
   * thing — the system back gesture, the key that closes things, a control the
   * page drew, and the navigation control of a window frame the page may be
   * running inside. A page that pushes nothing loses the first of those (back
   * leaves the page entirely, from under the record); a page that pushes on
   * every disclosure loses it the other way, and it takes a dozen gestures to
   * leave a page somebody opened from a message.
   *
   * What this owns is the browser mechanics: one entry per place entered,
   * `popstate` in both directions, Escape, the focus that goes back to whatever
   * opened the place, and the frame handshake below. What it renders is
   * nothing. `show` and `hide` are the page's own, because what a place looks
   * like — a pane beside the list, a screen over it — is a question about this
   * page's width and this page's content, and no answer to it belongs here.
   *
   * `at` and `show` are a pair rather than one "you left" callback because a
   * place is somewhere a person can arrive: going forward again, or opening a
   * link that already names it. `at` is asked what the address names now, and
   * whatever it returns is handed to `show`.
   *
   *   const record = afui.place({
   *     at: () => new URLSearchParams(location.hash.slice(1)).get('record'),
   *     show: id => openRecord(id),
   *     hide: () => closeRecord(),
   *     onFrame: frame => backButton.hidden = frame.navigation,
   *   });
   *   // after the page has shown it, and before the page writes the address:
   *   record.enter(record.name);
   *
   * A page still has to be able to rebuild itself from its own URL; this makes
   * the place part of that URL's job explicit rather than adding a second store
   * beside it. */
  function place(options) {
    const settings = options || {};
    let inside = false;
    let returnFocus = null;

    /* The one thing a place tells the frame: that there is somewhere in here
     * to come back from. What the head then says about it is the page's, sent
     * through `frame` above — a place is navigation, not a title. */
    function announce() {
      frameChannel.tell({back: inside});
    }

    /* Entered by the person, here, now.
     *
     * Call it once the place is on screen and before writing the place into
     * the address: what is pushed is a duplicate of the address the person is
     * already at, so the entry below keeps the page as they left it and the
     * page's own `replaceState` writes the place into this new one.
     *
     * Entering a second place without leaving the first pushes nothing. Moving
     * from one record to another beside the same list is not somewhere new to
     * come back from; it is the same place showing something else. */
    function enter() {
      if (inside) return;
      history.pushState(null, '', location.href);
      inside = true;
      returnFocus = document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null;
      announce();
    }

    /* The person asked to leave, by whichever of the four ways.
     *
     * Going back rather than tearing the place down directly, so the entry
     * this page pushed is spent rather than left above the person: `popstate`
     * below does the leaving, once, whoever asked for it. */
    function leave() {
      if (inside) history.back();
    }

    function departed() {
      if (!inside) return;
      inside = false;
      announce();
      if (settings.hide) settings.hide();
      if (returnFocus && returnFocus.isConnected) returnFocus.focus();
      returnFocus = null;
    }

    /* The person moved through their own history. Match the address rather
     * than assume which way they went: out of the place closes it, back into
     * one opens it, and arriving this way pushes nothing — the entry is
     * already theirs. */
    window.addEventListener('popstate', () => {
      const token = settings.at ? settings.at() : null;
      if (token === null || token === undefined || token === false) {
        departed();
        return;
      }
      inside = true;
      if (settings.show) settings.show(token);
    });

    if (settings.escape !== false) {
      document.addEventListener('keydown', event => {
        if (event.key !== 'Escape' || !inside) return;
        event.preventDefault();
        leave();
      });
    }

    /* A frame's navigation control is one more way out of the same place. */
    frameChannel.onBack(leave);
    if (settings.onFrame) {
      frameChannel.onAnswer(state => settings.onFrame({navigation: state.navigation}));
    }
    frameChannel.hello();

    return {
      enter,
      leave,
      get entered() {
        return inside;
      },
    };
  }

  /* Warn before leaving with an unsaved edit.
   *
   * The browser decides the wording; all a page can do is say whether there is
   * something to lose. `isDirty` is asked at the moment of leaving rather than
   * tracked here, so a page that saved in between does not warn about nothing. */
  function guardUnload(isDirty) {
    window.addEventListener('beforeunload', event => {
      if (!isDirty()) return;
      event.preventDefault();
      event.returnValue = '';
    });
  }

  /* Poll only while somebody is looking.
   *
   * A background tab polling a session is traffic for nobody, and on a laptop
   * it is battery for nobody. Running once immediately on becoming visible is
   * the point: what a person sees when they come back should be current, not
   * one interval stale.
   *
   * The caller's `stop` and the pause a hidden tab causes are deliberately not
   * the same thing. Sharing one of them reads as an economy right up until a
   * page that stopped polling — because it navigated away from the view that
   * wanted it — starts again by itself the next time somebody switches tabs
   * back, and runs immediately on arrival while doing it. Stopping is final. */
  function whileVisible(run, intervalMs) {
    let timer = null;
    let stopped = false;
    function pause() {
      if (timer !== null) {
        clearInterval(timer);
        timer = null;
      }
    }
    function start() {
      if (stopped || timer !== null) return;
      run();
      timer = setInterval(run, intervalMs);
    }
    document.addEventListener('visibilitychange', () => {
      if (document.visibilityState === 'visible') start();
      else pause();
    });
    if (document.visibilityState !== 'hidden') start();
    return {
      stop() {
        stopped = true;
        pause();
      },
    };
  }

  /* End this session from inside its own page. Whoever created it sees the
   * same ending as a closed window. */
  async function end() {
    await fetch(url('__afui/end'), {method: 'POST', cache: 'no-store'});
  }

  return {
    sessionRoot,
    url,
    status,
    connection,
    inFlight,
    connect,
    frame,
    place,
    autosave,
    guardUnload,
    whileVisible,
    end,
  };
})();

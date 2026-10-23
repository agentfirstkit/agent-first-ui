(() => {
  const zh = (navigator.language || '').toLowerCase().startsWith('zh');
  const words = zh ? {
    title: '请回答', custom: '自己输入', answer: '你的回答', note: '补充说明（可选）',
    multiple: '可多选', recommended: '推荐',
    cancel: '取消', submit: '提交回答', connecting: '正在连接…',
    live: '请选择，或写下你自己的回答。', closedChoice: '请选择。',
    reconnecting: '连接已断开，正在重连；草稿会保留。',
    closed: '会话已结束。', sending: '正在提交…', finished: '已提交。',
    empty: '每个问题都需要选择一项，或写下你的回答。', tooLong: '文字不能超过 16384 字节。',
    failed: '提交失败，请重试。'
  } : {
    title: 'Answer', custom: 'Write your own answer', answer: 'Your answer',
    note: 'Add a note (optional)', multiple: 'Choose any that apply', recommended: 'Recommended',
    cancel: 'Cancel', submit: 'Submit answer', connecting: 'Connecting…',
    live: 'Choose, or write your own answer.', closedChoice: 'Choose an option.',
    reconnecting: 'Reconnecting; your draft is preserved.',
    closed: 'This session has ended.', sending: 'Submitting…', finished: 'Submitted.',
    empty: 'Answer every question: choose an option or write an answer.',
    tooLong: 'Text must be at most 16384 bytes.',
    failed: 'Could not submit. Please try again.'
  };
  document.documentElement.lang = zh ? 'zh-CN' : 'en';
  document.title = words.title;
  const byId = id => document.getElementById(id);
  const form = byId('answer-form'), container = byId('questions'), contextSection = byId('context');
  const submit = byId('submit'), cancel = byId('cancel');
  const status = afui.status(byId('status'));
  const connection = afui.connection(byId('connection'), words);
  byId('title').textContent = words.title;
  submit.textContent = words.submit;
  cancel.textContent = words.cancel;
  let request = null, questions = [], connectionState = 'connecting', pending = false, finished = false;
  const frame = afui.frame({ onHead(head) { byId('heading').hidden = head; } });

  function element(tag, className, text) {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (text !== undefined) node.textContent = text;
    return node;
  }

  function showContext(blocks) {
    for (const block of blocks) {
      if (block.type === 'paragraph') {
        contextSection.appendChild(element('p', '', block.text));
      } else if (block.type === 'list') {
        const list = element('ul');
        for (const item of block.items) list.appendChild(element('li', '', item));
        contextSection.appendChild(list);
      } else if (block.type === 'code') {
        const pre = element('pre');
        pre.appendChild(element('code', '', block.text));
        contextSection.appendChild(pre);
      }
    }
    contextSection.hidden = blocks.length === 0;
  }

  function card(input, copy) {
    const box = element('div', 'choice');
    const head = element('label', 'choice-head');
    head.append(input, copy);
    box.appendChild(head);
    return box;
  }

  function noteField() {
    const note = element('textarea', 'note');
    note.rows = 1; note.maxLength = 16384;
    note.placeholder = words.note;
    note.setAttribute('aria-label', words.note);
    note.addEventListener('input', update);
    return note;
  }

  function showQuestion(spec) {
    const fieldset = element('fieldset', 'question');
    fieldset.dataset.question = spec.id;
    fieldset.appendChild(element('legend', '', spec.question));
    if (spec.multiple) fieldset.appendChild(element('p', 'hint', words.multiple));
    const type = spec.multiple ? 'checkbox' : 'radio';
    const question = { spec, fieldset, options: [], custom: null, text: null, note: null };
    // A single-choice question has one note, which follows the selection so a
    // draft survives switching; a multiple-choice one has a note per option.
    if (spec.allow_custom && !spec.multiple) question.note = noteField();
    for (const option of spec.options) {
      const input = element('input');
      input.type = type; input.name = 'q-' + spec.id; input.value = option.id;
      input.dataset.option = option.id;
      const copy = element('span', 'copy');
      copy.appendChild(element('strong', '', option.label));
      if (option.recommended) copy.appendChild(element('span', 'badge', words.recommended));
      if (option.description) copy.appendChild(element('small', '', option.description));
      const box = card(input, copy);
      const note = spec.allow_custom && spec.multiple ? noteField() : null;
      if (note) box.appendChild(note);
      fieldset.appendChild(box);
      question.options.push({ input, box, note });
    }
    if (spec.allow_custom) {
      const input = element('input');
      input.type = type; input.name = 'q-' + spec.id; input.dataset.custom = '';
      const box = card(input, element('span', 'copy', words.custom));
      box.classList.add('custom');
      // Like a note, the box appears only once its card is chosen.
      const text = element('textarea', 'answer-text');
      text.rows = 1; text.maxLength = 16384; text.hidden = true;
      text.placeholder = words.answer;
      text.setAttribute('aria-label', words.answer);
      text.addEventListener('input', update);
      input.addEventListener('change', () => { update(); if (input.checked) text.focus(); });
      box.appendChild(text);
      fieldset.appendChild(box);
      question.custom = input; question.text = text;
    }
    container.appendChild(fieldset);
    return question;
  }

  function checked(question) {
    return question.options.filter(option => option.input.checked);
  }

  function noteOf(question, option) {
    return option.note || question.note;
  }

  function written(question) {
    return question.custom && question.custom.checked ? question.text.value : '';
  }

  function answered(question) {
    return checked(question).length > 0 || written(question).trim() !== '';
  }

  function update() {
    const available = request && connectionState === 'live' && !pending && !finished;
    for (const question of questions) {
      question.fieldset.disabled = !available;
      for (const option of question.options) {
        if (option.note) option.note.hidden = !option.input.checked;
      }
      if (question.text) question.text.hidden = !question.custom.checked;
      if (question.note) {
        const [selected] = checked(question);
        question.note.hidden = !selected;
        if (selected && question.note.parentNode !== selected.box) selected.box.appendChild(question.note);
      }
    }
    cancel.disabled = !available;
    submit.disabled = !available || !questions.every(answered);
  }

  function showRequest(state) {
    // Opening state is repeated on reconnect. Keeping the nodes keeps the draft.
    if (request) return;
    request = state;
    showContext(state.context || []);
    questions = state.questions.map(showQuestion);
    frame.title(state.questions[0].question);
    update();
  }

  function guidance() {
    return request && request.questions.every(q => !q.allow_custom) ? words.closedChoice : words.live;
  }

  const client = afui.connect({
    onState: showRequest,
    onConnectionState(state) {
      connectionState = state;
      connection(state);
      if (request && state === 'live') byId('connection').textContent = guidance();
      frame.status(byId('connection').textContent, state);
      update();
    }
  });
  form.addEventListener('change', update);

  async function send(action) {
    if (pending || finished || connectionState !== 'live') return;
    pending = true; update(); status.set(words.sending);
    try {
      await client.call(action);
      finished = true;
      status.set(words.finished);
    } catch (error) {
      status.set(error && error.message ? error.message : words.failed, 'error');
    } finally {
      pending = false; update();
    }
  }
  form.addEventListener('submit', event => {
    event.preventDefault();
    if (!request) return;
    if (!questions.every(answered)) { status.set(words.empty, 'error'); return; }
    const answers = [];
    const fits = text => new TextEncoder().encode(text).length <= 16384;
    const ownWords = text => (text.trim() ? text : null);
    for (const question of questions) {
      const choices = checked(question).map(option => {
        const note = noteOf(question, option);
        return { option_id: option.input.value, note: note ? ownWords(note.value) : null };
      });
      const text = written(question);
      if (!fits(text) || choices.some(choice => choice.note && !fits(choice.note))) {
        status.set(words.tooLong, 'error'); return;
      }
      answers.push({ question_id: question.spec.id, choices, text: ownWords(text) });
    }
    send({ type: 'submit', answers });
  });
  cancel.addEventListener('click', () => send({ type: 'cancel' }));
})();

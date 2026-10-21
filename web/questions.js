import { esc, icon, badge } from './manage-ui.js';

// Questions the model asked with a few choices each (its ask_user tool), as a card at
// the end of the reply. The person steps through them, picks options or writes their
// own, and Submit sends the answers as their next message. Only the newest reply's
// card can be answered; typing a message instead answers in their own words.

export function createQuestions({ state, send }) {
  const forms = new Map();
  const entryOf = m => (Array.isArray(m?.activity) ? m.activity : []).find(e => e.kind === 'question' && Array.isArray(e.questions) && e.questions.length);
  const keyOf = (m, index) => m.id || `message-${index}`;
  const messageAt = index => state.active?.messages?.[Number(index)];
  const isLast = index => Number(index) === (state.active?.messages?.length || 0) - 1;
  function formFor(key, count) {
    if (!forms.has(key)) forms.set(key, { step: 0, picks: Array.from({ length: count }, () => new Set()), other: Array(count).fill(''), skipped: false });
    return forms.get(key);
  }
  const answered = (form, i) => form.picks[i].size > 0 || !!form.other[i].trim();
  const open = (m, index) => !!entryOf(m) && isLast(index) && m.status === 'completed' && !state.streaming && !forms.get(keyOf(m, index))?.skipped;

  function closed(entry, skipped) {
    return `<div class="question-card is-closed">${icon('help')}<div class="grow">${entry.questions.map(q => `<p><strong>${esc(q.header)}</strong> ${esc(q.question)}</p>`).join('')}${skipped ? '<p class="faint">Skipped. Answer in your own words below.</p>' : ''}</div></div>`;
  }
  function card(m, index) {
    const entry = entryOf(m);
    if (!entry) return '';
    const key = keyOf(m, index);
    if (!open(m, index)) return closed(entry, isLast(index) && forms.get(key)?.skipped);
    const questions = entry.questions, form = formFor(key, questions.length), step = Math.min(form.step, questions.length - 1), q = questions[step];
    const options = q.options.map((o, i) => {
      const on = form.picks[step].has(i);
      return `<button type="button" class="question-option ${on ? 'on' : ''}" role="${q.multiple ? 'checkbox' : 'radio'}" aria-checked="${on}" data-action="question-pick" data-index="${index}" data-option="${i}"><span class="question-mark ${q.multiple ? 'square' : ''}">${on ? icon('check') : ''}</span><span class="grow"><strong>${esc(o.label)}</strong>${o.description ? `<small>${esc(o.description)}</small>` : ''}</span>${o.recommended ? badge('Recommended') : ''}</button>`;
    }).join('');
    const last = step === questions.length - 1, any = questions.some((_, i) => answered(form, i));
    const next = last
      ? `<button type="button" class="btn primary small" data-action="question-submit" data-index="${index}" ${any ? '' : 'disabled'}>Submit answers</button>`
      : `<button type="button" class="btn small" data-action="question-next" data-index="${index}">${answered(form, step) ? 'Next' : 'Skip question'}</button>`;
    return `<section class="question-card" data-question="${esc(key)}" aria-label="Questions from the model"><div class="question-head"><strong>${esc(q.header)}</strong>${questions.length > 1 ? `<span class="question-steps" aria-label="Question ${step + 1} of ${questions.length}">${questions.map((_, i) => `<span class="${i === step ? 'current' : answered(form, i) ? 'done' : ''}"></span>`).join('')}</span>` : ''}</div><p class="question-text">${esc(q.question)}</p><div class="question-options" role="${q.multiple ? 'group' : 'radiogroup'}" aria-label="${esc(q.question)}">${options}<label class="question-other"><span>Other</span><input type="text" data-question-other="${index}" value="${esc(form.other[step])}" placeholder="Type your answer" maxlength="500" aria-label="Another answer"></label></div><div class="question-foot"><button type="button" class="btn ghost small" data-action="question-skip" data-index="${index}">Skip</button>${step ? `<button type="button" class="btn ghost small" data-action="question-back" data-index="${index}">Previous</button>` : ''}<span class="grow"></span>${next}</div></section>`;
  }
  /** Whether the card's look changed, for redrawing it while a reply streams. */
  function signature(m, index) { const e = entryOf(m); return e ? `${e.id}:${open(m, index)}` : ''; }

  // Redraws one card in place, keeping focus near where it was.
  function redraw(index, focus = '.question-option.on, .question-option') {
    const article = document.querySelector(`[data-message="${index}"]`), current = article?.querySelector('.question-card');
    if (!current) return;
    current.outerHTML = card(messageAt(index), index);
    article.querySelector(`.question-card ${focus}`)?.focus();
  }
  function answers(entry, form) {
    return entry.questions.map((q, i) => {
      const picked = q.options.filter((_, n) => form.picks[i].has(n)).map(o => o.label);
      const other = form.other[i].trim();
      const answer = [...picked, ...(other ? [other] : [])].join(', ') || 'No preference';
      return `**${q.question}**\n${answer}`;
    }).join('\n\n');
  }
  const withForm = (el, act) => {
    const index = el.dataset.index, m = messageAt(index), entry = entryOf(m);
    if (!entry || !open(m, index)) return;
    act(formFor(keyOf(m, index), entry.questions.length), entry, index);
  };
  const actions = {
    'question-pick': el => withForm(el, (form, entry, index) => {
      const step = form.step, q = entry.questions[step], n = Number(el.dataset.option);
      if (q.multiple) { if (form.picks[step].has(n)) form.picks[step].delete(n); else form.picks[step].add(n); redraw(index, `[data-option="${n}"]`); return; }
      form.picks[step] = new Set([n]); form.other[step] = '';
      // One choice made: go on to the next question.
      if (step < entry.questions.length - 1) { form.step++; redraw(index); } else redraw(index, '[data-action="question-submit"]');
    }),
    'question-next': el => withForm(el, (form, entry, index) => { form.step = Math.min(form.step + 1, entry.questions.length - 1); redraw(index); }),
    'question-back': el => withForm(el, (form, entry, index) => { form.step = Math.max(form.step - 1, 0); redraw(index); }),
    'question-skip': el => withForm(el, (form, entry, index) => { form.skipped = true; redraw(index, 'none'); document.querySelector('#prompt')?.focus(); }),
    'question-submit': el => withForm(el, (form, entry) => { send(answers(entry, form)); }),
  };
  /** Typing another answer replaces a single choice, and keeps the text across redraws. */
  function input(event) {
    const index = event.target.dataset?.questionOther;
    if (index === undefined) return false;
    const m = messageAt(index), entry = entryOf(m);
    if (!entry) return true;
    const form = formFor(keyOf(m, index), entry.questions.length), step = form.step;
    form.other[step] = event.target.value;
    const card = event.target.closest('.question-card');
    if (!entry.questions[step].multiple && event.target.value.trim() && form.picks[step].size) {
      form.picks[step].clear();
      card.querySelectorAll('.question-option.on').forEach(b => { b.classList.remove('on'); b.setAttribute('aria-checked', 'false'); b.querySelector('.question-mark').innerHTML = ''; });
    }
    const next = card.querySelector('[data-action="question-next"]');
    if (next) next.textContent = answered(form, step) ? 'Next' : 'Skip question';
    const submit = card.querySelector('[data-action="question-submit"]');
    if (submit) submit.disabled = !entry.questions.some((_, i) => answered(form, i));
    return true;
  }
  /** Enter in the Other box moves on, like choosing an option. */
  function keydown(event) {
    if (event.key !== 'Enter' || event.target.dataset?.questionOther === undefined) return false;
    event.preventDefault();
    const card = event.target.closest('.question-card');
    (card.querySelector('[data-action="question-next"]') || card.querySelector('[data-action="question-submit"]:not([disabled])'))?.click();
    return true;
  }
  function clear() { forms.clear(); }
  return { card, signature, actions, input, keydown, clear };
}

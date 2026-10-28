import { esc, btn } from './manage-ui.js';
import { icon } from './icons.js';

// How long a reply may wait for its first words before Chat says what is holding it.
export const WAIT_NOTICE_MS = 3000;
// A gateway that can't say how busy its models are is asked again after this long.
const UNAVAILABLE_MS = 5 * 60 * 1000;

// How many run on how many slots. Running can pass slots for a while: when servers are
// removed, the requests they hold finish while the slots already count only the rest.
const running = l => `${l.inFlight} running on ${l.slots} slots`;
/** A model's load in words, for its tooltip and the waiting notice. Free says nothing. */
export const loadLabel = l => !l ? '' : l.state === 'full' ? `Full: ${running(l)}${l.waiting ? `, ${l.waiting} waiting` : ''}`
  : l.state === 'busy' ? `Busy: ${running(l)}` : l.state === 'offline' ? 'No servers running right now' : '';

/**
 * Live load per model, from the gateway through /api/models/load. Quiet by design: a
 * model's own icon turns amber while busy and red while full, and nothing changes while it is free.
 */
export function createLoad({ api, state, modelName }) {
  let data = { available: null, models: {} }, fetchedAt = 0, pending = null;
  const of = id => data.models?.[id];
  /** Fetches the load unless the copy in hand is younger than `maxAge`. */
  function refresh(maxAge = 5000) {
    const age = Date.now() - fetchedAt;
    if (age < (data.available === false ? UNAVAILABLE_MS : maxAge)) return Promise.resolve(data);
    pending ||= api.request('/api/models/load')
      .then(r => { data = { available: !!r?.available, models: r?.models || {} }; return data; })
      .catch(() => data)
      .finally(() => { fetchedAt = Date.now(); pending = null; });
    return pending;
  }
  /** The class that tints a model's icon while it is busy, full or has no servers. */
  function tint(id) {
    const state = of(id)?.state;
    return ['busy', 'full', 'offline'].includes(state) ? `load-${state}` : '';
  }
  /** A model's load in words while it isn't free, for its tooltip and screen readers. */
  const label = id => loadLabel(of(id));
  /** What holds a reply that hasn't started: the person's own limit in its model, or a full model. */
  function holdup(model) {
    const l = of(model), you = l?.you;
    if (you?.limit && you.inFlight >= you.limit) return { own: true, text: `You have ${you.inFlight} of ${you.limit} requests running on ${modelName(model)}, so this one starts when one of them finishes.` };
    if (l?.state === 'full') return { text: `${modelName(model)} is full right now: ${running(l)}${l.waiting ? `, ${l.waiting} waiting` : ''}. Your message is in line.` };
    if (l?.state === 'offline') return { text: `${modelName(model)} has no servers running right now. Your message waits until one starts.` };
    return null;
  }
  /** Another model to send the message to: a favorite or the default that isn't full, else Auto. */
  function alternative(model, needsVision) {
    const usable = id => id && id !== model && state.models.some(m => m.id === id && (id === 'auto' || !needsVision || m.metadata?.supportsVision))
      && (id === 'auto' || ['free', 'busy'].includes(of(id)?.state));
    return [...(state.preferences?.favorites || []), state.preferences?.defaultModel, 'auto'].find(usable) || '';
  }
  /**
   * The notice under a reply that waits for a slot, or that the gateway stopped waiting
   * for: what holds it, and a model to switch to. Empty when nothing is known to hold it.
   */
  function notice(m, index, { waitingSince = 0, needsVision = false } = {}) {
    const model = m.settings?.model;
    if (!model || model === 'auto' || !data.available) return '';
    const gaveUp = m.status === 'error' && /stopped waiting/.test(m.errorMessage || '');
    const waiting = !gaveUp && waitingSince && Date.now() - waitingSince >= WAIT_NOTICE_MS;
    const held = waiting ? holdup(model) : null;
    if (!gaveUp && !held) return '';
    const next = held?.own ? '' : alternative(model, needsVision);
    const label = next === 'auto' ? 'Let Auto choose' : `Switch to ${modelName(next)}`;
    return `<div class="load-notice" ${waiting ? 'role="status"' : ''}>${icon('clock')}<p>${esc(gaveUp ? `${modelName(model)} stayed full.` : held.text)}</p>${next ? btn(label, 'switch-waiting', { tone: 'small', attrs: `data-index="${index}" data-value="${esc(next)}"` }) : ''}</div>`;
  }
  /** Whether load is worth asking for while a reply waits. */
  const offered = () => data.available !== false;
  return { refresh, tint, label, notice, offered, current: () => data };
}

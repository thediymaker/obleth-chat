import { icon } from './icons.js';
export { icon };
export const esc = (value = '') => String(value ?? '').replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
export const titleCase = value => String(value || '').replace(/(^|[-_ ])\w/g, c => c.replace(/[-_]/g, ' ').toUpperCase());
export const displayName = (user, fallback = 'Unknown account') => user?.displayName || user?.display_name || user?.username || fallback;
export const when = value => value ? new Date(value).toLocaleString([], { dateStyle: 'medium', timeStyle: 'short' }) : '—';
export const badge = (label, tone = '') => `<span class="pill ${tone}">${esc(label)}</span>`;
// type '' omits the attribute (connected.js chat chrome renders bare buttons); attrs is raw markup.
export const btn = (label, action, { icon: glyph = '', tone = '', attrs = '', type = 'button' } = {}) => `<button ${type ? `type="${type}" ` : ''}class="btn ${tone}" data-action="${action}" ${attrs}>${glyph ? icon(glyph) : ''}${esc(label)}</button>`;
export const iconButton = (glyph, action, title, attrs = '') => `<button class="icon-btn" data-action="${action}" aria-label="${esc(title)}" title="${esc(title)}" ${attrs}>${icon(glyph)}</button>`;
export const empty = (title, text, action = '') => `<div class="management-empty">${icon('grid')}<h2>${esc(title)}</h2><p>${esc(text)}</p>${action}</div>`;
export const errorBox = (error, action = '') => error ? `<div class="connection-notice" role="alert"><span class="grow">${esc(error)}</span>${action ? btn('Retry', action, { icon: 'refresh', tone: 'small' }) : ''}</div>` : '';
export const field = (id, label, input, help = '') => `<div class="management-field"><label class="form-label" for="${id}">${esc(label)}</label>${input}${help ? `<p class="field-help">${esc(help)}</p>` : ''}</div>`;
export const textInput = (id, value = '', extra = '') => `<input id="${id}" name="${id}" class="form-control" value="${esc(value)}" ${extra}>`;
export const checkbox = (id, label, checked = false, description = '') => `<label class="check-option" for="${id}"><input type="checkbox" id="${id}" name="${id}" ${checked ? 'checked' : ''}><span><strong>${esc(label)}</strong>${description ? `<small>${esc(description)}</small>` : ''}</span></label>`;
export const formEnd = (label = 'Save changes') => `<div class="form-error" role="alert"></div><div class="modal-actions"><button class="btn primary" type="submit">${esc(label)}</button></div>`;
export const modelOptions = (models, selected, includeAuto = true) => models.filter(m => includeAuto || m.id !== 'auto').map(m => `<option value="${esc(m.id)}" ${m.id === selected ? 'selected' : ''}>${esc(m.name || m.id)}</option>`).join('');
export const getValue = (form, name) => form.elements.namedItem(name)?.value ?? '';
export const isChecked = (form, name) => !!form.elements.namedItem(name)?.checked;

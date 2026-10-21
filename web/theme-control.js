import { esc } from './manage-ui.js';
import { icon } from './icons.js';

const options = [['system', 'System', 'monitor'], ['light', 'Light', 'sun'], ['dark', 'Dark', 'moon']];

// Shared by sign-in, the chat header and personal settings. Apply immediately
// without re-rendering forms, so changing theme never loses a draft or password.
// A segmented switch rather than a <select>, whose open list is drawn by the OS.
export function themeControl(id = 'theme-preference', { labels = false } = {}) {
  const preference = globalThis.OblethTheme?.getThemePreference() || 'system';
  return `<div class="theme-control${labels ? ' with-labels' : ''}" id="${esc(id)}" role="radiogroup" aria-label="Color theme">${options.map(([value, label, i]) => `<button type="button" role="radio" data-theme-option="${value}" aria-checked="${value === preference}" title="${label} theme" aria-label="${label}">${icon(i)}${labels ? `<span>${label}</span>` : ''}</button>`).join('')}</div>`;
}

export function bindThemeControls() {
  document.addEventListener('click', event => {
    const option = event.target.closest?.('[data-theme-option]');
    if (option) globalThis.OblethTheme?.setThemePreference(option.dataset.themeOption);
  });
  window.addEventListener('obleth-theme-change', () => {
    const preference = globalThis.OblethTheme.getThemePreference();
    for (const option of document.querySelectorAll('[data-theme-option]')) option.setAttribute('aria-checked', String(option.dataset.themeOption === preference));
  });
}

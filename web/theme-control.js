import { esc } from './manage-ui.js';

// Shared by sign-in, the chat header and personal settings. Apply immediately
// without re-rendering forms, so changing theme never loses a draft or password.
export function themeControl(id = 'theme-preference') {
  const preference = globalThis.OblethTheme?.getThemePreference() || 'system';
  return `<label class="theme-control" for="${esc(id)}"><span>Theme</span><select id="${esc(id)}" data-theme-control aria-label="Color theme">${[['system', 'System'], ['light', 'Light'], ['dark', 'Dark']].map(([value, label]) => `<option value="${value}"${value === preference ? ' selected' : ''}>${label}</option>`).join('')}</select></label>`;
}

export function bindThemeControls() {
  document.addEventListener('change', event => {
    if (event.target.matches('[data-theme-control]')) globalThis.OblethTheme?.setThemePreference(event.target.value);
  });
  window.addEventListener('obleth-theme-change', () => {
    for (const select of document.querySelectorAll('[data-theme-control]')) select.value = globalThis.OblethTheme.getThemePreference();
  });
}

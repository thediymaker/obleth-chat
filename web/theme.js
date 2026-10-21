/* Loaded before styles.css so a saved appearance is applied before first paint. */
(() => {
  const storageKey = 'obleth.theme';
  const allowed = new Set(['system', 'light', 'dark']);
  const root = document.documentElement;
  const system = typeof window.matchMedia === 'function' ? window.matchMedia('(prefers-color-scheme: dark)') : null;
  let defaultPreference = allowed.has(window.OblethChatConfig?.appearance?.defaultTheme)
    ? window.OblethChatConfig.appearance.defaultTheme : 'system';
  let explicitPreference = null;
  let resolvedTheme;

  try {
    const saved = window.localStorage.getItem(storageKey);
    if (allowed.has(saved)) explicitPreference = saved;
  } catch { /* Appearance still works when browser storage is unavailable. */ }

  function getThemePreference() {
    return explicitPreference || defaultPreference;
  }

  function apply() {
    const preference = getThemePreference();
    const theme = preference === 'system' ? (system?.matches ? 'dark' : 'light') : preference;
    const changed = root.dataset.theme !== theme || root.dataset.themePreference !== preference;
    resolvedTheme = theme;
    root.dataset.theme = theme;
    root.dataset.themePreference = preference;
    root.style.colorScheme = theme;
    // Keep native browser UI (including the page background during navigation) in sync.
    const scheme = document.querySelector('meta[name="color-scheme"]');
    if (scheme) scheme.content = theme;
    if (changed) window.dispatchEvent(new CustomEvent('obleth-theme-change', { detail: { preference, theme } }));
    return theme;
  }

  function setThemePreference(preference) {
    if (!allowed.has(preference)) return getThemePreference();
    explicitPreference = preference;
    try { window.localStorage.setItem(storageKey, preference); } catch { /* Keep the choice for this page. */ }
    apply();
    return preference;
  }

  function init(preference = defaultPreference) {
    if (allowed.has(preference)) defaultPreference = preference;
    return apply();
  }

  if (system?.addEventListener) system.addEventListener('change', apply);
  else system?.addListener?.(apply);
  window.addEventListener('storage', (event) => {
    if (event.key !== storageKey && event.key !== null) return;
    explicitPreference = allowed.has(event.newValue) ? event.newValue : null;
    apply();
  });

  window.OblethTheme = Object.freeze({
    getThemePreference,
    getResolvedTheme: () => resolvedTheme,
    setThemePreference,
    init,
  });
  apply();
})();

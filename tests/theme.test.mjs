import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import vm from 'node:vm';

const source = await readFile(new URL('../web/theme.js', import.meta.url), 'utf8');

function browser({ saved = null, dark = false, defaultTheme, blockedStorage = false } = {}) {
  const listeners = new Map();
  const events = [];
  const document = {
    documentElement: { dataset: {}, style: {} },
    querySelector: () => meta,
  };
  const meta = { content: 'light dark' };
  let mediaListener;
  const media = { matches: dark, addEventListener: (_type, listener) => { mediaListener = listener; } };
  const storage = new Map(saved === null ? [] : [['obleth.theme', saved]]);
  const window = {
    OblethChatConfig: { appearance: { defaultTheme } },
    matchMedia: () => media,
    localStorage: {
      getItem: (key) => { if (blockedStorage) throw new Error('Storage denied'); return storage.get(key) ?? null; },
      setItem: (key, value) => { if (blockedStorage) throw new Error('Storage denied'); storage.set(key, value); },
    },
    addEventListener: (type, listener) => listeners.set(type, listener),
    dispatchEvent: (event) => events.push(event),
  };
  class CustomEvent { constructor(type, options) { this.type = type; this.detail = options.detail; } }
  vm.runInNewContext(source, { window, document, CustomEvent });
  return {
    api: window.OblethTheme, root: document.documentElement, meta, storage, events,
    systemDark: (value) => { media.matches = value; mediaListener(); },
    storageChanged: (key, newValue) => listeners.get('storage')({ key, newValue }),
  };
}

test('startup follows system appearance and tracks system changes', () => {
  const page = browser();
  assert.equal(page.root.dataset.theme, 'light');
  assert.equal(page.root.dataset.themePreference, 'system');
  assert.equal(page.meta.content, 'light');
  page.systemDark(true);
  assert.equal(page.api.getResolvedTheme(), 'dark');
  assert.equal(page.root.style.colorScheme, 'dark');
  assert.equal(page.events.at(-1).type, 'obleth-theme-change');
  assert.equal(page.events.at(-1).detail.preference, 'system');
});

test('saved and explicit choices override deployment and system preferences', () => {
  const page = browser({ saved: 'light', dark: true, defaultTheme: 'dark' });
  assert.equal(page.root.dataset.theme, 'light');
  page.api.init('dark');
  page.systemDark(false);
  assert.equal(page.api.getThemePreference(), 'light');
  page.api.setThemePreference('dark');
  assert.equal(page.root.dataset.theme, 'dark');
  assert.equal(page.storage.get('obleth.theme'), 'dark');
  page.api.setThemePreference('system');
  assert.equal(page.root.dataset.theme, 'light');
  assert.equal(page.storage.get('obleth.theme'), 'system');
});

test('deployment defaults apply before first paint without persisting them as user choices', () => {
  const page = browser({ defaultTheme: 'dark' });
  assert.equal(page.root.dataset.theme, 'dark');
  assert.equal(page.storage.size, 0);
  page.api.init('light');
  assert.equal(page.root.dataset.theme, 'light');
  assert.equal(page.storage.size, 0);
  assert.equal(browser({ defaultTheme: 'invalid' }).api.getThemePreference(), 'system');
});

test('appearance continues to work with blocked storage and ignores invalid preferences', () => {
  const page = browser({ blockedStorage: true });
  page.api.setThemePreference('dark');
  assert.equal(page.root.dataset.theme, 'dark');
  assert.equal(page.api.setThemePreference('invalid'), 'dark');
  assert.equal(page.root.dataset.theme, 'dark');
  assert.equal(browser({ saved: 'invalid' }).api.getThemePreference(), 'system');
});

test('other tabs update explicit preferences and clearing storage restores the configured default', () => {
  const page = browser({ defaultTheme: 'dark' });
  page.storageChanged('unrelated', 'light');
  assert.equal(page.root.dataset.theme, 'dark');
  page.storageChanged('obleth.theme', 'light');
  assert.equal(page.root.dataset.theme, 'light');
  page.storageChanged(null, null);
  assert.equal(page.root.dataset.theme, 'dark');
  assert.equal(page.api.getThemePreference(), 'dark');
});

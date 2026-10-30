import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { runInNewContext } from 'node:vm';
import { normalizeBranding, brand, brandLogo, brandIcon, siteBanner, siteFooter, copy, pageTitle } from '../web/branding.js';

test('partial or malformed branding keeps usable defaults and optional copy can be removed', () => {
  for (const input of [undefined, null, 42, [], 'invalid']) {
    const config = normalizeBranding(input);
    assert.equal(config.siteName, 'Obleth Chat');
    assert.equal(config.appearance.defaultTheme, 'system');
    assert.match(brandLogo(config), /obleth\.svg/);
    assert.deepEqual(config.links.map(link => link.label), ['Documentation', 'Obleth']);
  }
  const config = normalizeBranding({
    siteName: '  Acme Chat  ',
    assistantName: '',
    home: { eyebrow: '', description: false },
    login: { title: '', help: '' },
    icons: { greeting: '', assistant: 'book', loading: 'toString' },
    appearance: { defaultTheme: 'unknown' },
  });
  assert.equal(config.siteName, 'Acme Chat');
  assert.equal(config.assistantName, 'Assistant');
  assert.equal(config.home.eyebrow, '');
  assert.equal(config.login.title, '');
  assert.equal(config.login.help, '');
  assert.equal(config.home.description, 'Ask a question, explore an idea, or start something new.');
  assert.equal(brandIcon('greeting', config), '');
  assert.match(brandIcon('assistant', config), /<svg/);
  assert.equal(config.icons.loading, 'chat');
  assert.equal(config.appearance.defaultTheme, 'system');
});

test('public config is executable without a build and matches fallback branding', async () => {
  const window = {};
  runInNewContext(await readFile(new URL('../web/site.config.js', import.meta.url), 'utf8'), { window });
  assert.deepEqual(normalizeBranding(window.OblethChatConfig), normalizeBranding());
});

test('branding text is escaped and configured line breaks are preserved', () => {
  assert.equal(copy('<img src=x onerror="alert(1)">\nA & B\r\nC'), '&lt;img src=x onerror=&quot;alert(1)&quot;&gt;<br>A &amp; B<br>C');
  const config = normalizeBranding({ siteName: 'Acme <Chat>', siteSubtitle: 'Ask & learn', logo: { light: './assets/mark.svg', alt: '"Brand"' } });
  const html = brand(config);
  assert.match(html, /Acme &lt;Chat&gt; home/);
  assert.match(html, /Ask &amp; learn/);
  assert.match(html, /alt="&quot;Brand&quot;"/);
  assert.doesNotMatch(html, /Obleth|obleth\.svg|<Chat>/);
  assert.equal(pageTitle('Settings', config), 'Settings · Acme <Chat>');
  assert.equal(pageTitle('', config), 'Acme <Chat>');
});

test('a line break in the site name stacks it beside the logo and nowhere else', () => {
  const config = normalizeBranding({ siteName: ' Research \n Computing <Lab>\n\n' });
  assert.deepEqual(config.siteNameLines, ['Research', 'Computing <Lab>']);
  assert.equal(config.siteName, 'Research Computing <Lab>');
  const html = brand(config);
  assert.match(html, /<div class="brand-title">Research<br>Computing &lt;Lab&gt;<\/div>/);
  assert.match(html, /aria-label="Research Computing &lt;Lab&gt; home"/);
  assert.match(siteFooter(config, 2026), /© 2026 Research Computing &lt;Lab&gt;/);
  assert.equal(pageTitle('', config), 'Research Computing <Lab>');
  assert.equal(normalizeBranding({ siteName: '\n \n' }).siteName, 'Obleth Chat');
});

test('logo removal and light/dark asset fallback work without a mandatory mark', () => {
  const noLogo = normalizeBranding({ logo: { light: '', dark: '' } });
  assert.equal(brandLogo(noLogo), '');
  assert.doesNotMatch(brand(noLogo), /<img|brand-logo/);
  for (const variant of ['light', 'dark']) {
    const config = normalizeBranding({ logo: { [variant]: './assets/custom/mark.svg' }, favicon: { [variant]: '/custom/tab.ico' } });
    assert.equal(config.logo.light, './assets/custom/mark.svg');
    assert.equal(config.logo.dark, './assets/custom/mark.svg');
    assert.equal(config.favicon.light, '/custom/tab.ico');
    assert.equal(config.favicon.dark, '/custom/tab.ico');
    assert.match(brandLogo(config), /class="brand-logo-light"/);
    assert.match(brandLogo(config), /class="brand-logo-dark"/);
  }
  const config = normalizeBranding({ logo: { light: './light.svg', dark: './dark.svg' } });
  assert.equal(config.logo.light, './light.svg');
  assert.equal(config.logo.dark, './dark.svg');
});

test('asset paths reject external and executable URLs while preserving local image paths', () => {
  const unsafe = ['https://example.com/logo.svg', '//example.com/logo.svg', 'javascript:alert(1)', 'data:image/svg+xml,<svg/>', '\\example.com/logo.svg', '/\\example.com/logo.svg', '\njava\nscript:alert(1)', '#fragment'];
  for (const path of unsafe) {
    const config = normalizeBranding({ logo: { light: path, dark: path }, favicon: { light: path } });
    assert.equal(brandLogo(config), '', path);
    assert.equal(config.favicon.light, './assets/chat.svg', path);
  }
  const config = normalizeBranding({ logo: { light: './assets/my%20logo.svg?v=2' } });
  assert.equal(config.logo.light, './assets/my%20logo.svg?v=2');
});

test('browser metadata uses configured branding and favicon follows selected theme', async () => {
  const savedConfig = globalThis.OblethChatConfig;
  const savedObserver = globalThis.MutationObserver;
  let callback, observed;
  const element = () => ({
    attributes: {},
    setAttribute(name, value) { this.attributes[name] = value; },
    removeAttribute(name) { delete this.attributes[name]; },
  });
  const description = element(), favicon = element();
  favicon.attributes = { type: 'image/png', sizes: '32x32' };
  const doc = {
    documentElement: { dataset: { theme: 'light' } },
    querySelector(selector) { return selector.startsWith('meta') ? description : favicon; },
  };
  globalThis.OblethChatConfig = { siteName: 'Acme Chat', description: 'A place to think.', favicon: { light: './light.svg', dark: './dark.svg' } };
  globalThis.MutationObserver = class {
    constructor(fn) { callback = fn; }
    observe(node, options) { observed = { node, options }; }
    disconnect() {}
  };
  try {
    const { applyBranding } = await import('../web/branding.js?metadata-test');
    applyBranding(doc);
    assert.equal(doc.title, 'Acme Chat');
    assert.equal(description.attributes.content, 'A place to think.');
    assert.equal(favicon.attributes.href, './light.svg');
    assert.equal(favicon.attributes.type, undefined);
    assert.equal(favicon.attributes.sizes, undefined);
    assert.equal(observed.node, doc.documentElement);
    assert.deepEqual(observed.options.attributeFilter, ['data-theme']);
    doc.documentElement.dataset.theme = 'dark';
    callback();
    assert.equal(favicon.attributes.href, './dark.svg');
  } finally {
    if (savedConfig === undefined) delete globalThis.OblethChatConfig; else globalThis.OblethChatConfig = savedConfig;
    if (savedObserver === undefined) delete globalThis.MutationObserver; else globalThis.MutationObserver = savedObserver;
  }
});

test('footer links accept web, mail and same-site addresses only', () => {
  const config = normalizeBranding({ siteName: 'Acme', links: [
    { label: 'Help', url: 'https://help.example.com' },
    { label: 'Email', url: 'mailto:help@example.com' },
    { label: 'Terms', url: './terms.html' },
    { label: 'Bad', url: 'javascript:alert(1)' },
    { label: '', url: 'https://example.com' },
    'nope',
  ] });
  assert.deepEqual(config.links.map(link => link.label), ['Help', 'Email', 'Terms']);
  const html = siteFooter(config, 2026);
  assert.match(html, /© 2026 Acme/);
  assert.match(html, /href="https:\/\/help\.example\.com" target="_blank" rel="noopener noreferrer"/);
  assert.match(html, /href="\.\/terms\.html">Terms/);
  assert.doesNotMatch(html, /javascript:/);
  assert.doesNotMatch(siteFooter(normalizeBranding({ links: [] })), /<nav/);
});

test('sign-in copy keeps a usable button label and only a safe help link', () => {
  const config = normalizeBranding({ login: { ssoLabel: '  ', helpLink: { label: ' Request an account ', url: 'https://accounts.example.edu/new' } } });
  assert.equal(config.login.ssoLabel, 'Continue with single sign-on');
  assert.deepEqual(config.login.helpLink, { label: 'Request an account', url: 'https://accounts.example.edu/new' });
  for (const helpLink of [{ label: 'Bad', url: 'javascript:alert(1)' }, { label: '', url: 'https://example.edu' }, { url: 'https://example.edu' }, 'https://example.edu', null]) {
    assert.equal(normalizeBranding({ login: { helpLink } }).login.helpLink, null);
  }
});

test('the banner is hidden until it has text, reads as one line and keeps only a safe link', () => {
  assert.deepEqual(normalizeBranding().banner, { text: '', link: null });
  assert.equal(siteBanner(normalizeBranding()), '');
  assert.equal(siteBanner(normalizeBranding({ banner: { text: '  \n ' } })), '');
  const config = normalizeBranding({ banner: { text: ' In development:\n  features <may> change. ', link: { label: ' Send feedback ', url: 'https://example.edu/feedback' } } });
  assert.deepEqual(config.banner, { text: 'In development: features <may> change.', link: { label: 'Send feedback', url: 'https://example.edu/feedback' } });
  const html = siteBanner(config);
  assert.match(html, /^<div class="site-banner"><svg/);
  assert.match(html, /<span class="site-banner-text" title="In development: features &lt;may&gt; change\.">In development: features &lt;may&gt; change\.<\/span>/);
  assert.match(html, /<a href="https:\/\/example\.edu\/feedback" target="_blank" rel="noopener noreferrer">Send feedback<\/a><\/div>$/);
  for (const link of [{ label: 'Bad', url: 'javascript:alert(1)' }, { label: '', url: 'https://example.edu' }, 'https://example.edu']) {
    const banner = normalizeBranding({ banner: { text: 'In development', link } }).banner;
    assert.equal(banner.link, null);
    assert.doesNotMatch(siteBanner({ banner }), /<a /);
  }
});

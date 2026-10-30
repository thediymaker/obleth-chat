import test from 'node:test';
import assert from 'node:assert/strict';
import { siteBanner, closeBanner } from '../web/banner.js';

const memory = () => { const saved = new Map(); return { getItem: key => saved.get(key) ?? null, setItem: (key, value) => saved.set(key, String(value)) }; };

test('no banner, or one without text, shows nothing', () => {
  assert.equal(siteBanner(null, memory()), '');
  assert.equal(siteBanner({ text: '', tone: 'info' }, memory()), '');
});

test('a banner escapes its text, keeps its tone and opens its link in a new tab', () => {
  const html = siteBanner({ text: 'In development: <b>features</b> may change.', link: { label: 'Send feedback', url: 'https://example.edu/feedback' }, tone: 'warning', dismissible: false }, memory());
  assert.match(html, /^<div class="site-banner warning"><svg/);
  assert.match(html, /<span class="site-banner-text" title="In development: &lt;b&gt;features&lt;\/b&gt; may change\.">In development: &lt;b&gt;features&lt;\/b&gt; may change\.<\/span>/);
  assert.match(html, /<a href="https:\/\/example\.edu\/feedback" target="_blank" rel="noopener noreferrer">Send feedback<\/a><\/div>$/);
  assert.doesNotMatch(html, /close-banner/);
  assert.match(siteBanner({ text: 'Maintenance Friday', tone: 'anything' }, memory()), /class="site-banner info"/);
});

test('a banner people may close stays closed until its text or link changes', () => {
  const storage = memory();
  const banner = { text: 'Maintenance Friday', link: null, tone: 'info', dismissible: true };
  assert.match(siteBanner(banner, storage), /<button class="site-banner-close" type="button" data-action="close-banner" aria-label="Close the banner">/);
  closeBanner(banner, storage);
  assert.equal(siteBanner(banner, storage), '');
  assert.match(siteBanner({ ...banner, dismissible: false }, storage), /Maintenance Friday/);
  assert.match(siteBanner({ ...banner, text: 'Maintenance Saturday' }, storage), /Maintenance Saturday/);
  assert.match(siteBanner({ ...banner, link: { label: 'Details', url: 'https://status.example.edu' } }, storage), /Details/);
  // Without browser storage it stays closed for this page.
  const blocked = { getItem() { throw new Error('denied'); }, setItem() { throw new Error('denied'); } };
  const other = { ...banner, text: 'Another notice' };
  closeBanner(other, blocked);
  assert.equal(siteBanner(other, blocked), '');
});

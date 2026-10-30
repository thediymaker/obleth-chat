import { esc, icon } from './manage-ui.js';

// The banner an administrator sets in Administration › System, such as an announcement
// or a note that the service is in development. It comes with the session. One that
// people may close stays closed in that browser until its text or link changes.

const storageKey = 'obleth.banner-closed';
let closedHere = '';
const signature = banner => `${banner.text}\n${banner.link?.url || ''}`;
const external = url => /^(https?:|mailto:)/i.test(url) ? ' target="_blank" rel="noopener noreferrer"' : '';

function closed(banner, storage) {
  if (closedHere === signature(banner)) return true;
  try { return storage?.getItem(storageKey) === signature(banner); } catch { return false; }
}

export function closeBanner(banner, storage = globalThis.localStorage) {
  if (!banner) return;
  closedHere = signature(banner);
  try { storage?.setItem(storageKey, closedHere); } catch { /* Closed for this page only. */ }
}

/** The banner, or nothing when none is set or this person closed it: a tag in the top bar, a strip above the sign-in page. */
export function siteBanner(banner, storage = globalThis.localStorage) {
  if (!banner?.text || (banner.dismissible && closed(banner, storage))) return '';
  const link = banner.link ? `<a href="${esc(banner.link.url)}"${external(banner.link.url)}>${esc(banner.link.label)}</a>` : '';
  const close = banner.dismissible ? `<button class="site-banner-close" type="button" data-action="close-banner" aria-label="Close the banner">${icon('x')}</button>` : '';
  return `<div class="site-banner ${banner.tone === 'warning' ? 'warning' : 'info'}">${icon('info')}<span class="site-banner-text" title="${esc(banner.text)}">${esc(banner.text)}</span>${link}${close}</div>`;
}

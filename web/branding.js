import { icon, iconNames } from './icons.js';

const defaults = {
  siteName: 'Obleth Chat',
  siteSubtitle: '',
  assistantName: 'Assistant',
  description: 'A space to ask questions, explore ideas, and make something new.',
  logo: { light: './assets/obleth-ink.svg', dark: './assets/obleth.svg', alt: '' },
  favicon: { light: './assets/obleth-ink.svg', dark: './assets/obleth.svg' },
  icons: { greeting: 'chat', assistant: 'chat', loading: 'chat' },
  login: {
    title: '',
    description: '',
    ssoLabel: 'Continue with single sign-on',
    ssoHint: '',
    help: 'Need an account? Your administrator can create one for you.',
  },
  links: [
    { label: 'Documentation', url: 'https://www.obleth.com/docs/chat/introduction' },
    { label: 'Obleth', url: 'https://www.obleth.com' },
  ],
  home: {
    eyebrow: 'Your space to explore',
    title: 'What’s on your mind?',
    description: 'Ask a question, explore an idea, or start something new.',
    footer: '',
  },
  appearance: { defaultTheme: 'system' },
  banner: { text: '', link: null },
};
const supportedIcons = new Set(iconNames);
const plainObject = value => value && typeof value === 'object' && !Array.isArray(value) ? value : {};
const text = (value, fallback) => typeof value === 'string' ? value : fallback;
const esc = (value = '') => String(value).replace(/[&<>"']/g, character => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[character]));

// Keep assets on this site, matching the app's Content Security Policy.
function assetPath(value) {
  if (typeof value !== 'string') return '';
  const path = value.trim();
  if (!path || /[\u0000-\u0020\\]/.test(path) || path.startsWith('//') || /^[a-z][a-z\d+.-]*:/i.test(path) || /^[?#]/.test(path)) return '';
  try {
    const base = 'https://branding.invalid/';
    const resolved = new URL(path, base);
    return resolved.origin === new URL(base).origin ? path : '';
  } catch { return ''; }
}

function assets(value, fallback = '') {
  const source = plainObject(value);
  const light = assetPath(source.light), dark = assetPath(source.dark);
  return { light: light || dark || fallback, dark: dark || light || fallback };
}

// Footer links may leave the site, but only to web or mail addresses.
function linkUrl(value) {
  if (typeof value !== 'string') return '';
  const url = value.trim();
  if (/^(https?:\/\/|mailto:)/i.test(url)) return url;
  return assetPath(url);
}

function links(value) {
  if (!Array.isArray(value)) return defaults.links;
  return Array.from(value).slice(0, 8).map(item => ({ label: text(plainObject(item).label, '').trim(), url: linkUrl(plainObject(item).url) })).filter(item => item.label && item.url);
}

function copyFields(source, fallback) {
  const input = plainObject(source);
  return Object.fromEntries(Object.entries(fallback).map(([key, value]) => [key, text(input[key], value)]));
}

// The help line may end in one link, e.g. to where people request an account.
function loginCopy(value) {
  const input = plainObject(value);
  const login = copyFields(input, defaults.login);
  const link = plainObject(input.helpLink);
  const label = text(link.label, '').trim(), url = linkUrl(link.url);
  return { ...login, ssoLabel: login.ssoLabel.trim() || defaults.login.ssoLabel, helpLink: label && url ? { label, url } : null };
}

// The banner is one line of text that may end in one link. Blank text hides it.
function bannerCopy(value) {
  const input = plainObject(value);
  const link = plainObject(input.link);
  const label = text(link.label, '').trim(), url = linkUrl(link.url);
  return { text: text(input.text, '').replace(/\s+/g, ' ').trim(), link: label && url ? { label, url } : null };
}

export function normalizeBranding(value) {
  const input = plainObject(value);
  const configuredIcons = plainObject(input.icons);
  const defaultTheme = plainObject(input.appearance).defaultTheme;
  // A line break stacks the name beside the logo; everywhere else it reads as one line.
  const nameLines = text(input.siteName, defaults.siteName).split(/\r\n?|\n/).map(line => line.trim()).filter(Boolean);
  const siteNameLines = nameLines.length ? nameLines : [defaults.siteName];
  return {
    siteName: siteNameLines.join(' '),
    siteNameLines,
    siteSubtitle: text(input.siteSubtitle, defaults.siteSubtitle),
    assistantName: text(input.assistantName, defaults.assistantName).trim() || defaults.assistantName,
    description: text(input.description, defaults.description),
    // A missing logo keeps the Obleth mark; blank paths give a text-only brand.
    logo: input.logo === undefined ? defaults.logo : { ...assets(input.logo), alt: text(plainObject(input.logo).alt, '') },
    favicon: input.favicon === undefined ? defaults.favicon : assets(input.favicon, './assets/chat.svg'),
    icons: Object.fromEntries(Object.entries(defaults.icons).map(([role, fallback]) => {
      const name = configuredIcons[role];
      return [role, name === '' || supportedIcons.has(name) ? name : fallback];
    })),
    login: loginCopy(input.login),
    links: links(input.links),
    home: copyFields(input.home, defaults.home),
    appearance: { defaultTheme: ['system', 'light', 'dark'].includes(defaultTheme) ? defaultTheme : defaults.appearance.defaultTheme },
    banner: bannerCopy(input.banner),
  };
}

export const branding = normalizeBranding(globalThis.OblethChatConfig);

/** Escape configured plain text, preserving intentional line breaks. */
export const copy = value => esc(value ?? '').replace(/\r\n?|\n/g, '<br>');

export function brandLogo(config = branding) {
  if (!config.logo.light && !config.logo.dark) return '';
  return `<span class="brand-logo"><img class="brand-logo-light" src="${esc(config.logo.light)}" alt="${esc(config.logo.alt)}"><img class="brand-logo-dark" src="${esc(config.logo.dark)}" alt="${esc(config.logo.alt)}"></span>`;
}

export function brand(config = branding) {
  return `<a class="brand" href="#chat" aria-label="${esc(config.siteName)} home">${brandLogo(config)}<div class="brand-name"><div class="brand-title">${config.siteNameLines.map(esc).join('<br>')}</div>${config.siteSubtitle ? `<span>${esc(config.siteSubtitle)}</span>` : ''}</div></a>`;
}

export function siteFooter(config = branding, year = new Date().getFullYear()) {
  const items = config.links.map(link => `<a href="${esc(link.url)}"${/^(https?:|mailto:)/i.test(link.url) ? ' target="_blank" rel="noopener noreferrer"' : ''}>${esc(link.label)}</a>`);
  return `<footer class="site-footer"><span>© ${year} ${esc(config.siteName)}</span>${items.length ? `<nav aria-label="Site links">${items.join('')}</nav>` : ''}</footer>`;
}

/** The configured banner, or nothing when it has no text: a tag in the top bar, a strip above the sign-in page. */
export function siteBanner(config = branding) {
  const { text: message, link } = config.banner;
  if (!message) return '';
  const linkHtml = link ? `<a href="${esc(link.url)}"${/^(https?:|mailto:)/i.test(link.url) ? ' target="_blank" rel="noopener noreferrer"' : ''}>${esc(link.label)}</a>` : '';
  return `<div class="site-banner">${icon('info')}<span class="site-banner-text" title="${esc(message)}">${esc(message)}</span>${linkHtml}</div>`;
}

export function brandIcon(role, config = branding) {
  const name = config.icons[role];
  return name && supportedIcons.has(name) ? icon(name) : '';
}

export const pageTitle = (label = '', config = branding) => label ? `${label} · ${config.siteName}` : config.siteName;

let brandingObserver;
export function applyBranding(doc = globalThis.document) {
  if (!doc) return;
  doc.title = pageTitle();
  let description = doc.querySelector('meta[name="description"]');
  if (!description) {
    description = doc.createElement('meta');
    description.setAttribute('name', 'description');
    doc.head.append(description);
  }
  description.setAttribute('content', branding.description);
  let favicon = doc.querySelector('link[rel="icon"]');
  if (!favicon) {
    favicon = doc.createElement('link');
    favicon.setAttribute('rel', 'icon');
    doc.head.append(favicon);
  }
  // Custom icons can be SVG, PNG, or ICO: do not retain a previous type/size hint.
  favicon.removeAttribute('type');
  favicon.removeAttribute('sizes');
  const updateFavicon = () => {
    const dark = doc.documentElement.dataset.theme === 'dark';
    favicon.setAttribute('href', dark ? branding.favicon.dark : branding.favicon.light);
  };
  updateFavicon();
  brandingObserver?.disconnect();
  if (typeof globalThis.MutationObserver === 'function') {
    brandingObserver = new globalThis.MutationObserver(updateFavicon);
    brandingObserver.observe(doc.documentElement, { attributes: true, attributeFilter: ['data-theme'] });
  }
}

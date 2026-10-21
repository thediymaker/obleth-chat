import { icon, iconNames } from './icons.js';

const defaults = {
  siteName: 'Obleth Chat',
  siteSubtitle: '',
  assistantName: 'Assistant',
  description: 'A space to ask questions, explore ideas, and make something new.',
  logo: { light: '', dark: '', alt: '' },
  favicon: { light: '', dark: '' },
  icons: { greeting: 'chat', assistant: 'chat', loading: 'chat' },
  login: {
    eyebrow: 'A space for every idea',
    title: 'A little space\nfor big ideas.',
    description: 'Ask questions, explore ideas, and make something new.\nYour conversations, all in one place.',
    footer: '',
    welcomeTitle: 'Make yourself at home.',
    welcomeDescription: 'Sign in to your personal workspace.',
    help: 'Need an account? Your administrator can create one for you.',
  },
  home: {
    eyebrow: 'Your space to explore',
    title: 'What’s on your mind?',
    description: 'Ask a question, explore an idea, or start something new.',
    footer: '',
  },
  appearance: { defaultTheme: 'system' },
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

function copyFields(source, fallback) {
  const input = plainObject(source);
  return Object.fromEntries(Object.entries(fallback).map(([key, value]) => [key, text(input[key], value)]));
}

export function normalizeBranding(value) {
  const input = plainObject(value);
  const configuredIcons = plainObject(input.icons);
  const defaultTheme = plainObject(input.appearance).defaultTheme;
  return {
    siteName: text(input.siteName, defaults.siteName).trim() || defaults.siteName,
    siteSubtitle: text(input.siteSubtitle, defaults.siteSubtitle),
    assistantName: text(input.assistantName, defaults.assistantName).trim() || defaults.assistantName,
    description: text(input.description, defaults.description),
    logo: { ...assets(input.logo), alt: text(plainObject(input.logo).alt, '') },
    favicon: assets(input.favicon, './assets/chat.svg'),
    icons: Object.fromEntries(Object.entries(defaults.icons).map(([role, fallback]) => {
      const name = configuredIcons[role];
      return [role, name === '' || supportedIcons.has(name) ? name : fallback];
    })),
    login: copyFields(input.login, defaults.login),
    home: copyFields(input.home, defaults.home),
    appearance: { defaultTheme: ['system', 'light', 'dark'].includes(defaultTheme) ? defaultTheme : defaults.appearance.defaultTheme },
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
  return `<a class="brand" href="#chat" aria-label="${esc(config.siteName)} home">${brandLogo(config)}<div class="brand-name">${esc(config.siteName)}${config.siteSubtitle ? `<span>${esc(config.siteSubtitle)}</span>` : ''}</div></a>`;
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

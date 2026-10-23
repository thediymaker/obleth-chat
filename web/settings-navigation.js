import { icon } from './icons.js';

export function canonicalSettingsHash(hash) {
  if (/^#admin(?:\/|$)/.test(hash)) return hash.replace(/^#admin/, '#settings/admin');
  // The Infrastructure page is gone; old links land on Settings.
  if (hash === '#system' || hash === '#settings/infrastructure') return '#settings';
  return hash;
}

export function adminLocation(hash) {
  const parts = canonicalSettingsHash(hash).split('/');
  return { section: parts[2] || 'users', groupId: parts[3] || '' };
}

export function settingsTabs(section, admin = false, connections = false) {
  const tabs = [['personal', 'settings', 'Personal', '#settings']];
  if (connections) tabs.push(['connections', 'link', 'MCP servers', '#settings/mcp']);
  if (admin) tabs.push(['admin', 'shield', 'Administration', '#settings/admin/users']);
  return `<nav class="settings-tabs" aria-label="Settings sections">${tabs.map(([id, glyph, label, path]) => `<a class="settings-tab ${section === id ? 'active' : ''}" href="${path}" ${section === id ? 'aria-current="page"' : ''}>${icon(glyph)}<span>${label}</span></a>`).join('')}</nav>`;
}

import { icon } from './manage-ui.js';

// A temporary chat stays out of Recent, All chats and search, can't be shared, exported,
// moved into a project, branched or saved to the Library, and is deleted when the person
// leaves it. The server also deletes one that has been quiet for a day.

/** The new-chat toggle, next to the model picker. */
export const temporaryToggle = on => `<button type="button" class="temporary-toggle ${on ? 'active' : ''}" data-action="toggle-temporary" aria-pressed="${on}" title="${on ? 'Turn off to keep this chat' : 'A chat that isn’t kept'}">${icon('clock')}<span>Temporary chat</span></button>`;

/** The marker shown while a chat is, or is about to be, temporary. */
export const temporaryNote = (started = true) => `<p class="temporary-note" role="note">${icon('clock')}<span><strong>Temporary chat.</strong> ${started ? 'It isn’t in your chats or search, and it’s deleted when you leave it.' : 'It won’t be in your chats or search, and it’s deleted when you leave it.'}</span></p>`;

/** The conversation menu's items; a temporary chat can only be renamed or deleted. */
export function chatMenuItems(c) {
  const all = [['edit', 'Rename', 'rename-chat'], ['pin', c?.pinned ? 'Unpin' : 'Pin', 'pin-chat'], ['archive', c?.archived ? 'Restore' : 'Archive', 'archive-chat'], ['link', 'Share…', 'share-chat'], ['download', 'Export JSON', 'export-chat'], ['trash', 'Delete', 'delete-chat']];
  return c?.temporary ? all.filter(([, , action]) => ['rename-chat', 'delete-chat'].includes(action)) : all;
}

/** The temporary chat to delete now that the page shows `route`, if the person left it. */
export const leftTemporary = (open, route) => open && !(route.page === 'chat' && route.id === open) ? open : null;

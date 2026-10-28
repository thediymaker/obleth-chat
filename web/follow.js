// How close to the end of the page still counts as at the end.
const END_PX = 24;

/**
 * Whether a reply being written keeps the page at its end. Scrolling up, by wheel,
 * touch, keyboard or the scrollbar, stops following at once, so the person can read
 * back while the reply grows; scrolling back down to the end follows again. Chat's
 * own scrolls, and the page shrinking as it is drawn again, never stop it.
 */
export function createFollow(win = window) {
  const doc = win.document;
  let following = true, lastY = win.scrollY, touchY = null;
  const atEnd = () => win.innerHeight + win.scrollY >= doc.documentElement.scrollHeight - END_PX;
  // A wheel or key over a box that scrolls on its own, such as the thinking panel,
  // moves that box and not the page.
  const scrollsOwnBox = el => {
    for (; el?.parentElement && el !== doc.documentElement && el !== doc.body; el = el.parentElement) {
      if (el.scrollHeight <= el.clientHeight) continue;
      const style = win.getComputedStyle(el);
      if (/(auto|scroll)/.test(style.overflowY) && (el.scrollTop > 0 || /contain|none/.test(style.overscrollBehaviorY))) return true;
    }
    return false;
  };
  const leave = () => { following = false; };
  win.addEventListener('wheel', e => { if (e.deltaY < 0 && !scrollsOwnBox(e.target)) leave(); }, { passive: true });
  win.addEventListener('touchstart', e => { touchY = e.touches?.[0]?.clientY ?? null; }, { passive: true });
  win.addEventListener('touchmove', e => {
    const y = e.touches?.[0]?.clientY ?? null;
    if (touchY !== null && y !== null && y > touchY + 4 && !scrollsOwnBox(e.target)) leave();
    touchY = y;
  }, { passive: true });
  win.addEventListener('keydown', e => {
    if (!['ArrowUp', 'PageUp', 'Home'].includes(e.key) && !(e.key === ' ' && e.shiftKey)) return;
    if (e.target?.closest?.('input, textarea, select, [contenteditable]') || scrollsOwnBox(e.target)) return;
    leave();
  });
  // Pressing on the page's scrollbar, which sits past the document's width.
  win.addEventListener('mousedown', e => { if (e.clientX >= doc.documentElement.clientWidth) leave(); });
  win.addEventListener('scroll', () => {
    const y = win.scrollY;
    if (y > lastY && atEnd()) following = true;
    lastY = y;
  }, { passive: true });
  return {
    /** Whether new words should keep the page at its end. */
    on: () => following,
    /** Follows again: opening a chat, or sending a message, shows its end. */
    resume() { following = true; },
  };
}

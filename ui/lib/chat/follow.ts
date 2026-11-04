// How close to the end of the page still counts as at the end.
const END_PX = 24;

export interface FollowScroller extends EventTarget {
  scrollTop: number;
  scrollHeight: number;
  clientHeight: number;
  clientWidth: number;
  getBoundingClientRect?: () => { right: number; left: number };
}
interface FollowWindow extends EventTarget {
  getComputedStyle: (el: any) => { overflowY: string; overscrollBehaviorY: string };
}

/**
 * Whether a reply being written keeps the conversation at its end. Scrolling up, by
 * wheel, touch, keyboard or the scrollbar, stops following at once, so the person can
 * read back while the reply grows; scrolling back down to the end follows again.
 * Chat's own scrolls, and the page shrinking as it is drawn again, never stop it.
 * `scroller` is the element that scrolls (the app's main area); keys are heard on
 * `win`, since the focus may be anywhere.
 */
export function createFollow(scroller: FollowScroller, win: FollowWindow = window as unknown as FollowWindow) {
  let following = true,
    lastY = scroller.scrollTop,
    touchY: number | null = null;
  const atEnd = () => scroller.clientHeight + scroller.scrollTop >= scroller.scrollHeight - END_PX;
  // A wheel or key over a box that scrolls on its own, such as the thinking panel,
  // moves that box and not the page.
  const scrollsOwnBox = (el: any) => {
    for (; el?.parentElement && el !== scroller; el = el.parentElement) {
      if (el.scrollHeight <= el.clientHeight) continue;
      const style = win.getComputedStyle(el);
      if (/(auto|scroll)/.test(style.overflowY) && (el.scrollTop > 0 || /contain|none/.test(style.overscrollBehaviorY))) return true;
    }
    return false;
  };
  const leave = () => {
    following = false;
  };
  const onWheel = (e: any) => {
    if (e.deltaY < 0 && !scrollsOwnBox(e.target)) leave();
  };
  const onTouchStart = (e: any) => {
    touchY = e.touches?.[0]?.clientY ?? null;
  };
  const onTouchMove = (e: any) => {
    const y = e.touches?.[0]?.clientY ?? null;
    if (touchY !== null && y !== null && y > touchY + 4 && !scrollsOwnBox(e.target)) leave();
    touchY = y;
  };
  const onKey = (e: any) => {
    if (!["ArrowUp", "PageUp", "Home"].includes(e.key) && !(e.key === " " && e.shiftKey)) return;
    if (e.target?.closest?.("input, textarea, select, [contenteditable]") || scrollsOwnBox(e.target)) return;
    leave();
  };
  // Pressing on the scrollbar, which sits past the content's width.
  const onMouseDown = (e: any) => {
    const left = scroller.getBoundingClientRect?.().left ?? 0;
    if (e.clientX >= left + scroller.clientWidth) leave();
  };
  const onScroll = () => {
    const y = scroller.scrollTop;
    if (y > lastY && atEnd()) following = true;
    lastY = y;
  };
  scroller.addEventListener("wheel", onWheel, { passive: true });
  scroller.addEventListener("touchstart", onTouchStart, { passive: true });
  scroller.addEventListener("touchmove", onTouchMove, { passive: true });
  scroller.addEventListener("mousedown", onMouseDown);
  scroller.addEventListener("scroll", onScroll, { passive: true });
  win.addEventListener("keydown", onKey);
  return {
    /** Whether new words should keep the page at its end. */
    on: () => following,
    /** Follows again: opening a chat, or sending a message, shows its end. */
    resume() {
      following = true;
    },
    stop() {
      scroller.removeEventListener("wheel", onWheel);
      scroller.removeEventListener("touchstart", onTouchStart);
      scroller.removeEventListener("touchmove", onTouchMove);
      scroller.removeEventListener("mousedown", onMouseDown);
      scroller.removeEventListener("scroll", onScroll);
      win.removeEventListener("keydown", onKey);
    },
  };
}
export type Follow = ReturnType<typeof createFollow>;

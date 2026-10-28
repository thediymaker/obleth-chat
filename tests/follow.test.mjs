import test from 'node:test';
import assert from 'node:assert/strict';
import { createFollow } from '../web/follow.js';

// A page 3000px tall in a 1000px window, scrolled to its end.
function page() {
  const win = new EventTarget();
  Object.assign(win, {
    innerHeight: 1000, scrollY: 2000,
    document: { documentElement: { scrollHeight: 3000, clientWidth: 1200 }, body: {} },
    getComputedStyle: () => ({ overflowY: 'visible', overscrollBehaviorY: 'auto' }),
  });
  const fire = (type, props = {}) => win.dispatchEvent(Object.assign(new Event(type), props));
  const scroll = y => { win.scrollY = y; fire('scroll'); };
  return { win, fire, scroll };
}

test('scrolling up a little stops following while the reply grows', () => {
  const { win, fire, scroll } = page();
  const follow = createFollow(win);
  assert.equal(follow.on(), true);
  fire('wheel', { deltaY: -40 });
  scroll(1990);
  assert.equal(follow.on(), false);
  // The reply grows; the page stays where the person put it.
  win.document.documentElement.scrollHeight = 3400;
  assert.equal(follow.on(), false);
});

test('scrolling back down to the end follows again', () => {
  const { win, fire, scroll } = page();
  const follow = createFollow(win);
  fire('wheel', { deltaY: -100 });
  scroll(1900);
  scroll(1950);
  assert.equal(follow.on(), false);
  scroll(1990);
  assert.equal(follow.on(), true);
});

test('the page shrinking as it is drawn again does not stop following', () => {
  const { win, scroll } = page();
  const follow = createFollow(win);
  win.document.documentElement.scrollHeight = 2500;
  scroll(1500);
  assert.equal(follow.on(), true);
});

test('keys, touch and the scrollbar stop following; keys in the message box do not', () => {
  const key = (k, target) => {
    const e = Object.assign(new Event('keydown'), { key: k });
    if (target) Object.defineProperty(e, 'target', { value: target });
    return e;
  };
  let { win, fire } = page(), follow = createFollow(win);
  win.dispatchEvent(key('PageUp'));
  assert.equal(follow.on(), false);

  ({ win } = page()); follow = createFollow(win);
  const box = { closest: selector => (selector.includes('textarea') ? box : null) };
  win.dispatchEvent(key('ArrowUp', box));
  assert.equal(follow.on(), true);

  ({ win, fire } = page()); follow = createFollow(win);
  fire('touchstart', { touches: [{ clientY: 300 }] });
  fire('touchmove', { touches: [{ clientY: 360 }] });
  assert.equal(follow.on(), false);

  ({ win, fire } = page()); follow = createFollow(win);
  fire('mousedown', { clientX: 400 });
  assert.equal(follow.on(), true);
  fire('mousedown', { clientX: 1205 });
  assert.equal(follow.on(), false);
  follow.resume();
  assert.equal(follow.on(), true);
});

test('scrolling inside the thinking panel leaves the page following', () => {
  const { win, fire } = page();
  win.getComputedStyle = () => ({ overflowY: 'auto', overscrollBehaviorY: 'contain' });
  const panel = { scrollHeight: 900, clientHeight: 300, scrollTop: 0, parentElement: win.document.body };
  const follow = createFollow(win);
  const wheel = Object.assign(new Event('wheel'), { deltaY: -100 });
  Object.defineProperty(wheel, 'target', { value: panel });
  win.dispatchEvent(wheel);
  assert.equal(follow.on(), true);
  fire('wheel', { deltaY: -100 });
  assert.equal(follow.on(), false);
});

import { expect, test } from "vitest";
import { createFollow } from "./follow";

// A conversation 3000px tall in a 1000px pane, scrolled to its end.
function page() {
  const scroller = Object.assign(new EventTarget(), {
    clientHeight: 1000, scrollTop: 2000, scrollHeight: 3000, clientWidth: 1200,
    getBoundingClientRect: () => ({ left: 0, right: 1210 }),
  });
  const win = Object.assign(new EventTarget(), { getComputedStyle: () => ({ overflowY: "visible", overscrollBehaviorY: "auto" }) });
  const fire = (type: string, props = {}) => scroller.dispatchEvent(Object.assign(new Event(type), props));
  const scroll = (y: number) => {
    scroller.scrollTop = y;
    fire("scroll");
  };
  return { scroller, win, fire, scroll };
}

test("scrolling up a little stops following while the reply grows", () => {
  const { scroller, win, fire, scroll } = page();
  const follow = createFollow(scroller, win);
  expect(follow.on()).toBe(true);
  fire("wheel", { deltaY: -40 });
  scroll(1990);
  expect(follow.on()).toBe(false);
  scroller.scrollHeight = 3400;
  expect(follow.on()).toBe(false);
});

test("scrolling back down to the end follows again", () => {
  const { scroller, win, fire, scroll } = page();
  const follow = createFollow(scroller, win);
  fire("wheel", { deltaY: -100 });
  scroll(1900);
  scroll(1950);
  expect(follow.on()).toBe(false);
  scroll(1990);
  expect(follow.on()).toBe(true);
});

test("the page shrinking as it is drawn again does not stop following", () => {
  const { scroller, win, scroll } = page();
  const follow = createFollow(scroller, win);
  scroller.scrollHeight = 2500;
  scroll(1500);
  expect(follow.on()).toBe(true);
});

test("keys, touch and the scrollbar stop following; keys in the message box do not", () => {
  const key = (k: string, target?: unknown) => {
    const e = Object.assign(new Event("keydown"), { key: k });
    if (target) Object.defineProperty(e, "target", { value: target });
    return e;
  };
  let { scroller, win, fire } = page();
  let follow = createFollow(scroller, win);
  win.dispatchEvent(key("PageUp"));
  expect(follow.on()).toBe(false);

  ({ scroller, win } = page());
  follow = createFollow(scroller, win);
  const box: any = { closest: (selector: string) => (selector.includes("textarea") ? box : null) };
  win.dispatchEvent(key("ArrowUp", box));
  expect(follow.on()).toBe(true);

  ({ scroller, win, fire } = page());
  follow = createFollow(scroller, win);
  fire("touchstart", { touches: [{ clientY: 300 }] });
  fire("touchmove", { touches: [{ clientY: 360 }] });
  expect(follow.on()).toBe(false);

  ({ scroller, win, fire } = page());
  follow = createFollow(scroller, win);
  fire("mousedown", { clientX: 400 });
  expect(follow.on()).toBe(true);
  fire("mousedown", { clientX: 1205 });
  expect(follow.on()).toBe(false);
  follow.resume();
  expect(follow.on()).toBe(true);
});

test("scrolling inside the thinking panel leaves the page following", () => {
  const { scroller, win, fire } = page();
  win.getComputedStyle = () => ({ overflowY: "auto", overscrollBehaviorY: "contain" });
  const panel = { scrollHeight: 900, clientHeight: 300, scrollTop: 0, parentElement: scroller };
  const follow = createFollow(scroller, win);
  const wheel = Object.assign(new Event("wheel"), { deltaY: -100 });
  Object.defineProperty(wheel, "target", { value: panel });
  scroller.dispatchEvent(wheel);
  expect(follow.on()).toBe(true);
  fire("wheel", { deltaY: -100 });
  expect(follow.on()).toBe(false);
});

import { describe, expect, it } from "vitest";
import { buttonOf, keyMessage, modifiers, pagePoint, viewport } from "./input";

const key = (k: Partial<KeyboardEvent>) => ({ key: "a", code: "KeyA", keyCode: 65, altKey: false, ctrlKey: false, metaKey: false, shiftKey: false, ...k });

describe("the live view's input", () => {
  it("maps a point on a letterboxed canvas to the page", () => {
    // A 1280x720 frame in a 640x480 box: scaled by half, 60px bands above and below.
    const rect = { left: 0, top: 0, width: 640, height: 480 };
    expect(pagePoint(rect, { width: 1280, height: 720 }, { w: 1280, h: 720 }, { clientX: 320, clientY: 240 })).toEqual({ x: 640, y: 360 });
    expect(pagePoint(rect, { width: 1280, height: 720 }, { w: 1280, h: 720 }, { clientX: 0, clientY: 60 })).toEqual({ x: 0, y: 0 });
  });
  it("keeps a desktop-width page in a narrow pane", () => {
    expect(viewport(400, 600)).toEqual({ width: 1024, height: 1536 });
    expect(viewport(2000, 1000)).toEqual({ width: 1280, height: 640 });
    expect(viewport(30, 600)).toBeNull();
  });
  it("sends keys with their text, and leaves paste to the paste event", () => {
    expect(keyMessage(key({}))).toMatchObject({ t: "key", type: "keyDown", text: "a", modifiers: 0 });
    expect(keyMessage(key({ key: "Enter", code: "Enter", keyCode: 13 }))).toMatchObject({ type: "keyDown", text: "\r" });
    expect(keyMessage(key({ key: "ArrowLeft" }))).toMatchObject({ type: "rawKeyDown", text: "" });
    expect(keyMessage(key({ key: "c", ctrlKey: true }))).toMatchObject({ type: "rawKeyDown", text: "", modifiers: 2 });
    expect(keyMessage(key({ key: "v", metaKey: true }))).toBeNull();
    expect(keyMessage(key({}), true)).toMatchObject({ type: "keyUp" });
    expect(modifiers({ altKey: true, ctrlKey: true, metaKey: true, shiftKey: true })).toBe(15);
    expect([0, 1, 2, 3].map((button) => buttonOf({ button }))).toEqual(["left", "middle", "right", "none"]);
  });
});

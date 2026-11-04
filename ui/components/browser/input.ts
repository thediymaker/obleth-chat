// The live view's messages (api/src/browser/live.rs): the page's size for the tab,
// and the person's mouse and keys while they have control.

export const modifiers = (e: { altKey: boolean; ctrlKey: boolean; metaKey: boolean; shiftKey: boolean }) => (e.altKey ? 1 : 0) | (e.ctrlKey ? 2 : 0) | (e.metaKey ? 4 : 0) | (e.shiftKey ? 8 : 0);
export const buttonOf = (e: { button: number }) => ["left", "middle", "right"][e.button] || "none";

/** Where a point on the canvas falls on the page. The frame is drawn contained in the
 * canvas box, so this allows for any letterboxing. */
export function pagePoint(
  rect: { left: number; top: number; width: number; height: number },
  canvas: { width: number; height: number },
  page: { w: number; h: number },
  client: { clientX: number; clientY: number },
) {
  const scale = Math.min(rect.width / canvas.width, rect.height / canvas.height);
  const left = rect.left + (rect.width - canvas.width * scale) / 2,
    top = rect.top + (rect.height - canvas.height * scale) / 2;
  return { x: ((client.clientX - left) / (canvas.width * scale)) * page.w, y: ((client.clientY - top) / (canvas.height * scale)) * page.h };
}

/** The tab takes the pane's shape, so the page fills it. Narrow panes keep a
 * desktop-width page, scaled down, rather than a phone layout. */
export function viewport(w: number, h: number) {
  if (w < 50 || h < 50) return null;
  const width = Math.round(Math.min(1280, Math.max(1024, w)));
  const height = Math.round(Math.min(2400, Math.max(240, (width * h) / w)));
  return { width, height };
}

/** A key press as the tab takes it; Ctrl/⌘+V is left to the paste event. */
export function keyMessage(e: { key: string; code: string; keyCode: number; altKey: boolean; ctrlKey: boolean; metaKey: boolean; shiftKey: boolean }, up = false) {
  if (up) return { t: "key", type: "keyUp", key: e.key, code: e.code, keyCode: e.keyCode, modifiers: modifiers(e) };
  if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "v") return null;
  const text = e.key === "Enter" ? "\r" : e.key.length === 1 && !e.ctrlKey && !e.metaKey ? e.key : "";
  return { t: "key", type: text ? "keyDown" : "rawKeyDown", key: e.key, code: e.code, keyCode: e.keyCode, text, modifiers: modifiers(e) };
}

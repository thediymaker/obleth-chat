// Point and say: Chat's preview frame can ask this page which element the person
// points at. It runs on the preview's own origin, added by Chat's preview proxy to
// pages opened in the frame. It does nothing until Chat's page (and only Chat's page)
// asks, tells Chat only about the one element the person clicks, and never changes
// the app.
(() => {
  if (window.parent === window || window.__oblethPick) return;
  window.__oblethPick = true;
  const CHAT = "__CHAT_ORIGIN__";
  let on = false,
    box = null,
    label = null,
    color = "#3b82f6";
  const say = (message) => window.parent.postMessage({ source: "obleth-preview", ...message }, CHAT);
  const clip = (text, most) => {
    const flat = String(text || "").replace(/\s+/g, " ").trim();
    return flat.length > most ? `${flat.slice(0, most - 1)}\u2026` : flat;
  };
  const safe = (name) => (window.CSS && CSS.escape ? CSS.escape(name) : name.replace(/[^\w-]/g, "\\$&"));
  // A selector that finds the element again: its id when it has one, otherwise the
  // tags from a nearby id (or the body) down to it, with its place among its kind.
  function selector(el) {
    const parts = [];
    for (let node = el; node && node.nodeType === 1 && node !== document.documentElement && parts.length < 6; node = node.parentElement) {
      if (node.id && !/\d{4,}/.test(node.id)) {
        parts.unshift(`#${safe(node.id)}`);
        break;
      }
      const tag = node.tagName.toLowerCase();
      if (tag === "body") {
        parts.unshift("body");
        break;
      }
      const same = node.parentElement ? [...node.parentElement.children].filter((c) => c.tagName === node.tagName) : [];
      const classes = [...node.classList].filter((c) => /^[a-zA-Z][\w-]{0,30}$/.test(c)).slice(0, 2);
      parts.unshift(`${tag}${classes.map((c) => `.${c}`).join("")}${same.length > 1 ? `:nth-of-type(${same.indexOf(node) + 1})` : ""}`);
    }
    return parts.join(" > ");
  }
  function describe(el) {
    const rect = el.getBoundingClientRect();
    return {
      tag: el.tagName.toLowerCase(),
      selector: clip(selector(el), 300),
      text: clip(el.getAttribute("aria-label") || el.getAttribute("alt") || el.getAttribute("placeholder") || el.innerText || el.value || "", 140),
      path: clip(location.pathname + location.search, 200),
      width: Math.round(rect.width),
      height: Math.round(rect.height),
    };
  }
  function draw(el) {
    if (!box) return;
    const rect = el.getBoundingClientRect();
    Object.assign(box.style, { left: `${rect.left}px`, top: `${rect.top}px`, width: `${rect.width}px`, height: `${rect.height}px`, display: "block" });
    label.textContent = `${el.tagName.toLowerCase()} \u00b7 ${Math.round(rect.width)} \u00d7 ${Math.round(rect.height)}`;
    label.style.top = rect.top > 22 ? "-20px" : "100%";
  }
  const target = (event) => {
    const el = event.target;
    return el && el.nodeType === 1 && el !== box && el !== document.documentElement ? el : null;
  };
  const move = (event) => {
    const el = target(event);
    if (el) draw(el);
  };
  const swallow = (event) => {
    event.preventDefault();
    event.stopPropagation();
    event.stopImmediatePropagation();
  };
  function click(event) {
    swallow(event);
    const el = target(event);
    stop();
    if (el) say({ type: "picked", element: describe(el) });
    else say({ type: "cancelled" });
  }
  function key(event) {
    if (event.key !== "Escape") return;
    swallow(event);
    stop();
    say({ type: "cancelled" });
  }
  const EVENTS = [
    ["mousemove", move],
    ["click", click],
    ["mousedown", swallow],
    ["mouseup", swallow],
    ["pointerdown", swallow],
    ["pointerup", swallow],
    ["keydown", key],
  ];
  function start() {
    if (on) return;
    on = true;
    box = document.createElement("div");
    box.setAttribute("aria-hidden", "true");
    Object.assign(box.style, { position: "fixed", zIndex: "2147483647", pointerEvents: "none", display: "none", boxSizing: "border-box", border: `2px solid ${color}`, background: `${color}22`, borderRadius: "3px", boxShadow: "0 0 0 1px rgba(0,0,0,.55)" });
    label = document.createElement("span");
    Object.assign(label.style, { position: "absolute", left: "-2px", padding: "1px 6px", background: color, color: "#fff", font: "11px/16px ui-monospace, SFMono-Regular, Menlo, monospace", whiteSpace: "nowrap", borderRadius: "3px" });
    box.appendChild(label);
    document.documentElement.appendChild(box);
    document.documentElement.style.cursor = "crosshair";
    for (const [name, handler] of EVENTS) window.addEventListener(name, handler, true);
  }
  function stop() {
    if (!on) return;
    on = false;
    for (const [name, handler] of EVENTS) window.removeEventListener(name, handler, true);
    document.documentElement.style.cursor = "";
    box?.remove();
    box = label = null;
  }
  window.addEventListener("message", (event) => {
    if (event.origin !== CHAT || event.source !== window.parent) return;
    const data = event.data;
    if (!data || data.source !== "obleth-chat") return;
    if (data.type === "pick-start") {
      if (typeof data.color === "string" && /^#[0-9a-f]{6}$/i.test(data.color)) color = data.color;
      start();
    } else if (data.type === "pick-stop") stop();
  });
  say({ type: "ready" });
})();

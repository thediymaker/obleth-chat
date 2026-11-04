// What the chat store asks of a chat's browser, and the state its tab and cards
// show. Chat's browser is one Chrome per person; each
// chat with the browser on has its own tab, which the model drives and the person
// can take over. The live view itself is components/browser/browser-panel.tsx.

import type { StreamEvent } from "@/lib/api";

/** A chat's tab (GET /api/conversations/:id/browser, and the live view's state messages). */
export interface BrowserChat {
  on: boolean;
  /** starting, running, stopped (asleep) or error. */
  status: string;
  /** Who drives the tab: the model, or the person after taking over. */
  controller: "model" | "user" | string;
  url: string;
  title: string;
  errorMessage: string;
  loaded: boolean;
  /** The Browser tab has been shown for this chat; the first browser step shows it. */
  shown: boolean;
}

/** A browser-request card's answer: being turned on, turned on, or Not now. */
export type Offer = "busy" | "done" | "dismissed";

export interface BrowserBridge {
  /** The browser is configured on this server (capabilities.features.browser). */
  enabled(): boolean;
  /** The model may browse in this chat. */
  on(id: string): boolean;
  load(id: string): Promise<void>;
  setOn(id: string, on: boolean): Promise<void>;
  observe(event: StreamEvent, id: string): void;
  reset(): void;

  // ---- For the Browser tab and the cards ----
  chat(id: string): BrowserChat;
  /** Takes over (user) or hands back (model). */
  setController(id: string, controller: "model" | "user"): Promise<void>;
  /** The live view's state messages. */
  update(id: string, values: Partial<BrowserChat>): void;
  /** Shows the Browser tab. */
  show(id: string): void;
  /** The Browser tab was hidden by its button. */
  dismiss(id: string): void;
  offer(key: string): Offer | undefined;
  setOffer(key: string, value: Offer | null): void;
  /** The live view listens: a browser step arrived while it shows (connect if it is not). */
  onActivity(id: string, listener: () => void): () => void;
}

interface Context {
  api: { request: (path: string, options?: any) => Promise<any> };
  enabled: () => boolean;
  changed: () => void;
  showTab: (id: string, tab: string) => void;
}

export const browserPath = (id: string) => `/api/conversations/${encodeURIComponent(id)}/browser`;
export const liveUrl = (id: string, wake = false, loc: { protocol: string; host: string } = location) =>
  `${loc.protocol === "https:" ? "wss" : "ws"}://${loc.host}${browserPath(id)}/live${wake ? "?wake=1" : ""}`;

export function createBrowserBridge({ api, enabled, changed, showTab }: Context): BrowserBridge {
  const chats = new Map<string, BrowserChat>();
  const offers = new Map<string, Offer>();
  const activity = new Map<string, Set<() => void>>();
  const chat = (id: string): BrowserChat => {
    let c = chats.get(id);
    if (!c) chats.set(id, (c = { on: false, status: "stopped", controller: "model", url: "", title: "", errorMessage: "", loaded: false, shown: false }));
    return c;
  };
  const take = (id: string, r: any) => {
    const b = r?.browser ?? r ?? {};
    Object.assign(chat(id), { on: !!b.on, status: b.status || "stopped", controller: b.controller || "model", url: b.url || "", title: b.title || "", errorMessage: b.errorMessage || "", loaded: true });
  };
  return {
    enabled,
    on: (id) => !!id && !!chats.get(id)?.on,
    async load(id) {
      if (!enabled() || !id) return;
      try {
        take(id, await api.request(browserPath(id)));
      } catch {
        chat(id).loaded = true;
      }
      changed();
    },
    async setOn(id, value) {
      take(id, await api.request(browserPath(id), { method: "PUT", body: { on: value } }));
      chat(id).shown = value;
      if (value) showTab(id, "browser");
      changed();
    },
    /** Watches the reply's events: browsing shows the tab, a handoff hands it over. */
    observe(event, id) {
      if (event.event !== "tool" || !enabled()) return;
      const data = event.data || {};
      const tool = String(data.tool || "");
      if (!tool.startsWith("browser_") && data.kind !== "browser-handoff") return;
      const c = chat(id);
      c.on = true;
      c.status = "running";
      if (data.kind === "browser-handoff") c.controller = "user";
      if (data.url) {
        c.url = data.url;
        c.title = data.title || c.title;
      }
      if (!c.shown) {
        c.shown = true;
        showTab(id, "browser");
      }
      activity.get(id)?.forEach((l) => l());
      changed();
    },
    reset() {
      chats.clear();
      offers.clear();
    },

    chat,
    async setController(id, controller) {
      await api.request(`${browserPath(id)}/control`, { method: "POST", body: { controller } });
      chat(id).controller = controller;
      changed();
    },
    update(id, values) {
      Object.assign(chat(id), values);
      changed();
    },
    show(id) {
      chat(id).shown = true;
      showTab(id, "browser");
    },
    dismiss(id) {
      chat(id).shown = false;
    },
    offer: (key) => offers.get(key),
    setOffer(key, value) {
      if (value) offers.set(key, value);
      else offers.delete(key);
      changed();
    },
    onActivity(id, listener) {
      let set = activity.get(id);
      if (!set) activity.set(id, (set = new Set()));
      set.add(listener);
      return () => {
        set.delete(listener);
      };
    },
  };
}

// The chat's state and the bookkeeping around replies: sending and streaming,
// reconnecting to a reply still being written after a dropped connection or a reload,
// stopping, versions, regenerate, edit and resend, branches, the model and its
// parameters, temporary chats and shared chats.
//
// It lives above the pages (in ChatProvider), so a reply keeps streaming while the
// person looks at another page, and React components read it through useChat().

import type { QueryClient } from "@tanstack/react-query";
import type { ApiClient, StreamEvent } from "@/lib/api";
import { applyChatDraft, chatDraft } from "@/components/projects/chat-draft";
import { readEvents as readEventsDefault } from "@/lib/api";
import { createBrowserBridge, type BrowserBridge } from "@/components/browser/bridge";
import { createCodeBridge, type CodeBridge } from "@/components/code/bridge";
import { createDevicesBridge, type DevicesBridge } from "@/lib/devices/bridge";
import { createWorkspaceBridge, type WorkspaceBridge } from "@/components/workspace/bridge";
import { createAttachments, type Attachments } from "./attachments";
import { EFFORTS } from "./composer";
import { conversationBusy, isPendingMessage, mergeProgress, renderSignature, suggestionsPending } from "./conversation-state";
import { applyToolsDraft, chatToolsKey, enableAuto, resetToolsDraft } from "@/components/connections/chat-tools";
import { resetSteerDraft } from "@/lib/chat/steer-draft";
import { nextRating } from "./feedback";
import { createFollow, type Follow } from "./follow";
import { generationKeys, generationValues, isLocked, type Generation } from "./generation";
import { createLoad, WAIT_NOTICE_MS } from "./load";
import { leftTemporary } from "./temporary";
import { isLive } from "./turn";
import type { Attachment, Conversation, Message, ModelInfo, Preferences, Preset, ShareInfo } from "./types";

export interface Route {
  /** chat for / and /chat/<id>; otherwise the first part of the path. */
  page: string;
  id: string;
}
export const routeOf = (path: string): Route => {
  const [, first = "", second = ""] = path.split("/");
  if (first === "") return { page: "chat", id: "" };
  if (first === "chat") return { page: "chat", id: decodeURIComponent(second) };
  return { page: first, id: decodeURIComponent(second) };
};

export interface Operation {
  key: string;
  draftKey?: number;
  sequence?: number;
  epoch: number;
  conversation: Conversation | null;
  controller?: AbortController;
  transportActive: boolean;
  accepted: boolean;
  terminal?: boolean;
  phase: "sending" | "generating" | "recovering";
  kind?: "send" | "edit" | "regenerate";
  prompt?: string;
  attached?: Attachment[];
  beforeMessageIds?: (string | undefined)[];
  stopRequested: boolean;
  stopSent: boolean;
  generationId?: string | null;
  assistant?: Message;
  userMessage?: Message;
  rejected?: boolean;
  restoreDraft?: boolean;
  error?: string;
  finished?: boolean;
}

export type Dialog =
  | { kind: "rename"; id: string; title: string }
  | { kind: "delete"; id: string }
  | { kind: "share"; id: string }
  | { kind: "edit"; messageId: string; content: string; required: boolean }
  | { kind: "feedback"; index: number }
  | { kind: "parameters" }
  | { kind: "presets" }
  | { kind: "alternatives"; conversations: { id: string; title: string; model: string }[] }
  | { kind: "shortcuts" }
  | { kind: "knowledge" }
  | { kind: "tools" }
  | { kind: "files" }
  | { kind: "pasted"; id: string; name: string }
  | { kind: "copy"; text: string }
  | { kind: "save"; item: SaveItem };

/** Something from a chat for the Library's save dialog. */
export interface SaveItem {
  kind: "code" | "excerpt" | "note" | "link";
  title?: string;
  body?: string;
  language?: string;
  conversationId?: string;
  messageId?: string;
  projectId?: string;
}

export interface PaneState {
  open: boolean;
  tab: string;
  details: boolean;
  before: string | null;
}

export interface Usage {
  today?: { requests?: number; totalTokens?: number };
  limits?: { dailyRequests?: number; dailyTokens?: number };
  [key: string]: unknown;
}

export interface Capabilities {
  features?: Record<string, boolean | undefined>;
  [key: string]: unknown;
}

export interface StoreOptions {
  api: Pick<ApiClient, "request" | "send">;
  readEvents?: typeof readEventsDefault;
  toast: (message: string) => void;
  /** Goes to a path, or replaces the current one. */
  go: (path: string, replace?: boolean) => void;
  /** The Recent list changed: a chat to put first, one to drop, or reload it. */
  chats: {
    upsert: (c: Conversation) => void;
    remove: (id: string) => void;
    refresh: () => Promise<void> | void;
    find: (id: string) => { id: string; title: string; pinned?: boolean; archived?: boolean; temporary?: boolean } | undefined;
  };
  /** The shared query cache, for the project and knowledge parts' own data. */
  queries?: QueryClient;
  /** The person's preferences changed on the server. */
  preferencesChanged?: (r: unknown) => void;
}

const chatPath = (id: string) => `/api/conversations/${encodeURIComponent(id)}`;
const sharedPath = (id: string) => `/api/shared/${encodeURIComponent(id)}`;
const unwrap = (data: any): Conversation => data.conversation || data;

export class ChatStore {
  // ---- What the pages read ----
  route: Route = { page: "chat", id: "" };
  capabilities: Capabilities = { features: {} };
  isAdmin = false;
  models: ModelInfo[] = [];
  modelsLoaded = false;
  preferences: Preferences | null = null;
  preferenceOverrides: Record<string, unknown> = {};
  presets: Preset[] = [];
  usage: Usage | null = null;
  model = "";
  systemPrompt = "";
  temperature = 0.7;
  topP = 1;
  maxTokens = 0;
  reasoningEffort = "medium";
  compact = false;
  context = true;
  focus = false;
  private draftText = "";
  /** Counts changes to the draft made other than by typing, so the composer shows them. */
  draftRevision = 0;
  /** The message being written. Setting it (a starter, a quote, a failed send) shows it in the box. */
  get draft() {
    return this.draftText;
  }
  set draft(text: string) {
    this.draftText = text;
    this.draftRevision++;
  }
  draftKey = 0;
  workspaceDraft = false;
  browserDraft = false;
  temporaryDraft = false;
  active: Conversation | null = null;
  shared: ShareInfo | null = null;
  loadingChat = false;
  chatError = "";
  error = "";
  catalogError = "";
  streaming = false;
  switchingVersion = false;
  streamChatId: string | null = null;
  dialog: Dialog | null = null;
  /** Open/closed state that outlives a re-render: thinking panels, details, sources, activity. */
  thinkingOpen = new Map<string, boolean>();
  detailsOpen = new Set<string>();
  sourcesOpen = new Set<string>();
  openActivity = new Set<string>();
  panes = new Map<string, PaneState>();
  /** Notes sent with Steer per chat, until the reply reads them. */
  queuedNotes = new Map<string, string[]>();

  // ---- Parts ----
  readonly attachments: Attachments;
  readonly load;
  workspace: WorkspaceBridge;
  browser: BrowserBridge;
  code: CodeBridge;
  devices: DevicesBridge;
  follow: Follow | null = null;
  scroller: HTMLElement | null = null;

  // ---- Bookkeeping ----
  readonly operations = new Map<string, Operation>();
  private revisions = new Map<string, number>();
  private watches = new Map<string, ReturnType<typeof setTimeout>>();
  private versionSelections = new Map<string, number>();
  private pendingEfforts = new Map<string, { value: string; sequence: number }>();
  private effortSaveQueue: Promise<unknown> = Promise.resolve();
  private effortSequence = 0;
  private operationSequence = 0;
  private loadSequence = 0;
  private epoch = 0;
  private temporaryOpen: string | null = null;
  private switchAfterStop: { conversation: string; message: string } | null = null;
  private sharedSignature = "";
  private timers: ReturnType<typeof setInterval>[] = [];
  private disposed = false;
  private listeners = new Set<() => void>();
  version = 0;
  private paintCost = 0;
  /** Set when the next render should show the end of the conversation. */
  wantsEnd = false;
  /** A route was opened since the last start (a dispose cancels its load). */
  private routed = false;

  constructor(private options: StoreOptions) {
    const { api, toast } = options;
    this.attachments = createAttachments({ api, toast, streaming: () => this.streaming });
    this.attachments.subscribe(() => this.emit());
    this.load = createLoad({ api, state: () => ({ models: this.models, preferences: this.preferences }), modelName: (id) => this.modelName(id) });
    this.load.subscribe(() => this.emit());
    const showTab = (id: string, tab: string) => this.showTab(id, tab);
    this.workspace = createWorkspaceBridge({ api, enabled: () => this.feature("workspaces"), changed: () => this.emit(), showTab });
    this.browser = createBrowserBridge({ api, enabled: () => this.feature("browser"), changed: () => this.emit(), showTab });
    this.code = createCodeBridge();
    this.devices = createDevicesBridge({ api, enabled: () => this.feature("devices"), changed: () => this.emit() });
  }

  // ---- Subscription ----
  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };
  getVersion = () => this.version;
  emit() {
    if (this.disposed) return;
    this.syncStreamingState();
    this.version++;
    this.listeners.forEach((l) => l());
  }
  /** How long the last render of the conversation took, to pace repaints while streaming. */
  reportPaint(ms: number) {
    this.paintCost = ms;
  }

  start() {
    // React runs effects twice in development: a dispose, then start again.
    this.disposed = false;
    // While a reply waits for its first words, ask how busy its model is, so its notice
    // can say.
    this.timers.push(
      setInterval(() => {
        const c = this.active,
          index = c?.messages?.findIndex((m) => isPendingMessage(m) && !String(m.content || "").trim());
        if (index == null || index < 0 || !this.load.offered() || this.route.id !== c!.id) return;
        const m = c!.messages[index]!;
        if (Date.now() - (Date.parse(m._began || m.createdAt || "") || Date.now()) < WAIT_NOTICE_MS - 1000) return;
        this.load.refresh(2500).then(() => this.emit());
      }, 1000),
    );
    // The mark on the model button stays roughly current while the page is open.
    this.timers.push(
      setInterval(() => {
        if (typeof document !== "undefined" && document.visibilityState !== "visible") return;
        if (!this.load.offered()) return;
        this.load.refresh(55000);
      }, 60000),
    );
    this.refreshUsage();
  }

  /** The person signed out or the page closed: stop everything this session started. */
  dispose() {
    this.disposed = true;
    this.routed = false;
    this.epoch++;
    this.loadSequence++;
    for (const op of this.operations.values()) op.controller?.abort();
    this.operations.clear();
    for (const timer of this.watches.values()) clearTimeout(timer);
    this.watches.clear();
    this.timers.forEach(clearInterval);
    this.follow?.stop();
    this.attachments.clear();
    this.workspace.reset();
    this.browser.reset();
    this.devices.reset();
    // What was set up for a chat not yet started, or typed and not sent, is this
    // person's: the next one to sign in on this page starts clean, with Auto off.
    resetToolsDraft();
    chatDraft.reset();
    resetSteerDraft();
  }

  // ---- Helpers ----
  /** The chat runs its tools without asking (Auto mode), as the tools' own query says. */
  isAuto(id: string) {
    return !!this.options.queries?.getQueryData<{ auto?: boolean }>(chatToolsKey(id))?.auto;
  }
  /** Research turns Auto mode on for its chat, where the installation allows it. */
  enableAuto(id: string) {
    return enableAuto(id, this.feature("autoMode"), this.options.queries);
  }
  /** The shared query cache (for the project and knowledge parts' own data). */
  get queries() {
    return this.options.queries;
  }
  feature(name: string) {
    return !!this.capabilities.features?.[name];
  }
  modelName = (id?: string) => this.models.find((m) => m.id === id)?.name || id || "Choose a model";
  policyModel() {
    return this.models.find((m) => m.id === this.model);
  }
  isLocked(key: string) {
    return isLocked(this.policyModel(), key);
  }
  generation(): Generation {
    return { systemPrompt: this.systemPrompt, temperature: this.temperature, topP: this.topP, maxTokens: this.maxTokens, reasoningEffort: this.reasoningEffort };
  }
  private setGeneration(values: Generation) {
    this.systemPrompt = values.systemPrompt;
    this.temperature = values.temperature;
    this.topP = values.topP;
    this.maxTokens = values.maxTokens;
    this.reasoningEffort = values.reasoningEffort;
  }
  resetGeneration(conversation: Record<string, unknown> = {}) {
    this.setGeneration(generationValues(this.policyModel(), this.preferenceOverrides, conversation));
  }
  applyModelPolicy(all = false) {
    const model = this.policyModel();
    for (const key of generationKeys) if ((all || this.isLocked(key)) && model?.defaults?.[key] != null) (this as any)[key] = model.defaults[key];
  }
  /** Whether the chat on screen can be answered in now. */
  get available() {
    return this.models.length > 0;
  }
  private toast(message: string) {
    this.options.toast(message);
  }
  private perform = async (action: () => Promise<unknown> | unknown) => {
    try {
      await action();
    } catch (error) {
      if (!this.disposed && (error as Error).name !== "AbortError") this.toast((error as Error).message);
    }
  };
  /** Runs an action from a button, toasting what goes wrong. */
  run(action: () => Promise<unknown> | unknown) {
    return this.perform(action);
  }

  // ---- Catalog and preferences ----
  setCatalog({ models, error }: { models?: ModelInfo[]; error?: string }) {
    if (error !== undefined) {
      this.catalogError = error;
      this.emit();
      return;
    }
    this.models = models || [];
    this.modelsLoaded = true;
    if (!this.models.some((m) => m.id === this.model)) this.model = this.models[0]?.id || "";
    this.catalogError = this.models.length ? "" : "No models are available to this account. Contact your administrator.";
    if (this.preferences) this.applyPreferences(this.preferences, { generation: !this.active });
    this.emit();
  }
  applyPreferences(preferences: Preferences | null | undefined, { generation = true, overrides = this.preferenceOverrides }: { generation?: boolean; overrides?: Record<string, unknown> } = {}) {
    if (!preferences) return;
    this.preferenceOverrides = overrides || {};
    const preferred = this.models.find((m) => m.id === preferences.defaultModel);
    this.preferences = { ...preferences, ...generationValues(preferred, this.preferenceOverrides) };
    this.compact = preferences.density === "compact";
    this.context = preferences.context !== false;
    this.focus = !!preferences.focus;
    if (generation) {
      this.model = this.models.some((m) => m.id === preferences.defaultModel) ? preferences.defaultModel! : this.models[0]?.id || "";
      this.resetGeneration();
    }
    this.emit();
  }
  /** The preferences response: { preferences, overrides }. */
  setPreferencesResponse(r: { preferences?: Preferences; overrides?: Record<string, unknown> }, generation: boolean) {
    this.applyPreferences(r.preferences, { generation, overrides: r.overrides });
  }
  async savePreference(body: Record<string, unknown>) {
    const r = await this.options.api.request<any>("/api/preferences", { method: "PATCH", body });
    this.applyPreferences(r.preferences, { generation: false, overrides: r.overrides });
    this.options.preferencesChanged?.(r);
  }
  setPresets(presets: Preset[]) {
    this.presets = presets;
    this.emit();
  }
  applyPreset(preset?: Preset) {
    if (!preset) return;
    if (!this.models.some((m) => m.id === preset.model)) {
      this.toast("This preset’s model is not available to your account. Edit the preset to choose another model.");
      return;
    }
    this.model = preset.model;
    this.resetGeneration(preset.parameters || {});
    this.dialog = null;
    if (this.route.page !== "chat") this.options.go("/");
    this.emit();
    this.toast(`Applied ${preset.name}`);
  }
  /** The model you pick becomes your default, so new chats start with the last one you used. */
  async selectModel(id: string) {
    this.model = id;
    this.resetGeneration();
    this.emit();
    if (this.preferences?.defaultModel !== this.model) await this.perform(() => this.savePreference({ defaultModel: this.model }));
  }
  /** Moves the effort slider; a chat keeps its effort, saved once the slider settles. */
  setEffort(effort: string) {
    this.reasoningEffort = EFFORTS.includes(effort as never) ? effort : "none";
    this.emit();
  }
  saveEffort() {
    if (!this.active || this.streaming || this.shared) return;
    const id = this.active.id,
      effort = this.reasoningEffort,
      epoch = this.epoch,
      sequence = ++this.effortSequence;
    this.pendingEfforts.set(id, { value: effort, sequence });
    this.effortSaveQueue = this.effortSaveQueue
      .catch(() => {})
      .then(async () => {
        if (epoch !== this.epoch || this.pendingEfforts.get(id)?.sequence !== sequence) return;
        try {
          await this.options.api.request(chatPath(id), { method: "PATCH", body: { reasoningEffort: effort } });
          if (this.pendingEfforts.get(id)?.sequence === sequence) {
            this.pendingEfforts.delete(id);
            if (this.active?.id === id) this.active.reasoningEffort = effort;
          }
        } catch (error) {
          if (this.pendingEfforts.get(id)?.sequence === sequence) this.pendingEfforts.delete(id);
          if ((error as any).status !== 409 && (error as Error).name !== "AbortError" && epoch === this.epoch) this.toast((error as Error).message);
        }
      });
  }
  /** The Parameters dialog: applies to the next message, and to this chat when one is open. */
  async saveParameters(values: Omit<Generation, "reasoningEffort">) {
    Object.assign(this, values);
    this.applyModelPolicy();
    if (this.active && !this.shared) {
      const settings = this.generation();
      const c = unwrap(await this.options.api.request(chatPath(this.active.id), { method: "PATCH", body: { settings } }));
      this.active.settings = c.settings || { ...settings };
    }
    this.dialog = null;
    this.emit();
  }
  async refreshUsage() {
    const epoch = this.epoch;
    try {
      const usage = await this.options.api.request<Usage>("/api/usage");
      if (epoch !== this.epoch) return;
      this.usage = usage;
      this.emit();
    } catch {
      // The status bar keeps what it had.
    }
  }

  // ---- The side pane (details, workspace, browser) ----
  pane(id: string): PaneState {
    let pane = this.panes.get(id);
    if (!pane) this.panes.set(id, (pane = { open: false, tab: "details", details: false, before: null }));
    return pane;
  }
  /** Tabs the pane offers for a chat. */
  /** The server has a preview domain; without one a workspace has no Preview tab. */
  previews() {
    // Only once the server says so: before it knows who is signed in it leaves it out.
    return this.capabilities.features?.previews === true;
  }
  paneTabs(id: string): string[] {
    const tabs: string[] = [];
    if (this.workspace.has(id)) tabs.push(...(this.previews() ? ["preview"] : []), "files", "terminal", ...(this.feature("code") ? ["code"] : []));
    if (this.browser.on(id)) tabs.push("browser");
    const pane = this.pane(id);
    if (pane.details || !tabs.length) tabs.push("details");
    return tabs;
  }
  paneVisible(id: string) {
    if (!id || this.route.id !== id || this.shared) return false;
    const pane = this.pane(id);
    return pane.open && this.paneTabs(id).includes(pane.tab);
  }
  showing(id: string, tab: string) {
    return this.paneVisible(id) && this.pane(id).tab === tab;
  }
  showTab(id: string, tab: string) {
    const pane = this.pane(id);
    pane.tab = tab === "preview" && !this.previews() ? "files" : tab;
    pane.open = true;
    this.emit();
  }
  closePane(id: string) {
    const pane = this.pane(id);
    pane.open = false;
    pane.details = false;
    this.emit();
  }
  /** Opens the chat's details in the pane, or closes them and goes back to the tab before. */
  toggleDetails(id = this.route.id) {
    if (!id || this.shared) return;
    const pane = this.pane(id);
    if (this.showing(id, "details")) {
      pane.details = false;
      if (pane.before && pane.before !== "details" && this.paneTabs(id).includes(pane.before)) pane.tab = pane.before;
      else pane.open = false;
    } else {
      pane.before = this.paneVisible(id) ? pane.tab : null;
      pane.details = true;
      pane.tab = "details";
      pane.open = true;
    }
    this.emit();
  }
  async toggleWorkspace() {
    const id = this.route.id;
    if (!id) {
      this.workspaceDraft = !this.workspaceDraft;
      this.emit();
      return;
    }
    // A press shows the workspace's tabs, or hides the pane when they are showing.
    if (this.workspace.has(id)) {
      const own = ["preview", "files", "terminal"];
      const pane = this.pane(id);
      if (own.some((tab) => this.showing(id, tab))) {
        pane.open = false;
        pane.details = false;
      } else {
        pane.open = true;
        if (!own.includes(pane.tab)) pane.tab = this.previews() ? "preview" : "files";
      }
      this.emit();
      return;
    }
    await this.workspace.create(id);
    this.toast("Workspace created. Ask for something to build.");
    this.emit();
  }
  async toggleBrowser() {
    const id = this.route.id;
    if (!id) {
      this.browserDraft = !this.browserDraft;
      this.emit();
      return;
    }
    if (this.browser.on(id) && !this.showing(id, "browser")) {
      this.showTab(id, "browser");
      return;
    }
    await this.browser.setOn(id, !this.browser.on(id));
    this.emit();
  }

  // ---- Routes ----
  attachScroller(el: HTMLElement | null) {
    if (this.scroller === el) return;
    this.follow?.stop();
    this.scroller = el;
    this.follow = el ? createFollow(el) : null;
  }
  /** Opening a conversation shows its latest message. */
  scrollToEnd() {
    this.follow?.resume();
    this.wantsEnd = true;
  }
  setRoute(route: Route) {
    if (route.page === this.route.page && route.id === this.route.id && this.routed) return;
    this.routed = true;
    const newPage = route.page !== this.route.page || route.id !== this.route.id;
    this.route = route;
    this.navigate(newPage);
  }
  private conversationIdIs(id: string) {
    return this.route.page === "chat" && this.route.id === id;
  }
  async navigate(_newPage = true) {
    const left = leftTemporary(this.temporaryOpen, this.route);
    if (left) this.leaveTemporary(left);
    const { page, id } = this.route;
    this.chatError = "";
    this.shared = null;
    this.loadSequence++;
    if (page !== "chat" || !id) {
      this.loadingChat = false;
      this.active = null;
      this.emit();
      return;
    }
    const running = this.operations.get(id);
    if (running?.conversation) {
      this.active = running.conversation;
      this.loadingChat = false;
      this.loadParts(id);
      if (this.models.some((m) => m.id === this.active!.model)) this.model = this.active!.model;
      this.resetGeneration({ ...(this.active.settings || {}), reasoningEffort: this.active.reasoningEffort });
      this.scrollToEnd();
      this.emit();
      this.scheduleWatch(id, 100);
      return;
    }
    const sequence = this.loadSequence,
      revision = this.revisions.get(id) || 0,
      requestedEffort = this.pendingEfforts.get(id);
    this.loadingChat = true;
    this.emit();
    try {
      let c = unwrap(await this.options.api.request(chatPath(id)));
      if (sequence !== this.loadSequence || this.disposed) return;
      if ((this.revisions.get(id) || 0) !== revision) {
        if (this.operations.get(id)?.conversation) c = this.operations.get(id)!.conversation!;
        else {
          this.scheduleWatch(id, 0);
          return;
        }
      }
      const desiredEffort = this.pendingEfforts.get(id) || requestedEffort;
      if (desiredEffort) c.reasoningEffort = desiredEffort.value;
      c.messages ||= [];
      this.active = c;
      if (c.temporary) this.temporaryOpen = c.id;
      this.loadParts(c.id);
      if (c.model && this.models.some((m) => m.id === c.model)) this.model = c.model;
      this.resetGeneration({ ...(c.settings || {}), ...(EFFORTS.includes(c.reasoningEffort as never) ? { reasoningEffort: c.reasoningEffort } : {}) });
      this.observeConversation(c);
    } catch (error) {
      if (sequence !== this.loadSequence || this.disposed) return;
      this.active = null;
      this.chatError = (error as Error).message;
      // Not theirs: it may be someone else's chat shared with them.
      if ((error as any).status === 404) {
        const r = await this.options.api.request<any>(sharedPath(id)).catch(() => null);
        if (sequence !== this.loadSequence || this.disposed) return;
        if (r && !r.mine) this.openShared(r, sequence);
        else this.chatError = "This chat does not exist, or it is not shared with you.";
      }
    } finally {
      if (sequence === this.loadSequence && !this.disposed) {
        this.loadingChat = false;
        if (this.active) this.scrollToEnd();
        this.emit();
      }
    }
  }
  private loadParts(id: string) {
    this.workspace.load(id);
    this.browser.load(id);
  }
  /** Asks the server for the chat on screen again (the Retry button). */
  reload() {
    this.navigate();
  }

  // ---- Someone else's chat ----
  private openShared(r: { conversation: Conversation; share: ShareInfo }, sequence: number) {
    r.conversation.messages ||= [];
    this.active = r.conversation;
    this.shared = r.share;
    this.chatError = "";
    this.sharedSignature = sharedSignatureOf(r.conversation);
    this.watchShared(r.conversation.id, sequence);
  }
  /** Fetched again while open, so it grows live. */
  private watchShared(id: string, sequence: number) {
    const busy = (this.active?.messages || []).some(isPendingMessage);
    setTimeout(
      async () => {
        if (sequence !== this.loadSequence || !this.shared || this.active?.id !== id || this.disposed) return;
        if (typeof document === "undefined" || !document.hidden) {
          try {
            const r = await this.options.api.request<any>(sharedPath(id));
            if (sequence !== this.loadSequence || !this.shared) return;
            const next = sharedSignatureOf(r.conversation);
            if (next !== this.sharedSignature) {
              const follow = this.follow?.on() ?? true;
              this.sharedSignature = next;
              r.conversation.messages ||= [];
              this.active = r.conversation;
              this.shared = r.share;
              if (follow) this.scrollToEnd();
              this.emit();
            }
          } catch (e) {
            if ((e as any).status === 404 && sequence === this.loadSequence) {
              this.active = null;
              this.shared = null;
              this.chatError = "This chat is no longer shared with you.";
              this.emit();
              return;
            }
          }
        }
        this.watchShared(id, sequence);
      },
      busy ? 1500 : 4000,
    );
  }
  async forkShared() {
    const id = this.active?.id;
    if (!id || !this.shared) return;
    const r = await this.options.api.request<any>(`${sharedPath(id)}/fork`, { method: "POST" });
    this.toast("Forked into a chat of your own");
    await this.options.chats.refresh();
    this.options.go(`/chat/${encodeURIComponent(r.conversation.id)}`);
  }

  // ---- Temporary chats ----
  /** A new chat from the home screen is temporary when the toggle is on, outside a project. */
  temporaryDraftOn() {
    return this.temporaryDraft && !chatDraft.get().project;
  }
  toggleTemporary() {
    this.temporaryDraft = !this.temporaryDraft;
    this.emit();
  }
  /** The person left a temporary chat: stop following it and have the server delete it. */
  leaveTemporary(id: string) {
    if (this.temporaryOpen === id) this.temporaryOpen = null;
    const op = this.operations.get(id);
    if (op) {
      op.controller?.abort();
      this.operations.delete(id);
    }
    clearTimeout(this.watches.get(id));
    this.watches.delete(id);
    return this.options.api.send(`${chatPath(id)}/leave`);
  }
  /** Signing out or closing the tab leaves a temporary chat too. */
  leaveOpenTemporary() {
    return this.temporaryOpen ? this.leaveTemporary(this.temporaryOpen) : Promise.resolve();
  }

  // ---- Operations ----
  private upsert(c: Conversation) {
    if (c.temporary) return;
    this.options.chats.upsert(c);
  }
  currentOperation() {
    const r = this.route;
    return r.page === "chat" ? this.operations.get(r.id || `draft:${this.draftKey}`) : undefined;
  }
  private syncStreamingState() {
    const op = this.currentOperation(),
      r = this.route;
    this.streaming = !!op || (r.page === "chat" && r.id === this.active?.id && conversationBusy(this.active));
    this.switchingVersion = r.page === "chat" && this.versionSelections.has(r.id);
    this.streamChatId = this.streaming ? op?.conversation?.id || r.id || null : null;
  }
  private touchConversation(id: string) {
    this.revisions.set(id, (this.revisions.get(id) || 0) + 1);
  }
  private operationIsCurrent(op: Operation) {
    return op.epoch === this.epoch && this.operations.get(op.key) === op && !this.disposed;
  }
  private showConversation(c: Conversation) {
    this.upsert(c);
    if (this.conversationIdIs(c.id)) this.active = c;
  }
  private observeConversation(c: Conversation) {
    // A device step saved mid-call is carried out by this tab (a reloaded page).
    if (conversationBusy(c)) for (const m of c.messages) if (isPendingMessage(m) && Array.isArray(m.activity)) this.devices.sync(c.id, m.activity);
    if (conversationBusy(c) && !this.operations.has(c.id)) {
      this.operations.set(c.id, {
        key: c.id,
        epoch: this.epoch,
        conversation: c,
        generationId: c.activeGenerationId,
        assistant: c.messages.find(isPendingMessage),
        accepted: true,
        transportActive: false,
        phase: "recovering",
        stopRequested: !!c.generationCancelRequested,
        stopSent: !!c.generationCancelRequested,
      });
    }
    if (conversationBusy(c) || suggestionsPending(c)) this.scheduleWatch(c.id);
    // Auto mode may start a turn on its own; an open chat looks for it.
    else if (this.isAuto(c.id) && this.conversationIdIs(c.id)) this.scheduleWatch(c.id, 30000);
  }
  scheduleWatch(id: string, delay?: number) {
    if (!id || this.disposed) return;
    const op = this.operations.get(id);
    delay ??= op?.transportActive && op.accepted && !op.stopRequested && !op.terminal ? 12000 : this.route.id === id ? 1200 : 3000;
    clearTimeout(this.watches.get(id));
    this.watches.set(
      id,
      setTimeout(() => {
        this.watches.delete(id);
        this.reconcileConversation(id);
      }, delay),
    );
  }
  private finishOperation(op: Operation, saved?: Conversation) {
    if (!this.operationIsCurrent(op)) return;
    this.operations.delete(op.key);
    op.finished = true;
    this.refreshUsage();
    clearTimeout(this.watches.get(op.key));
    this.watches.delete(op.key);
    op.controller?.abort();
    if (saved) this.showConversation(saved);
    if ((op.rejected || op.restoreDraft) && op.kind === "send" && this.conversationIdIs(saved?.id || "") && !this.draft) {
      this.draft = op.prompt || "";
      this.attachments.restore(op.attached || []);
    }
    if (saved) this.observeConversation(saved);
    if (this.switchAfterStop && saved?.id === this.switchAfterStop.conversation) {
      const { message: id } = this.switchAfterStop;
      this.switchAfterStop = null;
      setTimeout(() => {
        this.syncStreamingState();
        if (this.route.id === saved.id) this.forkAndSend(id, "regenerate");
      }, 0);
    }
  }
  async reconcileConversation(id: string) {
    if (this.versionSelections.has(id)) {
      this.scheduleWatch(id);
      return;
    }
    const epoch = this.epoch,
      revision = this.revisions.get(id) || 0,
      op = this.operations.get(id);
    try {
      const saved = unwrap(await this.options.api.request(chatPath(id)));
      if (epoch !== this.epoch || this.disposed) return;
      saved.messages ||= [];
      if (this.versionSelections.has(id) || (this.revisions.get(id) || 0) !== revision || (op && !this.operationIsCurrent(op))) {
        this.scheduleWatch(id);
        return;
      }
      const errorBefore = this.error;
      if (this.error === "Connection interrupted. Reconnecting to the saved response…" && this.route.id === id) this.error = "";
      const ownMessage = op?.assistant?.id && saved.messages?.find((m) => m.id === op.assistant!.id);
      const authoritativeTerminal = ownMessage && !isPendingMessage(ownMessage);
      const unconfirmedTransport = op && !op.accepted && !op.transportActive;
      if (unconfirmedTransport && !op.rejected) {
        op.restoreDraft = saved.messages.every((m) => op.beforeMessageIds?.includes(m.id));
        if (op.restoreDraft && this.route.id === id) this.error = "The connection ended before the message was confirmed. Your draft is ready to try again.";
      }
      if (op && (unconfirmedTransport || authoritativeTerminal || (!conversationBusy(saved) && (!op.transportActive || op.terminal)) || op.rejected)) {
        this.finishOperation(op, saved);
      } else if (op) {
        if (!op.transportActive || op.terminal) {
          op.conversation = mergeProgress(saved, op.conversation);
          op.assistant = op.conversation.messages.find((m) => m.id === op.assistant?.id) || op.conversation.messages.find(isPendingMessage);
        } else {
          op.conversation!.activeGenerationId = saved.activeGenerationId || op.generationId;
          op.conversation!.generationCancelRequested = saved.generationCancelRequested;
        }
        if (saved.generationCancelRequested) op.stopRequested = true;
        this.showConversation(op.conversation!);
        if (op.stopRequested && !op.stopSent && op.generationId) this.requestStop(op);
        this.scheduleWatch(id);
      } else {
        const shown = this.conversationIdIs(id) && this.active?.id === id ? renderSignature(this.active) : null;
        this.showConversation(saved);
        this.observeConversation(saved);
        if (shown && shown === renderSignature(saved) && this.error === errorBefore) {
          // Only follow-up suggestions changed.
          this.emit();
          return;
        }
      }
      if (this.conversationIdIs(id) && (!op?.transportActive || op.finished || op.terminal)) this.emit();
    } catch (error) {
      if (epoch !== this.epoch || (error as Error).name === "AbortError" || this.disposed) return;
      if (this.versionSelections.has(id) || (this.revisions.get(id) || 0) !== revision) {
        this.scheduleWatch(id);
        return;
      }
      if (op && !this.operationIsCurrent(op)) return;
      if (op?.terminal) {
        op.conversation!.activeGenerationId = null;
        op.conversation!.generationCancelRequested = false;
        this.finishOperation(op, op.conversation!);
        if (this.route.id === id) {
          this.error = "The response finished, but saved details could not be refreshed. Try reloading when your connection returns.";
          this.emit();
        }
      } else if ((error as any).status === 404) {
        if (op) this.finishOperation(op);
        this.watches.delete(id);
        if (this.route.id === id) {
          this.chatError = (error as Error).message;
          this.emit();
        }
      } else {
        if (op) op.phase = "recovering";
        if (this.conversationIdIs(id)) {
          this.error = "Connection interrupted. Reconnecting to the saved response…";
          this.emit();
        }
        this.scheduleWatch(id, 3000);
      }
    }
  }
  private async requestStop(op: Operation) {
    if (!this.operationIsCurrent(op) || op.stopSent || !op.generationId || !op.conversation?.id) return;
    op.stopSent = true;
    try {
      const saved = unwrap(await this.options.api.request(`${chatPath(op.conversation.id)}/stop`, { method: "POST", body: { generationId: op.generationId } }));
      if (!this.operationIsCurrent(op)) return;
      saved.messages ||= [];
      if (!conversationBusy(saved)) this.finishOperation(op, saved);
      else {
        op.conversation.generationCancelRequested = true;
        this.scheduleWatch(saved.id, 100);
      }
      if (this.route.id === saved.id) this.emit();
    } catch (error) {
      if (!this.operationIsCurrent(op) || (error as Error).name === "AbortError") return;
      op.stopSent = false;
      op.stopRequested = false;
      if (this.route.id === op.conversation?.id) this.emit();
      this.toast((error as Error).message);
    }
  }
  async stopCurrent() {
    const op = this.currentOperation();
    if (!op || op.stopRequested) return;
    op.stopRequested = true;
    if (!op.conversation?.id) {
      op.rejected = true;
      op.controller?.abort();
    } else if (op.generationId) await this.requestStop(op);
    else this.scheduleWatch(op.conversation.id, 100);
    this.emit();
  }
  async selectMessageVersion(messageId: string | null) {
    const c = this.active;
    if (!c || this.streaming || this.switchingVersion || !messageId) return;
    const epoch = this.epoch;
    this.touchConversation(c.id);
    const revision = this.revisions.get(c.id)!;
    this.versionSelections.set(c.id, revision);
    this.emit();
    try {
      const saved = unwrap(await this.options.api.request(`${chatPath(c.id)}/messages/${encodeURIComponent(messageId)}/select`, { method: "POST" }));
      if (epoch !== this.epoch || this.revisions.get(c.id) !== revision) return;
      saved.messages ||= [];
      this.touchConversation(c.id);
      this.showConversation(saved);
      this.observeConversation(saved);
    } catch (error) {
      this.toast((error as Error).message);
    } finally {
      if (epoch === this.epoch && this.versionSelections.get(c.id) === revision) {
        this.versionSelections.delete(c.id);
        if (this.route.id === c.id) this.emit();
      }
    }
  }

  // ---- Feedback ----
  /** Rates a reply up or down, or clears the rating when its thumb is clicked again. Returns the new rating. */
  async rate(index: number, clicked: number) {
    const m = this.active?.messages?.[index];
    if (!m?.id || !this.active) return 0;
    const next = nextRating(m.feedback?.rating || 0, clicked);
    if (!next) {
      await this.options.api.request(`${chatPath(this.active.id)}/messages/${encodeURIComponent(m.id)}/feedback`, { method: "DELETE" });
      m.feedback = null;
      this.emit();
      return 0;
    }
    await this.saveRating(index, { rating: next });
    return next;
  }
  async saveRating(index: number, body: { rating: number; reasons?: string[]; comment?: string }) {
    const c = this.active,
      m = c?.messages?.[index];
    if (!m?.id || !c) return;
    const r = await this.options.api.request<any>(`${chatPath(c.id)}/messages/${encodeURIComponent(m.id)}/feedback`, { method: "PUT", body });
    m.feedback = r.feedback;
    this.emit();
  }
  async rateDown(index: number) {
    if ((await this.rate(index, -1)) === -1) this.openDialog({ kind: "feedback", index });
  }

  // ---- Sending ----
  async forkAndSend(messageId: string, mode: "edit" | "regenerate", content?: string) {
    const c = this.active;
    if (!c || this.streaming || this.switchingVersion) return;
    const index = c.messages.findIndex((m) => m.id === messageId);
    if (index < 0) return;
    const userIndex = c.messages[index]!.role === "user" ? index : index - 1;
    const user = c.messages[userIndex];
    if (!user || user.role !== "user") return;
    const previousSettings = c.messages[index]!.settings || c.settings || {};
    await this.send(mode === "edit" ? content || "" : user.content, {
      kind: mode,
      messageId,
      userIndex,
      attached: user.attachments || [],
      settings: { ...previousSettings, model: this.model, ...this.generation() },
    });
  }
  /** Sends the waiting reply to another model: stops it first when it is still waiting. */
  async switchWaiting(index: number, model: string) {
    const c = this.active,
      m = c?.messages?.[index];
    if (!c || !m?.id || (isPendingMessage(m) && !this.currentOperation())) return;
    this.model = model;
    this.resetGeneration();
    if (isPendingMessage(m)) {
      this.switchAfterStop = { conversation: c.id, message: m.id };
      await this.stopCurrent();
    } else {
      this.emit();
      await this.forkAndSend(m.id, "regenerate");
    }
  }
  async send(
    input: string,
    options: { kind?: "send" | "edit" | "regenerate"; messageId?: string; userIndex?: number; attached?: Attachment[]; settings?: Record<string, unknown> } = {},
  ) {
    this.syncStreamingState();
    const prompt = String(input || "").trim();
    const kind = options.kind || "send",
      attached = options.attached || this.attachments.current();
    if ((!prompt && !attached.length && kind !== "regenerate") || this.streaming || this.switchingVersion || !this.model || this.disposed || this.attachments.busy()) return;
    if (attached.some((a) => a.kind === "image") && this.model !== "auto" && this.policyModel()?.metadata?.supportsVision !== true) {
      this.toast("Choose a vision-capable model to send an image.");
      return;
    }
    const settings = options.settings || { model: this.model, ...this.generation() };
    const selected = this.route.page === "chat" && this.route.id === this.active?.id ? this.active : null;
    const op: Operation = {
      key: selected?.id || `draft:${this.draftKey}`,
      draftKey: this.draftKey,
      sequence: ++this.operationSequence,
      epoch: this.epoch,
      conversation: selected,
      controller: new AbortController(),
      transportActive: true,
      accepted: false,
      terminal: false,
      phase: "sending",
      kind,
      prompt,
      attached,
      beforeMessageIds: selected?.messages?.map((m) => m.id) || [],
      stopRequested: false,
      stopSent: false,
    };
    this.operations.set(op.key, op);
    if (selected) this.touchConversation(selected.id);
    if (kind === "send") {
      this.draft = "";
      this.attachments.consume();
    }
    this.error = "";
    this.dialog = null;
    this.scrollToEnd();
    this.emit();
    let paintTimer: ReturnType<typeof setTimeout> | null = null;
    const paint = () => {
      if (paintTimer) clearTimeout(paintTimer);
      paintTimer = null;
      if (!this.operationIsCurrent(op) || this.active !== op.conversation || this.route.page !== "chat" || !op.assistant) return;
      this.emit();
    };
    const readEvents = this.options.readEvents || readEventsDefault;
    try {
      if (!op.conversation) {
        const c = unwrap(
          await this.options.api.request("/api/conversations", {
            method: "POST",
            body: {
              title: (prompt || attached.map((a) => a.name).join(", ")).slice(0, 80),
              model: settings.model,
              reasoningEffort: settings.reasoningEffort,
              ...(this.temporaryDraftOn() ? { temporary: true } : {}),
            },
            signal: op.controller!.signal,
          }),
        );
        if (c.temporary) this.temporaryOpen = c.id;
        if (!this.operationIsCurrent(op)) return;
        this.operations.delete(op.key);
        op.key = c.id;
        op.conversation = c;
        c.messages ||= [];
        this.operations.set(c.id, op);
        this.touchConversation(c.id);
        this.upsert(c);
        if (this.route.page === "chat" && !this.route.id && this.draftKey === op.draftKey) {
          this.active = c;
          // The new address; navigate() finds the running operation and keeps it.
          this.route = { page: "chat", id: c.id };
          this.options.go(`/chat/${encodeURIComponent(c.id)}`, true);
        }
        // The project comes first: a project chat's new workspace starts with its code.
        (await applyChatDraft(c.id)).forEach((message) => this.toast(message));
        await this.code.applyStart(c.id);
        if (!this.operationIsCurrent(op)) return;
        if (this.workspaceDraft) {
          this.workspaceDraft = false;
          await this.workspace.create(c.id);
          if (!this.operationIsCurrent(op)) return;
        }
        if (this.browserDraft) {
          this.browserDraft = false;
          await this.browser.setOn(c.id, true);
          if (!this.operationIsCurrent(op)) return;
        }
        await applyToolsDraft(c.id, this.options.queries);
      }
      const c = op.conversation!;
      c.model = settings.model as string;
      c.reasoningEffort = settings.reasoningEffort as string;
      c.settings = { ...settings };
      if (kind === "regenerate") {
        c.messages = c.messages.slice(0, options.userIndex! + 1);
        op.userMessage = c.messages[options.userIndex!];
      } else {
        if (kind === "edit") c.messages = c.messages.slice(0, options.userIndex);
        op.userMessage = { role: "user", content: prompt, attachments: attached, status: "completed" };
        c.messages.push(op.userMessage);
      }
      op.assistant = { role: "assistant", content: "", reasoningContent: "", status: "pending" };
      c.messages.push(op.assistant);
      this.showConversation(c);
      this.emit();
      this.scheduleWatch(c.id);
      const body = {
        content: kind === "regenerate" ? "" : prompt,
        ...settings,
        ...(kind === "regenerate" ? { regenerateMessageId: options.messageId } : { attachmentIds: attached.map((a) => a.id), ...(kind === "edit" ? { editMessageId: options.messageId } : {}) }),
      };
      const response = await this.options.api.request(`${chatPath(c.id)}/messages`, { method: "POST", body, signal: op.controller!.signal, stream: true });
      for await (const event of readEvents(response.body)) {
        if (!this.operationIsCurrent(op)) return;
        const assistant = op.assistant!;
        this.applyEvent(op, c, assistant, event);
        if (op.terminal) {
          const unread = this.unreadNotes(c.id, assistant);
          if (unread.length) {
            this.draft = [this.draft, ...unread].filter(Boolean).join("\n");
            this.toast("The reply finished before reading your note, so it is back in the composer.");
          }
          this.touchConversation(c.id);
          paint();
          break;
        }
        if (!paintTimer) paintTimer = setTimeout(paint, Math.max(40, Math.min(250, this.paintCost * 4)));
      }
    } catch (error) {
      if (!this.operationIsCurrent(op)) return;
      if ((error as Error).name !== "AbortError") {
        op.rejected = !!(error as any).status;
        if (op.rejected) op.error = (error as Error).message;
        else op.phase = "recovering";
      }
    } finally {
      if (paintTimer) clearTimeout(paintTimer);
      if (this.operationIsCurrent(op)) {
        op.transportActive = false;
        if (!op.conversation?.id) {
          this.operations.delete(op.key);
          if (this.route.page === "chat" && !this.route.id && this.draftKey === op.draftKey) {
            if (!this.draft) {
              this.draft = prompt;
              this.attachments.restore(attached);
            }
            this.error = op.error || (op.stopRequested ? "" : "The conversation could not be created. Please try again.");
            this.emit();
          }
        } else {
          if (op.error && this.route.id === op.conversation.id) this.error = op.error;
          await this.reconcileConversation(op.conversation.id);
        }
      }
    }
  }
  /** One event of a reply's stream, applied to the reply being written. */
  private applyEvent(op: Operation, c: Conversation, assistant: Message, event: StreamEvent) {
    const data = event.data || {};
    switch (event.event) {
      case "meta":
        assistant._began ||= new Date().toISOString();
        op.accepted = true;
        op.phase = "generating";
        op.generationId = data.generationId;
        c.activeGenerationId = op.generationId;
        this.scheduleWatch(c.id);
        Object.assign(assistant, { id: data.messageId, requestId: data.requestId, settings: { ...data.settings, resolvedModel: data.resolvedModel } });
        if (data.userMessageId && op.userMessage) op.userMessage.id = data.userMessageId;
        c.settings = data.settings || c.settings;
        if (this.route.id === c.id) this.emit();
        if (op.stopRequested) this.requestStop(op);
        break;
      // Auto: the gateway says which model it chose once the reply starts.
      case "model":
        assistant.settings = { ...assistant.settings, resolvedModel: data.model, auto: true };
        break;
      case "delta":
        assistant.content += data.content || "";
        assistant.status = "streaming";
        break;
      // Chat took tool calls the model wrote as text out of the reply.
      case "content":
        assistant.content = data.content || "";
        break;
      case "reasoning":
        assistant.reasoningContent = (assistant.reasoningContent || "") + (data.content || "");
        assistant.status = "streaming";
        assistant._draft = null;
        break;
      case "tool": {
        const steps = (assistant.activity ||= []);
        const index = steps.findIndex((s) => s.id === data.id);
        if (index >= 0) steps[index] = data;
        else steps.push(data);
        assistant.activity = [...steps];
        break;
      }
      case "draft":
        assistant._draft = data;
        break;
      case "done":
        Object.assign(assistant, data);
        assistant.status = data.status || "completed";
        op.terminal = true;
        if (data.titlePending) for (const wait of [3000, 9000]) setTimeout(() => this.reconcileConversation(c.id), wait);
        // Memory reads the exchange in the background; its card joins the reply when done.
        if (data.memoryPending) for (const wait of [5000, 12000, 25000]) setTimeout(() => this.reconcileConversation(c.id), wait);
        break;
      case "error":
        assistant.status = "error";
        assistant.errorMessage = data.message || "The model could not complete this response.";
        op.terminal = true;
        break;
    }
    this.browser.observe(event, c.id);
    this.devices.observe(event, c.id);
    if (this.workspace.onEvent(event, assistant, c.id) && !op.terminal) assistant.status = "streaming";
  }

  // ---- Steer ----
  async steer(text: string) {
    const id = this.route.id;
    text = text.trim();
    if (!text || !id) return;
    await this.options.api.request(`${chatPath(id)}/steer`, { method: "POST", body: { text } });
    const list = this.queuedNotes.get(id) || [];
    list.push(text);
    this.queuedNotes.set(id, list);
    this.toast("Sent. The model reads it at its next step.");
    this.emit();
  }
  /** Notes sent with Steer that this reply hasn't read yet. */
  queued(id: string, m: Message) {
    return (this.queuedNotes.get(id) || []).filter((n) => !(m.activity || []).some((e) => e.kind === "note" && e.summary === n));
  }
  /** A finished reply never read some notes: give them back to the composer. */
  private unreadNotes(id: string, m: Message) {
    const list = this.queuedNotes.get(id);
    if (!list?.length || isLive(m)) return [];
    const read = new Set((m.activity || []).filter((e) => e.kind === "note").map((e) => e.summary));
    this.queuedNotes.delete(id);
    return list.filter((n) => !read.has(n));
  }
  /** Allow, allow for the chat, or deny a step waiting for approval. */
  async decide(callId: string, choice: "allow" | "chat" | "deny") {
    const id = this.route.id,
      remember = choice === "chat",
      decision = remember ? "allow" : choice;
    try {
      await this.options.api.request(`${chatPath(id)}/approvals`, { method: "POST", body: { callId, decision, remember } });
    } catch (e) {
      this.toast((e as Error).message);
    }
    const waiting = (this.active?.messages || []).flatMap((m) => m.activity || []).find((s) => s.id === callId && s.status === "awaiting");
    if (waiting) waiting.status = decision === "allow" ? "running" : "declined";
    this.emit();
  }

  // ---- Chats ----
  newChat() {
    this.draftKey++;
    this.draft = "";
    this.workspaceDraft = false;
    this.browserDraft = false;
    resetToolsDraft();
    chatDraft.reset();
    this.code.resetDraft();
    this.attachments.clear();
    this.applyPreferences(this.preferences);
    this.dialog = null;
    this.emit();
  }
  /** A starter on the home screen: its prompt in the box, and a workspace for building. */
  starter(prompt: string, workspace: boolean) {
    this.draft = prompt;
    if (workspace && !this.route.id) this.workspaceDraft = true;
    this.emit();
  }
  /** Typing in the box: kept without redrawing the page. */
  setDraft(text: string) {
    this.draftText = text;
  }
  async rename(id: string, title: string) {
    const c = unwrap(await this.options.api.request(chatPath(id), { method: "PATCH", body: { title } }));
    if (this.active?.id === id) this.active.title = c.title || title;
    this.options.chats.refresh();
    this.dialog = null;
    this.emit();
  }
  async remove(id: string) {
    await this.options.api.request(chatPath(id), { method: "DELETE" });
    this.options.chats.remove(id);
    this.dialog = null;
    if (this.active?.id === id) {
      this.active = null;
      this.options.go("/");
    }
    this.emit();
    this.toast("Conversation deleted");
  }
  /** Pin or archive: PATCH the chat, then show the change everywhere. */
  async update(id: string, body: Record<string, unknown>) {
    const c = unwrap(await this.options.api.request(chatPath(id), { method: "PATCH", body }));
    if (this.active?.id === id) Object.assign(this.active, { ...c, messages: this.active.messages });
    await this.options.chats.refresh();
    this.emit();
  }
  /** Who can see the chat changed in the share dialog. */
  shareChanged(id: string, scope: string) {
    if (this.active?.id === id) {
      this.active.shareScope = scope;
      this.emit();
    }
  }
  async branch(index: number) {
    const source = this.active;
    if (!source) return;
    const c = unwrap(await this.options.api.request(`${chatPath(source.id)}/branch`, { method: "POST", body: { throughMessageId: source.messages[index]!.id } }));
    this.upsert(c);
    this.options.go(`/chat/${encodeURIComponent(c.id)}`);
    this.toast("Created a separate conversation branch");
  }
  async alternatives() {
    if (!this.active) return;
    const r = await this.options.api.request<any>(`${chatPath(this.active.id)}/branches`);
    this.openDialog({ kind: "alternatives", conversations: r.conversations || [] });
  }
  openDialog(dialog: Dialog | null) {
    this.dialog = dialog;
    this.emit();
  }
  /** Copies text, or shows it to copy by hand when the clipboard is unavailable. */
  async copy(text: string) {
    try {
      await navigator.clipboard.writeText(text);
      this.toast("Copied to clipboard");
    } catch {
      this.openDialog({ kind: "copy", text });
    }
  }
  /** Opens the Library's save dialog for part of the chat on screen, linked to its message; `projectId` is the chat's project. */
  saveFromChat(item: SaveItem, messageIndex?: number, projectId = "") {
    if (!this.feature("knowledge") || !this.active) return;
    if (this.active.temporary) {
      this.toast("Nothing in a temporary chat can be saved to the Library.");
      return;
    }
    const m = messageIndex == null ? undefined : this.active.messages[messageIndex];
    // From someone else's chat it is saved as the viewer's own, without a link back.
    if (this.shared) this.openDialog({ kind: "save", item });
    else this.openDialog({ kind: "save", item: { ...item, conversationId: this.active.id, messageId: m?.id, projectId } });
  }
  /** The selection bar's Ask about this: the text quoted at the top of the message. */
  quote(text: string) {
    this.draft = `${text
      .split("\n")
      .map((l) => `> ${l}`)
      .join("\n")}\n\n${this.draft}`;
    this.emit();
  }
}

const sharedSignatureOf = (c: Conversation) =>
  JSON.stringify([c.title, (c.messages || []).map((m) => [m.id, m.status, m.content?.length, m.reasoningContent?.length, (m.activity || []).length])]);

// The shapes the chat pages read from Chat's API (api/src). Fields the server may
// add later stay reachable through the index signatures.

export type Effort = "none" | "low" | "medium" | "high";

export interface Attachment {
  id: string;
  name: string;
  size: number;
  /** image, document (PDF, Word, PowerPoint, Excel), text or file (anything else). */
  kind: string;
  /** Pages, slides, sheets or the files in an archive. */
  parts?: number;
  /** A large document Chat searches instead of reading whole. */
  searched?: boolean;
  url?: string;
  createdAt?: string;
  [key: string]: unknown;
}

/** One entry of a reply's activity: a tool step, a thought, a card the model asked for. */
export interface ActivityEntry {
  id?: string;
  /** Missing for tool steps; thought, note, checkpoint, sources, saved, question, workspace, research, app, browser-…. */
  kind?: string;
  tool?: string;
  status?: string;
  summary?: string;
  detail?: string;
  startedAt?: string;
  durationMs?: number;
  [key: string]: any;
}

export interface MessageSettings {
  model?: string;
  resolvedModel?: string;
  auto?: boolean;
  systemPrompt?: string;
  temperature?: number;
  topP?: number;
  maxTokens?: number;
  reasoningEffort?: string;
  [key: string]: unknown;
}

export interface Usage {
  prompt_tokens?: number;
  completion_tokens?: number;
  total_tokens?: number;
  last_prompt_tokens?: number;
  last_completion_tokens?: number;
  steps?: number;
  generation_ms?: number;
  first_token_ms?: number;
  energy_wh?: number;
  co2_g?: number;
  [key: string]: unknown;
}

export interface Feedback {
  rating: number;
  reasons?: string[];
  comment?: string;
}

/** The file a tool call is writing, as the reply streams it. */
export interface Draft {
  id?: string;
  path?: string;
  lines: number;
  tail: string;
  [key: string]: unknown;
}

export interface Message {
  id?: string;
  role: "user" | "assistant" | "system" | string;
  content: string;
  reasoningContent?: string;
  status?: string;
  attachments?: Attachment[];
  activity?: ActivityEntry[];
  settings?: MessageSettings;
  usage?: Usage;
  finishReason?: string | null;
  errorMessage?: string;
  requestId?: string;
  createdAt?: string;
  versions?: { id: string; [key: string]: unknown }[];
  versionIndex?: number;
  followUps?: string[];
  followUpsStatus?: string;
  feedback?: Feedback | null;
  /** Client only: when this reply started streaming here. */
  _began?: string;
  /** Client only: the file the model is writing right now. */
  _draft?: Draft | null;
  [key: string]: unknown;
}

export interface Conversation {
  id: string;
  title: string;
  model: string;
  reasoningEffort?: string;
  settings?: MessageSettings;
  messages: Message[];
  temporary?: boolean;
  archived?: boolean;
  pinned?: boolean;
  shareScope?: string;
  activeGenerationId?: string | null;
  generationCancelRequested?: boolean;
  createdAt?: string;
  updatedAt?: string;
  [key: string]: unknown;
}

export interface ModelInfo {
  id: string;
  name?: string;
  description?: string;
  detail?: string;
  tags?: string[];
  shortList?: boolean;
  isDefault?: boolean;
  lockedParameters?: string[];
  allowedOverrides?: string[];
  defaults?: Partial<Record<string, unknown>>;
  metadata?: { supportsVision?: boolean; contextWindow?: number; [key: string]: unknown };
  [key: string]: unknown;
}

export interface Preferences {
  defaultModel?: string;
  favorites?: string[];
  density?: string;
  context?: boolean;
  focus?: boolean;
  systemPrompt?: string;
  temperature?: number;
  topP?: number;
  maxTokens?: number;
  reasoningEffort?: string;
  [key: string]: unknown;
}

export interface Preset {
  id: string;
  name: string;
  model: string;
  parameters?: Record<string, unknown>;
  [key: string]: unknown;
}

/** Someone else's chat shared with the viewer (GET /api/shared/:id). */
export interface ShareInfo {
  scope: string;
  groupId?: string;
  groupName?: string;
  ownerName?: string;
  [key: string]: unknown;
}

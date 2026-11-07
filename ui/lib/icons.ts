// The icon names site.config.js may use for its greeting, assistant and loading
// icons, drawn with lucide.
import {
  Archive, ArrowRight, Bookmark, BookOpen, Box, ChartColumn, Check, ChevronDown, ChevronUp, CircleHelp, Clock,
  Code, Columns2, Copy, Download, Ellipsis, ExternalLink, File, FlaskConical, Folder, Gauge, Gauge as Meter,
  GitBranch, Globe, Grid2x2, Hand, Image, Info, Link, LockKeyhole, LogIn, MessageSquare, Monitor, Moon,
  MousePointer2, Orbit, PanelLeft, Paperclip, Pause, Pencil, Pin, Play, Plus, Power, RefreshCw, Search, Server,
  Settings, Shield, Square, Sun, Terminal, Trash2, Undo2, Upload, Users, X, Zap, type LucideIcon,
} from "lucide-react";

export const BRAND_ICONS: Record<string, LucideIcon> = {
  chat: MessageSquare, plus: Plus, arrow: ArrowRight, right: ArrowRight, down: ChevronDown, up: ChevronUp,
  search: Search, folder: Folder, book: BookOpen, flask: FlaskConical, terminal: Terminal, users: Users,
  settings: Settings, panel: PanelLeft, paperclip: Paperclip, image: Image, pin: Pin, archive: Archive,
  bookmark: Bookmark, gauge: Gauge, orbit: Orbit, edit: Pencil, trash: Trash2, link: Link, file: File,
  code: Code, cube: Box, server: Server, clock: Clock, check: Check, x: X, lock: LockKeyhole, copy: Copy,
  refresh: RefreshCw, download: Download, upload: Upload, more: Ellipsis, bolt: Zap, chart: ChartColumn,
  undo: Undo2, compare: Columns2, branch: GitBranch, play: Play, stop: Square, external: ExternalLink,
  shield: Shield, grid: Grid2x2, help: CircleHelp, globe: Globe, cursor: MousePointer2, hand: Hand,
  power: Power, pause: Pause, signin: LogIn, info: Info, sun: Sun, moon: Moon, monitor: Monitor, meter: Meter,
};
export const BRAND_ICON_NAMES: ReadonlySet<string> = new Set(Object.keys(BRAND_ICONS));

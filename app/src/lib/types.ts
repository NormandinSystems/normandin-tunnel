// Mirrors the serialized views of the Rust engine (crates/tunnel-core/src/events.rs).

export interface PeerInfo {
  name: string;
  device_type: "desktop" | "phone" | "tablet" | "server";
  app_version: string;
}

export interface ErrorView {
  code: string;
  message: string;
  shortfall?: number;
}

export type PathKind = "unknown" | "direct" | "relay";

export type SessionState =
  | { state: "idle" }
  | { state: "creating_code" }
  | { state: "showing_code"; code: string; uri: string; qr_svg: string; expires_at: number }
  | { state: "joining" }
  | { state: "securing" }
  | { state: "connecting" }
  | { state: "verify_safety"; safety_code: string; peer: PeerInfo; confirmed: boolean; peer_confirmed: boolean }
  | { state: "active"; peer: PeerInfo; path: PathKind }
  | { state: "reconnecting"; peer: PeerInfo }
  | { state: "ended"; reason: "local" | "peer" | "error"; error: ErrorView | null; peer: PeerInfo | null };

export type TransferState =
  | "preparing"
  | "offered"
  | "active"
  | "paused"
  | "done"
  | "declined"
  | "cancelled"
  | "failed";

export interface Issue {
  path: string;
  kind: "skipped" | "failed" | "renamed";
  reason: string;
  detail: string | null;
}

export interface TransferView {
  id: string;
  direction: "send" | "receive";
  state: TransferState;
  paused_by: "none" | "local" | "peer" | "disk_full";
  label: string;
  top_level: number;
  files_total: number;
  folders_total: number;
  bytes_total: number;
  bytes_done: number;
  files_done: number;
  files_skipped: number;
  files_failed: number;
  speed: number;
  eta_secs: number | null;
  folder: string | null;
  issues: Issue[];
  issues_truncated: boolean;
  error: ErrorView | null;
  started_at: number;
}

export type OfferWarning =
  | { kind: "executables"; count: number }
  | { kind: "double_extensions"; count: number }
  | { kind: "bidi_names"; count: number }
  | { kind: "renamed"; count: number }
  | { kind: "low_space" };

export interface OfferView {
  id: string;
  peer: PeerInfo;
  files: number;
  folders: number;
  bytes: number;
  names: string[];
  warnings: OfferWarning[];
  folder: string;
  shortfall: number;
  ask_where: boolean;
}

export interface TextView {
  id: number;
  from_peer: boolean;
  text: string;
  at: number;
}

export interface Snapshot {
  session: SessionState;
  offers: OfferView[];
  transfers: TransferView[];
  texts: TextView[];
}

export type TunnelEvent =
  | { type: "session"; session: SessionState }
  | { type: "offer"; offer: OfferView }
  | { type: "offer_closed"; id: string }
  | { type: "offer_blocked"; peer: PeerInfo }
  | { type: "transfer"; transfer: TransferView }
  | { type: "text"; message: TextView }
  | { type: "resync" };

export interface Settings {
  device_name: string;
  language: "system" | "en" | "fr";
  theme: "system" | "light" | "dark";
  notifications: boolean;
  onboarded: boolean;
  save_dir: string;
  ask_where_to_save: boolean;
  conflict_rule: "keep_both" | "replace" | "skip";
  keep_modified_times: boolean;
  keep_executable: boolean;
  mark_downloaded: boolean;
  history: "off" | "days7" | "days30" | "days90" | "forever";
  history_names: boolean;
  rendezvous_url: string;
  relay_urls: string[];
  max_entries: number;
  debug_logging: boolean;
}

export interface HistoryEntry {
  id: number;
  at: number;
  peer: string;
  direction: "send" | "receive";
  files: number;
  folders: number;
  bytes: number;
  result: "done" | "partial" | "cancelled" | "declined" | "failed";
  names: string[];
  folder: string | null;
}

export interface AppInfo {
  version: string;
  platform: string;
  data_dir: string;
  default_save_dir: string;
  system_language: "en" | "fr";
}

export interface CodeCheck {
  code: string;
  other_host: string | null;
}

/** Everything the UI can ask of the engine. */
export interface Backend {
  getState(): Promise<Snapshot>;
  getSettings(): Promise<Settings>;
  saveSettings(settings: Settings): Promise<Settings>;
  getHistory(): Promise<HistoryEntry[]>;
  clearHistory(): Promise<void>;
  appInfo(): Promise<AppInfo>;
  completeWord(prefix: string): Promise<string[]>;
  checkCode(input: string): Promise<CodeCheck>;
  createCode(): Promise<void>;
  joinCode(input: string, allowOtherHost: boolean): Promise<void>;
  confirmSafety(matches: boolean): Promise<void>;
  sendPaths(paths: string[]): Promise<string>;
  sendText(text: string): Promise<void>;
  respondOffer(id: string, accept: boolean, folder?: string): Promise<void>;
  pauseTransfer(id: string): Promise<void>;
  resumeTransfer(id: string): Promise<void>;
  cancelTransfer(id: string): Promise<void>;
  endSession(): Promise<void>;
  dismissSession(): Promise<void>;
  openFolder(path: string): Promise<void>;
  pickFiles(): Promise<string[]>;
  pickFolder(title?: string): Promise<string | null>;
  copyText(text: string): Promise<void>;
  onEvent(handler: (event: TunnelEvent) => void): Promise<() => void>;
  onDrop(handler: (drop: { kind: "over" | "leave" } | { kind: "drop"; paths: string[] }) => void): Promise<() => void>;
}

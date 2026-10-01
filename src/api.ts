import { invoke } from "@tauri-apps/api/core";

export type Mode = "prompt" | "dictation" | "answer";
export type Stage = "transcribing" | "researching" | "generating" | "revising" | "inserting";

export type Outcome =
  | { kind: "inserted"; text: string }
  | { kind: "answered"; text: string }
  | { kind: "blocked"; text: string; reason: "focus_changed" | "focus_unknown" | "output_truncated" | "insert_failed"; detail: string | null }
  | { kind: "no_speech" }
  | { kind: "cancelled" }
  | { kind: "failed"; reason: string; detail: string | null };

export interface JobReport {
  job_id: number;
  profile_id: string;
  outcome: Outcome;
  elapsed_ms: number;
  history_saved: boolean;
  structure: "unstructured" | "valid" | "repaired" | "kept_original" | null;
}

export type OverlayEvent =
  | { type: "listening"; mode: Mode; profile: string; target: string; latched: boolean }
  | { type: "level"; level: number }
  | { type: "partial"; text: string }
  | { type: "stage"; stage: Stage }
  | { type: "transcript"; text: string }
  | { type: "token"; text: string }
  | { type: "finished"; report: JobReport; capped: boolean }
  | { type: "error"; message: string }
  | { type: "cancelled" };

export interface Vocabulary {
  words: string[];
  replacements: { from: string; to: string }[];
}

export interface AppInfo {
  input_device: string | null;
  hotkeys: { prompt: string; dictation: string; answer: string | null; cancel: string };
  hotkey_errors: string[];
  engines_ready: boolean;
  history_enabled: boolean;
  data_dir: string;
  use_gpu: boolean;
  gpu_device: string | null;
  modifier_hold: boolean;
  auto_mode: boolean;
  vocabulary: Vocabulary;
  screen_text_apps: string[];
}

export interface ModelStatus {
  id: string;
  kind: "stt" | "llm";
  tier: "small" | "balanced" | "quality";
  display_name: string;
  size_bytes: number;
  license: string;
  min_ram_gb: number;
  installed: boolean;
  selected: boolean;
  downloading: boolean;
  partial_bytes: number;
}

export interface DownloadEvent {
  id: string;
  downloaded: number;
  total: number;
  verifying: boolean;
  done: boolean;
  error: string | null;
}

export interface HistoryEntry {
  id: number;
  created_ms: number;
  mode: Mode;
  profile_id: string;
  app_key: string;
  transcript: string;
  output: string;
  inserted: boolean;
}

export interface ProfileSummary {
  id: string;
  name: string;
  kind: string;
}

export interface ChatMessage {
  role: "system" | "user" | "assistant";
  content: string;
}

export interface PreviewOutput {
  profileId: string;
  profileName: string;
  messages: ChatMessage[];
}

export interface PreviewInput {
  transcript: string;
  processName: string;
  windowTitle: string;
  url: string | null;
  surroundingText: string | null;
}

export interface Device {
  id: string;
  name: string;
  public_key: string;
  paired_unix: number;
  last_seen_unix: number | null;
}

export type RelayStatus =
  | { state: "disabled" }
  | { state: "connecting" }
  | { state: "connected" }
  | { state: "error"; message: string };

export interface RemoteInfo {
  enabled: boolean;
  relay_url: string | null;
  lan_direct: boolean;
  lan_discovery: boolean;
  status: { relay: RelayStatus; listen: string | null; sessions: number; devices: number; pairing_expires_unix: number | null } | null;
  error: string | null;
  devices: Device[];
  api_token_path: string;
}

export interface OfferInfo {
  uri: string;
  svg: string;
  expires_unix: number;
}

export interface McpServerInfo {
  name: string;
  remote: boolean;
  enabled: boolean;
  profiles: string[];
  hooks: number;
  loop_tools: string[];
  transcript_allowed: boolean;
}

export interface McpInfo {
  path: string;
  error: string | null;
  active: boolean;
  servers: McpServerInfo[];
}

export interface McpFile {
  text: string;
  exists: boolean;
  template: string;
}

export const api = {
  appInfo: () => invoke<AppInfo>("app_info"),
  listProfiles: () => invoke<ProfileSummary[]>("list_profiles"),
  previewPrompt: (input: PreviewInput) => invoke<PreviewOutput>("preview_prompt", { input }),
  cleanDictation: (text: string) => invoke<string>("clean_dictation", { text }),
  copyLastResult: () => invoke<void>("copy_last_result"),
  hideOverlay: () => invoke<void>("hide_overlay"),
  listModels: () => invoke<ModelStatus[]>("list_models"),
  downloadModel: (id: string) => invoke<void>("download_model", { id }),
  cancelDownload: (id: string) => invoke<boolean>("cancel_download", { id }),
  deleteModel: (id: string) => invoke<void>("delete_model", { id }),
  selectModel: (id: string) => invoke<void>("select_model", { id }),
  listHistory: () => invoke<HistoryEntry[]>("list_history"),
  deleteHistoryEntry: (id: number) => invoke<boolean>("delete_history_entry", { id }),
  clearHistory: () => invoke<void>("clear_history"),
  setHistoryEnabled: (enabled: boolean) => invoke<void>("set_history_enabled", { enabled }),
  setHotkey: (mode: Mode, accelerator: string) => invoke<void>("set_hotkey", { mode, accelerator }),
  setUseGpu: (enabled: boolean) => invoke<void>("set_use_gpu", { enabled }),
  setModifierHold: (enabled: boolean) => invoke<void>("set_modifier_hold", { enabled }),
  setAutoMode: (enabled: boolean) => invoke<void>("set_auto_mode", { enabled }),
  setVocabulary: (vocabulary: Vocabulary) => invoke<Vocabulary>("set_vocabulary", { vocabulary }),
  setScreenTextApps: (apps: string[]) => invoke<void>("set_screen_text_apps", { apps }),
  remoteInfo: () => invoke<RemoteInfo>("remote_info"),
  setRemoteSettings: (enabled: boolean, relayUrl: string | null, lanDirect: boolean, lanDiscovery: boolean) =>
    invoke<void>("set_remote_settings", { enabled, relayUrl, lanDirect, lanDiscovery }),
  createPairingOffer: () => invoke<OfferInfo>("create_pairing_offer"),
  cancelPairing: () => invoke<void>("cancel_pairing"),
  removeDevice: (id: string) => invoke<boolean>("remove_device", { id }),
  renameDevice: (id: string, name: string) => invoke<boolean>("rename_device", { id, name }),
  mcpInfo: () => invoke<McpInfo>("mcp_info"),
  mcpReload: () => invoke<McpInfo>("mcp_reload"),
  mcpTest: (name: string) => invoke<string[]>("mcp_test", { name }),
  mcpRead: () => invoke<McpFile>("mcp_read"),
  mcpValidate: (text: string) => invoke<number>("mcp_validate", { text }),
  mcpSave: (text: string, original: string) => invoke<McpInfo>("mcp_save", { text, original }),
};

import { invoke } from "@tauri-apps/api/core";

export type Mode = "prompt" | "dictation";
export type Stage = "transcribing" | "generating" | "inserting";

export type Outcome =
  | { kind: "inserted"; text: string }
  | { kind: "blocked"; text: string; reason: "focus_changed" | "focus_unknown" | "output_truncated" | "insert_failed"; detail: string | null }
  | { kind: "no_speech" }
  | { kind: "cancelled" }
  | { kind: "failed"; reason: string; detail: string | null };

export interface JobReport {
  job_id: number;
  profile_id: string;
  outcome: Outcome;
  elapsed_ms: number;
}

export type OverlayEvent =
  | { type: "listening"; mode: Mode; profile: string; target: string; latched: boolean }
  | { type: "level"; level: number }
  | { type: "stage"; stage: Stage }
  | { type: "transcript"; text: string }
  | { type: "token"; text: string }
  | { type: "finished"; report: JobReport; capped: boolean }
  | { type: "error"; message: string }
  | { type: "cancelled" };

export interface AppInfo {
  input_device: string | null;
  hotkeys: { prompt: string; dictation: string; cancel: string };
  hotkey_errors: string[];
  engines_ready: boolean;
  history_enabled: boolean;
  data_dir: string;
  use_gpu: boolean;
  gpu_device: string | null;
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
};

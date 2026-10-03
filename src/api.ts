import { invoke } from "@tauri-apps/api/core";

export type Mode = "prompt" | "dictation" | "answer";
export type Stage = "transcribing" | "researching" | "generating" | "revising" | "inserting";
export type Rendering = "legacy" | "adaptive";
export type PromptForm = "graph" | "inline_graph";
export type PromptSurface =
  | "chat" | "code_chat" | "research_chat" | "source_chat" | "document_chat"
  | "spreadsheet_chat" | "sql_chat" | "builder_chat" | "presentation_chat"
  | "image_prompt" | "video_prompt" | "music_description" | "music_style"
  | "sound_prompt" | "voice_design" | "object_prompt" | "texture_prompt"
  | "search_query" | "literal" | "unknown";

export interface RoutingOptions {
  rendering: Rendering;
  task_type: string | null;
  surface: PromptSurface | null;
}

export interface RoutingState {
  rendering: Rendering;
  error: string | null;
}

export interface PromptType {
  id: string;
  family: string;
  label: string;
  inputs: string;
  priority: "P1" | "P2" | "P3" | "B";
  feasibility: "F1" | "F2" | "F3";
  status: "proposed" | "validated" | "enabled" | "retired";
  form: PromptForm;
  conversational: boolean;
}

export interface PromptCatalog {
  version: number;
  tasks: PromptType[];
  surfaces: PromptSurface[];
}

export interface PromptRouting {
  version: number;
  target_profile_id: string;
  target_name: string;
  surface: PromptSurface;
  task_type: string;
  task_label: string;
  secondary_tasks: string[];
  form: PromptForm;
  conversational: boolean;
  reason: "explicit" | "matched" | "surface_default" | "uncertain";
  auto_paste: boolean;
  warnings: string[];
  max_chars: number | null;
  newlines: "keep" | "collapse";
}

export type Outcome =
  | { kind: "inserted"; text: string }
  | { kind: "answered"; text: string }
  | { kind: "blocked"; text: string; reason: "focus_changed" | "focus_unknown" | "output_truncated" | "insert_failed" | "surface_unconfirmed" | "graph_unsupported"; detail: string | null }
  | { kind: "no_speech" }
  | { kind: "cancelled" }
  | { kind: "failed"; reason: string; detail: string | null };

export interface JobReport {
  job_id: number;
  profile_id: string;
  outcome: Outcome;
  elapsed_ms: number;
  history_saved: boolean;
  structure: "valid" | "repaired" | "kept_original" | null;
  routing?: PromptRouting | null;
}

export type OverlayEvent =
  | { type: "listening"; mode: Mode; profile: string; target: string; latched: boolean }
  | { type: "level"; level: number }
  | { type: "partial"; text: string }
  | { type: "stage"; stage: Stage }
  | { type: "transcript"; text: string }
  | { type: "token"; text: string }
  | { type: "routing"; routing: PromptRouting }
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
  prompt_hotkey_error: string | null;
  hotkeys_paused: boolean;
  engines_ready: boolean;
  models_installed: boolean;
  engine_error: string | null;
  history_enabled: boolean;
  data_dir: string;
  use_gpu: boolean;
  gpu_device: string | null;
  modifier_hold: boolean;
  modifier_hold_requested: boolean;
  modifier_hold_error: string | null;
  modifier_keyboard: string | null;
  modifier_keyboard_devices: { path: string; name: string }[];
  auto_mode: boolean;
  vocabulary: Vocabulary;
  screen_text_apps: string[];
  paste_permission: "not_needed" | "granted" | "required";
  desktop_error: string | null;
}

export type OnboardingStep = "models" | "input" | "practice";
export type EngineStatus = { state: "missing" | "loading" | "ready" } | { state: "error"; message: string };
export type PracticePhase = "armed" | "recording" | "processing" | "awaiting_paste" | "passed" | "failed";

export interface OnboardingStatus {
  required: boolean;
  step: OnboardingStep;
  speech: EngineStatus;
  language: EngineStatus;
  startup_error: string | null;
  practice: {
    phase: PracticePhase | null;
    attempt_id: number | null;
    job_id: number | null;
    error: string | null;
  };
}

export interface PracticeEvent {
  attempt_id: number;
  event: OverlayEvent;
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
  routing?: PromptRouting | null;
}

export interface PreviewInput {
  transcript: string;
  processName: string;
  windowTitle: string;
  url: string | null;
  surroundingText: string | null;
  routing?: RoutingOptions;
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
  mobile_available: boolean;
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
  grantPastePermission: () => invoke<void>("grant_paste_permission"),
  onboardingStatus: () => invoke<OnboardingStatus>("onboarding_status"),
  retryOnboardingStartup: () => invoke<void>("retry_onboarding_startup"),
  retryModelLoading: () => invoke<void>("retry_model_loading"),
  setOnboardingStep: (step: OnboardingStep) => invoke<OnboardingStatus>("set_onboarding_step", { step }),
  armOnboardingPractice: () => invoke<OnboardingStatus>("arm_onboarding_practice"),
  disarmOnboardingPractice: () => invoke<void>("disarm_onboarding_practice"),
  confirmOnboardingPaste: (attemptId: number, jobId: number, text: string) =>
    invoke<OnboardingStatus>("confirm_onboarding_paste", { attemptId, jobId, text }),
  completeOnboarding: () => invoke<OnboardingStatus>("complete_onboarding"),
  resumeOnboardingHotkeys: () => invoke<OnboardingStatus>("resume_onboarding_hotkeys"),
  listProfiles: () => invoke<ProfileSummary[]>("list_profiles"),
  routingState: () => invoke<RoutingState>("routing_state"),
  setRendering: (rendering: Rendering) => invoke<RoutingState>("set_rendering", { rendering }),
  promptCatalog: () => invoke<PromptCatalog>("prompt_catalog"),
  queuePromptRouting: (options: RoutingOptions) => invoke<void>("queue_prompt_routing", { options }),
  previewPrompt: (input: PreviewInput) => invoke<PreviewOutput>("preview_prompt", { input }),
  cleanDictation: (text: string) => invoke<string>("clean_dictation", { text }),
  copyLastResult: () => invoke<void>("copy_last_result"),
  hideOverlay: () => invoke<void>("hide_overlay"),
  resizeOverlay: (height: number) => invoke<void>("resize_overlay", { height }),
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
  setModifierKeyboard: (path: string) => invoke<void>("set_modifier_keyboard", { path }),
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

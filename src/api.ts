import { invoke } from "@tauri-apps/api/core";

export type Mode = "prompt" | "dictation";
export type Stage = "transcribing" | "generating" | "revising" | "reviewing_quality" | "improving_wording" | "inserting";
export type DictationTone = "clean_transcript" | "natural" | "casual" | "formal" | "concise" | "unhinged";
export type QualityStatus = "checked" | "corrected" | "rejected" | "unavailable";
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
  | { kind: "blocked"; text: string; reason: "focus_changed" | "focus_unknown" | "output_truncated" | "insert_failed" | "surface_unconfirmed" | "graph_unsupported" | "quality_review"; detail: string | null }
  | { kind: "no_speech" }
  | { kind: "cancelled" }
  | { kind: "failed"; reason: string; detail: string | null };

export interface JobReport {
  delivery?: "sent_unverified" | null;
  job_id: number;
  profile_id: string;
  outcome: Outcome;
  elapsed_ms: number;
  history_saved: boolean;
  structure: "valid" | "repaired" | "kept_original" | null;
  generation_elapsed_ms: number;
  structure_repair_attempts: number;
  quality?: { status: QualityStatus; review_calls: number; rewrite_calls: number; generation_elapsed_ms: number; review_elapsed_ms: number; rewrite_elapsed_ms: number; deadline_exhausted: boolean } | null;
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
  desktop_integration_enabled?: boolean;
  activation_bindings?: { id: string; trigger_description: string }[];
  input_device: string | null;
  hotkeys: { prompt: string; dictation: string; cancel: string };
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
  dictation_tone: DictationTone;
  code_chat_paste: boolean;
  vocabulary: Vocabulary;
  screen_text_apps: string[];
  paste_permission: "not_needed" | "granted" | "required";
  desktop_error: string | null;
  clipboard_restore_pending?: boolean;
}

export interface UpdateInfo {
  current_version: string;
  check_on_startup: boolean;
  checking: boolean;
  checked: boolean;
  release: {
    version: string;
    url: string;
    update_available: boolean;
    install_error: string | null;
  } | null;
  error: string | null;
  installation: "idle" | "downloading" | "installing" | "installed" | "installer_started" | "failed";
  installation_error: string | null;
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
  compatibility: {
    supported: boolean | null;
    total_ram_bytes: number | null;
    reason: string | null;
  };
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

export const api = {
  openRecoverySettings: (section: "general" | "models" | "prompts") => invoke<void>("open_recovery_settings", { section }),
  openDataFolder: () => invoke<void>("open_data_folder"),
  openSystemSettings: (section: "microphone" | "accessibility" | "input_monitoring") => invoke<void>("open_system_settings", { section }),
  updateInfo: () => invoke<UpdateInfo>("update_info"),
  checkForUpdates: () => invoke<UpdateInfo>("check_for_updates"),
  setUpdateChecks: (enabled: boolean) => invoke<UpdateInfo>("set_update_checks", { enabled }),
  installUpdate: () => invoke<UpdateInfo>("install_update"),
  restartAfterUpdate: () => invoke<void>("restart_after_update"),
  openUpdateRelease: () => invoke<void>("open_update_release"),
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
  routingState: () => invoke<RoutingState>("routing_state"),
  setRendering: (rendering: Rendering) => invoke<RoutingState>("set_rendering", { rendering }),
  resetRouting: () => invoke<RoutingState>("reset_routing"),
  retryFocusDetection: () => invoke<void>("retry_focus_detection"),
  promptCatalog: () => invoke<PromptCatalog>("prompt_catalog"),
  queuePromptRouting: (options: RoutingOptions) => invoke<void>("queue_prompt_routing", { options }),
  cleanDictation: (text: string) => invoke<string>("clean_dictation", { text }),
  copyLastResult: () => invoke<void>("copy_last_result"),
  hideOverlay: () => invoke<void>("hide_overlay"),
  resizeOverlay: (height: number) => invoke<void>("resize_overlay", { height }),
  overlayMaxHeight: () => invoke<number>("overlay_max_height"),
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
  setDictationTone: (tone: DictationTone) => invoke<void>("set_dictation_tone", { tone }),
  setCodeChatPaste: (enabled: boolean) => invoke<void>("set_code_chat_paste", { enabled }),
  disableDesktopIntegration: () => invoke<void>("disable_desktop_integration"),
  restoreInsertionClipboard: () => invoke<void>("restore_insertion_clipboard"),
  setVocabulary: (vocabulary: Vocabulary) => invoke<Vocabulary>("set_vocabulary", { vocabulary }),
  setScreenTextApps: (apps: string[]) => invoke<void>("set_screen_text_apps", { apps }),
};

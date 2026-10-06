import { expect, test, type Page } from "@playwright/test";
import type { AppInfo, InferenceConfig, InferenceConnectionInput, InferenceStatus, ModelStatus, OnboardingStatus, OverlayEvent, PromptCatalog, Rendering, UpdateInfo } from "../src/api";

interface Fixture {
  info: AppInfo;
  status: OnboardingStatus;
  models: ModelStatus[];
  failures: Record<string, string>;
  delays?: Record<string, number>;
  catalog: PromptCatalog;
  routingError?: string | null;
  updates: UpdateInfo;
  inference: InferenceConfig;
  inferenceStatus: InferenceStatus;
  inferenceTestError?: string;
}

interface TestBridge {
  state: Fixture;
  calls: string[];
  change: (update: Partial<Fixture>) => void;
  emit: (event: string, payload: unknown) => void;
  practice: (event: OverlayEvent, attempt?: number) => void;
  lastRouting: unknown;
  overlaySizes: number[];
  inferenceWrites: { input: Omit<InferenceConnectionInput, "secret">; hasSecret: boolean }[];
}

declare global {
  interface Window {
    onboardingTest: TestBridge;
    resizeTestOverlay?: (height: number) => Promise<void>;
  }
}

function fixture(ready = false): Fixture {
  return {
    info: {
      input_device: "Test microphone",
      hotkeys: { prompt: "CommandOrControl+Alt+Space", dictation: "CommandOrControl+Alt+Shift+Space", cancel: "Escape" },
      hotkey_errors: [], prompt_hotkey_error: null, hotkeys_paused: false,
      engines_ready: ready, models_installed: ready, engine_error: null,
      history_enabled: true, data_dir: "isolated-test-data", use_gpu: true, gpu_device: null,
      dictation_tone: "natural",
      modifier_hold: false, auto_mode: false, vocabulary: { words: [], replacements: [] }, screen_text_apps: [],
      code_chat_paste: false,
      modifier_hold_error: null,
      modifier_hold_requested: false,
      modifier_keyboard: null,
      modifier_keyboard_devices: [],
      paste_permission: "not_needed",
      desktop_error: null,
      desktop_integration_enabled: true,
      activation_bindings: [],
    },
    status: {
      required: true, step: "models",
      speech: { state: ready ? "ready" : "missing" }, language: { state: ready ? "ready" : "missing" },
      startup_error: null, practice: { phase: null, attempt_id: null, job_id: null, error: null },
    },
    models: [
      { id: "whisper-small-en", kind: "stt", tier: "balanced", display_name: "Whisper small English", size_bytes: 487614201, license: "mit", min_ram_gb: 4, compatibility: { supported: true, total_ram_bytes: 8_000_000_000, reason: null }, installed: ready, selected: ready, downloading: false, partial_bytes: 0 },
      { id: "qwen3.5-4b-q4km", kind: "llm", tier: "balanced", display_name: "Qwen3.5 4B", size_bytes: 3013027808, license: "apache-2.0", min_ram_gb: 8, compatibility: { supported: true, total_ram_bytes: 8_000_000_000, reason: null }, installed: ready, selected: ready, downloading: false, partial_bytes: 0 },
    ],
    failures: {},
    inference: { version: 1, revision: 0, selection: { kind: "bundled_local" }, connections: [] },
    inferenceStatus: { state: ready ? "ready" : "configured", selection: { kind: "bundled_local" }, revision: 0, message: null },
    updates: {
      current_version: "1.1.1", check_on_startup: true, checking: false, checked: true,
      release: { version: "1.1.1", url: "https://github.com/jrynks/promptify/releases/tag/v1.1.1", update_available: false, install_error: null },
      error: null, installation: "idle", installation_error: null,
    },
    catalog: {
      version: 1,
      tasks: [
        { id: "code.debug", family: "Software development", label: "Diagnose and fix a defect", inputs: "Symptoms and expected behavior", priority: "P1", feasibility: "F1", status: "enabled", form: "graph", conversational: false },
        { id: "communication.email", family: "Communication", label: "Draft an email", inputs: "Recipient, purpose, facts", priority: "P1", feasibility: "F1", status: "enabled", form: "graph", conversational: false },
        { id: "audio.song", family: "Music", label: "Describe a song", inputs: "Genre and instrumentation", priority: "P2", feasibility: "F2", status: "proposed", form: "graph", conversational: false },
      ],
      surfaces: ["chat", "code_chat", "spreadsheet_chat", "image_prompt", "literal"],
    },
  };
}

async function launch(page: Page, state = fixture(), path = "/") {
  if (path === "/overlay.html") {
    // Playwright changes screen metrics with setViewportSize; native window resizing does not
    // change the monitor's available height.
    await page.addInitScript(() => Object.defineProperty(window.screen, "availHeight", { value: 1080, configurable: true }));
    await page.exposeFunction("resizeTestOverlay", async (height: number) => {
      await page.setViewportSize({ width: 560, height: Math.max(64, Math.min(1064, height)) });
    });
  }
  await page.addInitScript((initial: Fixture) => {
    const callbacks = new Map<number, (value: unknown) => void>();
    const listeners = new Map<number, { event: string; handler: number }>();
    let sequence = 0;
    let nextAttempt = 0;
    let heard = false;
    let generated = false;
    let expectedPaste: string | null = null;
    let rendering: Rendering = sessionStorage.getItem("test.routing") === "adaptive" ? "adaptive" : "legacy";
    const savedStep = sessionStorage.getItem("test.setup.step");
    if (savedStep === "models" || savedStep === "input" || savedStep === "practice") initial.status.step = savedStep;
    if (sessionStorage.getItem("test.setup.complete") === "true") initial.status.required = false;
    const integrationEnabled = sessionStorage.getItem("test.desktop.integration");
    if (integrationEnabled !== null) initial.info.desktop_integration_enabled = integrationEnabled === "true";
    initial.info.code_chat_paste = sessionStorage.getItem("test.code.chat.paste") === "true";
    initial.info.modifier_keyboard = sessionStorage.getItem("test.modifier.keyboard") ?? initial.info.modifier_keyboard;
    for (const model of initial.models) {
      if (sessionStorage.getItem(`test.model.${model.id}`) === "true") model.installed = model.selected = true;
    }
    if (initial.models.every((m) => m.installed) && !initial.info.engine_error) {
      initial.info.engines_ready = initial.info.models_installed = true;
      initial.status.speech = initial.status.language = { state: "ready" };
    }
    const persist = () => {
      sessionStorage.setItem("test.setup.step", initial.status.step);
      sessionStorage.setItem("test.setup.complete", String(!initial.status.required));
      sessionStorage.setItem("test.desktop.integration", String(initial.info.desktop_integration_enabled));
      if (initial.info.modifier_keyboard) sessionStorage.setItem("test.modifier.keyboard", initial.info.modifier_keyboard);
      for (const model of initial.models) sessionStorage.setItem(`test.model.${model.id}`, String(model.installed));
    };
    const emit = (event: string, payload: unknown) => {
      for (const [id, listener] of listeners) {
        if (listener.event === event) callbacks.get(listener.handler)?.({ event, id, payload });
      }
    };
    const changed = () => { persist(); emit("onboarding-changed", null); };
    const ready = () => {
      initial.info.engines_ready = initial.info.models_installed = true;
      initial.info.engine_error = null;
      initial.status.speech = initial.status.language = { state: "ready" };
    };
    window.onboardingTest = {
      state: initial, calls: [], emit, lastRouting: null, overlaySizes: [], inferenceWrites: [],
      change: (update) => { Object.assign(initial, update); changed(); },
      practice: (event, id = initial.status.practice.attempt_id ?? -1) => {
        const practice = initial.status.practice;
        if (id === practice.attempt_id) {
          if (event.type === "transcript") heard ||= event.text.trim().length > 0;
          if (event.type === "stage") { generated ||= event.stage === "generating"; practice.phase = "processing"; }
          if (event.type === "finished") {
            practice.job_id = event.report.job_id;
            if (event.report.outcome.kind === "inserted" && heard && generated) {
              practice.phase = "awaiting_paste";
              expectedPaste = event.report.outcome.text;
            } else {
              practice.phase = "failed";
              practice.error = "No successful prompt insertion. Check your input and try again.";
            }
          }
          if (event.type === "error" || event.type === "cancelled") {
            practice.phase = "failed";
            practice.error = event.type === "error" ? event.message : "Practice cancelled.";
          }
        }
        emit("onboarding-practice", { attempt_id: id, event });
        changed();
      },
    };
    Object.defineProperty(window, "__TAURI_EVENT_PLUGIN_INTERNALS__", {
      value: { unregisterListener: (_event: string, id: number) => { listeners.delete(id); } },
    });
    Object.defineProperty(window, "__TAURI_INTERNALS__", {
      value: {
        transformCallback: (callback: (value: unknown) => void) => { const id = ++sequence; callbacks.set(id, callback); return id; },
        invoke: async (command: string, args: Record<string, unknown> = {}) => {
          window.onboardingTest.calls.push(command);
          if (initial.delays?.[command]) await new Promise((resolve) => window.setTimeout(resolve, initial.delays?.[command]));
          if (initial.failures[command]) throw new Error(initial.failures[command]);
          switch (command) {
            case "plugin:event|listen": {
              if (typeof args.event !== "string" || typeof args.handler !== "number") throw new Error("Invalid listener");
              if (initial.failures[`listen:${args.event}`]) throw new Error(initial.failures[`listen:${args.event}`]);
              const id = ++sequence;
              listeners.set(id, { event: args.event, handler: args.handler });
              return id;
            }
            case "plugin:event|unlisten": listeners.delete(Number(args.eventId)); return;
            case "app_info": return structuredClone(initial.info);
            case "inference_config": return structuredClone(initial.inference);
            case "inference_status": return structuredClone(initial.inferenceStatus);
            case "save_inference_connection": {
              const { secret, ...input } = args.input as InferenceConnectionInput;
              const old = initial.inference.connections.find((connection) => connection.id === input.id);
              window.onboardingTest.inferenceWrites.push({ input: structuredClone(input), hasSecret: !!secret });
              const connection = {
                id: input.id, name: input.name, provider: input.provider, base_url: input.base_url,
                protocol: input.protocol, auth: input.auth, model: input.model, stream: input.stream,
                allow_insecure_lan: input.allow_insecure_lan, consent_remote: input.consent_remote,
                credential_present: input.auth === "api_key" && !input.remove_secret && (!!secret || !!old?.credential_present),
                revision: ++initial.inference.revision,
              };
              initial.inference.connections = [...initial.inference.connections.filter((item) => item.id !== connection.id), connection];
              initial.inferenceStatus = { state: "configured", selection: initial.inference.selection, revision: initial.inference.revision, message: null };
              emit("inference-changed", null);
              return structuredClone(initial.inference);
            }
            case "select_inference":
              initial.inference.selection = args.selection as InferenceConfig["selection"];
              initial.inferenceStatus = { state: "configured", selection: initial.inference.selection, revision: ++initial.inference.revision, message: null };
              if (initial.inference.selection.kind === "connection") {
                initial.info.engines_ready = false;
                initial.status.language = { state: "missing" };
              } else if (initial.models.every((model) => model.installed)) ready();
              emit("inference-changed", null); changed();
              return structuredClone(initial.inference);
            case "discover_inference_models": return [{ id: "discovered-text-model", name: "Discovered text model" }];
            case "test_inference_connection":
              if (initial.inferenceTestError) {
                initial.inferenceStatus = { state: "error", selection: initial.inference.selection, revision: initial.inference.revision, message: initial.inferenceTestError };
                emit("inference-changed", null);
                return structuredClone(initial.inferenceStatus);
              }
              initial.inferenceStatus = { state: "ready", selection: initial.inference.selection, revision: initial.inference.revision, message: null };
              if (initial.inference.selection.kind === "connection" && initial.inference.selection.connection_id === args.id && initial.inference.selection.model === args.model) {
                initial.status.language = { state: "ready" };
                initial.info.engines_ready = initial.status.speech.state === "ready";
              }
              emit("inference-changed", null); changed();
              return structuredClone(initial.inferenceStatus);
            case "remove_inference_connection":
              if (initial.inference.selection.kind === "connection" && initial.inference.selection.connection_id === args.id) throw new Error("Select a replacement first");
              initial.inference.connections = initial.inference.connections.filter((connection) => connection.id !== args.id);
              emit("inference-changed", null);
              return structuredClone(initial.inference);
            case "reset_inference":
              initial.inference = { version: 1, revision: initial.inference.revision + 1, selection: { kind: "bundled_local" }, connections: [] };
              initial.inferenceStatus = { state: "configured", selection: initial.inference.selection, revision: initial.inference.revision, message: null };
              emit("inference-changed", null);
              return structuredClone(initial.inference);
            case "retry_focus_detection":
              initial.info.desktop_error = null;
              changed();
              return;
            case "update_info": return structuredClone(initial.updates);
            case "check_for_updates":
              emit("updates-changed", null);
              return structuredClone(initial.updates);
            case "set_update_checks":
              initial.updates.check_on_startup = args.enabled === true;
              emit("updates-changed", null);
              return structuredClone(initial.updates);
            case "install_update":
              initial.updates.installation = "installed";
              emit("updates-changed", null);
              return structuredClone(initial.updates);
            case "open_update_release":
            case "open_data_folder":
            case "open_system_settings":
            case "restart_after_update": return;
            case "open_recovery_settings":
              emit("open-settings-section", args.section);
              return;
            case "grant_paste_permission":
              initial.info.paste_permission = "granted";
              initial.info.desktop_integration_enabled = true;
              initial.info.hotkey_errors = [];
              initial.info.prompt_hotkey_error = null;
              changed();
              return;
            case "disable_desktop_integration":
              initial.info.desktop_integration_enabled = false;
              initial.info.activation_bindings = [];
              changed();
              return;
            case "restore_insertion_clipboard":
              initial.info.clipboard_restore_pending = false;
              changed();
              return;
            case "set_code_chat_paste":
              initial.info.code_chat_paste = args.enabled === true;
              sessionStorage.setItem("test.code.chat.paste", String(initial.info.code_chat_paste));
              changed();
              return;
            case "set_modifier_hold":
              initial.info.modifier_hold = args.enabled === true;
              initial.info.modifier_hold_requested = args.enabled === true;
              initial.info.modifier_hold_error = null;
              changed();
              return;
            case "set_modifier_keyboard":
              initial.info.modifier_keyboard = String(args.path);
              changed();
              return;
            case "onboarding_status": return structuredClone(initial.status);
            case "list_models": return structuredClone(initial.models);
            case "list_history": return [];
            case "set_vocabulary":
              initial.info.vocabulary = args.vocabulary as AppInfo["vocabulary"];
              return structuredClone(initial.info.vocabulary);
            case "set_screen_text_apps":
              initial.info.screen_text_apps = args.apps as string[];
              return;
            case "resize_overlay":
              if (typeof args.height !== "number" || !Number.isInteger(args.height) || args.height <= 0) throw new Error("Invalid overlay size");
              window.onboardingTest.overlaySizes.push(args.height);
              if (window.resizeTestOverlay) await window.resizeTestOverlay(args.height);
              return;
            case "overlay_max_height": return 1064;
            case "copy_last_result":
            case "hide_overlay": return;
            case "routing_state": return { rendering, error: initial.routingError ?? null };
            case "prompt_catalog": return structuredClone(initial.catalog);
            case "set_rendering":
              if (initial.routingError) throw new Error(initial.routingError);
              if (args.rendering !== "legacy" && args.rendering !== "adaptive") throw new Error("Invalid rendering");
              rendering = args.rendering;
              sessionStorage.setItem("test.routing", rendering);
              return { rendering, error: null };
            case "reset_routing":
              initial.routingError = null;
              rendering = "legacy";
              sessionStorage.setItem("test.routing", rendering);
              return { rendering, error: null };
            case "queue_prompt_routing":
              window.onboardingTest.lastRouting = structuredClone(args.options);
              return;
            case "download_model":
            case "select_model": {
              const model = initial.models.find((m) => m.id === args.id);
              if (!model) throw new Error("Unknown model");
              model.installed = model.selected = true;
              model.downloading = false;
              if (initial.models.every((m) => m.installed)) ready();
              emit("model-download", { id: model.id, downloaded: model.size_bytes, total: model.size_bytes, verifying: false, done: true, error: null });
              changed();
              return;
            }
            case "cancel_download": {
              const model = initial.models.find((m) => m.id === args.id);
              if (!model) throw new Error("Unknown model");
              model.downloading = false;
              model.partial_bytes = 123_000_000;
              emit("model-download", { id: model.id, downloaded: model.partial_bytes, total: model.size_bytes, verifying: false, done: true, error: "cancelled" });
              changed();
              return true;
            }
            case "retry_model_loading": ready(); changed(); return;
            case "set_use_gpu": initial.info.use_gpu = args.enabled === true; ready(); changed(); return;
            case "set_hotkey":
              if (typeof args.accelerator !== "string") throw new Error("Invalid shortcut");
              initial.info.hotkeys.prompt = args.accelerator;
              initial.info.prompt_hotkey_error = null;
              initial.info.hotkey_errors = [];
              changed();
              return;
            case "resume_onboarding_hotkeys": initial.info.hotkeys_paused = false; changed(); return structuredClone(initial.status);
            case "set_onboarding_step": {
              const step = args.step;
              if (step !== "models" && step !== "input" && step !== "practice") throw new Error("Invalid step");
              if (step !== "models" && !initial.info.engines_ready) throw new Error("Models are not ready");
              initial.status.step = step;
              initial.status.practice = { phase: null, attempt_id: null, job_id: null, error: null };
              changed();
              return structuredClone(initial.status);
            }
            case "arm_onboarding_practice":
              if (document.activeElement?.id !== "setup-practice") throw new Error("Focus the practice field");
              heard = generated = false;
              expectedPaste = null;
              initial.status.practice = { phase: "armed", attempt_id: ++nextAttempt, job_id: null, error: null };
              changed();
              return structuredClone(initial.status);
            case "disarm_onboarding_practice":
              if (initial.status.practice.phase !== "passed") {
                initial.status.practice = { phase: null, attempt_id: null, job_id: null, error: null };
                changed();
              }
              return;
            case "confirm_onboarding_paste":
              if (args.attemptId !== initial.status.practice.attempt_id || args.jobId !== initial.status.practice.job_id || args.text !== expectedPaste || initial.status.practice.phase !== "awaiting_paste") throw new Error("No matching native receipt");
              initial.status.practice.phase = "passed";
              changed();
              return structuredClone(initial.status);
            case "complete_onboarding":
              if (initial.status.practice.phase !== "passed") throw new Error("Practice must pass");
              initial.status.required = false;
              changed();
              return structuredClone(initial.status);
            default: throw new Error(`Unexpected test IPC: ${command}`);
          }
        },
      },
    });
  }, state);
  await page.goto(path);
}

async function emitSpeech(page: Page) {
  await page.evaluate(() => {
    window.onboardingTest.practice({ type: "transcript", text: "write a friendly greeting" });
    window.onboardingTest.practice({ type: "stage", stage: "generating" });
  });
}

async function finishPractice(page: Page, paste = true) {
  const output = "Write a friendly greeting for a new colleague.";
  await emitSpeech(page);
  if (paste) {
    await page.locator("#setup-practice").evaluate((field, text) => {
      const clipboardData = new DataTransfer();
      clipboardData.setData("text/plain", text);
      field.dispatchEvent(new ClipboardEvent("paste", { bubbles: true, clipboardData }));
    }, output);
    await page.locator("#setup-practice").fill(output);
  }
  await page.evaluate((text) => window.onboardingTest.practice({
    type: "finished",
    report: { job_id: 7, profile_id: "generic", outcome: { kind: "inserted", text }, elapsed_ms: 1, history_saved: false, structure: null, generation_elapsed_ms: 0, structure_repair_attempts: 0 },
    capped: false,
  }), output);
}

async function launchPractice(page: Page) {
  const state = fixture(true);
  state.status.step = "practice";
  await launch(page, state);
  await page.getByRole("button", { name: "Prepare practice" }).click();
  await expect(page.locator("#setup-practice")).toBeFocused();
}

test("inference presets keep bundled default and require explicit remote context consent", async ({ page }) => {
  const state = fixture(true);
  state.status.required = false;
  await launch(page, state);
  await page.getByRole("button", { name: "Inference", exact: true }).click();
  await expect(page.getByRole("status").filter({ hasText: "Selected:" })).toContainText("Bundled local");
  for (const provider of ["OpenAI", "Anthropic", "Google"]) {
    await page.getByRole("button", { name: provider, exact: true }).click();
    await expect(page.getByLabel("API base URL")).toHaveValue(provider === "OpenAI" ? "https://api.openai.com/v1" : provider === "Anthropic" ? "https://api.anthropic.com/v1" : "https://generativelanguage.googleapis.com/v1beta/openai");
    await page.getByLabel("Model ID", { exact: true }).fill("text-model");
    await page.getByLabel("API key / token", { exact: true }).fill("mock-write-only-token");
    await expect(page.getByRole("button", { name: "Save connection" })).toBeDisabled();
    await expect(page.getByText(/transcripts, prompt instructions, permitted app\/context metadata/)).toBeVisible();
    await page.getByLabel("I consent to sending this context to this endpoint.").check();
    await expect(page.getByRole("button", { name: "Save connection" })).toBeEnabled();
  }
  await expect.poll(() => page.evaluate(() => window.onboardingTest.calls.filter((call) => call === "test_inference_connection" || call === "save_inference_connection").length)).toBe(0);
});

test("inference keys are write-only, cleared after save, and cannot follow endpoint edits silently", async ({ page }) => {
  const state = fixture(true);
  state.status.required = false;
  await launch(page, state);
  await page.getByRole("button", { name: "Inference", exact: true }).click();
  await page.getByRole("button", { name: "OpenAI", exact: true }).click();
  await page.getByLabel("Model ID", { exact: true }).fill("text-model");
  await page.getByLabel("API key / token", { exact: true }).fill("mock-write-only-token");
  await page.getByLabel("I consent to sending this context to this endpoint.").check();
  await page.getByRole("button", { name: "Save connection" }).click();
  await expect(page.getByLabel("Replace saved API key / token (leave blank to keep)")).toHaveValue("");
  await expect(page.getByText("Key saved (never displayed)", { exact: false })).toBeVisible();
  expect(await page.evaluate(() => JSON.stringify(window.onboardingTest.state.inference))).not.toContain("mock-write-only-token");
  expect(await page.evaluate(() => JSON.stringify({ ...localStorage, ...sessionStorage }))).not.toContain("mock-write-only-token");
  expect(await page.evaluate(() => window.onboardingTest.inferenceWrites[0].hasSecret)).toBe(true);
  await page.getByLabel("API base URL").fill("https://gateway.example/v1");
  await page.getByLabel("I consent to sending this context to this endpoint.").check();
  await expect(page.getByRole("button", { name: "Save connection" })).toBeDisabled();
  await expect(page.getByText(/cannot silently reuse a saved key/)).toBeVisible();
  await page.getByLabel("Remove saved key / token").check();
  await expect(page.getByRole("button", { name: "Save connection" })).toBeEnabled();
  await page.getByRole("button", { name: "Save connection" }).click();
  await expect(page.getByText("No key saved", { exact: false })).toBeVisible();
});

test("inference LAN HTTP requires both context consent and unencrypted transit acknowledgment", async ({ page }) => {
  const state = fixture(true);
  state.status.required = false;
  await launch(page, state);
  await page.getByRole("button", { name: "Inference", exact: true }).click();
  await page.getByRole("button", { name: "Add custom model" }).click();
  await page.getByLabel("API base URL").fill("http://192.168.1.20:1234/v1");
  await page.getByLabel("Protocol", { exact: true }).selectOption("openai_responses");
  await page.getByLabel("Model ID", { exact: true }).fill("lan-text-model");
  await page.getByLabel("I consent to sending this context to this endpoint.").check();
  await expect(page.getByRole("button", { name: "Save connection" })).toBeDisabled();
  await expect(page.getByText(/LAN HTTP is unencrypted/)).toBeVisible();
  await page.getByLabel("I understand and allow unencrypted LAN HTTP.").check();
  await page.getByRole("button", { name: "Save connection" }).click();
  expect(await page.evaluate(() => window.onboardingTest.inferenceWrites[0].input)).toMatchObject({ protocol: "openai_responses", auth: "none", consent_remote: true, allow_insecure_lan: true });
});

test("guided inference supports LM Studio discovery and selected-model test without bundled LLM download", async ({ page }) => {
  const state = fixture();
  state.status.speech = { state: "ready" };
  state.models[0].installed = state.models[0].selected = true;
  await launch(page, state);
  await expect(page.getByRole("heading", { name: "Speech and inference" })).toBeVisible();
  await page.getByRole("button", { name: "LM Studio", exact: true }).click();
  await expect(page.getByLabel("API base URL")).toHaveValue("http://localhost:1234/v1");
  await expect(page.getByLabel("Authentication")).toHaveValue("none");
  await page.getByLabel("Model ID", { exact: true }).fill("manual-text-model");
  await page.getByRole("button", { name: "Save connection" }).click();
  await page.getByRole("button", { name: "Refresh models", exact: true }).first().click();
  await page.getByLabel("Discovered model").selectOption("discovered-text-model");
  await expect(page.getByRole("button", { name: "Continue to microphone and shortcut" })).toBeDisabled();
  await page.getByRole("button", { name: "Save connection" }).click();
  await page.getByRole("button", { name: "Use LM Studio", exact: true }).click();
  await expect(page.getByText("No bundled language-model download is required.", { exact: false })).toBeVisible();
  await expect(page.getByRole("button", { name: "Continue to microphone and shortcut" })).toBeDisabled();
  await expect(page.getByText(/Tests send tiny synthetic, non-sensitive content/)).toBeVisible();
  await page.getByRole("button", { name: "Test LM Studio", exact: true }).click();
  await expect(page.getByRole("button", { name: "Continue to microphone and shortcut" })).toBeEnabled();
  expect(await page.evaluate(() => window.onboardingTest.state.inference.selection)).toMatchObject({ kind: "connection", model: "discovered-text-model" });
  expect(await page.evaluate(() => window.onboardingTest.calls.filter((call) => call === "download_model"))).toEqual([]);
  await expect(page.getByRole("button", { name: "Remove LM Studio" })).toBeDisabled();
  await page.getByLabel("Model ID", { exact: true }).fill("replacement-model");
  await page.getByRole("button", { name: "Save connection" }).click();
  await expect(page.getByRole("button", { name: "Use LM Studio", exact: true })).toBeEnabled();
  await page.getByRole("button", { name: "Use LM Studio", exact: true }).click();
  expect(await page.evaluate(() => window.onboardingTest.state.inference.selection)).toMatchObject({ model: "replacement-model" });
});

test("inference editor preserves pane edits and errors while disabling concurrent actions", async ({ page }) => {
  const state = fixture(true);
  state.status.required = false;
  state.failures.save_inference_connection = "Credential store is locked. Unlock it and try again.";
  state.delays = { save_inference_connection: 500 };
  await launch(page, state);
  await page.getByRole("button", { name: "Inference", exact: true }).click();
  await page.getByRole("button", { name: "Add custom model" }).click();
  await page.getByLabel("API base URL").fill("http://localhost:8000/v1");
  await page.getByLabel("Protocol", { exact: true }).selectOption("anthropic_messages");
  await page.getByLabel("Authentication").selectOption("api_key");
  await page.getByLabel("API key / token", { exact: true }).fill("mock-transient-token");
  await page.getByLabel("Model ID", { exact: true }).fill("persistent-model");
  await page.getByRole("button", { name: "General", exact: true }).click();
  await page.getByRole("button", { name: "Inference", exact: true }).click();
  await expect(page.getByLabel("Model ID", { exact: true })).toHaveValue("persistent-model");
  await page.getByRole("button", { name: "Save connection" }).click();
  await expect(page.getByLabel("API base URL")).toBeDisabled();
  await expect(page.getByRole("button", { name: "LM Studio", exact: true })).toBeDisabled();
  await expect(page.getByRole("alert").filter({ hasText: "Credential store is locked" })).toBeVisible();
  await expect(page.getByLabel("Model ID", { exact: true })).toHaveValue("persistent-model");
  await expect(page.getByLabel("API key / token", { exact: true })).toHaveValue("");
  await expect(page.getByLabel("API base URL")).toBeEnabled();
});

test("inference recovery navigation and external history disclosures are available", async ({ page }) => {
  const state = fixture(true);
  state.status.required = false;
  state.info.models_installed = false;
  state.inference.connections = [{
    id: "remote", name: "Hosted model", provider: "custom", base_url: "https://example.com/v1",
    protocol: "openai_chat_completions", auth: "none", model: "text-model", stream: true,
    allow_insecure_lan: false, consent_remote: true, credential_present: false, revision: 1,
  }];
  state.inference.selection = { kind: "connection", connection_id: "remote", model: "text-model" };
  state.inferenceStatus = { state: "ready", selection: state.inference.selection, revision: 1, message: null };
  await launch(page, state);
  await expect(page.getByText(/language-model work uses your selected endpoint/)).toBeVisible();
  await page.evaluate(() => window.onboardingTest.emit("open-settings-section", "inference"));
  await expect(page.getByRole("heading", { name: "Inference", exact: true })).toBeVisible();
  await page.getByRole("button", { name: "History", exact: true }).click();
  await expect(page.getByText(/Local storage does not mean inference context stays local/)).toBeVisible();
  await page.getByRole("button", { name: "Inference", exact: true }).click();
  await page.evaluate(() => { window.onboardingTest.state.inferenceTestError = "Selected model unavailable. Check server loading."; });
  await page.getByRole("button", { name: "Test Hosted model", exact: true }).click();
  await expect(page.getByRole("alert").filter({ hasText: "Selected model unavailable" }).first()).toBeVisible();
  await expect(page.getByText(/Synthetic test completed/)).toHaveCount(0);
});

test("update indicator installs only after the user clicks and then offers restart", async ({ page }) => {
  const state = fixture(true);
  state.status.required = false;
  state.updates.release = { version: "1.2.0", url: "https://github.com/jrynks/promptify/releases/tag/v1.2.0", update_available: true, install_error: null };
  await launch(page, state);
  const notice = page.getByRole("complementary", { name: "Available update" });
  await expect(notice).toContainText("Promptify 1.2.0 is available.");
  expect(await page.evaluate(() => window.onboardingTest.calls.includes("install_update"))).toBe(false);
  await notice.getByRole("button", { name: "Install update 1.2.0" }).click();
  await expect(notice).toContainText("Update installed.");
  await notice.getByRole("button", { name: "Restart Promptify" }).click();
  expect(await page.evaluate(() => window.onboardingTest.calls.filter((call) => call === "install_update").length)).toBe(1);
  expect(await page.evaluate(() => window.onboardingTest.calls.includes("restart_after_update"))).toBe(true);
});

test("manual update check and startup preference are available without automatic installs", async ({ page }) => {
  const state = fixture(true);
  state.status.required = false;
  state.updates.check_on_startup = false;
  await launch(page, state);
  await page.getByRole("button", { name: "Updates", exact: true }).click();
  const section = page.getByRole("region", { name: "Update settings" });
  await expect(section).toContainText("Installed version: 1.1.1");
  await expect(section).toContainText("You are up to date.");
  await section.getByRole("button", { name: "Check for updates", exact: true }).click();
  expect(await page.evaluate(() => window.onboardingTest.calls.includes("check_for_updates"))).toBe(true);
  await section.getByLabel("Check for new versions when Promptify starts").check();
  expect(await page.evaluate(() => window.onboardingTest.state.updates.check_on_startup)).toBe(true);
  expect(await page.evaluate(() => window.onboardingTest.calls.includes("install_update"))).toBe(false);
});

test("update API errors and no-release status are not reported as up to date", async ({ page }) => {
  const state = fixture(true);
  state.status.required = false;
  state.updates.release = null;
  state.updates.checked = false;
  state.updates.error = "GitHub refused the update check (access denied or rate limited). Try again later.";
  await launch(page, state);
  await page.getByRole("button", { name: "Updates", exact: true }).click();
  const section = page.getByRole("region", { name: "Update settings" });
  await expect(section.getByRole("alert")).toContainText("rate limited");
  await expect(section).not.toContainText("You are up to date.");
  await page.evaluate(() => {
    window.onboardingTest.state.updates.error = null;
    window.onboardingTest.state.updates.checked = true;
    window.onboardingTest.emit("updates-changed", null);
  });
  await expect(section).toContainText("No published stable release is available.");
});

test("unsupported update installer remains visible but cannot be launched", async ({ page }) => {
  const state = fixture(true);
  state.status.required = false;
  state.updates.release = { version: "1.2.0", url: "https://github.com/jrynks/promptify/releases/tag/v1.2.0", update_available: true, install_error: "No installer is published for this operating system." };
  await launch(page, state);
  const notice = page.getByRole("complementary", { name: "Available update" });
  await expect(notice.getByRole("button", { name: "Install update 1.2.0" })).toBeDisabled();
  await expect(notice).toContainText("No installer is published");
});

test("update download integrity and installation errors are shown without claiming success", async ({ page }) => {
  const state = fixture(true);
  state.status.required = false;
  state.updates.release = { version: "1.2.0", url: "https://github.com/jrynks/promptify/releases/tag/v1.2.0", update_available: true, install_error: null };
  state.failures.install_update = "Downloaded installer failed its SHA-256 integrity check; installation was blocked.";
  await launch(page, state);
  const notice = page.getByRole("complementary", { name: "Available update" });
  await notice.getByRole("button", { name: "Install update 1.2.0" }).click();
  await expect(notice.getByRole("alert")).toContainText("SHA-256");
  await expect(notice.getByRole("button", { name: "Restart Promptify" })).toHaveCount(0);
});

test("in-progress updates block duplicate clicks and react to completion events", async ({ page }) => {
  const state = fixture(true);
  state.status.required = false;
  state.updates.release = { version: "1.2.0", url: "https://github.com/jrynks/promptify/releases/tag/v1.2.0", update_available: true, install_error: null };
  state.updates.installation = "downloading";
  await launch(page, state);
  const notice = page.getByRole("complementary", { name: "Available update" });
  await expect(notice.getByRole("button", { name: "Downloading update..." })).toBeDisabled();
  await page.evaluate(() => {
    window.onboardingTest.state.updates.installation = "installed";
    window.onboardingTest.emit("updates-changed", null);
  });
  await expect(notice.getByRole("button", { name: "Restart Promptify" })).toBeEnabled();
});

test("failed update preference save preserves the selected value", async ({ page }) => {
  const state = fixture(true);
  state.status.required = false;
  state.failures.set_update_checks = "Could not save settings: permission denied";
  await launch(page, state);
  await page.getByRole("button", { name: "Updates", exact: true }).click();
  const section = page.getByRole("region", { name: "Update settings" });
  await section.getByLabel("Check for new versions when Promptify starts").click();
  await expect(section.getByRole("alert")).toContainText("permission denied");
  await expect(section.getByLabel("Check for new versions when Promptify starts")).toBeChecked();
});

test("models below minimum RAM are grayed out without hiding compatible models", async ({ page }) => {
  const state = fixture();
  state.models[1].compatibility = {
    supported: false, total_ram_bytes: 4_000_000_000,
    reason: "Below minimum RAM: requires 8 GB; this computer has 4.0 GB.",
  };
  await launch(page, state);
  const unsupported = page.getByRole("row").filter({ hasText: "Qwen3.5 4B" });
  const supported = page.getByRole("row").filter({ hasText: "Whisper small English" });
  await expect(unsupported).toHaveClass(/model-unsupported/);
  await expect(unsupported).toContainText("requires 8 GB");
  await expect(unsupported.getByText("Recommended")).toHaveCount(0);
  await expect(supported).not.toHaveClass(/model-unsupported/);
  await expect(supported).toBeVisible();
  expect(await unsupported.evaluate((element) => getComputedStyle(element).color))
    .not.toBe(await supported.evaluate((element) => getComputedStyle(element).color));
  await expect(unsupported.getByRole("button", { name: /Download/ })).toBeEnabled();
});

test("automatic input inspection does not require software-specific settings", async ({ page }) => {
  const state = fixture(true);
  state.status.required = false;
  await launch(page, state);
  await expect(page.getByRole("checkbox", { name: "Allow automatic prompt paste in VS Code and Cursor AI chat" })).toHaveCount(0);
  await expect(page.getByText("Automatic input inspection is independent", { exact: false })).toBeVisible();
  await page.reload();
  await expect(page.getByText("Automatic input inspection is independent", { exact: false })).toBeVisible();
});

test("shared shortcut permission error appears once with an adjacent recovery action", async ({ page }) => {
  const state = fixture(true);
  state.status.required = false;
  const message = "Wayland shortcuts need permission. Open Settings and enable desktop integration again.";
  state.info.paste_permission = "required";
  state.info.prompt_hotkey_error = message;
  state.info.hotkey_errors = [message, message];
  await launch(page, state);
  await expect(page.getByText(message, { exact: true })).toHaveCount(1);
  const enable = page.getByRole("button", { name: "Enable desktop integration" });
  await expect(enable).toBeInViewport();
  await expect(enable).toHaveCount(1);
  await enable.click();
  await expect(page.getByText(message, { exact: true })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Disable desktop integration" })).toHaveCount(1);
});

test("unavailable optional modifier hold offers a direct disable action", async ({ page }) => {
  const state = fixture(true);
  state.status.required = false;
  state.info.modifier_hold_requested = true;
  state.info.modifier_hold_error = "This session does not expose permissioned modifier-only monitoring.";
  await launch(page, state);
  await expect(page.getByRole("alert").filter({ hasText: "modifier-only monitoring" })).toBeVisible();
  await page.getByRole("button", { name: "Disable Ctrl+Shift hold" }).click();
  await expect(page.getByRole("alert").filter({ hasText: "modifier-only monitoring" })).toHaveCount(0);
  await expect(page.getByRole("checkbox", { name: /Also start a prompt by holding/ })).not.toBeChecked();
});

test("disabled modifier hold does not display stale monitoring errors or retry controls", async ({ page }) => {
  const state = fixture(true);
  state.status.required = false;
  state.info.modifier_hold_requested = false;
  state.info.modifier_hold_error = "This session does not expose permissioned modifier-only monitoring.";
  await launch(page, state);
  const checkbox = page.getByRole("checkbox", { name: /Also start a prompt by holding/ });
  await expect(checkbox).not.toBeChecked();
  await expect(page.getByRole("alert").filter({ hasText: "modifier-only monitoring" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Retry Ctrl+Shift monitoring" })).toHaveCount(0);
  await page.evaluate(() => { window.onboardingTest.state.failures.set_modifier_hold = "Monitoring permission unavailable"; });
  await checkbox.click();
  await expect(checkbox).not.toBeChecked();
  await expect(page.getByRole("alert").filter({ hasText: "Monitoring permission unavailable" })).toBeVisible();
});

test("one desktop integration control disables and re-enables insertion", async ({ page }) => {
  const state = fixture(true);
  state.status.required = false;
  await launch(page, state);
  await page.getByRole("button", { name: "Disable desktop integration" }).click();
  await expect(page.getByText("Desktop integration is disabled.", { exact: false })).toBeVisible();
  await page.reload();
  await expect(page.getByRole("button", { name: "Enable desktop integration" })).toBeVisible();
  await page.getByRole("button", { name: "Enable desktop integration" }).click();
  await expect(page.getByRole("button", { name: "Disable desktop integration" })).toBeVisible();
});

test("failed disable remains enabled and reports the persistence error", async ({ page }) => {
  const state = fixture(true);
  state.status.required = false;
  state.failures.disable_desktop_integration = "Could not save desktop integration setting";
  await launch(page, state);
  await page.getByRole("button", { name: "Disable desktop integration" }).click();
  await expect(page.getByRole("alert").filter({ hasText: state.failures.disable_desktop_integration })).toBeVisible();
  await expect(page.getByRole("button", { name: "Disable desktop integration" })).toBeVisible();
  expect(await page.evaluate(() => window.onboardingTest.state.info.desktop_integration_enabled)).toBe(true);
});

test("disabled desktop integration gates practice even without OS permission requirements", async ({ page }) => {
  const state = fixture(true);
  state.status.step = "input";
  state.info.desktop_integration_enabled = false;
  await launch(page, state);
  await expect(page.getByRole("button", { name: "Continue to practice" })).toBeDisabled();
  await page.getByRole("button", { name: "Enable desktop integration" }).click();
  await expect(page.getByRole("button", { name: "Continue to practice" })).toBeEnabled();
});

test("desktop-assigned shortcuts are displayed rather than assumed", async ({ page }) => {
  const state = fixture(true);
  state.status.required = false;
  state.info.activation_bindings = [{ id: "prompt", trigger_description: "Super+P" }];
  await launch(page, state);
  await expect(page.getByLabel("Active desktop shortcuts").getByText("Super+P")).toBeVisible();
});

test("clipboard recovery requires an explicit action and surfaces errors", async ({ page }) => {
  const state = fixture(true);
  state.status.required = false;
  state.info.clipboard_restore_pending = true;
  state.failures.restore_insertion_clipboard = "The clipboard changed since insertion";
  await launch(page, state);
  await page.getByRole("button", { name: "Restore previous clipboard" }).click();
  await expect(page.getByRole("alert").filter({ hasText: state.failures.restore_insertion_clipboard })).toBeVisible();
  await page.evaluate(() => { delete window.onboardingTest.state.failures.restore_insertion_clipboard; });
  await page.getByRole("button", { name: "Restore previous clipboard" }).click();
  await expect(page.getByRole("button", { name: "Restore previous clipboard" })).toHaveCount(0);
});

test("unknown hardware compatibility stays visible and reports detection failure", async ({ page }) => {
  const state = fixture();
  for (const model of state.models) {
    model.compatibility = { supported: null, total_ram_bytes: null, reason: "Could not detect total system RAM; model compatibility is unknown." };
  }
  await launch(page, state);
  await expect(page.getByRole("row").filter({ hasText: "Qwen3.5 4B" })).not.toHaveClass(/model-unsupported/);
  await expect(page.getByText("Could not detect total system RAM", { exact: false })).toHaveCount(2);
  await expect(page.getByRole("button", { name: "Download 3.0 GB" })).toBeEnabled();
});

test("model recommendations fall back to a compatible small model without a GPU", async ({ page }) => {
  const state = fixture();
  state.info.use_gpu = false;
  state.models[1].compatibility = { supported: false, total_ram_bytes: 4_000_000_000, reason: "Below minimum RAM: requires 8 GB; this computer has 4.0 GB." };
  state.models.push({
    ...state.models[1], id: "small-writer", tier: "small", display_name: "Small writer", min_ram_gb: 4,
    compatibility: { supported: true, total_ram_bytes: 4_000_000_000, reason: null },
  });
  await launch(page, state);
  const small = page.getByRole("row").filter({ hasText: "Small writer" });
  await expect(small).toContainText("Recommended");
  await expect(small).not.toHaveClass(/model-unsupported/);
  await expect(small.getByRole("button", { name: /Download/ })).toBeEnabled();
});

test("installed unsupported models retain selection and deletion controls", async ({ page }) => {
  const state = fixture(true);
  state.status.required = false;
  state.models[1].compatibility = { supported: false, total_ram_bytes: 4_000_000_000, reason: "Below minimum RAM: requires 8 GB; this computer has 4.0 GB." };
  await launch(page, state);
  await page.getByRole("button", { name: "Models", exact: true }).click();
  const row = page.getByRole("row").filter({ hasText: "Qwen3.5 4B" });
  await expect(row).toHaveClass(/model-unsupported/);
  await expect(row.getByRole("radio")).toBeChecked();
  await expect(row.getByRole("button", { name: "Delete", exact: true })).toBeEnabled();
});

test("fresh setup overrides remembered tabs and excludes optional features", async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem("promptify.settings.pane", "advanced"));
  await launch(page);
  await expect(page.getByRole("heading", { name: "Speech and inference" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Continue to microphone and shortcut" })).toBeDisabled();
  await expect(page.getByRole("button", { name: "Prompt types", exact: true })).toBeDisabled();
  await expect(page.getByRole("button", { name: "Advanced", exact: true })).toHaveCount(0);
});

test("balanced downloads unlock input checks only when both engines are ready", async ({ page }) => {
  await launch(page);
  await expect(page.getByRole("region", { name: "Model settings" }).getByText("Recommended", { exact: true })).toHaveCount(2);
  await page.getByRole("button", { name: "Download 0.49 GB", exact: true }).click();
  await expect(page.getByRole("button", { name: "Continue to microphone and shortcut" })).toBeDisabled();
  await page.getByRole("button", { name: "Download 3.0 GB", exact: true }).click();
  await page.getByRole("button", { name: "Continue to microphone and shortcut" }).click();
  await expect(page.getByRole("heading", { name: "Microphone and shortcut" })).toBeFocused();
  await page.reload();
  await expect(page.getByRole("heading", { name: "Microphone and shortcut" })).toBeVisible();
});

test("fresh profile with cached models still requires onboarding", async ({ page }) => {
  await launch(page, fixture(true));
  await expect(page.getByRole("heading", { name: "Speech and inference" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Continue to microphone and shortcut" })).toBeEnabled();
});

test("download cancellation retains a resume action", async ({ page }) => {
  const state = fixture();
  state.models[0].downloading = true;
  await launch(page, state);
  await page.getByRole("button", { name: "Cancel", exact: true }).click();
  await expect(page.getByRole("button", { name: /Resume/ })).toBeVisible();
  await expect(page.getByRole("button", { name: "Continue to microphone and shortcut" })).toBeDisabled();
});

test("loading errors are explicit and CPU retry rechecks readiness", async ({ page }) => {
  const state = fixture(true);
  state.info.engines_ready = false;
  state.info.engine_error = "The language model could not load";
  state.status.language = { state: "error", message: state.info.engine_error };
  await launch(page, state);
  await expect(page.getByText(/Prompt writer: The language model could not load/)).toBeVisible();
  await expect(page.getByRole("button", { name: "Continue to microphone and shortcut" })).toBeDisabled();
  await page.getByRole("button", { name: "Try loading on CPU" }).click();
  await expect(page.getByRole("button", { name: "Continue to microphone and shortcut" })).toBeEnabled();
  expect(await page.evaluate(() => window.onboardingTest.state.info.use_gpu)).toBe(false);
});

test("missing microphone, paused shortcuts, and shortcut collisions block practice", async ({ page }) => {
  const state = fixture(true);
  state.status.step = "input";
  state.info.input_device = null;
  state.info.hotkeys_paused = true;
  state.info.prompt_hotkey_error = "Prompt shortcut is already in use";
  await launch(page, state);
  await expect(page.getByRole("button", { name: "Continue to practice" })).toBeDisabled();
  await page.evaluate(() => {
    window.onboardingTest.state.info.input_device = "Connected microphone";
    window.onboardingTest.change({});
  });
  await page.getByRole("button", { name: "Resume shortcuts" }).click();
  await page.getByRole("button", { name: "Change", exact: true }).click();
  await page.locator("input.capture").press("Control+Alt+P");
  await expect(page.getByRole("button", { name: "Continue to practice" })).toBeEnabled();
});

test("Wayland permission gates practice and can be granted", async ({ page }) => {
  const state = fixture(true);
  state.status.step = "input";
  state.info.paste_permission = "required";
  await launch(page, state);
  await expect(page.getByRole("button", { name: "Continue to practice" })).toBeDisabled();
  await page.getByRole("button", { name: "Enable desktop integration" }).click();
  await expect(page.getByText("Desktop integration permission is active.")).toBeVisible();
  await expect(page.getByRole("button", { name: "Continue to practice" })).toBeEnabled();
});

test("denied paste permission is explicit and retryable", async ({ page }) => {
  const state = fixture(true);
  state.status.step = "input";
  state.info.paste_permission = "required";
  state.failures.grant_paste_permission = "Paste permission was not granted";
  await launch(page, state);
  await page.getByRole("button", { name: "Enable desktop integration" }).click();
  await expect(page.getByRole("alert").filter({ hasText: "Paste permission was not granted" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Continue to practice" })).toBeDisabled();
  await page.evaluate(() => { delete window.onboardingTest.state.failures.grant_paste_permission; });
  await page.getByRole("button", { name: "Enable desktop integration" }).click();
  await expect(page.getByRole("button", { name: "Continue to practice" })).toBeEnabled();
});

test("unknown desktop focus blocks practice even with permission", async ({ page }) => {
  const state = fixture(true);
  state.status.step = "input";
  state.info.paste_permission = "granted";
  state.info.desktop_error = "KDE window tracking unavailable: KWin did not return a focus snapshot";
  await launch(page, state);
  await expect(page.getByRole("alert").filter({ hasText: state.info.desktop_error })).toBeVisible();
  await expect(page.getByRole("button", { name: "Continue to practice" })).toBeDisabled();
  expect(await page.evaluate(() => window.onboardingTest.calls)).not.toContain("retry_focus_detection");
  await page.evaluate(() => {
    window.onboardingTest.state.failures.retry_focus_detection = "Wait for the active recording or paste job to finish";
  });
  await page.getByRole("button", { name: "Retry focus detection" }).click();
  await expect(page.getByRole("alert").filter({ hasText: "Wait for the active" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Continue to practice" })).toBeDisabled();
  await page.evaluate(() => { delete window.onboardingTest.state.failures.retry_focus_detection; });
  await page.getByRole("button", { name: "Retry focus detection" }).click();
  await expect(page.getByRole("button", { name: "Continue to practice" })).toBeEnabled();
});

test("permission loss during practice offers recovery without skipping verification", async ({ page }) => {
  await launchPractice(page);
  await page.evaluate(() => {
    window.onboardingTest.state.info.paste_permission = "required";
    window.onboardingTest.change({});
  });
  await expect(page.getByRole("button", { name: "Prepare practice" })).toBeDisabled();
  await expect(page.getByRole("button", { name: "Finish setup" })).toBeDisabled();
  await page.getByRole("button", { name: "Enable desktop integration" }).click();
  await expect(page.getByRole("button", { name: "Prepare practice" })).toBeEnabled();
  await expect(page.getByRole("button", { name: "Finish setup" })).toBeDisabled();
});

test("configured users can recover paste permission in General Settings", async ({ page }) => {
  const state = fixture(true);
  state.status.required = false;
  state.info.paste_permission = "required";
  await launch(page, state);
  await page.getByRole("button", { name: "Enable desktop integration" }).click();
  await expect(page.getByText("Desktop integration permission is active.")).toBeVisible();
});

test("Ctrl+Shift hold can be enabled and disabled outside Windows", async ({ page }) => {
  const state = fixture(true);
  state.status.required = false;
  await launch(page, state);
  const checkbox = page.getByRole("checkbox", { name: /Also start a prompt by holding/ });
  await checkbox.check();
  await expect(checkbox).toBeChecked();
  expect(await page.evaluate(() => window.onboardingTest.state.info.modifier_hold)).toBe(true);
  await checkbox.uncheck();
  await expect(checkbox).not.toBeChecked();
});

test("desktop activation does not expose physical keyboard selection", async ({ page }) => {
  const state = fixture(true);
  state.status.required = false;
  state.info.modifier_keyboard_devices = [
    { path: "/dev/input/by-id/keyboard-event-kbd", name: "Physical keyboard" },
    { path: "/dev/input/by-id/consumer-event-kbd", name: "Consumer controls" },
  ];
  await launch(page, state);
  await expect(page.getByRole("combobox", { name: "Ctrl+Shift keyboard" })).toHaveCount(0);
  await expect(page.getByText("never requires selecting a physical keyboard", { exact: false })).toBeVisible();
  await page.reload();
  await expect(page.getByRole("combobox", { name: "Ctrl+Shift keyboard" })).toHaveCount(0);
});

test("Ctrl+Shift permission denial stays explicit and retryable", async ({ page }) => {
  const state = fixture(true);
  state.status.required = false;
  state.failures.set_modifier_hold = "Ctrl+Shift on Wayland needs read permission for keyboard SteelSeries";
  await launch(page, state);
  await page.getByRole("checkbox", { name: /Also start a prompt by holding/ }).click();
  await expect(page.getByRole("alert").filter({ hasText: "needs read permission" })).toBeVisible();
  await expect(page.getByRole("checkbox", { name: /Also start a prompt by holding/ })).not.toBeChecked();
  await page.evaluate(() => { delete window.onboardingTest.state.failures.set_modifier_hold; });
  await page.getByRole("checkbox", { name: /Also start a prompt by holding/ }).check();
  await expect(page.getByRole("checkbox", { name: /Also start a prompt by holding/ })).toBeChecked();
});

test("Ctrl+Shift runtime failure offers retry and preserves the requested setting", async ({ page }) => {
  const state = fixture(true);
  state.status.required = false;
  state.info.modifier_hold_requested = true;
  state.info.modifier_hold_error = "The keyboard devices changed. Retry Ctrl+Shift monitoring in Settings.";
  await launch(page, state);
  await expect(page.getByRole("checkbox", { name: /Also start a prompt by holding/ })).toBeChecked();
  await expect(page.getByRole("alert").filter({ hasText: "keyboard devices changed" })).toBeVisible();
  await page.getByRole("button", { name: "Retry Ctrl+Shift monitoring" }).click();
  await expect(page.getByRole("button", { name: "Retry Ctrl+Shift monitoring" })).toHaveCount(0);
  expect(await page.evaluate(() => window.onboardingTest.state.info.modifier_hold)).toBe(true);
});

test("Ctrl+Shift saved startup failure can be disabled instead of retried", async ({ page }) => {
  const state = fixture(true);
  state.status.required = false;
  state.info.modifier_hold_requested = true;
  state.info.modifier_hold_error = "Input Monitoring permission is required";
  await launch(page, state);
  await page.getByRole("checkbox", { name: /Also start a prompt by holding/ }).uncheck();
  await expect(page.getByRole("button", { name: "Retry Ctrl+Shift monitoring" })).toHaveCount(0);
  expect(await page.evaluate(() => window.onboardingTest.state.info.modifier_hold_requested)).toBe(false);
});

test("typing and a native result without a paste cannot complete practice", async ({ page }) => {
  await launchPractice(page);
  await page.locator("#setup-practice").fill("Manual text");
  await expect(page.getByRole("button", { name: "Finish setup" })).toBeDisabled();
  await finishPractice(page, false);
  await expect(page.getByText(/no matching paste reached this field/)).toBeVisible();
  await expect(page.getByRole("button", { name: "Finish setup" })).toBeDisabled();
  expect(await page.evaluate(() => window.onboardingTest.calls.includes("confirm_onboarding_paste"))).toBe(false);
});

test("verified practice completes once and stays complete after relaunch", async ({ page }, testInfo) => {
  const started = performance.now();
  await launchPractice(page);
  const prepared = performance.now();
  await finishPractice(page);
  await expect(page.getByRole("button", { name: "Finish setup" })).toBeEnabled();
  await page.getByRole("button", { name: "Finish setup" }).click();
  await expect(page.getByRole("heading", { name: "Your first prompt" })).toHaveCount(0);
  await page.reload();
  await expect(page.getByRole("heading", { name: "Your first prompt" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Prompt types", exact: true })).toBeEnabled();
  await testInfo.attach("ui-test-timings", {
    body: JSON.stringify({ startup_and_preparation_ms: prepared - started, practice_and_relaunch_ms: performance.now() - prepared, note: "Mocked UI timing only; native download and inference timings require the desktop acceptance run." }),
    contentType: "application/json",
  });
});

test("a failed completion save leaves setup required and retryable", async ({ page }) => {
  await launchPractice(page);
  await finishPractice(page);
  await page.evaluate(() => { window.onboardingTest.state.failures.complete_onboarding = "Could not save settings: disk is read-only"; });
  await page.getByRole("button", { name: "Finish setup" }).click();
  await expect(page.getByRole("alert").filter({ hasText: "disk is read-only" })).toBeVisible();
  expect(await page.evaluate(() => window.onboardingTest.state.status.required)).toBe(true);
  await page.evaluate(() => { delete window.onboardingTest.state.failures.complete_onboarding; });
  await page.getByRole("button", { name: "Finish setup" }).click();
  await expect(page.getByRole("heading", { name: "Your first prompt" })).toHaveCount(0);
});

test("focus loss and old events cannot verify a new attempt", async ({ page }) => {
  await launchPractice(page);
  await page.locator("#setup-title").focus();
  await page.getByRole("button", { name: "Prepare practice" }).click();
  await page.evaluate(() => window.onboardingTest.practice({
    type: "finished",
    report: { job_id: 2, profile_id: "generic", outcome: { kind: "inserted", text: "Stale result" }, elapsed_ms: 1, history_saved: false, structure: null, generation_elapsed_ms: 0, structure_repair_attempts: 0 },
    capped: false,
  }, 1));
  await expect(page.getByRole("button", { name: "Finish setup" })).toBeDisabled();
  await expect(page.locator("#setup-practice")).toHaveValue("");
});

test("silence and recording errors preserve the retry path", async ({ page }) => {
  await launchPractice(page);
  await page.evaluate(() => window.onboardingTest.practice({ type: "error", message: "Microphone access was denied" }));
  await expect(page.getByRole("alert").filter({ hasText: "Microphone access was denied" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Prepare practice" })).toBeEnabled();
  await expect(page.getByRole("button", { name: "Finish setup" })).toBeDisabled();
});

test("IPC failures do not fall through to ordinary settings", async ({ page }) => {
  const state = fixture(true);
  state.failures.onboarding_status = "Native state unavailable";
  await launch(page, state);
  await expect(page.getByRole("alert")).toContainText("Native state unavailable");
  await expect(page.getByRole("navigation")).toHaveCount(0);
  await page.evaluate(() => { delete window.onboardingTest.state.failures.onboarding_status; });
  await page.getByRole("button", { name: "Retry connection" }).click();
  await expect(page.getByRole("heading", { name: "Speech and inference" })).toBeVisible();
});

test("a failed setup refresh disarms practice when the field is unmounted", async ({ page }) => {
  await launchPractice(page);
  await page.evaluate(() => {
    window.onboardingTest.state.failures.app_info = "The native connection was interrupted";
    window.onboardingTest.change({});
  });
  await expect(page.getByRole("alert")).toContainText("The native connection was interrupted");
  await expect.poll(() => page.evaluate(() => window.onboardingTest.state.status.practice.phase)).toBe(null);
  await expect(page.locator("#setup-practice")).toHaveCount(0);
});

test("invalid persisted setup presents recovery without configuration controls", async ({ page }) => {
  const state = fixture();
  state.status.startup_error = "settings.json is invalid";
  await launch(page, state);
  await expect(page.getByRole("heading", { name: "Setup needs attention" })).toBeVisible();
  await expect(page.getByRole("button", { name: /Download/ })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Retry startup" })).toBeVisible();
});

test("configured existing users keep their remembered tab", async ({ page }) => {
  const state = fixture(true);
  state.status.required = false;
  await page.addInitScript(() => localStorage.setItem("promptify.settings.pane", "words"));
  await launch(page, state);
  await expect(page.getByRole("button", { name: "Your words", exact: true })).toHaveAttribute("aria-current", "page");
  await expect(page.getByLabel("Setup progress")).toHaveCount(0);
});

test("the tour fits the minimum window and supports dark mode", async ({ page }, testInfo) => {
  await page.setViewportSize({ width: 640, height: 480 });
  await page.emulateMedia({ colorScheme: "dark", reducedMotion: "reduce" });
  await launch(page);
  await expect(page.getByRole("heading", { name: "Speech and inference" })).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(640);
  await page.screenshot({ path: testInfo.outputPath("onboarding-minimum-window.png"), fullPage: true });
});

test("prompt routing defaults to legacy and persists explicit opt-in", async ({ page }) => {
  const state = fixture(true);
  state.status.required = false;
  await launch(page, state);
  await page.getByRole("button", { name: "Prompt types", exact: true }).click();
  const enabled = page.getByLabel("Use task-aware prompt adaptation");
  await expect(enabled).not.toBeChecked();
  await enabled.check();
  await expect(enabled).toBeChecked();
  await page.getByRole("button", { name: "General", exact: true }).click();
  await page.reload();
  await page.getByRole("button", { name: "Prompt types", exact: true }).click();
  await expect(enabled).toBeChecked();
  await page.evaluate(() => { window.onboardingTest.state.failures.set_rendering = "Routing settings cannot be saved"; });
  await enabled.click();
  await expect(page.getByRole("alert").filter({ hasText: "cannot be saved" })).toBeVisible();
  await expect(enabled).toBeChecked();
});

test("corrupt routing can only be repaired by explicit backed-up reset", async ({ page }) => {
  const state = fixture(true);
  state.status.required = false;
  state.routingError = "routing-settings.json is invalid";
  await launch(page, state);
  await page.getByRole("button", { name: "Prompt types", exact: true }).click();
  await expect(page.getByRole("alert").filter({ hasText: state.routingError })).toBeVisible();
  await page.evaluate(() => { window.onboardingTest.state.failures.reset_routing = "Could not back up routing settings"; });
  await page.getByRole("button", { name: "Restore default routing" }).click();
  await expect(page.getByRole("alert").filter({ hasText: "Could not back up" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Restore default routing" })).toBeVisible();
  await page.evaluate(() => { delete window.onboardingTest.state.failures.reset_routing; });
  await page.getByRole("button", { name: "Restore default routing" }).click();
  await expect(page.getByText("Default routing restored. The previous routing file was backed up in the app data folder.")).toBeVisible();
  await expect(page.getByRole("button", { name: "Restore default routing" })).toHaveCount(0);
  const calls = await page.evaluate(() => window.onboardingTest.calls);
  expect(calls.filter((command) => command === "reset_routing")).toHaveLength(2);
  expect(calls).not.toContain("set_rendering");
});

test("prompt routing queues scoped overrides and distinguishes proposed types", async ({ page }) => {
  const state = fixture(true);
  state.status.required = false;
  await launch(page, state);
  await page.getByRole("button", { name: "Prompt types", exact: true }).click();
  await page.getByText("Choose a type or surface for the next prompt", { exact: true }).click();
  const panel = page.locator("section").filter({ has: page.getByRole("heading", { name: "Prompt types", exact: true }) });
  await panel.getByRole("combobox", { name: "Prompt type", exact: true }).selectOption("code.debug");
  await panel.getByRole("combobox", { name: "Input surface", exact: true }).selectOption("code_chat");
  await panel.getByRole("button", { name: "Use for next prompt" }).click();
  await expect(panel.getByRole("status").filter({ hasText: "next Prompt hotkey" })).toBeVisible();
  expect(await page.evaluate(() => window.onboardingTest.lastRouting)).toEqual({
    rendering: "adaptive", task_type: "code.debug", surface: "code_chat",
  });
  await panel.getByRole("combobox", { name: "Input surface", exact: true }).selectOption("literal");
  await expect(panel.getByRole("button", { name: "Use for next prompt" })).toBeDisabled();
  await panel.getByLabel("Find a prompt type").fill("audio.song");
  await expect(panel.getByText("0 matching types", { exact: true })).toBeVisible();
  await panel.getByLabel("Include proposed types").check();
  await expect(panel.getByText("audio.song", { exact: true })).toBeVisible();
  await expect(panel.getByRole("combobox", { name: "Prompt type", exact: true }).locator("option[value='audio.song']")).toHaveCount(0);
});

test("quality recommendations preserve selections and never start downloads", async ({ page }) => {
  const state = fixture(true);
  state.status.required = false;
  state.models.push({
    ...state.models[1], id: "quality-writer", tier: "quality", display_name: "Quality writer",
    min_ram_gb: 16, compatibility: { supported: true, total_ram_bytes: 32_000_000_000, reason: null },
    installed: false, selected: false,
  });
  await launch(page, state);
  await page.getByRole("button", { name: "Models", exact: true }).click();
  const quality = page.getByRole("row").filter({ hasText: "Quality writer" });
  await expect(quality).toContainText("Recommended");
  await expect(quality.getByRole("radio")).not.toBeChecked();
  await expect(page.getByRole("row").filter({ hasText: "Qwen3.5 4B" }).getByRole("radio")).toBeChecked();
  expect(await page.evaluate(() => window.onboardingTest.calls.filter((command) =>
    command === "select_model" || command === "download_model"))).toEqual([]);
});

test("quality recommendations exclude unsupported and unknown models", async ({ page }) => {
  const state = fixture(true);
  state.status.required = false;
  for (const supported of [false, null]) {
    state.models.push({
      ...state.models[1], id: `quality-${supported}`, tier: "quality", display_name: `Quality ${supported}`,
      compatibility: { supported, total_ram_bytes: supported === null ? null : 8_000_000_000, reason: "Compatibility not confirmed" },
      installed: false, selected: false,
    });
  }
  await launch(page, state);
  await page.getByRole("button", { name: "Models", exact: true }).click();
  await expect(page.getByRole("row").filter({ hasText: "Qwen3.5 4B" })).toContainText("Recommended");
  for (const supported of [false, null]) {
    await expect(page.getByRole("row").filter({ hasText: `Quality ${supported}` })).not.toContainText("Recommended");
  }
});

test("word drafts and routing filters survive navigation and setup refresh", async ({ page }) => {
  const state = fixture(true);
  state.status.required = false;
  await launch(page, state);
  await page.getByRole("button", { name: "Your words", exact: true }).click();
  const words = page.getByLabel("Names and terms to expect, one per line");
  await words.fill("My unsaved vocabulary");
  await page.getByRole("button", { name: "Prompt types", exact: true }).click();
  await page.getByLabel("Find a prompt type").fill("code.debug");
  await page.evaluate(() => window.onboardingTest.emit("onboarding-changed", {}));
  await page.getByRole("button", { name: "Your words", exact: true }).click();
  await expect(words).toHaveValue("My unsaved vocabulary");
  await page.getByRole("button", { name: "Save", exact: true }).click();
  await expect(page.getByText("Saved.", { exact: true })).toBeVisible();
  expect(await page.evaluate(() => window.onboardingTest.state.info.vocabulary.words)).toEqual(["My unsaved vocabulary"]);
  await page.getByRole("button", { name: "Prompt types", exact: true }).click();
  await expect(page.getByLabel("Find a prompt type")).toHaveValue("code.debug");
  expect(await page.evaluate(() => window.onboardingTest.calls.filter((command) => command === "prompt_catalog").length)).toBe(1);
});

test("retained settings remain usable at minimum window size", async ({ page }, testInfo) => {
  const state = fixture(true);
  state.status.required = false;
  await page.setViewportSize({ width: 640, height: 480 });
  await launch(page, state);
  for (const pane of ["General", "Models", "Prompt types", "Your words", "History", "Updates"]) {
    await page.getByRole("button", { name: pane, exact: true }).click();
    await expect(page.getByRole("heading", { name: pane, exact: true })).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  }
  await page.getByRole("button", { name: "Models", exact: true }).click();
  await page.screenshot({ path: testInfo.outputPath("simplified-models.png"), fullPage: true });
});

test("all other tabs share General spacing scale without overflowing", async ({ page }, testInfo) => {
  const state = fixture(true);
  state.status.required = false;
  await launch(page, state);
  for (const width of [960, 640]) {
    await page.setViewportSize({ width, height: width === 640 ? 480 : 760 });
    for (const pane of ["Models", "Prompt types", "Your words", "History", "Updates"]) {
      await page.getByRole("button", { name: pane, exact: true }).click();
      const section = page.locator(".settings-pane:not([hidden]) > section");
      await expect(section.getByRole("heading", { name: pane, exact: true })).toBeVisible();
      expect(await section.evaluate((element) => {
        const style = getComputedStyle(element);
        return { gap: style.gap, direction: style.flexDirection };
      })).toEqual({ gap: "16px", direction: "column" });
      if (pane === "Prompt types") await section.locator("details").evaluate((element) => { element.setAttribute("open", ""); });
      const actions = section.locator(".actions:not(td)");
      for (const action of await actions.all()) {
        expect(await action.evaluate((element) => {
          const style = getComputedStyle(element);
          return { gap: style.columnGap, wrap: style.flexWrap };
        })).toEqual({ gap: "8px", wrap: "wrap" });
      }
      expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width);
      await page.screenshot({ path: testInfo.outputPath(`spacing-${pane.replaceAll(" ", "-")}-${width}.png`), fullPage: true });
    }
  }
});

test("simplified settings retire remembered tabs and defer unvisited panes", async ({ page }) => {
  const state = fixture(true);
  state.status.required = false;
  await page.addInitScript(() => localStorage.setItem("promptify.settings.pane", "advanced"));
  await launch(page, state);
  await expect(page.getByRole("heading", { name: "General", exact: true })).toBeVisible();
  await expect(page.getByRole("navigation").getByRole("button")).toHaveText([
    "General", "Models", "Inference", "Prompt types", "Your words", "History", "Updates",
  ]);
  await expect(page.getByText("Answer", { exact: true })).toHaveCount(0);
  expect(await page.evaluate(() => window.onboardingTest.calls.filter((command) =>
    ["remote_info", "mcp_info", "mcp_read", "preview_prompt", "list_profiles", "list_models", "list_history", "prompt_catalog"].includes(command)))).toEqual([]);
  await page.getByRole("button", { name: "Models", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Models", exact: true })).toBeVisible();
  await page.getByRole("button", { name: "General", exact: true }).click();
  await page.getByRole("button", { name: "Models", exact: true }).click();
  expect(await page.evaluate(() => window.onboardingTest.calls.filter((command) => command === "list_models").length)).toBe(1);
});

test("prompt catalog stays usable in the minimum window", async ({ page }, testInfo) => {
  const state = fixture(true);
  state.status.required = false;
  await page.setViewportSize({ width: 640, height: 480 });
  await page.emulateMedia({ colorScheme: "dark", reducedMotion: "reduce" });
  await launch(page, state);
  await page.getByRole("button", { name: "Prompt types", exact: true }).click();
  await page.getByText("Choose a type or surface for the next prompt", { exact: true }).click();
  await expect(page.getByLabel("Use task-aware prompt adaptation")).toBeVisible();
  await expect(page.getByRole("combobox", { name: "Input surface", exact: true })).toBeVisible();
  expect(await page.locator(".routing-controls").filter({ visible: true }).evaluate((element) =>
    getComputedStyle(element).gridTemplateColumns.split(" ").length)).toBe(1);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(640);
  await page.screenshot({ path: testInfo.outputPath("prompt-types-minimum-window.png"), fullPage: true });
});

test.describe("actionable recovery controls", () => {
  test("General spacing separates recovery controls and stays stable at minimum width", async ({ page }, testInfo) => {
    const state = fixture(true);
    state.status.required = false;
    state.info.paste_permission = "required";
    state.info.modifier_hold_requested = true;
    state.info.modifier_hold_error = "Modifier-only monitoring is unavailable on this desktop.";
    state.info.desktop_error = "Focus detection needs attention.";
    state.info.clipboard_restore_pending = true;
    await launch(page, state);
    const general = page.getByRole("region", { name: "General settings", exact: true });
    const integration = general.getByLabel("Desktop integration", { exact: true });
    const dictationTone = general.getByRole("combobox", { name: "Dictation tone" });
    await expect(dictationTone).toHaveValue("natural");
    await expect(dictationTone.locator("option")).toHaveCount(6);
    await expect(integration.getByRole("button", { name: "Enable desktop integration", exact: true })).toBeVisible();
    for (const width of [960, 640]) {
      await page.setViewportSize({ width, height: width === 640 ? 480 : 760 });
      expect(await general.locator(".stack > div.inline").evaluate((element) => {
        const style = getComputedStyle(element);
        return { gap: style.gap, wrap: style.flexWrap };
      })).toEqual({ gap: "8px", wrap: "wrap" });
      expect(await general.locator("dl").first().evaluate((element) => getComputedStyle(element).rowGap)).toBe(width === 640 ? "8px" : "24px");
      expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width);
      const toneBounds = await dictationTone.boundingBox();
      expect(toneBounds).not.toBeNull();
      expect(toneBounds!.x).toBeGreaterThanOrEqual(0);
      expect(toneBounds!.x + toneBounds!.width).toBeLessThanOrEqual(width);
      const buttons = await integration.getByRole("button").evaluateAll((elements) => elements.map((element) => {
        const rect = element.getBoundingClientRect();
        return { left: rect.left, right: rect.right, top: rect.top, bottom: rect.bottom };
      }));
      for (let i = 0; i < buttons.length; i++) {
        for (let j = i + 1; j < buttons.length; j++) {
          const a = buttons[i], b = buttons[j];
          const sameRow = a.top < b.bottom && b.top < a.bottom;
          expect(sameRow ? Math.max(b.left - a.right, a.left - b.right) : Math.max(b.top - a.bottom, a.top - b.bottom)).toBeGreaterThanOrEqual(8);
        }
      }
      await page.screenshot({ path: testInfo.outputPath(`general-spacing-${width}.png`), fullPage: true });
    }
  });

  test("listener failures remain visible after successful reads and reconnect locally", async ({ page }) => {
    const state = fixture(true);
    state.status.required = false;
    state.failures["listen:history-changed"] = "History events disconnected";
    state.failures["listen:updates-changed"] = "Update events disconnected";
    await launch(page, state);
    await page.getByRole("button", { name: "History", exact: true }).click();
    await expect(page.getByRole("alert")).toContainText("History events disconnected");
    await page.evaluate(() => { delete window.onboardingTest.state.failures["listen:history-changed"]; });
    await page.getByRole("button", { name: "Refresh history", exact: true }).click();
    await expect(page.getByRole("alert")).toHaveCount(0);
    await page.getByRole("button", { name: "Updates", exact: true }).click();
    await expect(page.getByRole("alert")).toContainText("Update events disconnected");
    await page.evaluate(() => { delete window.onboardingTest.state.failures["listen:updates-changed"]; });
    await page.getByRole("button", { name: "Retry update connection", exact: true }).click();
    await expect(page.getByRole("alert")).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Check for updates", exact: true })).toBeEnabled();
  });

  test("ordinary Settings resumes shortcuts and opens microphone settings with explicit launch errors", async ({ page }) => {
    const state = fixture(true);
    state.status.required = false;
    state.info.hotkeys_paused = true;
    state.info.input_device = null;
    state.failures.open_system_settings = "System settings launcher is unavailable";
    await launch(page, state);
    await page.getByRole("button", { name: "Resume shortcuts", exact: true }).click();
    await expect(page.getByRole("button", { name: "Resume shortcuts", exact: true })).toHaveCount(0);
    await page.getByRole("button", { name: "Open microphone settings", exact: true }).click();
    await expect(page.getByRole("alert").filter({ hasText: "launcher is unavailable" })).toBeVisible();
    await expect(page.getByRole("button", { name: "Refresh microphone and shortcut", exact: true })).toBeVisible();
  });

  test("ordinary Settings offers load retry, CPU fallback and direct model navigation", async ({ page }) => {
    const state = fixture(true);
    state.status.required = false;
    state.info.engines_ready = false;
    state.info.engine_error = "Vulkan device failed";
    await launch(page, state);
    await expect(page.getByRole("button", { name: "Retry model loading", exact: true })).toBeVisible();
    await page.getByRole("button", { name: "Try loading on CPU", exact: true }).click();
    await expect(page.getByLabel("Use the GPU when available")).not.toBeChecked();
    await page.evaluate(() => {
      window.onboardingTest.state.info.engines_ready = false;
      window.onboardingTest.state.info.engine_error = "Model unavailable";
      window.onboardingTest.change({});
    });
    await page.getByRole("button", { name: "Choose models", exact: true }).click();
    await expect(page.getByRole("button", { name: "Models", exact: true })).toHaveAttribute("aria-current", "page");
  });

  test("startup errors provide non-destructive access to the data folder", async ({ page }) => {
    const state = fixture();
    state.status.startup_error = "settings.json is invalid";
    await launch(page, state);
    await page.getByRole("button", { name: "Open app data folder", exact: true }).click();
    expect(await page.evaluate(() => window.onboardingTest.calls.includes("open_data_folder"))).toBe(true);
    await expect(page.getByRole("button", { name: "Retry startup", exact: true })).toBeVisible();
    expect(await page.evaluate(() => window.onboardingTest.state.status.startup_error)).toBe("settings.json is invalid");
  });

  test("history load failures retry and clear on success", async ({ page }) => {
    const state = fixture(true);
    state.status.required = false;
    state.failures.list_history = "History unavailable";
    await launch(page, state);
    await page.getByRole("button", { name: "History", exact: true }).click();
    await expect(page.getByRole("alert")).toContainText("History unavailable");
    await page.evaluate(() => { delete window.onboardingTest.state.failures.list_history; });
    await page.getByRole("button", { name: "Refresh history", exact: true }).click();
    await expect(page.getByRole("alert")).toHaveCount(0);
    await expect(page.locator(".history li")).toHaveCount(0);
  });

  test("malformed word corrections stay editable and never save silently", async ({ page }) => {
    const state = fixture(true);
    state.status.required = false;
    await launch(page, state);
    await page.getByRole("button", { name: "Your words", exact: true }).click();
    const corrections = page.getByRole("textbox", { name: /Corrections/ });
    await corrections.fill("missing delimiter");
    await page.getByRole("button", { name: "Save", exact: true }).click();
    await expect(page.getByRole("alert")).toContainText("Invalid correction");
    expect(await page.evaluate(() => window.onboardingTest.calls.includes("set_vocabulary"))).toBe(false);
    await corrections.fill("wrong => right");
    await page.getByRole("button", { name: "Retry save", exact: true }).click();
    await expect(page.getByRole("status")).toContainText("Saved");
    expect(await page.evaluate(() => window.onboardingTest.state.info.vocabulary.replacements)).toEqual([{ from: "wrong", to: "right" }]);
  });

  test("update status failure reconnects even when Check for updates is disabled", async ({ page }) => {
    const state = fixture(true);
    state.status.required = false;
    state.failures.update_info = "Update status unavailable";
    await launch(page, state);
    await page.getByRole("button", { name: "Updates", exact: true }).click();
    await expect(page.getByRole("alert")).toContainText("Update status unavailable");
    await expect(page.getByRole("button", { name: "Check for updates", exact: true })).toBeDisabled();
    await page.evaluate(() => { delete window.onboardingTest.state.failures.update_info; });
    await page.getByRole("button", { name: "Retry update connection", exact: true }).click();
    await expect(page.getByRole("button", { name: "Check for updates", exact: true })).toBeEnabled();
    await expect(page.getByRole("alert")).toHaveCount(0);
  });

  test("unsupported update installer offers an adjacent release download", async ({ page }) => {
    const state = fixture(true);
    state.status.required = false;
    state.updates.release!.update_available = true;
    state.updates.release!.version = "1.2.0";
    state.updates.release!.install_error = "No supported installer for this OS";
    await launch(page, state);
    const notice = page.getByLabel("Available update");
    await expect(notice.getByText("No supported installer for this OS", { exact: true })).toBeVisible();
    await notice.getByRole("button", { name: "Download from release page", exact: true }).click();
    expect(await page.evaluate(() => window.onboardingTest.calls.includes("open_update_release"))).toBe(true);
    expect(await page.evaluate(() => window.onboardingTest.calls.includes("install_update"))).toBe(false);
  });
});

test.describe("native overlay sizing", () => {
  test.use({ viewport: { width: 560, height: 180 }, contextOptions: { screen: { width: 1920, height: 1080 } }, deviceScaleFactor: 1.5 });

  async function openOverlay(page: Page) {
    await launch(page, fixture(true), "/overlay.html");
    await expect.poll(() => page.evaluate(() => window.onboardingTest.calls.includes("plugin:event|listen"))).toBe(true);
  }

  async function emitOverlay(page: Page, event: OverlayEvent) {
    await page.evaluate((payload) => window.onboardingTest.emit("overlay-event", payload), event);
  }

  async function fitsWindow(page: Page) {
    await expect.poll(() => page.locator(".pill").evaluate((element) => {
      const bounds = element.getBoundingClientRect();
      return bounds.bottom <= window.innerHeight && bounds.right <= window.innerWidth;
    })).toBe(true);
  }

  test("failed results stay actionable and offer Settings without retrying insertion", async ({ page }) => {
    await openOverlay(page);
    await emitOverlay(page, { type: "error", message: "Microphone access was denied" });
    await expect(page.getByRole("button", { name: "Dismiss", exact: true })).toBeVisible();
    await expect(page.getByRole("button", { name: "Copy", exact: true })).toHaveCount(0);
    await page.waitForTimeout(4200);
    await expect(page.getByText("Microphone access was denied", { exact: true })).toBeVisible();
    await page.evaluate(() => { window.onboardingTest.state.failures.open_recovery_settings = "Settings window unavailable"; });
    await page.getByRole("button", { name: "Open Settings", exact: true }).click();
    await expect(page.getByRole("alert")).toContainText("Settings window unavailable");
    await page.evaluate(() => { delete window.onboardingTest.state.failures.open_recovery_settings; });
    await page.getByRole("button", { name: "Open Settings", exact: true }).click();
    await expect(page.getByRole("alert")).toHaveCount(0);
    await fitsWindow(page);
    await page.getByRole("button", { name: "Dismiss", exact: true }).click();
    await expect(page.locator(".pill")).toHaveCount(0);
  });

  test("generation failures link to inference and remain dismissible", async ({ page }) => {
    await openOverlay(page);
    await emitOverlay(page, {
      type: "finished", capped: false,
      report: { job_id: 2, profile_id: "generic", elapsed_ms: 100, history_saved: false, structure: null, generation_elapsed_ms: 0, structure_repair_attempts: 0,
        outcome: { kind: "failed", reason: "generation_failed", detail: "Model unavailable" } },
    });
    await expect(page.getByRole("button", { name: "Inference settings", exact: true })).toBeVisible();
    await expect(page.getByRole("button", { name: "Dismiss", exact: true })).toBeVisible();
    await page.getByRole("button", { name: "Inference settings", exact: true }).click();
    expect(await page.evaluate(() => window.onboardingTest.calls.includes("open_recovery_settings"))).toBe(true);
  });

  test("quality review hides reviewer output and preserves copy-only recovery", async ({ page }) => {
    await openOverlay(page);
    await emitOverlay(page, { type: "listening", mode: "prompt", profile: "ChatGPT", target: "chatgpt.com", latched: false });
    await emitOverlay(page, { type: "stage", stage: "generating" });
    await emitOverlay(page, { type: "token", text: "A valid draft." });
    await expect(page.getByRole("region", { name: "Prompt preview" })).toContainText("A valid draft.");
    await emitOverlay(page, { type: "stage", stage: "reviewing_quality" });
    await expect(page.getByRole("status")).toHaveText("Reviewing wording quality…");
    await expect(page.getByRole("region", { name: "Prompt preview" })).toHaveCount(0);
    await emitOverlay(page, {
      type: "finished",
      capped: false,
      report: {
        job_id: 3, profile_id: "chatgpt", elapsed_ms: 100, history_saved: false, structure: "valid",
        generation_elapsed_ms: 50, structure_repair_attempts: 0,
        quality: { status: "rejected", review_calls: 2, rewrite_calls: 1, generation_elapsed_ms: 50, review_elapsed_ms: 20, rewrite_elapsed_ms: 10, deadline_exhausted: false },
        outcome: { kind: "blocked", text: "Review-only wording", reason: "quality_review", detail: "It was not pasted." },
      },
    });
    await expect(page.getByText("Quality review did not approve this text. Review and copy it; it was not pasted.", { exact: true })).toBeVisible();
    await expect(page.getByText("Review-only wording", { exact: true })).toBeVisible();
    await expect(page.getByRole("button", { name: "Copy", exact: true })).toBeVisible();
    await expect(page.getByRole("button", { name: "Dismiss", exact: true })).toBeVisible();
  });

  test("overlay event connection failure offers a functional reconnect", async ({ page }) => {
    const state = fixture(true);
    state.failures["plugin:event|listen"] = "IPC interrupted";
    await launch(page, state, "/overlay.html");
    await expect(page.getByRole("alert")).toContainText("IPC interrupted");
    await expect(page.getByRole("button", { name: "Dismiss", exact: true })).toBeVisible();
    await page.evaluate(() => { delete window.onboardingTest.state.failures["plugin:event|listen"]; });
    await page.getByRole("button", { name: "Retry overlay connection" }).click();
    await expect(page.getByRole("button", { name: "Retry overlay connection" })).toHaveCount(0);
    await emitOverlay(page, { type: "error", message: "New event received" });
    await expect(page.getByText("New event received", { exact: true })).toBeVisible();
  });

  const longGraph = [
    "Explain a heat pump to a beginner.",
    "Step 1: Explain how a heat pump transfers heat, using a relatable example.",
    ...Array.from({ length: 20 }, () => "Preserve the user's stated context, constraints, and desired level of detail."),
    "Step 2 (after 1): Verify that the explanation is accurate and understandable.",
    "Loop: if Step 2 fails the accuracy or clarity checks, return to Step 1 to correct errors and unclear wording; then recheck Step 2 (max 2 rounds).",
    "Done when: the explanation is clear and accurate.",
  ].join("\n");

  const blocked: OverlayEvent = {
    type: "finished", capped: false,
    report: {
      job_id: 1, profile_id: "grok", elapsed_ms: 1000, history_saved: true, structure: "valid", generation_elapsed_ms: 0, structure_repair_attempts: 0,
      outcome: {
        kind: "blocked", text: longGraph, reason: "surface_unconfirmed",
        detail: "The focused AI input is not confirmed. Review and copy the result, or explicitly select the input surface for this request.",
      },
      routing: {
        version: 1, target_profile_id: "grok", target_name: "Grok", surface: "chat", task_type: "info.explain",
        task_label: "Explain a concept", secondary_tasks: [], form: "graph", conversational: false,
        reason: "matched", auto_paste: false, warnings: [], max_chars: null, newlines: "keep",
      },
    },
  };

  test("transcript and generation expand without clipping at 150% scale", async ({ page }) => {
    await openOverlay(page);
    await emitOverlay(page, { type: "listening", mode: "prompt", profile: "Grok", target: "zen", latched: false });
    await emitOverlay(page, { type: "partial", text: longGraph });
    await expect(page.getByRole("region", { name: "Live transcript" })).toBeVisible();
    await fitsWindow(page);
    await expect.poll(() => page.evaluate(() => innerHeight)).toBeGreaterThan(180);
    await emitOverlay(page, { type: "stage", stage: "generating" });
    await emitOverlay(page, { type: "token", text: longGraph });
    await fitsWindow(page);
    const preview = page.getByRole("region", { name: "Prompt preview" });
    expect(await preview.evaluate((element) => element.scrollHeight > element.clientHeight && element.scrollTop > 0)).toBe(true);
  });

  test("final prompts expand fully even beyond the old 400px limit", async ({ page }) => {
    await openOverlay(page);
    // Some native webviews expose the tiny overlay viewport as screen.availHeight.
    await page.evaluate(() => Object.defineProperty(window.screen, "availHeight", { value: 96, configurable: true }));
    await emitOverlay(page, blocked);
    await fitsWindow(page);
    const result = page.getByRole("region", { name: "Prompt result" });
    await expect.poll(() => page.evaluate(() => innerHeight)).toBeGreaterThan(440);
    expect(await result.evaluate((element) => element.scrollHeight <= element.clientHeight)).toBe(true);
    await expect(result).toContainText("Done when:");
  });

  test("screen-height blocked results keep Copy and Dismiss inside the native window", async ({ page }, testInfo) => {
    await openOverlay(page);
    const oversized = structuredClone(blocked);
    if (oversized.type !== "finished" || oversized.report.outcome.kind !== "blocked") throw new Error("Expected blocked fixture");
    oversized.report.outcome.text = longGraph.repeat(4);
    await emitOverlay(page, oversized);
    await fitsWindow(page);
    const copy = page.getByRole("button", { name: "Copy", exact: true });
    await expect(copy).toBeVisible();
    expect(await copy.evaluate((element) => element.getBoundingClientRect().bottom <= innerHeight)).toBe(true);
    const result = page.getByRole("region", { name: "Prompt result" });
    expect(await result.evaluate((element) => element.clientHeight)).toBeGreaterThanOrEqual(120);
    expect(await result.evaluate((element) => element.scrollHeight > element.clientHeight)).toBe(true);
    expect(await page.evaluate(() => innerHeight)).toBeGreaterThanOrEqual(1050);
    expect(await page.evaluate(() => innerHeight)).toBeLessThanOrEqual(1064);
    await result.evaluate((element) => { element.scrollTop = element.scrollHeight; });
    expect(await result.evaluate((element) => element.scrollTop > 0)).toBe(true);
    await page.screenshot({ path: testInfo.outputPath("overlay-result-fits.png") });
    await page.getByRole("button", { name: "Dismiss", exact: true }).click();
    await expect(page.locator(".pill")).toHaveCount(0);
    expect(await page.evaluate(() => window.onboardingTest.calls.includes("hide_overlay"))).toBe(true);
  });

  test("clipboard errors are visible and retryable instead of unhandled", async ({ page }) => {
    await openOverlay(page);
    await emitOverlay(page, blocked);
    await fitsWindow(page);
    await page.evaluate(() => { window.onboardingTest.state.failures.copy_last_result = "Clipboard is busy"; });
    await page.getByRole("button", { name: "Copy", exact: true }).click();
    await expect(page.getByRole("alert")).toContainText("Clipboard is busy");
    await fitsWindow(page);
    expect(await page.evaluate(() => window.onboardingTest.calls.includes("hide_overlay"))).toBe(false);
    await page.evaluate(() => { delete window.onboardingTest.state.failures.copy_last_result; });
    await page.getByRole("button", { name: "Copy", exact: true }).click();
    await expect(page.locator(".pill")).toHaveCount(0);
  });

  test("dispatched paste is not presented as verified delivery and retains recovery", async ({ page }) => {
    await openOverlay(page);
    await emitOverlay(page, {
      type: "finished", capped: false,
      report: {
        job_id: 1, profile_id: "generic", elapsed_ms: 100, history_saved: false, structure: "valid", generation_elapsed_ms: 0, structure_repair_attempts: 0,
        delivery: "sent_unverified", outcome: { kind: "inserted", text: longGraph },
      },
    });
    await expect(page.getByText("Paste sent", { exact: true })).toBeVisible();
    await expect(page.getByText("sending a paste does not verify", { exact: false })).toBeVisible();
    await expect(page.getByRole("button", { name: "Copy", exact: true })).toBeVisible();
    await expect(page.getByRole("button", { name: "Restore previous clipboard" })).toBeVisible();
    await fitsWindow(page);
    await page.evaluate(() => { window.onboardingTest.state.failures.restore_insertion_clipboard = "Clipboard is busy"; });
    await page.getByRole("button", { name: "Restore previous clipboard" }).click();
    await expect(page.getByRole("alert")).toContainText("Clipboard is busy");
    await page.evaluate(() => { delete window.onboardingTest.state.failures.restore_insertion_clipboard; });
    await page.getByRole("button", { name: "Restore previous clipboard" }).click();
    await expect(page.getByText("Previous clipboard restored.", { exact: true })).toBeVisible();
    await expect(page.getByRole("button", { name: "Restore previous clipboard" })).toHaveCount(0);
  });

  test("an old clipboard restore response cannot alter a newer overlay result", async ({ page }) => {
    await openOverlay(page);
    await emitOverlay(page, {
      type: "finished", capped: false,
      report: {
        job_id: 1, profile_id: "generic", elapsed_ms: 100, history_saved: false, structure: "valid", generation_elapsed_ms: 0, structure_repair_attempts: 0,
        delivery: "sent_unverified", outcome: { kind: "inserted", text: longGraph },
      },
    });
    await page.evaluate(() => { window.onboardingTest.state.delays = { restore_insertion_clipboard: 500 }; });
    await page.getByRole("button", { name: "Restore previous clipboard" }).click();
    await expect(page.getByRole("button", { name: "Restore previous clipboard" })).toBeDisabled();
    await emitOverlay(page, {
      type: "finished", capped: false,
      report: {
        job_id: 2, profile_id: "generic", elapsed_ms: 100, history_saved: false, structure: "valid", generation_elapsed_ms: 0, structure_repair_attempts: 0,
        outcome: { kind: "blocked", text: "New prompt", reason: "focus_changed", detail: null },
      },
    });
    await expect(page.getByText("New prompt", { exact: true })).toBeVisible();
    await expect(page.getByRole("button", { name: "Restore previous clipboard" })).toHaveCount(0);
    await page.waitForFunction(() => window.onboardingTest.state.info.clipboard_restore_pending === false);
    await expect(page.getByText("Previous clipboard restored.", { exact: true })).toHaveCount(0);
    await expect(page.getByText("New prompt", { exact: true })).toBeVisible();
  });
});

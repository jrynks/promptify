import { expect, test, type Page } from "@playwright/test";
import type { AppInfo, ModelStatus, OnboardingStatus, OverlayEvent, PromptCatalog, Rendering } from "../src/api";

interface Fixture {
  info: AppInfo;
  status: OnboardingStatus;
  models: ModelStatus[];
  failures: Record<string, string>;
  catalog: PromptCatalog;
}

interface TestBridge {
  state: Fixture;
  calls: string[];
  change: (update: Partial<Fixture>) => void;
  emit: (event: string, payload: unknown) => void;
  practice: (event: OverlayEvent, attempt?: number) => void;
  lastRouting: unknown;
  lastPreview: unknown;
  overlaySizes: number[];
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
      hotkeys: { prompt: "CommandOrControl+Alt+Space", dictation: "CommandOrControl+Alt+Shift+Space", answer: null, cancel: "Escape" },
      hotkey_errors: [], prompt_hotkey_error: null, hotkeys_paused: false,
      engines_ready: ready, models_installed: ready, engine_error: null,
      history_enabled: true, data_dir: "isolated-test-data", use_gpu: true, gpu_device: null,
      modifier_hold: false, auto_mode: false, vocabulary: { words: [], replacements: [] }, screen_text_apps: [],
      code_chat_paste: false,
      modifier_hold_error: null,
      modifier_hold_requested: false,
      modifier_keyboard: null,
      modifier_keyboard_devices: [],
      paste_permission: "not_needed",
      desktop_error: null,
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
      state: initial, calls: [], emit, lastRouting: null, lastPreview: null, overlaySizes: [],
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
          if (initial.failures[command]) throw new Error(initial.failures[command]);
          switch (command) {
            case "plugin:event|listen": {
              if (typeof args.event !== "string" || typeof args.handler !== "number") throw new Error("Invalid listener");
              const id = ++sequence;
              listeners.set(id, { event: args.event, handler: args.handler });
              return id;
            }
            case "plugin:event|unlisten": listeners.delete(Number(args.eventId)); return;
            case "app_info": return structuredClone(initial.info);
            case "grant_paste_permission":
              initial.info.paste_permission = "granted";
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
            case "list_profiles":
            case "list_history": return [];
            case "remote_info": return { mobile_available: false, enabled: false, relay_url: null, lan_direct: false, lan_discovery: false, status: null, error: null, devices: [], api_token_path: "test-token-path" };
            case "mcp_info": return { path: "test-mcp-path", error: null, active: false, servers: [] };
            case "mcp_read": return { text: "", exists: false, template: "" };
            case "resize_overlay":
              if (typeof args.height !== "number" || !Number.isInteger(args.height) || args.height <= 0) throw new Error("Invalid overlay size");
              window.onboardingTest.overlaySizes.push(args.height);
              if (window.resizeTestOverlay) await window.resizeTestOverlay(args.height);
              return;
            case "overlay_max_height": return 1064;
            case "copy_last_result":
            case "hide_overlay": return;
            case "routing_state": return { rendering, error: null };
            case "prompt_catalog": return structuredClone(initial.catalog);
            case "set_rendering":
              if (args.rendering !== "legacy" && args.rendering !== "adaptive") throw new Error("Invalid rendering");
              rendering = args.rendering;
              sessionStorage.setItem("test.routing", rendering);
              return { rendering, error: null };
            case "queue_prompt_routing":
              window.onboardingTest.lastRouting = structuredClone(args.options);
              return;
            case "preview_prompt": {
              window.onboardingTest.lastPreview = structuredClone(args.input);
              const input = args.input;
              if (!input || typeof input !== "object" || !("routing" in input)) throw new Error("Missing preview routing");
              const options = input.routing;
              if (!options || typeof options !== "object" || !("surface" in options) || !("rendering" in options)) throw new Error("Invalid preview routing");
              if (options.surface === "literal") throw new Error("This is a literal-content field. Use Dictation.");
              return {
                profileId: "chatgpt", profileName: "ChatGPT",
                messages: [{ role: "system", content: "Write only the prompt." }, { role: "user", content: "Debug the checkout crash" }],
                routing: options.rendering === "adaptive" ? {
                  version: 1, target_profile_id: "chatgpt", target_name: "ChatGPT", surface: options.surface ?? "chat",
                  task_type: "code.debug", task_label: "Diagnose and fix a defect", secondary_tasks: [],
                  form: "graph", conversational: false, reason: "matched", auto_paste: true, warnings: [], max_chars: null, newlines: "keep",
                } : null,
              };
            }
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
    report: { job_id: 7, profile_id: "generic", outcome: { kind: "inserted", text }, elapsed_ms: 1, history_saved: false, structure: null },
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

test("code chat automatic paste consent is explicit, remembered and reversible", async ({ page }) => {
  const state = fixture(true);
  state.status.required = false;
  await launch(page, state);
  const consent = page.getByRole("checkbox", { name: "Allow automatic prompt paste in VS Code and Cursor AI chat" });
  await expect(consent).not.toBeChecked();
  await expect(page.getByText("Promptify cannot distinguish it from an editor", { exact: false })).toBeVisible();
  await consent.check();
  await page.reload();
  await expect(consent).toBeChecked();
  await consent.uncheck();
  await page.reload();
  await expect(consent).not.toBeChecked();
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
  await expect(page.getByRole("heading", { name: "Local models" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Continue to microphone and shortcut" })).toBeDisabled();
  await expect(page.getByRole("button", { name: "Tools (MCP)", exact: true })).toBeDisabled();
  expect(await page.evaluate(() => window.onboardingTest.calls.includes("remote_info"))).toBe(false);
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
  await expect(page.getByRole("heading", { name: "Local models" })).toBeVisible();
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
  await page.getByRole("button", { name: "Grant paste permission" }).click();
  await expect(page.getByText("Automatic paste: keyboard permission is active.")).toBeVisible();
  await expect(page.getByRole("button", { name: "Continue to practice" })).toBeEnabled();
});

test("denied paste permission is explicit and retryable", async ({ page }) => {
  const state = fixture(true);
  state.status.step = "input";
  state.info.paste_permission = "required";
  state.failures.grant_paste_permission = "Paste permission was not granted";
  await launch(page, state);
  await page.getByRole("button", { name: "Grant paste permission" }).click();
  await expect(page.getByRole("alert").filter({ hasText: "Paste permission was not granted" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Continue to practice" })).toBeDisabled();
  await page.evaluate(() => { delete window.onboardingTest.state.failures.grant_paste_permission; });
  await page.getByRole("button", { name: "Grant paste permission" }).click();
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
});

test("permission loss during practice offers recovery without skipping verification", async ({ page }) => {
  await launchPractice(page);
  await page.evaluate(() => {
    window.onboardingTest.state.info.paste_permission = "required";
    window.onboardingTest.change({});
  });
  await expect(page.getByRole("button", { name: "Prepare practice" })).toBeDisabled();
  await expect(page.getByRole("button", { name: "Finish setup" })).toBeDisabled();
  await page.getByRole("button", { name: "Grant paste permission" }).click();
  await expect(page.getByRole("button", { name: "Prepare practice" })).toBeEnabled();
  await expect(page.getByRole("button", { name: "Finish setup" })).toBeDisabled();
});

test("configured users can recover paste permission in General Settings", async ({ page }) => {
  const state = fixture(true);
  state.status.required = false;
  state.info.paste_permission = "required";
  await launch(page, state);
  await page.getByRole("button", { name: "Grant paste permission" }).click();
  await expect(page.getByText("Automatic paste: keyboard permission is active.")).toBeVisible();
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

test("Wayland Ctrl+Shift monitors an explicitly selected keyboard only", async ({ page }) => {
  const state = fixture(true);
  state.status.required = false;
  state.info.modifier_keyboard_devices = [
    { path: "/dev/input/by-id/keyboard-event-kbd", name: "Physical keyboard" },
    { path: "/dev/input/by-id/consumer-event-kbd", name: "Consumer controls" },
  ];
  await launch(page, state);
  await page.getByRole("combobox", { name: "Ctrl+Shift keyboard" }).selectOption("/dev/input/by-id/keyboard-event-kbd");
  expect(await page.evaluate(() => window.onboardingTest.state.info.modifier_keyboard)).toBe("/dev/input/by-id/keyboard-event-kbd");
  await page.getByRole("checkbox", { name: /Also start a prompt by holding/ }).check();
  await expect(page.getByRole("checkbox", { name: /Also start a prompt by holding/ })).toBeChecked();
  await page.reload();
  await expect(page.getByRole("combobox", { name: "Ctrl+Shift keyboard" })).toHaveValue("/dev/input/by-id/keyboard-event-kbd");
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
  await expect(page.getByRole("button", { name: "Tools (MCP)", exact: true })).toBeEnabled();
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
    report: { job_id: 2, profile_id: "generic", outcome: { kind: "inserted", text: "Stale result" }, elapsed_ms: 1, history_saved: false, structure: null },
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
  await expect(page.getByRole("heading", { name: "Local models" })).toBeVisible();
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
  await expect(page.getByRole("heading", { name: "Local models" })).toBeVisible();
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
  await page.getByRole("button", { name: "Advanced", exact: true }).click();
  await expect(page.getByRole("combobox", { name: "Rendering", exact: true })).toHaveValue("adaptive");
  await page.reload();
  await page.getByRole("button", { name: "Prompt types", exact: true }).click();
  await expect(enabled).toBeChecked();
  await page.evaluate(() => { window.onboardingTest.state.failures.set_rendering = "Routing settings cannot be saved"; });
  await enabled.click();
  await expect(page.getByRole("alert").filter({ hasText: "cannot be saved" })).toBeVisible();
  await expect(enabled).toBeChecked();
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

test("adaptive playground exposes routing and rejects literal fields", async ({ page }, testInfo) => {
  const state = fixture(true);
  state.status.required = false;
  await launch(page, state);
  await page.getByRole("button", { name: "Advanced", exact: true }).click();
  const panel = page.locator("section").filter({ has: page.getByRole("heading", { name: "Context playground" }) });
  await panel.getByRole("combobox", { name: "Rendering", exact: true }).selectOption("adaptive");
  await panel.getByLabel("Spoken text").fill("Debug the checkout crash");
  await panel.getByRole("combobox", { name: "Input surface", exact: true }).selectOption("code_chat");
  await panel.getByRole("button", { name: "Preview", exact: true }).click();
  await expect(panel.locator(".routing-summary")).toContainText("code.debug");
  expect(await page.evaluate(() => window.onboardingTest.lastPreview)).toMatchObject({
    transcript: "Debug the checkout crash", routing: { rendering: "adaptive", task_type: null, surface: "code_chat" },
  });
  await page.screenshot({ path: testInfo.outputPath("adaptive-playground.png"), fullPage: true });
  await panel.getByRole("combobox", { name: "Input surface", exact: true }).selectOption("literal");
  await panel.getByRole("button", { name: "Preview", exact: true }).click();
  await expect(panel.locator(".error")).toContainText("literal-content field");
  await expect(panel.locator(".routing-summary")).toHaveCount(0);
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

  const longGraph = [
    "Explain a heat pump to a beginner.",
    "Step 1: Explain how a heat pump transfers heat, using a relatable example.",
    ...Array.from({ length: 20 }, () => "Preserve the user's stated context, constraints, and desired level of detail."),
    "Step 2 (after 1): Verify that the explanation is accurate and understandable.",
    "Loop: if the explanation is unclear, return to Step 1 (max 2 rounds).",
    "Done when: the explanation is clear and accurate.",
  ].join("\n");

  const blocked: OverlayEvent = {
    type: "finished", capped: false,
    report: {
      job_id: 1, profile_id: "grok", elapsed_ms: 1000, history_saved: true, structure: "valid",
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
});

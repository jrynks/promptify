use promptify_core::context::{ActiveContext, AdmittedText};
use promptify_core::dictation::remove_fillers;
use promptify_core::history::HistoryEntry;
use promptify_core::models::{ModelKind, ModelTier, final_path, is_installed, part_path};
use promptify_core::pipeline::Mode;
use promptify_core::profiles::ProfileKind;
use promptify_core::prompt::{ChatMessage, PromptRequest, build_prompt_messages};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::hotkeys::HotkeyConfig;
use crate::{AppState, ensure_selection, preload_engines, settings};

const MAX_TRANSCRIPT_CHARS: usize = 20_000;
const MAX_SURROUNDING_CHARS: usize = 4_000;
const MAX_FIELD_CHARS: usize = 2_048;
const MAX_HISTORY_LISTED: usize = 200;

fn check_len(label: &str, value: &str, max: usize) -> Result<(), String> {
    if value.chars().count() > max {
        return Err(format!("{label} is longer than {max} characters"));
    }
    Ok(())
}

fn save_settings(state: &AppState) -> Result<(), String> {
    let snapshot = state.settings.read().unwrap().clone();
    settings::save(&state.data_dir, &snapshot).map_err(|e| format!("could not save settings: {e}"))
}

#[derive(Serialize)]
pub struct AppInfo {
    input_device: Option<String>,
    hotkeys: HotkeyConfig,
    hotkey_errors: Vec<String>,
    engines_ready: bool,
    history_enabled: bool,
    data_dir: String,
    use_gpu: bool,
    gpu_device: Option<String>,
    modifier_hold: bool,
    auto_mode: bool,
    vocabulary: promptify_core::dictation::Vocabulary,
    screen_text_apps: Vec<String>,
}

#[tauri::command]
pub fn app_info(state: State<'_, AppState>) -> AppInfo {
    let settings = state.settings.read().unwrap().clone();
    let ready = |id: &Option<String>| id.as_deref().and_then(|id| state.manifest.get(id)).is_some_and(|e| is_installed(&state.models_dir, e));
    AppInfo {
        input_device: crate::audio::default_input_name(),
        hotkeys: state.hotkeys.read().unwrap().config.clone(),
        hotkey_errors: state.hotkeys.read().unwrap().errors(),
        engines_ready: ready(&settings.stt_model) && ready(&settings.llm_model),
        history_enabled: state.history.is_enabled(),
        data_dir: state.data_dir.to_string_lossy().into_owned(),
        use_gpu: settings.use_gpu,
        gpu_device: state.llm.device(),
        modifier_hold: state.modifier_hook.lock().unwrap().is_some(),
        auto_mode: settings.auto_mode,
        vocabulary: settings.vocabulary.clone(),
        screen_text_apps: settings.screen_text_apps.clone(),
    }
}

#[derive(Serialize)]
pub struct ModelStatus {
    id: String,
    kind: ModelKind,
    tier: ModelTier,
    display_name: String,
    size_bytes: u64,
    license: String,
    min_ram_gb: u32,
    installed: bool,
    selected: bool,
    downloading: bool,
    partial_bytes: u64,
}

#[tauri::command]
pub fn list_models(state: State<'_, AppState>) -> Vec<ModelStatus> {
    let settings = state.settings.read().unwrap().clone();
    let active = state.downloads.active_ids();
    state
        .manifest
        .models
        .iter()
        .map(|e| ModelStatus {
            id: e.id.clone(),
            kind: e.kind,
            tier: e.tier,
            display_name: e.display_name.clone(),
            size_bytes: e.size_bytes,
            license: e.license.clone(),
            min_ram_gb: e.min_ram_gb,
            installed: is_installed(&state.models_dir, e),
            selected: [&settings.stt_model, &settings.llm_model].iter().any(|s| s.as_deref() == Some(e.id.as_str())),
            downloading: active.contains(&e.id),
            partial_bytes: std::fs::metadata(part_path(&state.models_dir, e)).map_or(0, |m| m.len()),
        })
        .collect()
}

#[derive(Clone, Serialize)]
struct DownloadEvent {
    id: String,
    downloaded: u64,
    total: u64,
    verifying: bool,
    done: bool,
    error: Option<String>,
}

#[tauri::command]
pub fn download_model(app: AppHandle, state: State<'_, AppState>, id: String) -> Result<(), String> {
    let entry = state.manifest.get(&id).cloned().ok_or("unknown model")?;
    let cancel = state.downloads.start(&id).ok_or("already downloading")?;
    let models_dir = state.models_dir.clone();
    std::thread::Builder::new()
        .name(format!("download-{id}"))
        .spawn(move || {
            let emit = |event: DownloadEvent| {
                let _ = app.emit_to("settings", "model-download", event);
            };
            let result = crate::download::download(&models_dir, &entry, &cancel, &mut |p| {
                emit(DownloadEvent { id: entry.id.clone(), downloaded: p.downloaded, total: p.total, verifying: p.verifying, done: false, error: None });
            });
            let state = app.state::<AppState>();
            state.downloads.finish(&entry.id);
            let error = result.err();
            if error.is_none() {
                let changed = ensure_selection(&state.manifest, &state.models_dir, &mut state.settings.write().unwrap());
                if changed {
                    let _ = save_settings(&state);
                }
                preload_engines(state.stt.clone(), state.llm.clone());
            }
            log::info!("model download {} finished ok={}", entry.id, error.is_none());
            emit(DownloadEvent { id: entry.id.clone(), downloaded: 0, total: entry.size_bytes, verifying: false, done: true, error });
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn cancel_download(state: State<'_, AppState>, id: String) -> bool {
    state.downloads.cancel(&id)
}

#[tauri::command]
pub fn delete_model(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let entry = state.manifest.get(&id).ok_or("unknown model")?;
    if state.downloads.active_ids().contains(&id) {
        return Err("cancel the download first".into());
    }
    for path in [final_path(&state.models_dir, entry), part_path(&state.models_dir, entry)] {
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("could not delete: {e}")),
        }
    }
    ensure_selection(&state.manifest, &state.models_dir, &mut state.settings.write().unwrap());
    save_settings(&state)
}

#[tauri::command]
pub fn select_model(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let entry = state.manifest.get(&id).ok_or("unknown model")?;
    if !is_installed(&state.models_dir, entry) {
        return Err("download the model first".into());
    }
    {
        let mut settings = state.settings.write().unwrap();
        match entry.kind {
            ModelKind::Stt => settings.stt_model = Some(id),
            ModelKind::Llm => settings.llm_model = Some(id),
        }
    }
    save_settings(&state)?;
    preload_engines(state.stt.clone(), state.llm.clone());
    Ok(())
}

/// Most recent first.
#[tauri::command]
pub fn list_history(state: State<'_, AppState>) -> Vec<HistoryEntry> {
    state.history.entries().into_iter().rev().take(MAX_HISTORY_LISTED).collect()
}

#[tauri::command]
pub fn delete_history_entry(state: State<'_, AppState>, id: u64) -> Result<bool, String> {
    state.history.delete(id).map_err(|e| format!("could not update history: {e}"))
}

#[tauri::command]
pub fn clear_history(state: State<'_, AppState>) -> Result<(), String> {
    state.history.clear().map_err(|e| format!("could not clear history: {e}"))
}

#[tauri::command]
pub fn set_history_enabled(state: State<'_, AppState>, enabled: bool) -> Result<(), String> {
    state.settings.write().unwrap().history_enabled = enabled;
    save_settings(&state)?;
    state.history.set_enabled(enabled);
    Ok(())
}

#[tauri::command]
pub fn set_use_gpu(state: State<'_, AppState>, enabled: bool) -> Result<(), String> {
    state.settings.write().unwrap().use_gpu = enabled;
    save_settings(&state)?;
    preload_engines(state.stt.clone(), state.llm.clone());
    Ok(())
}

#[tauri::command]
pub fn set_modifier_hold(app: AppHandle, state: State<'_, AppState>, enabled: bool) -> Result<(), String> {
    crate::modifier_hook::apply(&app, &state.modifier_hook, enabled)?;
    state.settings.write().unwrap().modifier_hold = enabled;
    save_settings(&state)
}

/// Pushes the text-related settings into the pipeline; called at startup and after each change.
pub fn apply_text_settings(orchestrator: &promptify_core::pipeline::Orchestrator, settings: &settings::AppSettings) {
    orchestrator.set_auto_mode(settings.auto_mode);
    orchestrator.service().set_vocabulary(settings.vocabulary.clone());
    let apps = settings.screen_text_apps.iter().map(|a| a.trim().to_lowercase()).filter(|a| !a.is_empty()).collect();
    orchestrator.set_policy(promptify_core::context::ContextPolicy { surrounding_text_apps: apps, ..Default::default() });
}

fn update_text_settings(state: &AppState, change: impl FnOnce(&mut settings::AppSettings)) -> Result<(), String> {
    change(&mut state.settings.write().unwrap());
    save_settings(state)?;
    apply_text_settings(&state.orchestrator, &state.settings.read().unwrap());
    Ok(())
}

#[tauri::command]
pub fn set_auto_mode(state: State<'_, AppState>, enabled: bool) -> Result<(), String> {
    update_text_settings(&state, |s| s.auto_mode = enabled)
}

#[tauri::command]
pub fn set_vocabulary(state: State<'_, AppState>, vocabulary: promptify_core::dictation::Vocabulary) -> Result<promptify_core::dictation::Vocabulary, String> {
    let clean = vocabulary.sanitized();
    let saved = clean.clone();
    update_text_settings(&state, |s| s.vocabulary = clean)?;
    Ok(saved)
}

#[tauri::command]
pub fn set_screen_text_apps(state: State<'_, AppState>, apps: Vec<String>) -> Result<(), String> {
    let apps: Vec<String> = apps.into_iter().map(|a| a.trim().to_lowercase()).filter(|a| !a.is_empty() && a.len() <= 100).take(50).collect();
    update_text_settings(&state, |s| s.screen_text_apps = apps)
}

#[tauri::command]
pub fn set_hotkey(app: AppHandle, state: State<'_, AppState>, mode: Mode, accelerator: String) -> Result<(), String> {
    check_len("hotkey", &accelerator, 64)?;
    crate::hotkeys::rebind(&app, &mut state.hotkeys.write().unwrap(), mode, &accelerator)?;
    crate::tray::refresh(&app);
    {
        let mut settings = state.settings.write().unwrap();
        match mode {
            Mode::Prompt => settings.prompt_hotkey = Some(accelerator),
            Mode::Dictation => settings.dictation_hotkey = Some(accelerator),
            Mode::Answer => settings.answer_hotkey = Some(accelerator),
        }
    }
    save_settings(&state)
}

#[derive(Serialize)]
pub struct ProfileSummary {
    id: String,
    name: String,
    kind: ProfileKind,
}

#[tauri::command]
pub fn list_profiles(state: State<'_, AppState>) -> Vec<ProfileSummary> {
    state
        .orchestrator
        .profiles()
        .all()
        .iter()
        .map(|p| ProfileSummary { id: p.id.clone(), name: p.name.clone(), kind: p.kind })
        .collect()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreviewInput {
    transcript: String,
    process_name: String,
    window_title: String,
    url: Option<String>,
    surrounding_text: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewOutput {
    profile_id: String,
    profile_name: String,
    messages: Vec<ChatMessage>,
}

/// Shows which profile a context resolves to and the exact messages the model would receive.
#[tauri::command]
pub fn preview_prompt(state: State<'_, AppState>, input: PreviewInput) -> Result<PreviewOutput, String> {
    check_len("transcript", &input.transcript, MAX_TRANSCRIPT_CHARS)?;
    check_len("process name", &input.process_name, MAX_FIELD_CHARS)?;
    check_len("window title", &input.window_title, MAX_FIELD_CHARS)?;
    check_len("url", input.url.as_deref().unwrap_or(""), MAX_FIELD_CHARS)?;
    check_len("surrounding text", input.surrounding_text.as_deref().unwrap_or(""), MAX_SURROUNDING_CHARS)?;
    if input.transcript.trim().is_empty() {
        return Err("transcript is empty".into());
    }
    let ctx = ActiveContext {
        process_name: input.process_name,
        window_title: input.window_title,
        url: input.url.filter(|u| !u.trim().is_empty()),
        ..Default::default()
    };
    let profile = state.orchestrator.profiles().resolve(&ctx);
    let surrounding = input
        .surrounding_text
        .filter(|s| !s.trim().is_empty())
        .map(|text| AdmittedText { text, truncated: false });
    let label = ctx.url_host().unwrap_or_else(|| ctx.normalized_process());
    let history = state.history.context(&profile.id, &ctx.app_key());
    let messages = build_prompt_messages(&PromptRequest {
        transcript: &input.transcript,
        profile,
        target_label: &label,
        surrounding: surrounding.as_ref(),
        history: &history,
        tool_context: &[],
        tool_context_omitted: 0,
    });
    Ok(PreviewOutput { profile_id: profile.id.clone(), profile_name: profile.name.clone(), messages })
}

#[tauri::command]
pub fn clean_dictation(text: String) -> Result<String, String> {
    check_len("text", &text, MAX_TRANSCRIPT_CHARS)?;
    Ok(remove_fillers(&text))
}

/// Copies the last result that could not be inserted. The text never comes from the webview.
#[tauri::command]
pub fn copy_last_result(state: State<'_, AppState>) -> Result<(), String> {
    let text = state.controller.last_result.lock().unwrap().clone().ok_or("nothing to copy")?;
    arboard::Clipboard::new().and_then(|mut c| c.set_text(text)).map_err(|e| format!("clipboard unavailable: {e}"))
}

#[tauri::command]
pub fn hide_overlay(app: AppHandle) {
    if let Some(overlay) = app.get_webview_window("overlay") {
        let _ = overlay.hide();
    }
}

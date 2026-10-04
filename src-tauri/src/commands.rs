use promptify_core::dictation::remove_fillers;
use promptify_core::history::HistoryEntry;
use promptify_core::models::{ModelKind, ModelTier, final_path, is_installed, part_path};
use promptify_core::pipeline::Mode;
use promptify_core::routing::{self, Rendering, RoutingOptions, Surface};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::hotkeys::HotkeyConfig;
use crate::{AppState, ensure_selection, onboarding, preload_engines, settings};

const MAX_TRANSCRIPT_CHARS: usize = 20_000;
const MAX_HISTORY_LISTED: usize = 200;
static DESKTOP_INTEGRATION_CHANGE: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Deserialize, Serialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum RecoverySection {
    General,
    Models,
    Prompts,
}

#[cfg(test)]
mod recovery_tests {
    use super::{RecoverySection, SystemSettings};

    #[test]
    fn recovery_targets_accept_only_fixed_sections() {
        for section in ["general", "models", "prompts"] {
            assert!(serde_json::from_value::<RecoverySection>(serde_json::json!(section)).is_ok());
        }
        for section in ["microphone", "accessibility", "input_monitoring"] {
            assert!(serde_json::from_value::<SystemSettings>(serde_json::json!(section)).is_ok());
        }
        for invalid in ["shell", "https://example.com", "models; arbitrary-command", "../settings"] {
            assert!(serde_json::from_value::<RecoverySection>(serde_json::json!(invalid)).is_err());
            assert!(serde_json::from_value::<SystemSettings>(serde_json::json!(invalid)).is_err());
        }

    }

    #[test]
    fn startup_restoration_requires_saved_authorization_and_enabled_preference() {
        let mut settings = crate::settings::AppSettings::default();
        assert!(!super::should_restore_integration(&settings));
        settings.desktop_integration_authorized = true;
        assert!(super::should_restore_integration(&settings));
        settings.desktop_integration_enabled = false;
        assert!(!super::should_restore_integration(&settings));
        settings.desktop_integration_authorized = false;
        assert!(!super::should_restore_integration(&settings));
    }

    #[test]
    fn only_actual_wayland_grants_record_startup_consent() {
        let mut settings = crate::settings::AppSettings::default();
        super::enable_preference(&mut settings, None);
        assert!(!settings.desktop_integration_authorized, "X11 enable is not a Wayland grant");
        super::enable_preference(&mut settings, Some(false));
        assert!(!super::should_restore_integration(&settings));
        super::enable_preference(&mut settings, Some(true));
        assert!(super::should_restore_integration(&settings));
        super::enable_preference(&mut settings, None);
        assert!(settings.desktop_integration_authorized, "a real earlier grant is retained on X11");
        super::enable_preference(&mut settings, Some(false));
        assert!(!super::should_restore_integration(&settings));
    }
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum SystemSettings {
    Microphone,
    Accessibility,
    InputMonitoring,
}

#[tauri::command]
pub fn open_recovery_settings(app: AppHandle, section: RecoverySection) -> Result<(), String> {
    let window = app.get_webview_window("settings").ok_or("The Settings window is unavailable.")?;
    window.show().map_err(|error| error.to_string())?;
    window.unminimize().map_err(|error| error.to_string())?;
    window.set_focus().map_err(|error| error.to_string())?;
    app.emit_to("settings", "open-settings-section", section).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn open_data_folder(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    app.opener().open_path(state.data_dir.to_string_lossy(), None::<&str>)
        .map_err(|error| format!("Could not open the app data folder: {error}"))
}

#[tauri::command]
pub fn open_system_settings(app: AppHandle, section: SystemSettings) -> Result<(), String> {
    #[cfg(any(windows, target_os = "macos"))]
    {
        use tauri_plugin_opener::OpenerExt;
        #[cfg(windows)]
        let url = match section {
            SystemSettings::Microphone => "ms-settings:privacy-microphone",
            SystemSettings::Accessibility => "ms-settings:easeofaccess",
            SystemSettings::InputMonitoring => return Err("Windows does not require Input Monitoring permission.".into()),
        };
        #[cfg(target_os = "macos")]
        let url = match section {
            SystemSettings::Microphone => "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone",
            SystemSettings::Accessibility => "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
            SystemSettings::InputMonitoring => "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent",
        };
        app.opener().open_url(url, None::<&str>).map_err(|error| format!("Could not open system settings: {error}"))
    }
    #[cfg(target_os = "linux")]
    {
        let _ = app;
        let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().to_lowercase();
        let (command, panel) = if desktop.split(':').any(|part| part == "kde") {
            ("systemsettings", match section {
                SystemSettings::Microphone => "kcm_pulseaudio",
                SystemSettings::Accessibility => "kcm_access",
                SystemSettings::InputMonitoring => return Err("This Wayland session does not provide permissioned modifier-only monitoring. Disable Ctrl+Shift hold and use the Prompt shortcut.".into()),
            })
        } else if desktop.split(':').any(|part| part == "gnome") {
            ("gnome-control-center", match section {
                SystemSettings::Microphone => "sound",
                SystemSettings::Accessibility => "universal-access",
                SystemSettings::InputMonitoring => return Err("This Wayland session does not provide permissioned modifier-only monitoring. Disable Ctrl+Shift hold and use the Prompt shortcut.".into()),
            })
        } else {
            return Err("No supported system-settings launcher was found for this desktop. Open your desktop's sound or accessibility settings, then retry detection.".into());
        };
        let child = std::process::Command::new(command).arg(panel).spawn()
            .map_err(|error| format!("Could not open system settings with {command}: {error}"))?;
        std::thread::spawn(move || {
            match child.wait_with_output() {
                Ok(output) if output.status.success() => {}
                Ok(output) => log::warn!("system settings exited with {}", output.status),
                Err(error) => log::warn!("could not wait for system settings: {error}"),
            }
        });
        Ok(())
    }
}

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
    prompt_hotkey_error: Option<String>,
    hotkeys_paused: bool,
    engines_ready: bool,
    models_installed: bool,
    engine_error: Option<String>,
    history_enabled: bool,
    data_dir: String,
    use_gpu: bool,
    gpu_device: Option<String>,
    modifier_hold: bool,
    modifier_hold_requested: bool,
    modifier_hold_error: Option<String>,
    modifier_keyboard: Option<String>,
    modifier_keyboard_devices: Vec<crate::modifier_hook::KeyboardDevice>,
    auto_mode: bool,
    code_chat_paste: bool,
    vocabulary: promptify_core::dictation::Vocabulary,
    screen_text_apps: Vec<String>,
    /// Wayland only: whether the one-time remote-input permission for pasting was granted.
    paste_permission: &'static str,
    desktop_error: Option<String>,
    activation_bindings: Vec<ActivationBinding>,
    desktop_integration_enabled: bool,
    clipboard_restore_pending: bool,
}

#[derive(Serialize)]
pub struct ActivationBinding {
    id: String,
    trigger_description: String,
}

fn activation_bindings() -> Vec<ActivationBinding> {
    #[cfg(target_os = "linux")]
    return crate::portal_shortcuts::bindings().into_iter().map(|binding| ActivationBinding {
        id: binding.id, trigger_description: binding.trigger_description,
    }).collect();
    #[cfg(not(target_os = "linux"))]
    Vec::new()
}

#[cfg(target_os = "linux")]
fn paste_permission() -> &'static str {
    if crate::wayland_paste::applies() && !crate::portal_shortcuts::active() {
        return "required";
    }
    match crate::wayland_paste::status() {
        crate::wayland_paste::PermissionState::NotNeeded => "not_needed",
        crate::wayland_paste::PermissionState::Granted => "granted",
        crate::wayland_paste::PermissionState::Required => "required",
    }
}

#[cfg(target_os = "macos")]
fn paste_permission() -> &'static str {
    if crate::destination_macos::trusted() { "granted" } else { "required" }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn paste_permission() -> &'static str {
    "not_needed"
}

#[tauri::command]
pub async fn grant_paste_permission(app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || grant_desktop_integration(&app, false))
        .await.map_err(|error| format!("Desktop integration permission request failed: {error}"))?
}

fn grant_desktop_integration(app: &AppHandle, restoring: bool) -> Result<(), String> {
    let _change = DESKTOP_INTEGRATION_CHANGE.try_lock()
        .map_err(|_| "A desktop integration change is already running. Finish or cancel the system dialog before trying again.".to_string())?;
    if restoring {
        let state = app.state::<AppState>();
        if !should_restore_integration(&state.settings.read().unwrap()) {
            return Ok(());
        }
    }
    #[cfg(target_os = "macos")]
    {
        let result = crate::destination_macos::request_permission().map_err(|error| error.0);
        onboarding::notify(app);
        result?;
        enable_integration(app)?;
        return Ok(());
    }
    #[cfg(target_os = "linux")]
    if crate::wayland_paste::applies() {
        let closed_app = app.clone();
        let shortcut_app = app.clone();
        let result = (|| {
            let denied_app = app.clone();
            crate::wayland_paste::grant_with_lifecycle(move |revoked| {
            let state = closed_app.state::<AppState>();
            if revoked {
                forget_desktop_authorization_if(&closed_app, || {
                    crate::wayland_paste::status() != crate::wayland_paste::PermissionState::Granted
                });
            }
            onboarding::invalidate(&state, Some("Automatic paste permission was closed. Grant permission and try practice again."));
            onboarding::notify(&closed_app);
            }, move || forget_desktop_authorization(&denied_app))?;
            if let Err(error) = crate::portal_shortcuts::grant(shortcut_app) {
                crate::portal_shortcuts::stop();
                crate::wayland_paste::shutdown();
                return Err(error);
            }
            Ok(())
        })();
        onboarding::notify(app);
        result?;
        if let Err(error) = enable_integration(app) {
            crate::portal_shortcuts::stop();
            crate::wayland_paste::shutdown();
            onboarding::notify(app);
            return Err(error);
        }
        return Ok(());
    }
    #[cfg(not(target_os = "macos"))]
    enable_integration(app)?;
    #[cfg(not(target_os = "macos"))]
    Ok(())
}

fn enable_integration(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let mut current = state.settings.write().unwrap();
    let mut next = current.clone();
    #[cfg(target_os = "linux")]
    let grants = if crate::wayland_paste::applies() {
        Some(crate::wayland_paste::status() == crate::wayland_paste::PermissionState::Granted
            && crate::portal_shortcuts::active())
    } else { None };
    #[cfg(not(target_os = "linux"))]
    let grants = None;
    enable_preference(&mut next, grants);
    settings::save(&state.data_dir, &next).map_err(|error| format!("could not save settings: {error}"))?;
    *current = next;
    // Serialize consent and delivery admission with revocation, including the
    // gap between persisting authorization and enabling native delivery.
    state.orchestrator.set_delivery_enabled(grants != Some(false));
    drop(current);
    if grants == Some(false) {
        return Err("Desktop permission was closed before authorization completed. Enable desktop integration again.".into());
    }
    onboarding::notify(app);
    Ok(())
}

fn enable_preference(settings: &mut settings::AppSettings, wayland_grants: Option<bool>) {
    settings.desktop_integration_enabled = true;
    if let Some(granted) = wayland_grants {
        settings.desktop_integration_authorized = granted;
    }
}

#[cfg(target_os = "linux")]
pub fn forget_desktop_authorization(app: &AppHandle) {
    forget_desktop_authorization_if(app, || true);
}

#[cfg(target_os = "linux")]
pub(crate) fn forget_desktop_authorization_if(app: &AppHandle, current: impl FnOnce() -> bool) {
    let state = app.state::<AppState>();
    if let Err(error) = settings::update(&state.settings, &state.data_dir, |settings| {
        if current() {
            settings.desktop_integration_authorized = false;
            state.orchestrator.set_delivery_enabled(false);
        }
    }) {
        log::error!("could not persist revoked desktop consent: {error}");
    }
}

fn should_restore_integration(settings: &settings::AppSettings) -> bool {
    settings.desktop_integration_enabled && settings.desktop_integration_authorized
}

#[cfg(target_os = "linux")]
pub fn restore_desktop_integration(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    if !crate::wayland_paste::applies()
        || !should_restore_integration(&state.settings.read().unwrap())
    {
        return Ok(());
    }
    let handle = app.clone();
    std::thread::Builder::new().name("restore-desktop-integration".into()).spawn(move || {
        log::info!("reconnecting previously authorized Wayland desktop integration");
        if let Err(error) = grant_desktop_integration(&handle, true) {
            let message = format!("Could not reconnect saved desktop integration: {error}");
            log::warn!("{message}");
            crate::portal_shortcuts::restoration_failed(&handle, &message);
        } else {
            log::info!("saved Wayland desktop integration restoration completed");
        }
    }).map(|_| ()).map_err(|error| format!("Could not start desktop integration restoration: {error}"))
}

#[tauri::command]
pub fn restore_insertion_clipboard() -> Result<(), String> {
    crate::insert::restore_previous().map_err(|error| error.0)
}

#[tauri::command]
pub fn disable_desktop_integration(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let _change = DESKTOP_INTEGRATION_CHANGE.try_lock()
        .map_err(|_| "A desktop integration change is already running. Finish or cancel the system dialog before disabling integration.".to_string())?;
    settings::update(&state.settings, &state.data_dir, |settings| {
        settings.desktop_integration_enabled = false;
        settings.desktop_integration_authorized = false;
    })?;
    state.orchestrator.set_delivery_enabled(false);
    #[cfg(target_os = "linux")]
    {
        crate::portal_shortcuts::stop();
        crate::wayland_paste::shutdown();
    }
    onboarding::invalidate(&state, Some("Desktop integration was disabled. Enable it before practice."));
    onboarding::notify(&app);
    Ok(())
}

#[tauri::command]
pub async fn app_info(app: AppHandle) -> Result<AppInfo, String> {
    tauri::async_runtime::spawn_blocking(move || app_info_snapshot(&app.state::<AppState>()))
        .await.map_err(|error| format!("Could not read desktop status: {error}"))
}

fn app_info_snapshot(state: &AppState) -> AppInfo {
    let settings = state.settings.read().unwrap().clone();
    let (modifier_hold, mut modifier_hold_error) = if settings.modifier_hold {
        crate::modifier_hook::status(&state.modifier_hook)
    } else {
        (false, None)
    };
    let modifier_keyboard_devices = match crate::modifier_hook::keyboards() {
        Ok(keyboards) => keyboards,
        Err(error) => {
            log::warn!("could not list Ctrl+Shift keyboards: {error}");
            modifier_hold_error = Some(error);
            Vec::new()
        }
    };
    AppInfo {
        input_device: crate::audio::default_input_name(),
        hotkeys: state.hotkeys.read().unwrap().config.clone(),
        hotkey_errors: state.hotkeys.read().unwrap().errors(),
        prompt_hotkey_error: state.hotkeys.read().unwrap().prompt_error.clone(),
        hotkeys_paused: state.hotkeys.read().unwrap().paused,
        engines_ready: state.onboarding.ready(&settings) && onboarding::models_installed(&state, &settings),
        models_installed: onboarding::models_installed(&state, &settings),
        engine_error: state.onboarding.engine_error(),
        history_enabled: state.history.is_enabled(),
        data_dir: state.data_dir.to_string_lossy().into_owned(),
        use_gpu: settings.use_gpu,
        gpu_device: state.llm.device(),
        modifier_hold,
        modifier_hold_requested: settings.modifier_hold,
        modifier_hold_error,
        modifier_keyboard: settings.modifier_keyboard.clone(),
        modifier_keyboard_devices,
        auto_mode: settings.auto_mode,
        code_chat_paste: false,
        vocabulary: settings.vocabulary.clone(),
        screen_text_apps: settings.screen_text_apps.clone(),
        paste_permission: paste_permission(),
        desktop_error: crate::system_context::last_error(),
        activation_bindings: activation_bindings(),
        desktop_integration_enabled: settings.desktop_integration_enabled,
        clipboard_restore_pending: crate::insert::restoration_pending(),
    }
}

#[tauri::command]
pub async fn retry_focus_detection(app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        match state.orchestrator.inspect_context_if_idle() {
            Ok(Some(_)) => Ok(()),
            Ok(None) => Err("Wait for the active recording or paste job to finish before retrying focus detection.".into()),
            Err(error) => Err(error.0),
        }
    }).await.map_err(|error| format!("Could not retry focus detection: {error}"))?
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
    compatibility: crate::hardware::ModelCompatibility,
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
            compatibility: crate::hardware::model_compatibility(e.min_ram_gb),
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
    onboarding::available(&state)?;
    let entry = state.manifest.get(&id).cloned().ok_or("unknown model")?;
    let cancel = state.downloads.start(&id).ok_or("already downloading")?;
    let models_dir = state.models_dir.clone();
    let spawned = std::thread::Builder::new()
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
            let mut error = result.err();
            if error.is_none() {
                if let Err(message) = settings::update(&state.settings, &state.data_dir, |settings| {
                    ensure_selection(&state.manifest, &state.models_dir, settings);
                }) {
                    error = Some(message);
                } else {
                    preload_engines(&app);
                }
            }
            log::info!("model download {} finished ok={}", entry.id, error.is_none());
            emit(DownloadEvent { id: entry.id.clone(), downloaded: 0, total: entry.size_bytes, verifying: false, done: true, error });
        });
    if let Err(error) = spawned {
        state.downloads.finish(&id);
        return Err(format!("could not start model download: {error}"));
    }
    Ok(())
}

#[tauri::command]
pub fn cancel_download(state: State<'_, AppState>, id: String) -> bool {
    state.downloads.cancel(&id)
}

#[tauri::command]
pub fn delete_model(app: AppHandle, state: State<'_, AppState>, id: String) -> Result<(), String> {
    onboarding::available(&state)?;
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
    onboarding::invalidate(&state, Some("A model was removed. Check the models and try practice again."));
    let result = settings::update(&state.settings, &state.data_dir, |settings| {
        ensure_selection(&state.manifest, &state.models_dir, settings);
    });
    preload_engines(&app);
    result
}

#[tauri::command]
pub fn select_model(app: AppHandle, state: State<'_, AppState>, id: String) -> Result<(), String> {
    onboarding::available(&state)?;
    let entry = state.manifest.get(&id).ok_or("unknown model")?;
    if !is_installed(&state.models_dir, entry) {
        return Err("download the model first".into());
    }
    settings::update(&state.settings, &state.data_dir, |settings| {
        match entry.kind {
            ModelKind::Stt => settings.stt_model = Some(id),
            ModelKind::Llm => settings.llm_model = Some(id),
        }
    })?;
    preload_engines(&app);
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
pub fn set_use_gpu(app: AppHandle, state: State<'_, AppState>, enabled: bool) -> Result<(), String> {
    onboarding::available(&state)?;
    settings::update(&state.settings, &state.data_dir, |s| s.use_gpu = enabled)?;
    preload_engines(&app);
    Ok(())
}

#[tauri::command]
pub fn set_modifier_hold(app: AppHandle, state: State<'_, AppState>, enabled: bool) -> Result<(), String> {
    let previous = state.settings.read().unwrap().modifier_hold;
    if let Err(error) = crate::modifier_hook::apply(&app, &state.modifier_hook, enabled) {
        if previous != enabled {
            if let Err(restore_error) = crate::modifier_hook::apply(&app, &state.modifier_hook, previous) {
                return Err(format!("{error}; could not restore Ctrl+Shift monitoring: {restore_error}"));
            }
        }
        onboarding::notify(&app);
        return Err(error);
    }
    if let Err(error) = settings::update(&state.settings, &state.data_dir, |settings| settings.modifier_hold = enabled) {
        if let Err(restore_error) = crate::modifier_hook::apply(&app, &state.modifier_hook, previous) {
            return Err(format!("{error}; could not restore Ctrl+Shift monitoring: {restore_error}"));
        }
        return Err(error);
    }
    onboarding::notify(&app);
    Ok(())
}

#[tauri::command]
pub fn set_modifier_keyboard(app: AppHandle, state: State<'_, AppState>, path: String) -> Result<(), String> {
    let _ = (app, state, path);
    Err("Physical-keyboard selection has been retired. Use the configured shortcut through desktop integration.".into())
}

/// Pushes the text-related settings into the pipeline; called at startup and after each change.
pub fn apply_text_settings(orchestrator: &promptify_core::pipeline::Orchestrator, settings: &settings::AppSettings) {
    orchestrator.set_delivery_enabled(settings.desktop_integration_enabled);
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
pub fn set_code_chat_paste(state: State<'_, AppState>, enabled: bool) -> Result<(), String> {
    let _ = (state, enabled);
    Err("Software-specific paste permission has been retired. Keep a writable input focused; Promptify inspects the destination independently of its app.".into())
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
    onboarding::available(&state)?;
    check_len("hotkey", &accelerator, 64)?;
    let previous = state.hotkeys.read().unwrap().config.clone();
    crate::hotkeys::rebind(&app, &mut state.hotkeys.write().unwrap(), mode, &accelerator)?;
    let saved = settings::update(&state.settings, &state.data_dir, |settings| {
        match mode {
            Mode::Prompt => settings.prompt_hotkey = Some(accelerator),
            Mode::Dictation => settings.dictation_hotkey = Some(accelerator),
        }
    });
    if let Err(error) = saved {
        let restored = crate::hotkeys::restore_config(&app, &mut state.hotkeys.write().unwrap(), previous);
        crate::tray::refresh(&app);
        return Err(match restored {
            Ok(()) => error,
            Err(restore) => format!("{error}. The previous shortcut also could not be restored: {restore}"),
        });
    }
    onboarding::invalidate(&state, Some("The shortcut changed. Try practice again with the new shortcut."));
    onboarding::notify(&app);
    crate::tray::refresh(&app);
    Ok(())
}

#[tauri::command]
pub fn routing_state(state: State<'_, AppState>) -> settings::RoutingState {
    state.routing.lock().unwrap().clone()
}

#[tauri::command]
pub fn set_rendering(state: State<'_, AppState>, rendering: Rendering) -> Result<settings::RoutingState, String> {
    let mut current = state.routing.lock().unwrap();
    settings::save_routing(&state.data_dir, rendering)?;
    state.orchestrator.set_rendering(rendering);
    *current = settings::RoutingState { rendering, error: None };
    Ok(current.clone())
}

#[tauri::command]
pub fn reset_routing(state: State<'_, AppState>) -> Result<settings::RoutingState, String> {
    let mut current = state.routing.lock().unwrap();
    settings::reset_routing(&state.data_dir)?;
    let rendering = Rendering::default();
    state.orchestrator.set_rendering(rendering);
    *current = settings::RoutingState { rendering, error: None };
    Ok(current.clone())
}

#[tauri::command]
pub fn queue_prompt_routing(state: State<'_, AppState>, options: RoutingOptions) -> Result<(), String> {
    state.orchestrator.queue_routing(options)
}

#[tauri::command]
pub fn prompt_catalog() -> serde_json::Value {
    serde_json::json!({
        "version": routing::CATALOG_VERSION, "tasks": routing::catalog().all(), "surfaces": Surface::all(),
        "required_structure": { "numbered_steps": true, "bounded_loop": true, "done_when": true }
    })
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
pub fn hide_overlay(app: AppHandle) -> Result<(), String> {
    let overlay = app.get_webview_window("overlay").ok_or("overlay window is unavailable")?;
    overlay.hide().map_err(|error| format!("could not hide the overlay: {error}"))
}

fn fitted_overlay_height(requested: u32, monitor_height: f64) -> Result<f64, String> {
    if requested == 0 || !monitor_height.is_finite() || monitor_height <= 0.0 {
        return Err("invalid overlay height".into());
    }
    let maximum = (monitor_height - 16.0).max(1.0);
    Ok(f64::from(requested).max(64.0).min(maximum))
}

fn overlay_monitor(window: &tauri::WebviewWindow) -> Result<tauri::Monitor, String> {
    window.current_monitor().map_err(|error| format!("could not read overlay monitor: {error}"))?
        .ok_or_else(|| "overlay monitor is unavailable".into())
}

#[tauri::command]
pub fn overlay_max_height(app: AppHandle) -> Result<f64, String> {
    let overlay = app.get_webview_window("overlay").ok_or("overlay window is unavailable")?;
    let monitor = overlay_monitor(&overlay)?;
    fitted_overlay_height(u32::MAX, f64::from(monitor.work_area().size.height) / monitor.scale_factor())
}

#[tauri::command]
pub fn resize_overlay(app: AppHandle, height: u32) -> Result<(), String> {
    let overlay = app.get_webview_window("overlay").ok_or("overlay window is unavailable")?;
    let scale = overlay.scale_factor().map_err(|error| format!("could not read overlay scale: {error}"))?;
    let monitor = overlay_monitor(&overlay)?;
    let monitor_height = f64::from(monitor.work_area().size.height) / monitor.scale_factor();
    let target_height = fitted_overlay_height(height, monitor_height)?;
    let size = overlay.inner_size().map_err(|error| format!("could not read overlay size: {error}"))?.to_logical::<f64>(scale);
    if (size.height - target_height).abs() >= 1.0 {
        overlay.set_size(tauri::LogicalSize::new(size.width, target_height))
            .map_err(|error| format!("could not resize the overlay: {error}"))?;
        overlay.center().map_err(|error| format!("could not keep the overlay on screen: {error}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod overlay_tests {
    use super::fitted_overlay_height;

    #[test]
    fn overlay_size_is_bounded_and_respects_small_displays() {
        assert_eq!(fitted_overlay_height(180, 1080.0).unwrap(), 180.0);
        assert_eq!(fitted_overlay_height(416, 1080.0).unwrap(), 416.0);
        assert_eq!(fitted_overlay_height(900, 1080.0).unwrap(), 900.0);
        assert_eq!(fitted_overlay_height(1800, 1080.0).unwrap(), 1064.0);
        assert_eq!(fitted_overlay_height(416, 400.0).unwrap(), 384.0);
        assert_eq!(fitted_overlay_height(3000, 4320.0).unwrap(), 3000.0);
        assert!(fitted_overlay_height(0, 1080.0).is_err());
        assert!(fitted_overlay_height(180, f64::NAN).is_err());
    }
}

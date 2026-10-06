pub mod audio;
mod commands;
mod controller;
pub mod download;
mod hotkeys;
mod hardware;
pub mod insert;
#[cfg(windows)]
mod clipboard_restore_windows;
#[cfg(windows)]
mod destination_windows;
#[cfg(target_os = "linux")]
mod destination_linux;
#[cfg(target_os = "linux")]
mod portal_shortcuts;
#[cfg(target_os = "macos")]
mod destination_macos;
#[cfg(target_os = "linux")]
mod kwin;
pub mod llm_client;
pub mod inference;
pub mod jev;
mod modifier_hook;
pub mod onboarding;
pub mod settings;
pub mod stt;
pub mod system_context;
mod tray;
mod updates;
mod update_install;
#[cfg(target_os = "linux")]
pub mod wayland_paste;

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use promptify_core::context::ContextPolicy;
use promptify_core::history::{HistoryLimits, HistoryLog};
use promptify_core::models::{Manifest, ModelKind, ModelTier, is_installed};
use promptify_core::pipeline::{Backends, Limits, Orchestrator};
use promptify_core::profiles::ProfileSet;
use tauri::{AppHandle, Manager, PhysicalPosition, WindowEvent};

use crate::controller::Controller;
use crate::download::Downloads;
use crate::hotkeys::{HotkeyConfig, HotkeyState, Hotkeys};
use crate::inference::InferenceManager;
use crate::settings::{AppSettings, SharedSettings};
use crate::stt::WhisperEngine;

pub struct AppState {
    pub orchestrator: Arc<Orchestrator>,
    pub controller: Controller,
    pub hotkeys: RwLock<HotkeyState>,
    pub data_dir: PathBuf,
    pub models_dir: PathBuf,
    pub manifest: Manifest,
    pub settings: SharedSettings,
    pub routing: std::sync::Mutex<settings::RoutingState>,
    pub history: Arc<HistoryLog>,
    pub downloads: Arc<Downloads>,
    pub stt: Arc<WhisperEngine>,
    pub llm: Arc<InferenceManager>,
    pub onboarding: Arc<onboarding::Onboarding>,
    pub modifier_hook: std::sync::Mutex<Option<modifier_hook::Hook>>,
}

/// Selects an installed model for any kind that has none, preferring the balanced tier.
pub fn ensure_selection(manifest: &Manifest, models_dir: &std::path::Path, settings: &mut AppSettings) -> bool {
    let mut changed = false;
    for kind in [ModelKind::Stt, ModelKind::Llm] {
        let slot = match kind {
            ModelKind::Stt => &mut settings.stt_model,
            ModelKind::Llm => &mut settings.llm_model,
        };
        let current_ok = slot.as_deref().and_then(|id| manifest.get(id)).is_some_and(|e| e.kind == kind && is_installed(models_dir, e));
        if current_ok {
            continue;
        }
        let mut installed: Vec<_> = manifest.models.iter().filter(|e| e.kind == kind && is_installed(models_dir, e)).collect();
        installed.sort_by_key(|e| match e.tier {
            ModelTier::Balanced => 0,
            ModelTier::Small => 1,
            ModelTier::Quality => 2,
        });
        let next = installed.first().map(|e| e.id.clone());
        if *slot != next {
            *slot = next;
            changed = true;
        }
    }
    changed
}

/// Warms both engines off the UI thread so the first dictation is fast.
pub fn preload_engines(app: &AppHandle) {
    onboarding::load_engines(app);
}

fn place_overlay(app: &AppHandle) {
    let Some(overlay) = app.get_webview_window("overlay") else { return };
    let Ok(Some(monitor)) = overlay.primary_monitor() else { return };
    let scale = monitor.scale_factor();
    let width = overlay.outer_size().map(|s| s.width as i32).unwrap_or((560.0 * scale) as i32);
    let x = monitor.position().x + (monitor.size().width as i32 - width) / 2;
    let y = monitor.position().y + (16.0 * scale) as i32;
    let _ = overlay.set_position(PhysicalPosition::new(x, y));
}

pub fn run() {
    let previous_panic_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Some(location) = info.location() {
            log::error!("unhandled Rust panic at {}:{}:{}", location.file(), location.line(), location.column());
        } else {
            log::error!("unhandled Rust panic (location unavailable)");
        }
        previous_panic_hook(info);
    }));
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| tray::show_settings(app)))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, None))
        .plugin(tauri_plugin_log::Builder::new().level(log::LevelFilter::Info).build())
        .plugin(tauri_plugin_global_shortcut::Builder::new().with_handler(hotkeys::handle_shortcut).build())
        .invoke_handler(tauri::generate_handler![
            jev::jev_status,
            jev::save_jev_key,
            jev::delete_jev_key,
            jev::set_jev_enabled,
            updates::update_info,
            updates::check_for_updates,
            updates::set_update_checks,
            updates::open_update_release,
            updates::install_update,
            updates::restart_after_update,
            commands::app_info,
            commands::inference_config,
            commands::inference_status,
            commands::save_inference_connection,
            commands::remove_inference_connection,
            commands::select_inference,
            commands::discover_inference_models,
            commands::discover_inference_draft,
            commands::test_inference_connection,
            commands::reset_inference,
            commands::retry_focus_detection,
            commands::open_recovery_settings,
            commands::open_data_folder,
            commands::open_system_settings,
            commands::routing_state,
            commands::set_rendering,
            commands::reset_routing,
            commands::queue_prompt_routing,
            commands::prompt_catalog,
            commands::clean_dictation,
            commands::copy_last_result,
            commands::hide_overlay,
            commands::resize_overlay,
            commands::overlay_max_height,
            commands::list_models,
            commands::download_model,
            commands::cancel_download,
            commands::delete_model,
            commands::select_model,
            commands::list_history,
            commands::delete_history_entry,
            commands::clear_history,
            commands::set_history_enabled,
            commands::set_modifier_hold,
            commands::set_modifier_keyboard,
            commands::set_auto_mode,
            commands::set_code_chat_paste,
            commands::set_vocabulary,
            commands::set_screen_text_apps,
            commands::set_hotkey,
            commands::set_use_gpu,
            commands::grant_paste_permission,
            commands::disable_desktop_integration,
            commands::restore_insertion_clipboard,
            onboarding::onboarding_status,
            onboarding::retry_onboarding_startup,
            onboarding::retry_model_loading,
            onboarding::set_onboarding_step,
            onboarding::arm_onboarding_practice,
            onboarding::disarm_onboarding_practice,
            onboarding::confirm_onboarding_paste,
            onboarding::complete_onboarding,
            onboarding::resume_onboarding_hotkeys,
        ])
        .setup(|app| {
            let handle = app.handle().clone();

            let data_dir = settings::app_data_dir();
            std::fs::create_dir_all(&data_dir)?;
            let models_dir = settings::models_dir(&data_dir);
            #[cfg(target_os = "linux")]
            {
                wayland_paste::init(&data_dir);
                std::thread::Builder::new().name("kwin-focus".into()).spawn(kwin::init)?;
            }
            let manifest = Manifest::bundled();
            let (loaded, startup_error) = onboarding::initialize(&data_dir, &manifest, &models_dir);
            if let Some(error) = &startup_error {
                log::error!("setup could not initialize: {error}");
            }
            let mut hotkey_config = HotkeyConfig::from_settings(&loaded);
            let hotkeys = Hotkeys::parse(&hotkey_config).or_else(|e| {
                log::warn!("saved hotkeys are invalid, using defaults: {e}");
                hotkey_config = HotkeyConfig::defaults();
                Hotkeys::parse(&hotkey_config)
            })?;
            let (history, report) = HistoryLog::open(data_dir.join("history.jsonl"), HistoryLimits::default(), loaded.history_enabled)?;
            if report.skipped > 0 {
                log::warn!("dropped {} unreadable history lines", report.skipped);
            }
            let history = Arc::new(history);
            let settings: SharedSettings = Arc::new(RwLock::new(loaded));
            let stt = Arc::new(WhisperEngine::new(manifest.clone(), models_dir.clone(), settings.clone()));
            let llm = Arc::new(InferenceManager::new(data_dir.clone(), manifest.clone(), models_dir.clone(), settings.clone()));

            let backends = Backends {
                context: Arc::new(system_context::SystemContext),
                transcriber: stt.clone(),
                generator: llm.clone(),
                inserter: Arc::new(insert::ClipboardPaste),
                history: history.clone(),
            };
            let orchestrator = Arc::new(Orchestrator::new(backends, ProfileSet::bundled(), ContextPolicy::default(), Limits::default()));
            orchestrator.set_prompt_reviewer(Arc::new(jev::JevReviewer::new(settings.clone())));
            commands::apply_text_settings(&orchestrator, &settings.read().unwrap());
            let routing = match settings::load_routing(&data_dir) {
                Ok(rendering) => settings::RoutingState { rendering, error: None },
                Err(error) => {
                    log::warn!("adaptive routing settings unavailable: {error}");
                    settings::RoutingState { rendering: Default::default(), error: Some(error) }
                }
            };
            orchestrator.set_rendering(routing.rendering);
            let cancel = hotkeys.cancel;
            let esc_handle = handle.clone();
            let controller = Controller::spawn(handle.clone(), orchestrator.clone(), move |active| {
                hotkeys::set_cancel_registered(&esc_handle, cancel, active)
            });
            let mut hotkey_state = HotkeyState { config: hotkey_config, hotkeys, prompt_error: None, dictation_error: None, paused: false };
            hotkeys::register_mode_hotkeys(&handle, &mut hotkey_state);
            let tray_hotkeys = hotkey_state.config.clone();
            let engines_ready = settings.read().unwrap().stt_model.is_some()
                && llm.config().is_ok_and(|config| match config.selection {
                    inference::InferenceSelection::BundledLocal => settings.read().unwrap().llm_model.is_some(),
                    inference::InferenceSelection::Connection { .. } => true,
                });
            app.manage(AppState {
                orchestrator,
                controller,
                hotkeys: RwLock::new(hotkey_state),
                data_dir,
                models_dir,
                manifest,
                settings,
                routing: std::sync::Mutex::new(routing),
                history,
                downloads: Arc::default(),
                stt,
                llm,
                onboarding: Arc::new(onboarding::Onboarding::new(startup_error)),
                modifier_hook: Default::default(),
            });
            preload_engines(&handle);
            {
                let state = app.state::<AppState>();
                let enabled = state.settings.read().unwrap().modifier_hold;
                if let Err(e) = modifier_hook::apply(&handle, &state.modifier_hook, enabled) {
                    log::warn!("Ctrl+Shift hold hotkey: {e}");
                }
            }

            tray::build(&handle, &tray_hotkeys)?;
            app.manage(updates::Updates::default());
            #[cfg(target_os = "linux")]
            commands::restore_desktop_integration(&handle)?;
            updates::start(handle.clone());
            tray::refresh(&handle);
            place_overlay(&handle);
            log::info!("desktop started pid={} embedded_ui={}", std::process::id(), cfg!(feature = "custom-protocol"));
            if onboarding::required(&app.state::<AppState>()) || !engines_ready {
                tray::show_settings(&handle);
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == "settings" && matches!(event, WindowEvent::Focused(false) | WindowEvent::CloseRequested { .. }) {
                onboarding::interrupt(window.app_handle());
            }
            // Promptify lives in the tray; closing settings only hides it.
            if let WindowEvent::CloseRequested { api, .. } = event
                && window.label() == "settings"
            {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building Promptify")
        .run(|_, event| match event {
            tauri::RunEvent::ExitRequested { code, .. } => log::info!("desktop exit requested: code={code:?}"),
            tauri::RunEvent::Exit => {
                #[cfg(target_os = "linux")]
                {
                    portal_shortcuts::stop();
                    wayland_paste::shutdown();
                }
                system_context::shutdown();
                log::info!("desktop event loop exited")
            }
            _ => {}
        });
}

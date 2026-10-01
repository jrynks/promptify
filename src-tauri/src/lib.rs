pub mod audio;
mod commands;
mod controller;
pub mod download;
mod hotkeys;
pub mod insert;
pub mod llm_client;
mod mcp;
mod modifier_hook;
mod remote;
pub mod settings;
pub mod stt;
mod system_context;
mod tray;

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
use crate::llm_client::LlmWorker;
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
    pub history: Arc<HistoryLog>,
    pub downloads: Arc<Downloads>,
    pub stt: Arc<WhisperEngine>,
    pub llm: Arc<LlmWorker>,
    pub remote: remote::RemoteState,
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
pub fn preload_engines(stt: Arc<WhisperEngine>, llm: Arc<LlmWorker>) {
    std::thread::spawn(move || {
        if let Err(e) = stt.preload() {
            log::info!("speech model not ready: {e}");
        }
        if let Err(e) = llm.preload() {
            log::info!("language model not ready: {e}");
        }
    });
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
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| tray::show_settings(app)))
        .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, None))
        .plugin(tauri_plugin_log::Builder::new().level(log::LevelFilter::Info).level_for("rmcp", log::LevelFilter::Warn).build())
        .plugin(tauri_plugin_global_shortcut::Builder::new().with_handler(hotkeys::handle_shortcut).build())
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::list_profiles,
            commands::preview_prompt,
            commands::clean_dictation,
            commands::copy_last_result,
            commands::hide_overlay,
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
            mcp::mcp_info,
            mcp::mcp_reload,
            mcp::mcp_test,
            commands::set_hotkey,
            commands::set_use_gpu,
            remote::remote_info,
            remote::set_remote_settings,
            remote::create_pairing_offer,
            remote::cancel_pairing,
            remote::remove_device,
            remote::rename_device,
        ])
        .setup(|app| {
            let handle = app.handle().clone();

            let data_dir = settings::app_data_dir();
            std::fs::create_dir_all(&data_dir)?;
            let models_dir = settings::models_dir(&data_dir);
            let manifest = Manifest::bundled();
            let mut loaded = settings::load(&data_dir);
            if ensure_selection(&manifest, &models_dir, &mut loaded) {
                settings::save(&data_dir, &loaded)?;
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
            let llm = Arc::new(LlmWorker::new(llm_client::worker_exe(), manifest.clone(), models_dir.clone(), settings.clone()));

            let backends = Backends {
                context: Arc::new(system_context::SystemContext),
                transcriber: stt.clone(),
                generator: llm.clone(),
                inserter: Arc::new(insert::ClipboardPaste),
                history: history.clone(),
            };
            let orchestrator = Arc::new(Orchestrator::new(backends, ProfileSet::bundled(), ContextPolicy::default(), Limits::default()));
            let _ = mcp::reload(&data_dir, &orchestrator);
            let cancel = hotkeys.cancel;
            let esc_handle = handle.clone();
            let controller = Controller::spawn(handle.clone(), orchestrator.clone(), move |active| {
                hotkeys::set_cancel_registered(&esc_handle, cancel, active)
            });
            let mut hotkey_state = HotkeyState { config: hotkey_config, hotkeys, prompt_error: None, dictation_error: None, paused: false };
            hotkeys::register_mode_hotkeys(&handle, &mut hotkey_state);
            preload_engines(stt.clone(), llm.clone());
            let tray_hotkeys = hotkey_state.config.clone();
            let engines_ready = [&settings.read().unwrap().stt_model, &settings.read().unwrap().llm_model].iter().all(|m| m.is_some());
            app.manage(AppState {
                orchestrator,
                controller,
                hotkeys: RwLock::new(hotkey_state),
                data_dir,
                models_dir,
                manifest,
                settings,
                history,
                downloads: Arc::default(),
                stt,
                llm,
                remote: remote::RemoteState::default(),
                modifier_hook: Default::default(),
            });
            remote::apply(&app.state::<AppState>());
            {
                let state = app.state::<AppState>();
                let enabled = state.settings.read().unwrap().modifier_hold;
                if let Err(e) = modifier_hook::apply(&handle, &state.modifier_hook, enabled) {
                    log::warn!("Ctrl+Shift hold hotkey: {e}");
                }
            }

            tray::build(&handle, &tray_hotkeys)?;
            place_overlay(&handle);
            // Promptify lives in the tray; settings only open by themselves until models are set up.
            if !engines_ready {
                tray::show_settings(&handle);
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Promptify lives in the tray; closing settings only hides it.
            if let WindowEvent::CloseRequested { api, .. } = event
                && window.label() == "settings"
            {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running Promptify");
}

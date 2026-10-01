use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};
use tauri_plugin_autostart::ManagerExt;

use crate::hotkeys::HotkeyConfig;

const TRAY_ID: &str = "promptify";

/// Menu entries that change while the app runs.
pub struct TrayHandles {
    status: MenuItem<tauri::Wry>,
    prompt: MenuItem<tauri::Wry>,
    dictation: MenuItem<tauri::Wry>,
    pause: CheckMenuItem<tauri::Wry>,
}

pub fn show_settings(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("settings") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn pretty(accelerator: &str) -> String {
    let modifier = if cfg!(target_os = "macos") { "Cmd" } else { "Ctrl" };
    accelerator.replace("CommandOrControl", modifier)
}

pub fn build(app: &AppHandle, hotkeys: &HotkeyConfig) -> tauri::Result<()> {
    let status = MenuItem::with_id(app, "status", "Promptify \u{2014} ready", false, None::<&str>)?;
    let prompt = MenuItem::with_id(app, "hk_prompt", format!("Prompt: {}", pretty(&hotkeys.prompt)), false, None::<&str>)?;
    let dictation = MenuItem::with_id(app, "hk_dictation", format!("Dictation: {}", pretty(&hotkeys.dictation)), false, None::<&str>)?;
    let pause = CheckMenuItem::with_id(app, "pause", "Pause hotkeys", true, false, None::<&str>)?;
    let autostart_on = app.autolaunch().is_enabled().unwrap_or(false);
    let autostart = CheckMenuItem::with_id(app, "autostart", "Start at login", true, autostart_on, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "Settings\u{2026}", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Promptify", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &status,
            &prompt,
            &dictation,
            &PredefinedMenuItem::separator(app)?,
            &pause,
            &autostart,
            &settings,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    let icon = app.default_window_icon().cloned().ok_or(tauri::Error::InvalidIcon(std::io::Error::other("missing app icon")))?;
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .tooltip("Promptify \u{2014} ready")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                show_settings(tray.app_handle());
            }
        })
        .on_menu_event(move |app, event| match event.id.as_ref() {
            "settings" => show_settings(app),
            "quit" => app.exit(0),
            "pause" => {
                let paused = app.state::<TrayHandles>().pause.is_checked().unwrap_or(false);
                crate::hotkeys::set_paused(app, paused);
                refresh(app);
            }
            "autostart" => {
                let launcher = app.autolaunch();
                let result = if launcher.is_enabled().unwrap_or(false) { launcher.disable() } else { launcher.enable() };
                if let Err(e) = result {
                    log::warn!("could not change start at login: {e}");
                }
            }
            _ => {}
        })
        .build(app)?;

    app.manage(TrayHandles { status, prompt, dictation, pause });
    Ok(())
}

/// Shows what Promptify is doing in the tray tooltip and menu.
pub fn set_status(app: &AppHandle, status: &str) {
    let paused = app.try_state::<crate::AppState>().is_some_and(|s| s.hotkeys.read().unwrap().paused);
    let text = if paused && status == "ready" { "Promptify \u{2014} hotkeys paused".to_owned() } else { format!("Promptify \u{2014} {status}") };
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_tooltip(Some(&text));
    }
    if let Some(handles) = app.try_state::<TrayHandles>() {
        let _ = handles.status.set_text(&text);
    }
}

/// Re-reads hotkeys and pause state after they change.
pub fn refresh(app: &AppHandle) {
    let Some(handles) = app.try_state::<TrayHandles>() else { return };
    let Some(state) = app.try_state::<crate::AppState>() else { return };
    let hotkeys = state.hotkeys.read().unwrap();
    let _ = handles.prompt.set_text(format!("Prompt: {}", pretty(&hotkeys.config.prompt)));
    let _ = handles.dictation.set_text(format!("Dictation: {}", pretty(&hotkeys.config.dictation)));
    let _ = handles.pause.set_checked(hotkeys.paused);
    drop(hotkeys);
    set_status(app, "ready");
}

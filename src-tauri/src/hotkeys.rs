use promptify_core::pipeline::Mode;
use serde::Serialize;
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutEvent, ShortcutState};

use crate::AppState;
use crate::controller::Command;
use crate::settings::AppSettings;

// Ctrl+Shift+Space is claimed by other tools on many machines; users can rebind in Settings.
const DEFAULT_PROMPT: &str = "CommandOrControl+Alt+Space";
const DEFAULT_DICTATION: &str = "CommandOrControl+Alt+Shift+Space";

#[derive(Debug, Clone, Serialize)]
pub struct HotkeyConfig {
    pub prompt: String,
    pub dictation: String,
    pub cancel: String,
}

impl HotkeyConfig {
    pub fn from_settings(settings: &AppSettings) -> Self {
        Self {
            prompt: settings.prompt_hotkey.clone().unwrap_or_else(|| DEFAULT_PROMPT.into()),
            dictation: settings.dictation_hotkey.clone().unwrap_or_else(|| DEFAULT_DICTATION.into()),
            cancel: "Escape".into(),
        }
    }

    pub fn defaults() -> Self {
        Self::from_settings(&AppSettings::default())
    }
}

/// Live hotkey bindings and why any could not be claimed.
pub struct HotkeyState {
    pub config: HotkeyConfig,
    pub hotkeys: Hotkeys,
    pub prompt_error: Option<String>,
    pub dictation_error: Option<String>,
}

impl HotkeyState {
    pub fn errors(&self) -> Vec<String> {
        self.prompt_error.iter().chain(&self.dictation_error).cloned().collect()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Hotkeys {
    pub prompt: Shortcut,
    pub dictation: Shortcut,
    pub cancel: Shortcut,
}

impl Hotkeys {
    pub fn parse(config: &HotkeyConfig) -> Result<Self, String> {
        let parse = |label: &str, value: &str| value.parse::<Shortcut>().map_err(|e| format!("invalid {label} hotkey {value:?}: {e}"));
        Ok(Self {
            prompt: parse("prompt", &config.prompt)?,
            dictation: parse("dictation", &config.dictation)?,
            cancel: parse("cancel", &config.cancel)?,
        })
    }
}

fn unavailable(label: &str, accelerator: &str, e: impl std::fmt::Display) -> String {
    format!("The {label} hotkey {accelerator} is unavailable (another app may be using it): {e}")
}

/// Registers the always-on mode hotkeys, recording which could not be claimed.
pub fn register_mode_hotkeys<R: Runtime>(app: &AppHandle<R>, state: &mut HotkeyState) {
    if let Err(e) = app.global_shortcut().register(state.hotkeys.prompt) {
        log::warn!("could not register prompt hotkey: {e}");
        state.prompt_error = Some(unavailable("prompt", &state.config.prompt, e));
    }
    if let Err(e) = app.global_shortcut().register(state.hotkeys.dictation) {
        log::warn!("could not register dictation hotkey: {e}");
        state.dictation_error = Some(unavailable("dictation", &state.config.dictation, e));
    }
}

/// Rebinds one mode. The old binding is restored if the new one cannot be claimed.
pub fn rebind<R: Runtime>(app: &AppHandle<R>, state: &mut HotkeyState, mode: Mode, accelerator: &str) -> Result<(), String> {
    let shortcut = accelerator.parse::<Shortcut>().map_err(|e| format!("invalid hotkey {accelerator:?}: {e}"))?;
    let (current, other) = match mode {
        Mode::Prompt => (state.hotkeys.prompt, state.hotkeys.dictation),
        Mode::Dictation => (state.hotkeys.dictation, state.hotkeys.prompt),
    };
    if shortcut.id() == other.id() || shortcut.id() == state.hotkeys.cancel.id() {
        return Err("that hotkey is already used by Promptify".into());
    }
    let shortcuts = app.global_shortcut();
    let had_current = shortcuts.is_registered(current);
    if had_current && shortcut.id() != current.id() {
        shortcuts.unregister(current).map_err(|e| e.to_string())?;
    }
    if !shortcuts.is_registered(shortcut)
        && let Err(e) = shortcuts.register(shortcut)
    {
        if had_current {
            let _ = shortcuts.register(current);
        }
        let label = if mode == Mode::Prompt { "prompt" } else { "dictation" };
        return Err(unavailable(label, accelerator, e));
    }
    match mode {
        Mode::Prompt => {
            state.hotkeys.prompt = shortcut;
            state.config.prompt = accelerator.to_owned();
            state.prompt_error = None;
        }
        Mode::Dictation => {
            state.hotkeys.dictation = shortcut;
            state.config.dictation = accelerator.to_owned();
            state.dictation_error = None;
        }
    }
    Ok(())
}

/// Escape is only claimed while a job is active so it keeps working in other apps.
pub fn set_cancel_registered<R: Runtime>(app: &AppHandle<R>, cancel: Shortcut, active: bool) {
    let shortcuts = app.global_shortcut();
    let result = if active {
        if shortcuts.is_registered(cancel) { Ok(()) } else { shortcuts.register(cancel) }
    } else if shortcuts.is_registered(cancel) {
        shortcuts.unregister(cancel)
    } else {
        Ok(())
    };
    if let Err(e) = result {
        log::warn!("could not update cancel hotkey: {e}");
    }
}

pub fn handle_shortcut<R: Runtime>(app: &AppHandle<R>, shortcut: &Shortcut, event: ShortcutEvent) {
    let Some(state) = app.try_state::<AppState>() else { return };
    let hotkeys = state.hotkeys.read().unwrap().hotkeys;
    let pressed = event.state() == ShortcutState::Pressed;
    let command = if shortcut.id() == hotkeys.cancel.id() {
        if !pressed {
            return;
        }
        Command::Cancel
    } else {
        let mode = if shortcut.id() == hotkeys.prompt.id() {
            Mode::Prompt
        } else if shortcut.id() == hotkeys.dictation.id() {
            Mode::Dictation
        } else {
            return;
        };
        if pressed { Command::Press(mode) } else { Command::Release(mode) }
    };
    state.controller.send(command);
}

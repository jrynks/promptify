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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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
    /// Paused from the tray: mode hotkeys are released so other apps can use them.
    pub paused: bool,
}

impl HotkeyState {
    pub fn errors(&self) -> Vec<String> {
        let mut errors: Vec<String> = self.prompt_error.iter().cloned().collect();
        if let Some(error) = &self.dictation_error
            && !errors.contains(error)
        {
            errors.push(error.clone());
        }
        errors
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

    fn mode_shortcuts(&self) -> Vec<Shortcut> {
        vec![self.prompt, self.dictation]
    }

    fn get(&self, mode: Mode) -> Option<Shortcut> {
        match mode {
            Mode::Prompt => Some(self.prompt),
            Mode::Dictation => Some(self.dictation),
        }
    }
}

fn label(mode: Mode) -> &'static str {
    match mode {
        Mode::Prompt => "prompt",
        Mode::Dictation => "dictation",
    }
}

fn unavailable(label: &str, accelerator: &str, e: impl std::fmt::Display) -> String {
    format!("The {label} hotkey {accelerator} is unavailable (another app may be using it): {e}")
}

/// Registers the always-on mode hotkeys, recording which could not be claimed.
pub fn register_mode_hotkeys<R: Runtime>(app: &AppHandle<R>, state: &mut HotkeyState) {
    #[cfg(target_os = "linux")]
    if crate::wayland_paste::applies() {
        portal_errors(state);
        return;
    }
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
    let current = state.hotkeys.get(mode);
    let taken = [Mode::Prompt, Mode::Dictation]
        .into_iter()
        .filter(|m| *m != mode)
        .filter_map(|m| state.hotkeys.get(m))
        .chain([state.hotkeys.cancel])
        .any(|other| other.id() == shortcut.id());
    if taken {
        return Err("that hotkey is already used by Promptify".into());
    }
    #[cfg(target_os = "linux")]
    if crate::wayland_paste::applies() {
        crate::portal_shortcuts::preferred_trigger(shortcut)?;
        if current.is_none_or(|current| current.id() != shortcut.id()) {
            crate::portal_shortcuts::stop();
        }
        apply_binding(state, mode, shortcut, accelerator);
        portal_errors(state);
        return Ok(());
    }
    if state.paused {
        // Claimed when hotkeys resume.
        apply_binding(state, mode, shortcut, accelerator);
        return Ok(());
    }
    let shortcuts = app.global_shortcut();
    let had_current = current.filter(|c| shortcuts.is_registered(*c));
    if let Some(current) = had_current
        && shortcut.id() != current.id()
    {
        shortcuts.unregister(current).map_err(|e| e.to_string())?;
    }
    if !shortcuts.is_registered(shortcut)
        && let Err(e) = shortcuts.register(shortcut)
    {
        if let Some(current) = had_current {
            let _ = shortcuts.register(current);
        }
        return Err(unavailable(label(mode), accelerator, e));
    }
    apply_binding(state, mode, shortcut, accelerator);
    Ok(())
}

fn apply_binding(state: &mut HotkeyState, mode: Mode, shortcut: Shortcut, accelerator: &str) {
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
}

pub fn restore_config<R: Runtime>(app: &AppHandle<R>, state: &mut HotkeyState, config: HotkeyConfig) -> Result<(), String> {
    let hotkeys = Hotkeys::parse(&config)?;
    #[cfg(target_os = "linux")]
    if crate::wayland_paste::applies() {
        crate::portal_shortcuts::validate_config(&config)?;
        if state.config != config {
            crate::portal_shortcuts::stop();
        }
        state.config = config;
        state.hotkeys = hotkeys;
        portal_errors(state);
        return Ok(());
    }
    let shortcuts = app.global_shortcut();
    if !state.paused {
        for shortcut in state.hotkeys.mode_shortcuts() {
            if shortcuts.is_registered(shortcut) {
                shortcuts.unregister(shortcut).map_err(|e| format!("could not release shortcut: {e}"))?;
            }
        }
    }
    state.config = config;
    state.hotkeys = hotkeys;
    state.prompt_error = None;
    state.dictation_error = None;
    if !state.paused {
        register_mode_hotkeys(app, state);
    }
    let errors = state.errors();
    if errors.is_empty() { Ok(()) } else { Err(errors.join("; ")) }
}

/// Releases or reclaims the mode hotkeys.
pub fn set_paused<R: Runtime>(app: &AppHandle<R>, paused: bool) {
    let Some(state) = app.try_state::<AppState>() else { return };
    crate::onboarding::invalidate(&state, Some("Shortcut availability changed. Try practice again."));
    let mut hotkeys = state.hotkeys.write().unwrap();
    if hotkeys.paused == paused {
        return;
    }
    hotkeys.paused = paused;
    #[cfg(target_os = "linux")]
    if crate::wayland_paste::applies() {
        crate::portal_shortcuts::stop();
        portal_errors(&mut hotkeys);
        drop(hotkeys);
        state.controller.send(Command::Cancel);
        crate::onboarding::notify(app);
        return;
    }
    let shortcuts = app.global_shortcut();
    if paused {
        for shortcut in hotkeys.hotkeys.mode_shortcuts() {
            if shortcuts.is_registered(shortcut) {
                let _ = shortcuts.unregister(shortcut);
            }
        }
    } else {
        hotkeys.prompt_error = None;
        hotkeys.dictation_error = None;
        register_mode_hotkeys(app, &mut hotkeys);
    }
    drop(hotkeys);
    crate::onboarding::notify(app);
}

/// Escape is only claimed while a job is active so it keeps working in other apps.
pub fn set_cancel_registered<R: Runtime>(app: &AppHandle<R>, cancel: Shortcut, active: bool) {
    #[cfg(target_os = "linux")]
    if crate::wayland_paste::applies() {
        crate::portal_shortcuts::set_cancel(active);
        return;
    }
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
    #[cfg(target_os = "linux")]
    if crate::wayland_paste::applies() {
        return;
    }
    let Some(state) = app.try_state::<AppState>() else { return };
    let (hotkeys, paused) = {
        let guard = state.hotkeys.read().unwrap();
        (guard.hotkeys, guard.paused)
    };
    if paused {
        return;
    }
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

#[cfg(target_os = "linux")]
fn portal_errors(state: &mut HotkeyState) {
    let error = crate::portal_shortcuts::error();
    state.prompt_error = error.clone();
    state.dictation_error = error.clone();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_shortcut_errors_are_reported_once_without_hiding_distinct_errors() {
        let config = HotkeyConfig::defaults();
        let mut state = HotkeyState {
            hotkeys: Hotkeys::parse(&config).unwrap(),
            config,
            prompt_error: Some("Permission required".into()),
            dictation_error: Some("Permission required".into()),
            paused: false,
        };
        assert_eq!(state.errors(), vec!["Permission required"]);
        state.dictation_error = Some("Shortcut already in use".into());
        assert_eq!(state.errors(), vec!["Permission required", "Shortcut already in use"]);
    }
}

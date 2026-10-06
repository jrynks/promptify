//! Explicitly authorized Wayland activation. Never create a session from the paste path.
//! API/schema: ashpd 0.13.13 and org.freedesktop.portal.GlobalShortcuts version 1.
//! BindShortcuts is once per session; authorized launches reconnect a fresh session.
//! Escape is reserved for the session, but delivered only during an active job:
//! version 1 has no per-shortcut temporary registration or pass-through mechanism.

use std::sync::{Mutex, mpsc};

use ashpd::desktop::global_shortcuts::{Activated, Deactivated, GlobalShortcuts, NewShortcut};
use ashpd::desktop::Session;
use futures_util::future::{AbortHandle, Abortable};
use futures_util::{FutureExt, StreamExt};
use promptify_core::pipeline::Mode;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_global_shortcut::{Modifiers, Shortcut};

use crate::AppState;
use crate::controller::Command;
use crate::hotkeys::{HotkeyConfig, Hotkeys};

const HELP: &str = "Wayland shortcuts need permission. Open Settings and enable desktop integration again.";
const IDS: [&str; 3] = ["prompt", "dictation", "cancel"];

#[derive(Debug, Clone, serde::Serialize)]
pub struct Binding {
    pub id: String,
    pub trigger_description: String,
}

struct Backend {
    generation: u64,
    abort: Option<AbortHandle>,
    active: bool,
    cancel: bool,
    error: Option<String>,
    bindings: Vec<Binding>,
}

static BACKEND: Mutex<Backend> = Mutex::new(Backend {
    generation: 0, abort: None, active: false, cancel: false, error: None, bindings: Vec::new(),
});
static GRANT: Mutex<()> = Mutex::new(());

pub fn active() -> bool {
    BACKEND.lock().unwrap().active
}

pub fn error() -> Option<String> {
    let backend = BACKEND.lock().unwrap();
    if backend.active { None } else { Some(backend.error.clone().unwrap_or_else(|| HELP.into())) }
}

/// The desktop may override preferred triggers; show these actual descriptions in Settings.
pub fn bindings() -> Vec<Binding> {
    let backend = BACKEND.lock().unwrap();
    if backend.active { backend.bindings.clone() } else { Vec::new() }
}

/// Invalidates synchronously; the worker closes its session, including an open bind dialog.
pub fn stop() {
    let mut backend = BACKEND.lock().unwrap();
    backend.generation = backend.generation.wrapping_add(1);
    backend.active = false;
    backend.error = Some(HELP.into());
    backend.bindings.clear();
    if let Some(abort) = backend.abort.take() {
        abort.abort();
    }
}

pub fn set_cancel(enabled: bool) {
    BACKEND.lock().unwrap().cancel = enabled;
}

/// Called by explicit Enable or restoration after a previously completed authorization.
pub fn grant(app: AppHandle) -> Result<(), String> {
    if !crate::wayland_paste::applies() {
        return Err("GlobalShortcuts permission is only used in a Wayland session.".into());
    }
    let _grant = GRANT.try_lock().map_err(|_| "A shortcut permission request is already running.".to_string())?;
    let state = app.try_state::<AppState>().ok_or("Promptify is not initialized.")?;
    let config = {
        let hotkeys = state.hotkeys.read().unwrap();
        if hotkeys.paused {
            return Err("Resume shortcuts before enabling desktop integration.".into());
        }
        hotkeys.config.clone()
    };
    let shortcuts = validate_config(&config)?;
    if active() {
        return Ok(());
    }
    if BACKEND.lock().unwrap().cancel {
        return Err("Wait for the active recording, processing, or paste job to finish before granting shortcut permission.".into());
    }
    stop();
    let (abort, registration) = AbortHandle::new_pair();
    let generation = {
        let mut backend = BACKEND.lock().unwrap();
        backend.abort = Some(abort);
        backend.generation
    };
    let (tx, rx) = mpsc::channel();
    let worker_app = app.clone();
    let spawn = std::thread::Builder::new().name("portal-shortcuts".into()).spawn(move || {
        let result = zbus::block_on(async {
            let portal = GlobalShortcuts::new().await.map_err(|error| permission_error(&worker_app, generation, error))?;
            if portal.version() < 1 {
                return Err("The desktop does not expose a usable GlobalShortcuts portal.".into());
            }
            let session = portal.create_session(Default::default()).await.map_err(|error| permission_error(&worker_app, generation, error))?;
            let result = Abortable::new(
                run_session(&worker_app, &portal, &session, &config, &shortcuts, generation, &tx),
                registration,
            ).await.unwrap_or_else(|_| Err(HELP.into()));
            {
                let mut backend = BACKEND.lock().unwrap();
                if backend.generation == generation {
                    backend.active = false;
                }
            }
            // Losing the activation session must not leave a press latched in the controller.
            if !active() {
                worker_app.state::<AppState>().controller.send(Command::Cancel);
            }
            if let Err(error) = session.close().await {
                log::warn!("could not close GlobalShortcuts session: {error}");
            }
            result
        });
        if let Err(message) = result {
            publish(&worker_app, generation, Some(&message));
            let _ = tx.send(Err(message));
        }
    });
    if let Err(error) = spawn {
        let message = format!("Could not start the GlobalShortcuts worker: {error}");
        publish(&app, generation, Some(&message));
        return Err(message);
    }
    rx.recv().map_err(|error| format!("GlobalShortcuts worker stopped before binding: {error}"))?
}

fn portal_error(error: impl std::fmt::Display) -> String {
    format!("The desktop's GlobalShortcuts portal failed: {error}. {HELP}")
}

fn forget_authorization(app: &AppHandle, generation: u64) {
    crate::commands::forget_desktop_authorization_if(app, || {
        let mut backend = BACKEND.lock().unwrap();
        if backend.generation != generation { return false; }
        backend.active = false;
        true
    });
}

fn permission_error(app: &AppHandle, generation: u64, error: ashpd::Error) -> String {
    if crate::wayland_paste::consent_denied(&error) {
        forget_authorization(app, generation);
    }
    portal_error(error)
}

pub fn restoration_failed(app: &AppHandle, message: &str) {
    let generation = BACKEND.lock().unwrap().generation;
    publish(app, generation, Some(message));
}

/// Returns false for obsolete workers. Notifications never run under the hotkey lock.
fn publish(app: &AppHandle, generation: u64, error: Option<&str>) -> bool {
    let Some(state) = app.try_state::<AppState>() else { return false };
    {
        let mut hotkeys = state.hotkeys.write().unwrap();
        let mut backend = BACKEND.lock().unwrap();
        if backend.generation != generation {
            return false;
        }
        backend.active = error.is_none();
        backend.error = error.map(str::to_owned);
        if error.is_some() {
            backend.bindings.clear();
        }
        hotkeys.prompt_error = backend.error.clone();
        hotkeys.dictation_error = backend.error.clone();
    }
    if let Some(message) = error {
        log::warn!("{message}");
        state.controller.send(Command::Cancel);
        crate::onboarding::invalidate(&state, Some(message));
    }
    crate::onboarding::notify(app);
    if let Err(error) = app.emit_to("settings", "desktop-integration-changed", bindings()) {
        log::warn!("could not notify settings of shortcut permission: {error}");
    }
    true
}

async fn run_session(
    app: &AppHandle,
    portal: &GlobalShortcuts,
    session: &Session<GlobalShortcuts>,
    config: &HotkeyConfig,
    shortcuts: &[NewShortcut],
    generation: u64,
    tx: &mpsc::Sender<Result<(), String>>,
) -> Result<(), String> {
    // Session implements Serialize as its object path; ashpd keeps its path accessor private.
    let path: String = serde_json::from_value(serde_json::to_value(session).map_err(portal_error)?)
        .map_err(portal_error)?;
    let activated = portal.receive_activated().await.map_err(portal_error)?.fuse();
    let deactivated = portal.receive_deactivated().await.map_err(portal_error)?.fuse();
    let closed = session.receive_closed().await.map_err(portal_error)?.fuse();
    let changed = portal.receive_shortcuts_changed().await.map_err(portal_error)?.fuse();
    let owner = portal.receive_owner_changed().await.map_err(portal_error)?.fuse();
    futures_util::pin_mut!(activated, deactivated, closed, changed, owner);
    let binding = portal.bind_shortcuts(session, shortcuts, None, Default::default()).fuse();
    futures_util::pin_mut!(binding);
    let bound = futures_util::select! {
        result = binding => result.map_err(|error| permission_error(app, generation, error))?
            .response().map_err(|error| permission_error(app, generation, error))?,
        event = closed.next() => {
            if event.is_some() { forget_authorization(app, generation); }
            return Err(format!("Shortcut permission was closed while binding. {HELP}"));
        },
        _ = owner.next() => return Err(format!("The desktop portal disconnected while binding. {HELP}")),
    };
    check_bound(bound.shortcuts()).map_err(|error| {
        forget_authorization(app, generation);
        error
    })?;
    remember_bindings(generation, bound.shortcuts());
    {
        let state = app.state::<AppState>();
        let hotkeys = state.hotkeys.read().unwrap();
        if hotkeys.paused || hotkeys.config != *config {
            return Err("Shortcuts changed during authorization. Enable desktop integration again.".into());
        }
    }
    if !publish(app, generation, None) {
        return Err(HELP.into());
    }
    tx.send(Ok(())).map_err(portal_error)?;
    let mut pressed = Vec::new();
    loop {
        futures_util::select! {
            event = activated.next() => {
                let event = event.ok_or_else(|| portal_error("activation signal stream ended"))?;
                deliver(app, generation, &path, PortalEvent::Activated(&event), &mut pressed);
            },
            event = deactivated.next() => {
                let event = event.ok_or_else(|| portal_error("deactivation signal stream ended"))?;
                deliver(app, generation, &path, PortalEvent::Deactivated(&event), &mut pressed);
            },
            event = closed.next() => {
                if event.is_some() {
                    {
                        let mut backend = BACKEND.lock().unwrap();
                        if backend.generation != generation { return Err(HELP.into()); }
                        backend.active = false;
                    }
                    forget_authorization(app, generation);
                }
                return Err(format!("Shortcut permission was closed or revoked. {HELP}"));
            },
            _ = owner.next() => return Err(format!("The desktop portal disconnected or restarted. {HELP}")),
            event = changed.next() => {
                let event = event.ok_or_else(|| portal_error("shortcut change signal stream ended"))?;
                if event.session_handle().as_str() == path {
                    check_bound(event.shortcuts()).map_err(|error| {
                        forget_authorization(app, generation);
                        error
                    })?;
                    if remember_bindings(generation, event.shortcuts())
                        && let Err(error) = app.emit_to("settings", "desktop-integration-changed", bindings())
                    {
                        log::warn!("could not notify settings of changed portal shortcuts: {error}");
                    }
                }
            },
        }
    }
}

fn remember_bindings(generation: u64, shortcuts: &[ashpd::desktop::global_shortcuts::Shortcut]) -> bool {
    let mut backend = BACKEND.lock().unwrap();
    if backend.generation != generation { return false; }
    backend.bindings = shortcuts.iter().map(|shortcut| Binding {
        id: shortcut.id().to_owned(),
        trigger_description: shortcut.trigger_description().to_owned(),
    }).collect();
    true
}

fn check_bound(bound: &[ashpd::desktop::global_shortcuts::Shortcut]) -> Result<(), String> {
    let expected = IDS.into_iter();
    let missing: Vec<_> = expected.filter(|id| !bound.iter().any(|shortcut| shortcut.id() == *id
        && !shortcut.trigger_description().trim().is_empty())).collect();
    if missing.is_empty() { Ok(()) } else {
        Err(format!("The desktop did not bind these shortcuts: {}. {HELP}", missing.join(", ")))
    }
}

enum PortalEvent<'a> {
    Activated(&'a Activated),
    Deactivated(&'a Deactivated),
}

impl PortalEvent<'_> {
    fn details(&self) -> (String, &str, bool) {
        match self {
            Self::Activated(event) => (event.session_handle().to_string(), event.shortcut_id(), true),
            Self::Deactivated(event) => (event.session_handle().to_string(), event.shortcut_id(), false),
        }
    }
}

fn event_command(event: PortalEvent<'_>, session: &str, paused: bool, ready: bool, cancel: bool) -> Option<Command> {
    let (path, id, pressed) = event.details();
    if path != session || paused {
        return None;
    }
    if id == "cancel" {
        return (pressed && cancel).then_some(Command::Cancel);
    }
    let mode = match id {
        "prompt" => Mode::Prompt,
        "dictation" => Mode::Dictation,
        _ => return None,
    };
    if pressed { ready.then_some(Command::Press(mode)) } else { Some(Command::Release(mode)) }
}

fn deliver(app: &AppHandle, generation: u64, path: &str, event: PortalEvent<'_>, pressed: &mut Vec<String>) {
    let Some(state) = app.try_state::<AppState>() else { return };
    let paused = {
        let hotkeys = state.hotkeys.read().unwrap();
        hotkeys.paused
    };
    let (valid, cancel) = {
        let backend = BACKEND.lock().unwrap();
        (backend.active && backend.generation == generation, backend.cancel)
    };
    if !valid { return; }
    let settings = state.settings.read().unwrap().clone();
    let ready = crate::onboarding::engines_ready(&state, &settings);
    let (event_path, id, down) = event.details();
    let id = id.to_owned();
    if event_path == path && !paused && down && !ready && !cancel && IDS[..2].contains(&id.as_str()) {
        crate::onboarding::record_error(app, "The speech and language engines are not ready. Finish setup before using shortcuts.");
        crate::tray::show_settings(app);
        return;
    }
    let Some(command) = event_command(event, path, paused, ready || cancel, cancel) else { return };
    if id != "cancel" {
        if down {
            if pressed.contains(&id) { return; }
            pressed.push(id);
        } else {
            let Some(index) = pressed.iter().position(|pressed| *pressed == id) else { return };
            pressed.remove(index);
        }
    }
    state.controller.send(command);
}

pub(crate) fn validate_config(config: &HotkeyConfig) -> Result<Vec<NewShortcut>, String> {
    let hotkeys = Hotkeys::parse(config)?;
    let entries = [
        ("prompt", "Promptify: create a prompt", Some(hotkeys.prompt)),
        ("dictation", "Promptify: dictate text", Some(hotkeys.dictation)),
        ("cancel", "Promptify: cancel the active job", Some(hotkeys.cancel)),
    ];
    let mut ids = Vec::new();
    let mut triggers = Vec::new();
    entries.into_iter().filter_map(|(id, description, shortcut)| shortcut.map(|shortcut| (id, description, shortcut)))
        .map(|(id, description, shortcut)| {
            if ids.contains(&shortcut.id()) {
                return Err("That hotkey is already used by Promptify.".into());
            }
            ids.push(shortcut.id());
            let trigger = preferred_trigger(shortcut)?;
            if triggers.contains(&trigger) {
                return Err("Those hotkeys resolve to the same Wayland shortcut.".into());
            }
            triggers.push(trigger.clone());
            Ok(NewShortcut::new(id, description).preferred_trigger(trigger.as_str()))
        }).collect()
}

pub(crate) fn preferred_trigger(shortcut: Shortcut) -> Result<String, String> {
    let key = shortcut.key.to_string();
    let keysym = match key.as_str() {
        "Space" => "space", "Enter" => "Return", "Escape" => "Escape",
        "Tab" => "Tab", "Backspace" => "BackSpace", "Delete" => "Delete",
        "Insert" => "Insert", "Home" => "Home", "End" => "End",
        "PageUp" => "Prior", "PageDown" => "Next",
        "ArrowUp" => "Up", "ArrowDown" => "Down", "ArrowLeft" => "Left", "ArrowRight" => "Right",
        "Backquote" => "grave", "Backslash" => "backslash", "BracketLeft" => "bracketleft",
        "BracketRight" => "bracketright", "Comma" => "comma", "Equal" => "equal",
        "Minus" => "minus", "Period" => "period", "Quote" => "apostrophe",
        "Semicolon" => "semicolon", "Slash" => "slash",
        "CapsLock" => "Caps_Lock", "NumLock" => "Num_Lock", "ScrollLock" => "Scroll_Lock",
        "PrintScreen" => "Print", "Pause" => "Pause",
        "NumpadAdd" => "KP_Add", "NumpadDecimal" => "KP_Decimal", "NumpadDivide" => "KP_Divide",
        "NumpadEnter" => "KP_Enter", "NumpadEqual" => "KP_Equal",
        "NumpadMultiply" => "KP_Multiply", "NumpadSubtract" => "KP_Subtract",
        "Numpad0" => "KP_0", "Numpad1" => "KP_1", "Numpad2" => "KP_2",
        "Numpad3" => "KP_3", "Numpad4" => "KP_4", "Numpad5" => "KP_5",
        "Numpad6" => "KP_6", "Numpad7" => "KP_7", "Numpad8" => "KP_8", "Numpad9" => "KP_9",
        "AudioVolumeDown" => "XF86AudioLowerVolume", "AudioVolumeUp" => "XF86AudioRaiseVolume",
        "AudioVolumeMute" => "XF86AudioMute", "MediaPlay" | "MediaPlayPause" => "XF86AudioPlay",
        "MediaPause" => "XF86AudioPause", "MediaStop" => "XF86AudioStop",
        "MediaTrackNext" => "XF86AudioNext", "MediaTrackPrevious" => "XF86AudioPrev",
        key if key.starts_with("Key") && key.len() == 4 => &key[3..],
        key if key.starts_with("Digit") && key.len() == 6 => &key[5..],
        key if key.starts_with('F') && key[1..].parse::<u8>().is_ok_and(|n| (1..=35).contains(&n)) => key,
        _ => return Err(format!("The Wayland shortcut portal cannot express key {key:?}. Choose a letter, digit, function, navigation, or punctuation key.")),
    };
    let mut parts = Vec::new();
    for (flag, name) in [(Modifiers::CONTROL, "CTRL"), (Modifiers::ALT, "ALT"), (Modifiers::SHIFT, "SHIFT"), (Modifiers::SUPER, "LOGO")] {
        if shortcut.mods.contains(flag) { parts.push(name.to_owned()); }
    }
    parts.push(if key.starts_with("Key") { keysym.to_ascii_lowercase() } else { keysym.to_owned() });
    Ok(parts.join("+"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SESSION: &str = "/org/freedesktop/portal/desktop/session/test/shortcuts";

    fn activated(id: &str) -> Activated {
        wire_event(id)
    }
    fn deactivated(id: &str) -> Deactivated {
        wire_event(id)
    }

    fn wire_event<T: for<'de> serde::Deserialize<'de> + zbus::zvariant::Type>(id: &str) -> T {
        use zbus::zvariant::{Endian, OwnedObjectPath, OwnedValue, serialized::Context, to_bytes};
        let body = (OwnedObjectPath::try_from(SESSION).unwrap(), id.to_owned(), 1234_u64,
            std::collections::HashMap::<String, OwnedValue>::new());
        let bytes = to_bytes(Context::new_dbus(Endian::Little, 0), &body).unwrap();
        bytes.deserialize::<T>().unwrap().0
    }

    #[test]
    fn typed_portal_signals_map_all_modes_and_cancel() {
        for (id, expected) in [("prompt", Mode::Prompt), ("dictation", Mode::Dictation)] {
            assert!(matches!(event_command(PortalEvent::Activated(&activated(id)), SESSION, false, true, false), Some(Command::Press(mode)) if mode == expected));
            assert!(matches!(event_command(PortalEvent::Deactivated(&deactivated(id)), SESSION, false, false, false), Some(Command::Release(mode)) if mode == expected));
        }
        assert!(matches!(event_command(PortalEvent::Activated(&activated("cancel")), SESSION, false, false, true), Some(Command::Cancel)));
        assert!(event_command(PortalEvent::Deactivated(&deactivated("cancel")), SESSION, false, true, true).is_none());
        assert!(event_command(PortalEvent::Activated(&activated("cancel")), SESSION, false, true, false).is_none());
    }

    #[test]
    fn signals_require_our_session_readiness_and_unpaused_state() {
        let event = activated("prompt");
        assert!(event_command(PortalEvent::Activated(&event), "/another/session", false, true, true).is_none());
        assert!(event_command(PortalEvent::Activated(&event), SESSION, true, true, true).is_none());
        assert!(event_command(PortalEvent::Activated(&event), SESSION, false, false, true).is_none());
        assert!(event_command(PortalEvent::Activated(&activated("unknown")), SESSION, false, true, true).is_none());
    }

    #[test]
    fn preferred_triggers_use_xdg_not_plugin_accelerator_syntax() {
        for (accelerator, expected) in [
            ("CommandOrControl+Alt+Space", "CTRL+ALT+space"),
            ("CommandOrControl+Alt+Shift+Space", "CTRL+ALT+SHIFT+space"),
            ("Super+Shift+A", "SHIFT+LOGO+a"), ("Ctrl+Enter", "CTRL+Return"),
            ("Alt+1", "ALT+1"), ("F12", "F12"), ("Escape", "Escape"),
            ("Num1", "KP_1"), ("MediaPlayPause", "XF86AudioPlay"), ("CapsLock", "Caps_Lock"),
        ] {
            assert_eq!(preferred_trigger(accelerator.parse().unwrap()).unwrap(), expected);
        }
        assert_eq!(validate_config(&HotkeyConfig::defaults()).unwrap().len(), 3);
        let mut config = HotkeyConfig::defaults();
        config.dictation = config.prompt.clone();
        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn partial_or_revoked_bindings_are_not_success() {
        use ashpd::desktop::global_shortcuts::Shortcut;
        use zbus::zvariant::{Endian, Value, serialized::Context, to_bytes};
        let shortcut = |id: &str, trigger: &str| {
            let info = std::collections::HashMap::from([
                ("description", Value::from("Promptify action")),
                ("trigger_description", Value::from(trigger)),
            ]);
            let bytes = to_bytes(Context::new_dbus(Endian::Little, 0), &(id, info)).unwrap();
            bytes.deserialize::<Shortcut>().unwrap().0
        };
        let mut bound = vec![shortcut("prompt", "Ctrl+Alt+Space"), shortcut("dictation", "Ctrl+Alt+Shift+Space")];
        assert!(check_bound(&bound).unwrap_err().contains("cancel"));
        bound.push(shortcut("cancel", "Escape"));
        assert!(check_bound(&bound).is_ok());
        bound[0] = shortcut("prompt", "");
        assert!(check_bound(&bound).unwrap_err().contains("prompt"));
        bound[0] = shortcut("prompt", "Ctrl+Alt+Space");
        assert!(check_bound(&bound).is_ok());
    }
}

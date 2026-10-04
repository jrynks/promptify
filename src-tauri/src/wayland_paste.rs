//! Keyboard-only, user-authorized paste sessions on Wayland. Permission dialogs never run mid-paste.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use ashpd::desktop::remote_desktop::{DeviceType, KeyState, RemoteDesktop, SelectDevicesOptions};
use ashpd::desktop::{PersistMode, Session};
use ashpd::enumflags2::BitFlags;
use futures_util::StreamExt;
use promptify_core::context::WindowIdentity;
use promptify_core::pipeline::ContextProvider;
use serde::Serialize;

const CONTROL_L: i32 = 0xffe3;
const SHIFT_L: i32 = 0xffe1;
const KEY_V: i32 = 0x0076;
const KEY_GAP: Duration = Duration::from_millis(8);
const PERMISSION_HELP: &str = "Open Promptify Settings and choose Enable desktop integration.";

struct Active {
    proxy: RemoteDesktop,
    session: Session<RemoteDesktop>,
    closed: AtomicBool,
}

static TOKEN_PATH: OnceLock<PathBuf> = OnceLock::new();
static ACTIVE: Mutex<Option<Arc<Active>>> = Mutex::new(None);
static GRANT: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionState {
    NotNeeded,
    Granted,
    Required,
}

pub fn applies() -> bool {
    is_wayland(std::env::var("XDG_SESSION_TYPE").ok().as_deref(), std::env::var_os("WAYLAND_DISPLAY").is_some())
}

fn is_wayland(session: Option<&str>, display: bool) -> bool {
    match session {
        Some(session) => session.eq_ignore_ascii_case("wayland"),
        None => display,
    }
}

pub fn init(data_dir: &Path) {
    if TOKEN_PATH.set(data_dir.join("remote-desktop-token")).is_err() {
        log::warn!("paste permission storage was already initialized");
    }
}

fn read_token(path: &Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(path) {
        Ok(token) if !token.trim().is_empty() => Ok(Some(token.trim().to_owned())),
        Ok(_) => Err("The saved paste permission is empty. Remove remote-desktop-token from the data folder and grant permission again.".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("Could not read saved paste permission: {e}")),
    }
}

fn write_token(path: &Path, token: &str) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600)
            .open(&temporary).map_err(|e| format!("Could not save paste permission: {e}"))?;
        file.write_all(token.as_bytes()).and_then(|_| file.sync_all()).map_err(|e| format!("Could not save paste permission: {e}"))?;
        std::fs::rename(&temporary, path).map_err(|e| format!("Could not save paste permission: {e}"))
    })();
    if result.is_err() && let Err(e) = std::fs::remove_file(&temporary) && e.kind() != std::io::ErrorKind::NotFound {
        log::warn!("could not remove temporary paste permission: {e}");
    }
    result
}

fn active() -> Result<Arc<Active>, String> {
    ACTIVE.lock().unwrap().as_ref().filter(|active| !active.closed.load(Ordering::SeqCst)).cloned()
        .ok_or_else(|| format!("Automatic paste permission is not active. {PERMISSION_HELP}"))
}

pub fn status() -> PermissionState {
    if !applies() {
        PermissionState::NotNeeded
    } else if active().is_ok() {
        PermissionState::Granted
    } else {
        PermissionState::Required
    }
}

pub fn require_ready() -> Result<(), String> {
    if applies() {
        active()?;
    }
    Ok(())
}

fn close(active: &Active) {
    active.closed.store(true, Ordering::SeqCst);
    if let Err(e) = zbus::block_on(active.session.close()) {
        log::warn!("could not close remote-input session: {e}");
    }
}

async fn connect(token: Option<&str>) -> Result<(Arc<Active>, Option<String>), String> {
    let failed = |e: ashpd::Error| format!("The desktop's remote-input portal failed: {e}");
    let proxy = RemoteDesktop::new().await.map_err(failed)?;
    let session = proxy.create_session(Default::default()).await.map_err(failed)?;
    let result = async {
        proxy.select_devices(
            &session,
            SelectDevicesOptions::default()
                .set_devices(BitFlags::from_flag(DeviceType::Keyboard))
                .set_persist_mode(PersistMode::ExplicitlyRevoked)
                .set_restore_token(token),
        ).await.map_err(failed)?.response().map_err(failed)?;
        let selected = proxy.start(&session, None, Default::default()).await.map_err(failed)?
            .response().map_err(|e| format!("Paste permission was not granted: {e}"))?;
        if !selected.devices().contains(DeviceType::Keyboard) {
            return Err("Paste permission did not include the keyboard.".into());
        }
        Ok(selected.restore_token().map(str::to_owned))
    }.await;
    match result {
        Ok(token) => Ok((Arc::new(Active { proxy, session, closed: AtomicBool::new(false) }), token)),
        Err(e) => {
            if let Err(close_error) = session.close().await {
                log::warn!("could not close rejected remote-input session: {close_error}");
            }
            Err(e)
        }
    }
}

pub fn has_saved_permission() -> Result<bool, String> {
    let path = TOKEN_PATH.get().ok_or("Paste permission storage is not initialized.")?;
    read_token(path).map(|token| token.is_some())
}

/// Permission creation/restoration runs only on an explicit Settings or native-test action.
pub fn grant(on_closed: impl FnOnce() + Send + 'static) -> Result<(), String> {
    let _grant = GRANT.try_lock().map_err(|_| "A paste permission request is already running.".to_string())?;
    if active().is_ok() {
        return Ok(());
    }
    let path = TOKEN_PATH.get().ok_or("Paste permission storage is not initialized.")?;
    let token = read_token(path)?;
    let (next, restore_token) = zbus::block_on(connect(token.as_deref()))?;
    if let Some(token) = restore_token && let Err(e) = write_token(path, &token) {
        close(&next);
        return Err(e);
    }

    let watched = next.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let monitor = std::thread::Builder::new().name("paste-permission".into()).spawn(move || {
        zbus::block_on(async {
            match watched.session.receive_closed().await {
                Ok(mut stream) => {
                    let _ = tx.send(Ok(()));
                    stream.next().await;
                    watched.closed.store(true, Ordering::SeqCst);
                    on_closed();
                }
                Err(e) => { let _ = tx.send(Err(format!("Could not monitor paste permission: {e}"))); }
            }
        });
    });
    if let Err(e) = monitor {
        close(&next);
        return Err(format!("Could not monitor paste permission: {e}"));
    }
    if let Err(e) = rx.recv().map_err(|e| e.to_string()).and_then(|result| result) {
        close(&next);
        return Err(e);
    }
    if let Some(previous) = ACTIVE.lock().unwrap().replace(next) {
        close(&previous);
    }
    Ok(())
}

fn paste_keys(terminal: bool, mut send: impl FnMut(i32, KeyState) -> Result<(), String>) -> Result<(), String> {
    let keys = if terminal { vec![CONTROL_L, SHIFT_L, KEY_V] } else { vec![CONTROL_L, KEY_V] };
    let mut pressed = Vec::new();
    let mut result = Ok(());
    for key in keys {
        // Even a failed call may have reached the compositor; always attempt a release.
        pressed.push(key);
        if let Err(e) = send(key, KeyState::Pressed) {
            result = Err(e);
            break;
        }
    }
    for key in pressed.into_iter().rev() {
        if let Err(e) = send(key, KeyState::Released) {
            log::warn!("could not release paste key {key}: {e}");
            if result.is_ok() { result = Err(e); }
        }
    }
    result
}

pub fn paste(target: &WindowIdentity, terminal: bool) -> Result<(), String> {
    paste_guarded(target, terminal, &|| true)
}

pub fn paste_guarded(target: &WindowIdentity, terminal: bool, allowed: &dyn Fn() -> bool) -> Result<(), String> {
    let session = active()?;
    let current = crate::system_context::SystemContext.foreground().map_err(|e| e.0)?;
    if current != *target {
        return Err("The focused window changed before the paste keystroke. Nothing was pasted.".into());
    }
    if !allowed() {
        return Err("Insertion was cancelled or desktop integration changed before keyboard delivery. Nothing was pasted.".into());
    }
    let result = paste_keys(terminal, |key, state| {
        let result = zbus::block_on(session.proxy.notify_keyboard_keysym(&session.session, key, state, Default::default()))
            .map_err(|e| format!("Could not send paste keystroke: {e}. {PERMISSION_HELP}"));
        std::thread::sleep(KEY_GAP);
        result
    });
    // Never retry: a failed reply does not prove the keystroke was not delivered.
    if result.is_err() {
        close(&session);
    }
    result
}

pub fn shutdown() {
    let active = ACTIVE.lock().unwrap().take();
    if let Some(active) = active {
        close(&active);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_type_takes_precedence_over_inherited_display() {
        assert!(is_wayland(Some("Wayland"), false));
        assert!(!is_wayland(Some("x11"), true));
        assert!(is_wayland(None, true));
        assert!(!is_wayland(None, false));
    }

    #[test]
    fn chords_release_keys_in_reverse_order() {
        for terminal in [false, true] {
            let mut events = Vec::new();
            paste_keys(terminal, |key, state| { events.push((key, state as u32)); Ok(()) }).unwrap();
            let keys = if terminal { vec![CONTROL_L, SHIFT_L, KEY_V] } else { vec![CONTROL_L, KEY_V] };
            let expected: Vec<_> = keys.iter().map(|key| (*key, 1)).chain(keys.iter().rev().map(|key| (*key, 0))).collect();
            assert_eq!(events, expected);
        }
    }

    #[test]
    fn failed_press_releases_modifiers_without_retrying() {
        let mut events = Vec::new();
        assert!(paste_keys(true, |key, state| {
            events.push((key, state as u32));
            if key == KEY_V && state as u32 == 1 { Err("injection failed".into()) } else { Ok(()) }
        }).is_err());
        assert_eq!(events, vec![(CONTROL_L, 1), (SHIFT_L, 1), (KEY_V, 1), (KEY_V, 0), (SHIFT_L, 0), (CONTROL_L, 0)]);
    }

    #[test]
    fn failed_release_does_not_skip_remaining_releases() {
        let mut events = Vec::new();
        assert!(paste_keys(false, |key, state| {
            events.push((key, state as u32));
            if key == KEY_V && state as u32 == 0 { Err("release failed".into()) } else { Ok(()) }
        }).is_err());
        assert_eq!(events.last(), Some(&(CONTROL_L, 0)));
    }

    #[test]
    fn saved_token_is_private_and_missing_is_not_a_grant() {
        use std::os::unix::fs::PermissionsExt;
        let directory = std::env::temp_dir().join(format!("promptify-permission-test-{}", std::process::id()));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("token");
        assert_eq!(read_token(&path).unwrap(), None);
        write_token(&path, "first").unwrap();
        write_token(&path, "second").unwrap();
        assert_eq!(read_token(&path).unwrap().as_deref(), Some("second"));
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        std::fs::write(&path, "").unwrap();
        assert!(read_token(&path).is_err());
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }
}

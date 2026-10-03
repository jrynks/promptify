use std::thread::sleep;
use std::time::Duration;

use arboard::{Clipboard, ImageData};
use enigo::{Direction, Enigo, Key, Keyboard, Settings};
use promptify_core::context::WindowIdentity;
use promptify_core::pipeline::{BackendError, ContextProvider, Inserter};
use promptify_core::profiles::PasteChord;

/// Time for the target app to read the clipboard before the user's clipboard is restored.
const RESTORE_DELAY: Duration = Duration::from_millis(350);

enum Saved {
    Text(String),
    Image(ImageData<'static>),
    Nothing,
}

fn err(msg: impl std::fmt::Display) -> BackendError {
    BackendError(msg.to_string())
}

/// Pastes through the clipboard, then restores what the user had copied.
pub struct ClipboardPaste;

impl Inserter for ClipboardPaste {
    fn insert(&self, target: &WindowIdentity, text: &str, chord: PasteChord) -> Result<(), BackendError> {
        #[cfg(target_os = "linux")]
        crate::wayland_paste::require_ready().map_err(err)?;
        let mut clipboard = Clipboard::new().map_err(|e| err(format!("clipboard unavailable: {e}")))?;
        let saved = match clipboard.get_text() {
            Ok(previous) => Saved::Text(previous),
            Err(_) => clipboard.get_image().map_or(Saved::Nothing, |image| Saved::Image(image.to_owned_img())),
        };
        clipboard.set_text(text.to_owned()).map_err(|e| err(format!("could not set clipboard: {e}")))?;
        sleep(Duration::from_millis(30));

        let pasted = send_paste(target, chord);
        sleep(RESTORE_DELAY);

        // Only restore if the clipboard still holds our text; a newer copy by the user wins.
        if clipboard.get_text().is_ok_and(|current| current == text) {
            let _ = match saved {
                Saved::Text(previous) => clipboard.set_text(previous),
                Saved::Image(image) => clipboard.set_image(image),
                Saved::Nothing => clipboard.clear(),
            };
        }
        pasted
    }
}

fn send_paste(target: &WindowIdentity, chord: PasteChord) -> Result<(), BackendError> {
    #[cfg(target_os = "linux")]
    if crate::wayland_paste::applies() {
        return crate::wayland_paste::paste(target, chord == PasteChord::Terminal).map_err(err);
    }
    let mut enigo = Enigo::new(&Settings::default()).map_err(|e| err(format!("keyboard input unavailable: {e}")))?;
    if crate::system_context::SystemContext.foreground()? != *target {
        return Err(err("The focused window changed before the paste keystroke. Nothing was pasted."));
    }
    let modifier = if cfg!(target_os = "macos") { Key::Meta } else { Key::Control };
    #[cfg(target_os = "windows")]
    let v = Key::V;
    #[cfg(not(target_os = "windows"))]
    let v = Key::Unicode('v');
    let shift = chord == PasteChord::Terminal && cfg!(target_os = "linux");

    paste_chord(modifier, v, shift, |key, direction| enigo.key(key, direction).map_err(err))
}

fn paste_chord(modifier: Key, v: Key, shift: bool, mut send: impl FnMut(Key, Direction) -> Result<(), BackendError>) -> Result<(), BackendError> {
    let mut pressed = Vec::new();
    let mut result = Ok(());
    for key in [Some(modifier), shift.then_some(Key::Shift)].into_iter().flatten() {
        pressed.push(key);
        if let Err(error) = send(key, Direction::Press) {
            result = Err(error);
            break;
        }
    }
    if result.is_ok() {
        result = send(v, Direction::Click);
    }
    for key in pressed.into_iter().rev() {
        if let Err(error) = send(key, Direction::Release) {
            log::warn!("could not release paste modifier: {error}");
            if result.is_ok() {
                result = Err(error);
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_paste_uses_control_or_command_and_releases_modifiers() {
        for modifier in [Key::Control, Key::Meta] {
            let mut events = Vec::new();
            paste_chord(modifier, Key::Unicode('v'), false, |key, direction| {
                events.push((key, direction));
                Ok(())
            }).unwrap();
            assert_eq!(events, vec![
                (modifier, Direction::Press),
                (Key::Unicode('v'), Direction::Click),
                (modifier, Direction::Release),
            ]);
        }
    }

    #[test]
    fn failed_paste_releases_every_modifier_without_retrying() {
        let mut events = Vec::new();
        assert!(paste_chord(Key::Control, Key::Unicode('v'), true, |key, direction| {
            events.push((key, direction));
            if direction == Direction::Click { Err(err("injection failed")) } else { Ok(()) }
        }).is_err());
        assert_eq!(events, vec![
            (Key::Control, Direction::Press), (Key::Shift, Direction::Press),
            (Key::Unicode('v'), Direction::Click),
            (Key::Shift, Direction::Release), (Key::Control, Direction::Release),
        ]);
    }

    #[test]
    fn failed_press_or_release_is_not_reported_as_success() {
        for failure in [Direction::Press, Direction::Release] {
            let mut events = Vec::new();
            assert!(paste_chord(Key::Meta, Key::Unicode('v'), false, |key, direction| {
                events.push((key, direction));
                if direction == failure { Err(err("permission denied")) } else { Ok(()) }
            }).is_err());
            assert_eq!(events.last(), Some(&(Key::Meta, Direction::Release)));
            if failure == Direction::Press {
                assert!(!events.iter().any(|(_, direction)| *direction == Direction::Click));
            }
        }
    }
}

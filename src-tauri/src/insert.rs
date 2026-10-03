use std::thread::sleep;
use std::time::Duration;

use arboard::{Clipboard, ImageData};
use enigo::{Direction, Enigo, Key, Keyboard, Settings};
use promptify_core::context::WindowIdentity;
use promptify_core::pipeline::{BackendError, Inserter};
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

fn send_paste(_target: &WindowIdentity, chord: PasteChord) -> Result<(), BackendError> {
    #[cfg(target_os = "linux")]
    if crate::wayland_paste::applies() {
        return crate::wayland_paste::paste(_target, chord == PasteChord::Terminal).map_err(err);
    }
    let mut enigo = Enigo::new(&Settings::default()).map_err(|e| err(format!("keyboard input unavailable: {e}")))?;
    let modifier = if cfg!(target_os = "macos") { Key::Meta } else { Key::Control };
    #[cfg(target_os = "windows")]
    let v = Key::V;
    #[cfg(not(target_os = "windows"))]
    let v = Key::Unicode('v');
    let shift = chord == PasteChord::Terminal && cfg!(target_os = "linux");

    enigo.key(modifier, Direction::Press).map_err(err)?;
    if shift {
        enigo.key(Key::Shift, Direction::Press).map_err(err)?;
    }
    let clicked = enigo.key(v, Direction::Click).map_err(err);
    if shift {
        let _ = enigo.key(Key::Shift, Direction::Release);
    }
    let _ = enigo.key(modifier, Direction::Release);
    clicked
}

use std::sync::Mutex;
use std::thread::sleep;
use std::time::Duration;

use arboard::{Clipboard, ImageData};
use enigo::{Direction, Enigo, Key, Keyboard, Settings};
use promptify_core::context::WindowIdentity;
use promptify_core::pipeline::{BackendError, ContextProvider, Inserter};
use promptify_core::profiles::PasteChord;

static CLIPBOARD_TRANSACTION: Mutex<Option<PendingClipboard>> = Mutex::new(None);

struct PendingClipboard {
    previous: Saved,
    staged: String,
    revision: Option<u32>,
}

enum Saved {
    Text(String),
    Html { html: String, text: Option<String> },
    Files(Vec<std::path::PathBuf>),
    Image(ImageData<'static>),
    Nothing,
}

fn err(msg: impl std::fmt::Display) -> BackendError {
    BackendError(msg.to_string())
}

fn read_optional<T>(
    result: Result<T, arboard::Error>,
    operation: &str,
) -> Result<Option<T>, BackendError> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(arboard::Error::ContentNotAvailable) => Ok(None),
        Err(error) => Err(err(format!("{operation}: {error}"))),
    }
}

fn capture_previous(clipboard: &mut Clipboard) -> Result<Saved, BackendError> {
    let text = read_optional(
        clipboard.get_text(),
        "Could not preserve clipboard text; no paste was attempted",
    )?;
    let html = capture_html(clipboard)?;
    let files = read_optional(
        clipboard.get().file_list(),
        "Could not preserve copied files; no paste was attempted",
    )?;
    let image = read_optional(
        clipboard.get_image(),
        "Could not preserve the clipboard image; no paste was attempted",
    )?;
    select_snapshot(text, html, files, image)
}

fn capture_html(clipboard: &mut Clipboard) -> Result<Option<String>, BackendError> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::DataExchange::{
            IsClipboardFormatAvailable, RegisterClipboardFormatW,
        };
        let name: Vec<u16> = "HTML Format".encode_utf16().chain(Some(0)).collect();
        // arboard reports missing HTML as Unknown, so query availability independently.
        let format = unsafe { RegisterClipboardFormatW(name.as_ptr()) };
        if format == 0 {
            return Err(err(format!(
                "Could not identify the clipboard HTML format: {}",
                std::io::Error::last_os_error()
            )));
        }
        if unsafe { IsClipboardFormatAvailable(format) } == 0 {
            return Ok(None);
        }
    }
    read_optional(
        clipboard.get().html(),
        "Could not preserve clipboard HTML; no paste was attempted",
    )
}

fn clipboard_revision() -> Result<Option<u32>, BackendError> {
    #[cfg(windows)]
    {
        let revision =
            unsafe { windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber() };
        Ok(Some(revision))
    }
    #[cfg(not(windows))]
    Ok(None)
}

fn same_revision(expected: Option<u32>, current: Option<u32>) -> bool {
    expected == current
}

fn select_snapshot(
    text: Option<String>,
    html: Option<String>,
    files: Option<Vec<std::path::PathBuf>>,
    image: Option<ImageData<'static>>,
) -> Result<Saved, BackendError> {
    let files = files.filter(|files| !files.is_empty());
    let families = usize::from(text.is_some() || html.is_some())
        + usize::from(files.is_some())
        + usize::from(image.is_some());
    if families > 1 {
        return Err(err(
            "The clipboard contains multiple content types that cannot be restored together. No paste was attempted. Save its contents before using Copy to replace the clipboard manually.",
        ));
    }
    if let Some(html) = html {
        return Ok(Saved::Html { html, text });
    }
    if let Some(text) = text {
        return Ok(Saved::Text(text));
    }
    if let Some(files) = files {
        return Ok(Saved::Files(files));
    }
    Ok(image.map_or(Saved::Nothing, Saved::Image))
}

fn staged_text_matches(
    current: Result<String, arboard::Error>,
    expected: &str,
) -> Result<bool, BackendError> {
    Ok(
        read_optional(current, "Could not verify staged clipboard text")?
            .is_some_and(|current| current == expected),
    )
}

/// Keeps staged text available until explicit restoration; dispatch is not consumption.
pub struct ClipboardPaste;

impl Inserter for ClipboardPaste {
    fn insert(
        &self,
        target: &WindowIdentity,
        text: &str,
        chord: PasteChord,
    ) -> Result<(), BackendError> {
        paste(target, text, chord, None, &|| true)
    }

    fn insert_into(
        &self,
        target: &WindowIdentity,
        destination: &promptify_core::delivery::Destination,
        text: &str,
        chord: PasteChord,
    ) -> Result<(), BackendError> {
        paste(target, text, chord, Some(destination), &|| true)
    }

    fn insert_guarded(
        &self,
        target: &WindowIdentity,
        destination: &promptify_core::delivery::Destination,
        text: &str,
        chord: PasteChord,
        allowed: &dyn Fn() -> bool,
    ) -> Result<(), BackendError> {
        paste(target, text, chord, Some(destination), allowed)
    }
}

fn paste(
    target: &WindowIdentity,
    text: &str,
    chord: PasteChord,
    destination: Option<&promptify_core::delivery::Destination>,
    allowed: &dyn Fn() -> bool,
) -> Result<(), BackendError> {
    let mut transaction = CLIPBOARD_TRANSACTION.lock().map_err(|_| {
        err("The clipboard transaction failed previously. Restart Promptify before trying again.")
    })?;
    check_delivery(allowed)?;
    #[cfg(target_os = "linux")]
    crate::wayland_paste::require_ready().map_err(err)?;
    let mut clipboard = Clipboard::new().map_err(|e| err(format!("clipboard unavailable: {e}")))?;
    let previous_revision = clipboard_revision()?;
    let saved = if let Some(pending) = transaction.as_ref() {
        if same_revision(pending.revision, previous_revision)
            && staged_text_matches(clipboard.get_text(), &pending.staged)?
        {
            None
        } else {
            Some(capture_previous(&mut clipboard)?)
        }
    } else {
        Some(capture_previous(&mut clipboard)?)
    };
    if !same_revision(previous_revision, clipboard_revision()?) {
        return Err(err(
            "The clipboard changed while preserving its contents. No paste was attempted.",
        ));
    }
    #[cfg(windows)]
    let staged = {
        use arboard::SetExtWindows;
        clipboard
            .set()
            .exclude_from_monitoring()
            .exclude_from_cloud()
            .exclude_from_history()
            .text(text.to_owned())
    }
    .map_err(err);
    #[cfg(not(windows))]
    let staged = clipboard.set_text(text.to_owned()).map_err(err);
    remember_staging(&mut transaction, saved, text)?;
    staged.map_err(|e| err(format!("Could not stage clipboard text: {e}. No paste keystroke was sent; the previous snapshot remains in memory.")))?;
    let staged_revision = clipboard_revision().map_err(|error| err(format!("Clipboard text was staged, but its change counter could not be verified: {error}. No paste keystroke was sent; the previous snapshot remains in memory.")))?;
    if let Some(pending) = transaction.as_mut() {
        pending.revision = staged_revision;
    }
    sleep(Duration::from_millis(30));

    let staged_matches = staged_text_matches(clipboard.get_text(), text);
    match staged_matches {
        Err(error) => {
            log::error!("staged clipboard verification failed: {error}");
            Err(err(format!(
                "Clipboard text was staged, but could not be verified: {error}. No paste keystroke was sent."
            )))
        }
        Ok(false) => Err(err(
            "The clipboard changed before insertion. Nothing was pasted. Your newer clipboard contents were left untouched.",
        )),
        Ok(true) => {
            if !same_revision(staged_revision, clipboard_revision()?) {
                return Err(err(
                    "The clipboard changed before insertion. Nothing was pasted; newer clipboard contents were left untouched.",
                ));
            }
            if let Some(expected) = destination.filter(|destination| destination.token.is_some()) {
                crate::system_context::SystemContext.destination(target).and_then(|current| {
                        if expected.unchanged(&current) {
                            send_paste(target, chord, allowed)
                        } else {
                            Err(err("The focused input changed before the paste keystroke. Nothing was pasted."))
                        }
                    })
            } else {
                send_paste(target, chord, allowed)
            }
        }
    }
}

fn remember_staging(
    transaction: &mut Option<PendingClipboard>,
    saved: Option<Saved>,
    text: &str,
) -> Result<(), BackendError> {
    let previous =
        match saved {
            Some(saved) => saved,
            None => transaction
                .take()
                .ok_or_else(|| {
                    err("The previous clipboard snapshot is missing. No paste keystroke was sent.")
                })?
                .previous,
        };
    *transaction = Some(PendingClipboard {
        previous,
        staged: text.to_owned(),
        revision: None,
    });
    Ok(())
}

pub fn restore_previous() -> Result<(), BackendError> {
    let mut transaction = CLIPBOARD_TRANSACTION.lock().map_err(|_| {
        err("The clipboard transaction failed previously. Restart Promptify before trying again.")
    })?;
    let mut clipboard = Clipboard::new()
        .map_err(|error| err(format!("Clipboard restoration is unavailable: {error}")))?;
    restore_pending(
        &mut transaction,
        &mut clipboard,
        clipboard_revision,
        |clipboard, pending| staged_text_matches(clipboard.get_text(), &pending.staged),
        |_clipboard, previous, revision| match previous {
            #[cfg(windows)]
            Saved::Text(previous) => {
                crate::clipboard_restore_windows::restore_text(previous, revision).map_err(err)
            }
            #[cfg(not(windows))]
            Saved::Text(previous) => _clipboard.set_text(previous.clone()).map_err(err),
            #[cfg(windows)]
            Saved::Html { html, text } => {
                crate::clipboard_restore_windows::restore_html(html, text.as_deref(), revision)
                    .map_err(err)
            }
            #[cfg(not(windows))]
            Saved::Html { html, text } => {
                _clipboard.set_html(html.clone(), text.clone()).map_err(err)
            }
            #[cfg(windows)]
            Saved::Files(files) => {
                crate::clipboard_restore_windows::restore_files(files, revision).map_err(err)
            }
            #[cfg(not(windows))]
            Saved::Files(files) => _clipboard.set().file_list(files).map_err(err),
            #[cfg(windows)]
            Saved::Image(image) => {
                crate::clipboard_restore_windows::restore_image(image, revision).map_err(err)
            }
            #[cfg(not(windows))]
            Saved::Image(image) => _clipboard.set_image(image.to_owned_img()).map_err(err),
            #[cfg(windows)]
            Saved::Nothing => crate::clipboard_restore_windows::clear(revision).map_err(err),
            #[cfg(not(windows))]
            Saved::Nothing => _clipboard.clear().map_err(err),
        },
    )
}

fn restore_pending<C>(
    transaction: &mut Option<PendingClipboard>,
    clipboard: &mut C,
    mut revision: impl FnMut() -> Result<Option<u32>, BackendError>,
    matches: impl FnOnce(&mut C, &PendingClipboard) -> Result<bool, BackendError>,
    restore: impl FnOnce(&mut C, &Saved, Option<u32>) -> Result<(), BackendError>,
) -> Result<(), BackendError> {
    let pending = transaction
        .as_ref()
        .ok_or_else(|| err("There is no previous insertion clipboard to restore."))?;
    if !same_revision(pending.revision, revision()?)
        || !matches(clipboard, pending)?
        || !same_revision(pending.revision, revision()?)
    {
        *transaction = None;
        return Err(err(
            "The clipboard changed since insertion. Its newer contents were left untouched; the old snapshot was discarded.",
        ));
    }
    restore(clipboard, &pending.previous, pending.revision).map_err(|error| {
        log::error!("clipboard restoration failed: {error}");
        err(format!("Could not restore the previous clipboard: {error}. Its snapshot remains available for retry."))
    })?;
    *transaction = None;
    Ok(())
}

pub fn restoration_pending() -> bool {
    CLIPBOARD_TRANSACTION
        .lock()
        .is_ok_and(|transaction| transaction.is_some())
}
fn check_delivery(allowed: &dyn Fn() -> bool) -> Result<(), BackendError> {
    if allowed() {
        Ok(())
    } else {
        Err(err(
            "Insertion was cancelled or desktop integration changed before keyboard delivery. Nothing was pasted.",
        ))
    }
}

fn send_paste(
    target: &WindowIdentity,
    chord: PasteChord,
    allowed: &dyn Fn() -> bool,
) -> Result<(), BackendError> {
    #[cfg(target_os = "linux")]
    if crate::wayland_paste::applies() {
        return crate::wayland_paste::paste_guarded(target, chord == PasteChord::Terminal, allowed)
            .map_err(err);
    }
    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|e| err(format!("keyboard input unavailable: {e}")))?;
    if crate::system_context::SystemContext.foreground()? != *target {
        return Err(err(
            "The focused window changed before the paste keystroke. Nothing was pasted.",
        ));
    }
    let modifier = if cfg!(target_os = "macos") {
        Key::Meta
    } else {
        Key::Control
    };
    #[cfg(target_os = "windows")]
    let v = Key::V;
    #[cfg(not(target_os = "windows"))]
    let v = Key::Unicode('v');
    let shift = chord == PasteChord::Terminal && cfg!(target_os = "linux");

    check_delivery(allowed)?;
    paste_chord(modifier, v, shift, |key, direction| {
        enigo.key(key, direction).map_err(err)
    })
}

fn paste_chord(
    modifier: Key,
    v: Key,
    shift: bool,
    mut send: impl FnMut(Key, Direction) -> Result<(), BackendError>,
) -> Result<(), BackendError> {
    let mut pressed = Vec::new();
    let mut result = Ok(());
    for key in [Some(modifier), shift.then_some(Key::Shift)]
        .into_iter()
        .flatten()
    {
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
            })
            .unwrap();
            assert_eq!(
                events,
                vec![
                    (modifier, Direction::Press),
                    (Key::Unicode('v'), Direction::Click),
                    (modifier, Direction::Release),
                ]
            );
        }
    }

    #[test]
    fn failed_paste_releases_every_modifier_without_retrying() {
        let mut events = Vec::new();
        assert!(
            paste_chord(Key::Control, Key::Unicode('v'), true, |key, direction| {
                events.push((key, direction));
                if direction == Direction::Click {
                    Err(err("injection failed"))
                } else {
                    Ok(())
                }
            })
            .is_err()
        );
        assert_eq!(
            events,
            vec![
                (Key::Control, Direction::Press),
                (Key::Shift, Direction::Press),
                (Key::Unicode('v'), Direction::Click),
                (Key::Shift, Direction::Release),
                (Key::Control, Direction::Release),
            ]
        );
    }

    #[test]
    fn failed_press_or_release_is_not_reported_as_success() {
        for failure in [Direction::Press, Direction::Release] {
            let mut events = Vec::new();
            assert!(
                paste_chord(Key::Meta, Key::Unicode('v'), false, |key, direction| {
                    events.push((key, direction));
                    if direction == failure {
                        Err(err("permission denied"))
                    } else {
                        Ok(())
                    }
                })
                .is_err()
            );
            assert_eq!(events.last(), Some(&(Key::Meta, Direction::Release)));
            if failure == Direction::Press {
                assert!(
                    !events
                        .iter()
                        .any(|(_, direction)| *direction == Direction::Click)
                );
            }
        }
    }

    #[test]
    fn clipboard_unavailability_is_not_confused_with_an_empty_clipboard() {
        assert!(
            read_optional::<String>(Err(arboard::Error::ContentNotAvailable), "snapshot")
                .unwrap()
                .is_none()
        );
        for error in [
            arboard::Error::ClipboardOccupied,
            arboard::Error::ClipboardNotSupported,
            arboard::Error::ConversionFailure,
            arboard::Error::Unknown {
                description: "native read failed".into(),
            },
        ] {
            assert!(read_optional::<String>(Err(error), "snapshot").is_err());
        }
    }

    #[test]
    fn changed_clipboard_is_neither_pasted_nor_restored_over() {
        assert!(staged_text_matches(Ok("generated prompt".into()), "generated prompt").unwrap());
        assert!(!staged_text_matches(Ok("new user copy".into()), "generated prompt").unwrap());
        assert!(
            !staged_text_matches(Err(arboard::Error::ContentNotAvailable), "generated prompt")
                .unwrap()
        );
        assert!(
            staged_text_matches(Err(arboard::Error::ClipboardOccupied), "generated prompt")
                .is_err()
        );
    }

    #[test]
    fn cancelled_native_insertion_does_not_access_the_clipboard() {
        let target = WindowIdentity {
            handle: 0,
            process_id: 0,
        };
        let error = ClipboardPaste
            .insert_guarded(
                &target,
                &Default::default(),
                "text",
                PasteChord::Standard,
                &|| false,
            )
            .unwrap_err();
        assert!(error.0.contains("cancelled"));
    }

    fn pending_clipboard() -> Option<PendingClipboard> {
        Some(PendingClipboard {
            previous: Saved::Text("original".into()),
            staged: "generated".into(),
            revision: Some(42),
        })
    }

    #[test]
    fn consecutive_staging_retains_the_original_unless_the_user_copied() {
        let mut transaction = pending_clipboard();
        remember_staging(&mut transaction, None, "second generation").unwrap();
        let pending = transaction.as_ref().unwrap();
        assert!(matches!(&pending.previous, Saved::Text(text) if text == "original"));
        assert_eq!(pending.staged, "second generation");
        assert_eq!(pending.revision, None);
        remember_staging(
            &mut transaction,
            Some(Saved::Text("new user copy".into())),
            "third generation",
        )
        .unwrap();
        assert!(
            matches!(&transaction.unwrap().previous, Saved::Text(text) if text == "new user copy")
        );
        assert!(remember_staging(&mut None, None, "missing snapshot").is_err());
    }

    #[test]
    fn explicit_restoration_consumes_the_original_snapshot_once() {
        let mut transaction = pending_clipboard();
        restore_pending(
            &mut transaction,
            &mut (),
            || Ok(Some(42)),
            |_, pending| {
                assert_eq!(pending.staged, "generated");
                Ok(true)
            },
            |_, previous, revision| {
                assert_eq!(revision, Some(42));
                assert!(matches!(previous, Saved::Text(text) if text == "original"));
                Ok(())
            },
        )
        .unwrap();
        assert!(transaction.is_none());
        assert!(
            restore_pending(
                &mut transaction,
                &mut (),
                || panic!("no clipboard access"),
                |_, _| panic!("no read"),
                |_, _, _| panic!("no restore")
            )
            .is_err()
        );
    }

    #[test]
    fn restoration_errors_keep_the_snapshot_for_retry() {
        let mut transaction = pending_clipboard();
        assert!(
            restore_pending(
                &mut transaction,
                &mut (),
                || Err(err("counter unavailable")),
                |_, _| panic!("no read"),
                |_, _, _| panic!("no restore")
            )
            .is_err()
        );
        assert!(transaction.is_some());
        assert!(
            restore_pending(
                &mut transaction,
                &mut (),
                || Ok(Some(42)),
                |_, _| Err(err("read unavailable")),
                |_, _, _| panic!("no restore")
            )
            .is_err()
        );
        assert!(transaction.is_some());
        let error = restore_pending(
            &mut transaction,
            &mut (),
            || Ok(Some(42)),
            |_, _| Ok(true),
            |_, _, _| Err(err("write unavailable")),
        )
        .unwrap_err();
        assert!(error.0.contains("snapshot remains available"));
        assert!(transaction.is_some());
        restore_pending(
            &mut transaction,
            &mut (),
            || Ok(Some(42)),
            |_, _| Ok(true),
            |_, _, _| Ok(()),
        )
        .unwrap();
        assert!(transaction.is_none());
    }

    #[test]
    fn restoration_discards_old_snapshot_without_overwriting_a_newer_copy() {
        for (revision, matches) in [(43, true), (42, false)] {
            let mut transaction = pending_clipboard();
            assert!(
                restore_pending(
                    &mut transaction,
                    &mut (),
                    || Ok(Some(revision)),
                    |_, _| Ok(matches),
                    |_, _, _| panic!("do not overwrite")
                )
                .is_err()
            );
            assert!(transaction.is_none());
        }
        let mut transaction = pending_clipboard();
        let mut revision = 41;
        assert!(
            restore_pending(
                &mut transaction,
                &mut (),
                || {
                    revision += 1;
                    Ok(Some(revision))
                },
                |_, _| Ok(true),
                |_, _, _| panic!("copy changed during the read")
            )
            .is_err()
        );
        assert!(transaction.is_none());
    }

    #[test]
    fn snapshot_keeps_html_and_its_plain_text_alternative() {
        let saved = select_snapshot(
            Some("formatted".into()),
            Some("<b>formatted</b>".into()),
            None,
            None,
        )
        .unwrap();
        assert!(
            matches!(saved, Saved::Html { html, text: Some(text) } if html == "<b>formatted</b>" && text == "formatted")
        );
        assert!(matches!(
            select_snapshot(None, Some("<br>".into()), None, None).unwrap(),
            Saved::Html { text: None, .. }
        ));
    }

    #[test]
    fn snapshot_keeps_copied_files_without_converting_them_to_text() {
        let path = std::path::PathBuf::from("copied-file");
        assert!(
            matches!(select_snapshot(None, None, Some(vec![path.clone()]), None).unwrap(), Saved::Files(files) if files == vec![path])
        );
        assert!(matches!(
            select_snapshot(None, None, Some(vec![]), None).unwrap(),
            Saved::Nothing
        ));
    }

    #[test]
    fn snapshot_blocks_content_combinations_that_would_be_lost() {
        let image = || ImageData {
            width: 1,
            height: 1,
            bytes: std::borrow::Cow::Owned(vec![0; 4]),
        };
        assert!(select_snapshot(Some("caption".into()), None, None, Some(image())).is_err());
        assert!(
            select_snapshot(
                None,
                Some("<b>caption</b>".into()),
                Some(vec!["file".into()]),
                None
            )
            .is_err()
        );
        assert!(matches!(
            select_snapshot(None, None, None, Some(image())).unwrap(),
            Saved::Image(_)
        ));
    }

    #[test]
    fn clipboard_revision_change_wins_even_when_new_copy_has_identical_text() {
        assert!(same_revision(Some(0), Some(0)));
        assert!(same_revision(Some(42), Some(42)));
        assert!(!same_revision(Some(42), Some(43)));
        assert!(!same_revision(Some(u32::MAX), Some(1)));
        assert!(!same_revision(Some(42), None));
        assert!(same_revision(None, None));
    }
}

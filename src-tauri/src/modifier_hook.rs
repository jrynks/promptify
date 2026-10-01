//! Optional "hold Ctrl+Shift alone" hotkey through a low-level keyboard hook. The hook only
//! classifies keys as Ctrl, Shift or other; it never records, logs or blocks them.

use promptify_core::modifier_chord::{ChordAction, ChordKey, ModifierChord};
use promptify_core::pipeline::Mode;
use tauri::{AppHandle, Manager};

use crate::AppState;
use crate::controller::Command;

/// Turns chord transitions into the same commands as the prompt hotkey.
fn dispatch(app: &AppHandle, action: ChordAction, pressed: &mut bool) {
    let Some(state) = app.try_state::<AppState>() else { return };
    match action {
        ChordAction::Press if !state.hotkeys.read().unwrap().paused => {
            *pressed = true;
            state.controller.send(Command::Press(Mode::Prompt));
        }
        // Release is always delivered for a press we sent, even if hotkeys were paused meanwhile.
        ChordAction::Release if *pressed => {
            *pressed = false;
            state.controller.send(Command::Release(Mode::Prompt));
        }
        _ => {}
    }
}

#[cfg(windows)]
mod platform {
    use std::sync::Mutex;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL, VK_LCONTROL, VK_LSHIFT, VK_RCONTROL, VK_RSHIFT, VK_SHIFT};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, GetMessageW, KBDLLHOOKSTRUCT, LLKHF_INJECTED, MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW, SetWindowsHookExW,
        UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP,
    };

    use super::*;

    /// The hook procedure has no user data, so events reach the chord thread through this channel.
    static EVENTS: Mutex<Option<mpsc::Sender<(ChordKey, bool)>>> = Mutex::new(None);

    fn classify(vk: u32) -> ChordKey {
        match vk as u16 {
            VK_CONTROL | VK_LCONTROL | VK_RCONTROL => ChordKey::Ctrl,
            VK_SHIFT | VK_LSHIFT | VK_RSHIFT => ChordKey::Shift,
            _ => ChordKey::Other,
        }
    }

    unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code >= 0 {
            // SAFETY: for WH_KEYBOARD_LL with code >= 0, lparam points at a KBDLLHOOKSTRUCT.
            let info = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
            if let (Some(event), Ok(guard)) = (translate(wparam as u32, info.vkCode, info.flags), EVENTS.try_lock())
                && let Some(tx) = guard.as_ref()
            {
                let _ = tx.send(event);
            }
        }
        unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
    }

    /// Promptify's own Ctrl+V paste and other synthetic input must never start a recording.
    pub(super) fn translate(message: u32, vk: u32, flags: u32) -> Option<(ChordKey, bool)> {
        if flags & LLKHF_INJECTED != 0 {
            return None;
        }
        let down = match message {
            WM_KEYDOWN | WM_SYSKEYDOWN => true,
            WM_KEYUP | WM_SYSKEYUP => false,
            _ => return None,
        };
        Some((classify(vk), down))
    }

    fn physically_down(vk: u16) -> bool {
        (unsafe { GetAsyncKeyState(i32::from(vk)) } as u16) & 0x8000 != 0
    }

    pub struct Hook {
        hook_thread: u32,
        threads: Vec<std::thread::JoinHandle<()>>,
    }

    impl Hook {
        pub fn start(app: AppHandle) -> Result<Self, String> {
            let (tx, rx) = mpsc::channel::<(ChordKey, bool)>();
            *EVENTS.lock().unwrap() = Some(tx);
            let chord_thread = std::thread::Builder::new()
                .name("modifier-chord".into())
                .spawn(move || {
                    let mut chord = ModifierChord::default();
                    let mut pressed = false;
                    loop {
                        match rx.recv_timeout(Duration::from_millis(30)) {
                            Ok((key, true)) => {
                                let action = chord.key_down(key, Instant::now());
                                dispatch(&app, action, &mut pressed);
                            }
                            Ok((key, false)) => {
                                let action = chord.key_up(key);
                                dispatch(&app, action, &mut pressed);
                            }
                            Err(mpsc::RecvTimeoutError::Timeout) => {}
                            Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        }
                        // Key-ups can be missed (secure desktop, lock screen); trust the physical state.
                        let action = chord.reconcile(Instant::now(), physically_down(VK_CONTROL), physically_down(VK_SHIFT));
                        dispatch(&app, action, &mut pressed);
                        let action = chord.tick(Instant::now());
                        dispatch(&app, action, &mut pressed);
                    }
                    if pressed {
                        dispatch(&app, ChordAction::Release, &mut pressed);
                    }
                })
                .map_err(|e| e.to_string())?;

            let (ready_tx, ready_rx) = mpsc::channel::<Result<u32, String>>();
            let hook_thread = std::thread::Builder::new()
                .name("keyboard-hook".into())
                .spawn(move || {
                    // Creates this thread's message queue so the WM_QUIT posted on drop is never lost.
                    let mut msg: MSG = unsafe { std::mem::zeroed() };
                    unsafe { PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_NOREMOVE) };
                    let hook = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), std::ptr::null_mut(), 0) };
                    if hook.is_null() {
                        let _ = ready_tx.send(Err(format!("could not install the keyboard hook: {}", std::io::Error::last_os_error())));
                        return;
                    }
                    let _ = ready_tx.send(Ok(unsafe { GetCurrentThreadId() }));
                    // Low-level hooks are called through this thread's message loop.
                    while unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) } > 0 {}
                    unsafe { UnhookWindowsHookEx(hook) };
                })
                .map_err(|e| e.to_string())?;
            match ready_rx.recv_timeout(Duration::from_secs(5)) {
                Ok(Ok(thread_id)) => Ok(Self { hook_thread: thread_id, threads: vec![chord_thread, hook_thread] }),
                Ok(Err(e)) => {
                    *EVENTS.lock().unwrap() = None;
                    Err(e)
                }
                Err(_) => {
                    *EVENTS.lock().unwrap() = None;
                    Err("the keyboard hook did not start".into())
                }
            }
        }
    }

    impl Drop for Hook {
        fn drop(&mut self) {
            unsafe { PostThreadMessageW(self.hook_thread, WM_QUIT, 0, 0) };
            // Dropping the sender ends the chord thread, which releases any held press.
            *EVENTS.lock().unwrap() = None;
            for thread in self.threads.drain(..) {
                let _ = thread.join();
            }
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use super::*;

    pub struct Hook;

    impl Hook {
        pub fn start(_app: AppHandle) -> Result<Self, String> {
            let _ = (dispatch, ModifierChord::default, ChordKey::Other);
            Err("holding Ctrl+Shift is only available on Windows for now".into())
        }
    }
}

pub use platform::Hook;

#[cfg(all(test, windows))]
mod tests {
    use promptify_core::modifier_chord::ChordKey;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{VK_LCONTROL, VK_RSHIFT, VK_T};
    use windows_sys::Win32::UI::WindowsAndMessaging::{LLKHF_INJECTED, WM_CHAR, WM_KEYDOWN, WM_SYSKEYUP};

    use super::platform::translate;

    #[test]
    fn hook_events_ignore_injected_input_and_classify_keys() {
        assert_eq!(translate(WM_KEYDOWN, u32::from(VK_LCONTROL), 0), Some((ChordKey::Ctrl, true)));
        assert_eq!(translate(WM_SYSKEYUP, u32::from(VK_RSHIFT), 0), Some((ChordKey::Shift, false)));
        assert_eq!(translate(WM_KEYDOWN, u32::from(VK_T), 0), Some((ChordKey::Other, true)));
        assert_eq!(translate(WM_KEYDOWN, u32::from(VK_LCONTROL), LLKHF_INJECTED), None, "our own paste");
        assert_eq!(translate(WM_CHAR, u32::from(VK_LCONTROL), 0), None);
    }
}

/// Installs or removes the hook to match the setting.
pub fn apply(app: &AppHandle, slot: &std::sync::Mutex<Option<Hook>>, enabled: bool) -> Result<(), String> {
    let mut slot = slot.lock().unwrap();
    match (enabled, slot.is_some()) {
        (true, false) => {
            *slot = Some(Hook::start(app.clone())?);
            log::info!("Ctrl+Shift hold hotkey on");
        }
        (false, true) => {
            *slot = None;
            log::info!("Ctrl+Shift hold hotkey off");
        }
        _ => {}
    }
    Ok(())
}

use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use core_foundation::runloop::{CFRunLoop, kCFRunLoopCommonModes, kCFRunLoopDefaultMode};
use core_graphics::event::{
    CGEvent, CGEventField, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
    CGEventType, CallbackResult,
};

use super::*;

const KEYCODE_LEFT_SHIFT: u16 = 56;
const KEYCODE_LEFT_CONTROL: u16 = 59;
const KEYCODE_RIGHT_SHIFT: u16 = 60;
const KEYCODE_RIGHT_CONTROL: u16 = 62;
const KEYCODE_COUNT: u16 = 128;
const KEYBOARD_EVENT_KEYCODE: CGEventField = 9;
const EVENT_SOURCE_HID_SYSTEM_STATE: i32 = 1;

const EVENT_SOURCE_UNIX_PROCESS_ID: CGEventField = 41;
const EVENT_SOURCE_USER_DATA: CGEventField = 42;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventSourceKeyState(state_id: i32, keycode: u16) -> u8;
}

#[derive(Clone, Copy)]
enum InputEvent {
    Modifier { key: ChordKey, down: bool },
    Other,
    MonitorFailed,
}

fn is_synthetic(event: &CGEvent) -> bool {
    event.get_integer_value_field(EVENT_SOURCE_UNIX_PROCESS_ID) != 0
        || event.get_integer_value_field(EVENT_SOURCE_USER_DATA) != 0
}

fn classify_event(event_type: CGEventType, event: &CGEvent) -> Option<InputEvent> {
    if is_synthetic(event) {
        return None;
    }

    let keycode = event.get_integer_value_field(KEYBOARD_EVENT_KEYCODE) as u16;
    match event_type {
        CGEventType::FlagsChanged => match keycode {
            KEYCODE_LEFT_CONTROL | KEYCODE_RIGHT_CONTROL => Some(InputEvent::Modifier {
                key: ChordKey::Ctrl,
                down: event
                    .get_flags()
                    .contains(core_graphics::event::CGEventFlags::CGEventFlagControl),
            }),
            KEYCODE_LEFT_SHIFT | KEYCODE_RIGHT_SHIFT => Some(InputEvent::Modifier {
                key: ChordKey::Shift,
                down: event
                    .get_flags()
                    .contains(core_graphics::event::CGEventFlags::CGEventFlagShift),
            }),
            _ => Some(InputEvent::Other),
        },
        CGEventType::KeyDown => Some(match keycode {
            KEYCODE_LEFT_CONTROL | KEYCODE_RIGHT_CONTROL => InputEvent::Modifier {
                key: ChordKey::Ctrl,
                down: true,
            },
            KEYCODE_LEFT_SHIFT | KEYCODE_RIGHT_SHIFT => InputEvent::Modifier {
                key: ChordKey::Shift,
                down: true,
            },
            _ => InputEvent::Other,
        }),
        CGEventType::KeyUp => match keycode {
            KEYCODE_LEFT_CONTROL | KEYCODE_RIGHT_CONTROL => Some(InputEvent::Modifier {
                key: ChordKey::Ctrl,
                down: false,
            }),
            KEYCODE_LEFT_SHIFT | KEYCODE_RIGHT_SHIFT => Some(InputEvent::Modifier {
                key: ChordKey::Shift,
                down: false,
            }),
            _ => None,
        },
        _ => None,
    }
}

fn physically_down(keycode: u16) -> bool {
    unsafe { CGEventSourceKeyState(EVENT_SOURCE_HID_SYSTEM_STATE, keycode) != 0 }
}

fn modifiers_physically_down() -> (bool, bool) {
    (
        physically_down(KEYCODE_LEFT_CONTROL) || physically_down(KEYCODE_RIGHT_CONTROL),
        physically_down(KEYCODE_LEFT_SHIFT) || physically_down(KEYCODE_RIGHT_SHIFT),
    )
}

fn other_key_physically_down() -> bool {
    (0..KEYCODE_COUNT).any(|keycode| {
        !matches!(
            keycode,
            KEYCODE_LEFT_CONTROL | KEYCODE_RIGHT_CONTROL | KEYCODE_LEFT_SHIFT | KEYCODE_RIGHT_SHIFT
        ) && physically_down(keycode)
    })
}

fn join_worker(thread: JoinHandle<()>, name: &str) {
    if thread.join().is_err() {
        log::error!("macOS modifier hook {name} thread panicked while stopping");
    }
}

fn run_chord_loop(app: AppHandle, rx: mpsc::Receiver<InputEvent>) {
    let mut chord = ModifierChord::default();
    let mut pressed = false;
    let (mut ctrl_down, mut shift_down) = modifiers_physically_down();
    let mut armed = !ctrl_down && !shift_down;

    loop {
        match rx.recv_timeout(Duration::from_millis(30)) {
            Ok(InputEvent::Modifier { key, down }) => {
                let current = match key {
                    ChordKey::Ctrl => &mut ctrl_down,
                    ChordKey::Shift => &mut shift_down,
                    ChordKey::Other => continue,
                };
                if *current != down {
                    *current = down;
                    if armed {
                        let action = if down {
                            chord.key_down(key, Instant::now())
                        } else {
                            chord.key_up(key)
                        };
                        dispatch(&app, action, &mut pressed);
                    }
                }
            }
            Ok(InputEvent::Other) if armed => {
                let action = chord.key_down(ChordKey::Other, Instant::now());
                dispatch(&app, action, &mut pressed);
            }
            Ok(InputEvent::Other) => {}
            Ok(InputEvent::MonitorFailed) => break,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }

        let now = Instant::now();
        let (physical_ctrl, physical_shift) = modifiers_physically_down();
        if !armed {
            ctrl_down = physical_ctrl;
            shift_down = physical_shift;
            armed = !ctrl_down && !shift_down;
            continue;
        }

        if physical_ctrl && !ctrl_down {
            ctrl_down = true;
            let action = chord.key_down(ChordKey::Ctrl, now);
            dispatch(&app, action, &mut pressed);
        } else if !physical_ctrl {
            ctrl_down = false;
        }
        if physical_shift && !shift_down {
            shift_down = true;
            let action = chord.key_down(ChordKey::Shift, now);
            dispatch(&app, action, &mut pressed);
        } else if !physical_shift {
            shift_down = false;
        }

        let action = chord.reconcile(now, physical_ctrl, physical_shift);
        dispatch(&app, action, &mut pressed);
        if (physical_ctrl || physical_shift) && other_key_physically_down() {
            let action = chord.key_down(ChordKey::Other, now);
            dispatch(&app, action, &mut pressed);
        }
        let action = chord.tick(now);
        dispatch(&app, action, &mut pressed);
    }

    if pressed {
        dispatch(&app, ChordAction::Release, &mut pressed);
    }
}

pub struct Hook {
    run_loop: CFRunLoop,
    tap_thread: Option<JoinHandle<()>>,
    chord_thread: Option<JoinHandle<()>>,
    error: Arc<Mutex<Option<String>>>,
    stopping: Arc<std::sync::atomic::AtomicBool>,
}

impl Hook {
    pub fn error(&self) -> Option<String> {
        self.error.lock().unwrap().clone()
    }

    pub fn start(app: AppHandle) -> Result<Self, String> {
        let (event_tx, event_rx) = mpsc::channel();
        let notify_app = app.clone();
        let chord_thread = thread::Builder::new()
            .name("modifier-chord".into())
            .spawn(move || run_chord_loop(app, event_rx))
            .map_err(|error| error.to_string())?;

        let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<CFRunLoop, String>>(1);
        let (start_tx, start_rx) = mpsc::sync_channel::<bool>(1);
        let error = Arc::new(Mutex::new(None));
        let thread_error = Arc::clone(&error);
        let stopping = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let thread_stopping = Arc::clone(&stopping);
        let tap_thread = match thread::Builder::new()
            .name("macos-event-tap".into())
            .spawn(move || {
                let tap = match CGEventTap::new(
                    CGEventTapLocation::HID,
                    CGEventTapPlacement::HeadInsertEventTap,
                    CGEventTapOptions::ListenOnly,
                    vec![CGEventType::KeyDown, CGEventType::KeyUp, CGEventType::FlagsChanged],
                    move |_, event_type, event| {
                        if matches!(
                            event_type,
                            CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput
                        ) {
                            let message = match event_type {
                                CGEventType::TapDisabledByTimeout => {
                                    "The macOS keyboard event monitor was disabled after timing out."
                                }
                                _ => "The macOS keyboard event monitor was disabled by the system.",
                            };
                            let mut stored_error = thread_error.lock().unwrap();
                            if stored_error.is_none() {
                                *stored_error = Some(format!(
                                    "{message} Retry Ctrl+Shift monitoring or restart Promptify. Verify Input Monitoring permission under System Settings > Privacy & Security > Input Monitoring."
                                ));
                                drop(stored_error);
                                crate::onboarding::notify(&notify_app);
                            }
                            let _ = event_tx.send(InputEvent::MonitorFailed);
                            return CallbackResult::Keep;
                        }
                        if let Some(input) = classify_event(event_type, event) {
                            let _ = event_tx.send(input);
                        }
                        CallbackResult::Keep
                    },
                ) {
                    Ok(tap) => tap,
                    Err(()) => {
                        let _ = ready_tx.send(Err(
                            "Could not install the macOS keyboard event tap. Promptify needs Input Monitoring permission to detect Ctrl+Shift globally. Enable Promptify under System Settings > Privacy & Security > Input Monitoring, then restart Promptify.".into(),
                        ));
                        return;
                    }
                };

                let source = match tap.mach_port().create_runloop_source(0) {
                    Ok(source) => source,
                    Err(()) => {
                        let _ = ready_tx.send(Err("Could not create the macOS keyboard event run-loop source.".into()));
                        return;
                    }
                };
                let run_loop = CFRunLoop::get_current();
                run_loop.add_source(&source, unsafe { kCFRunLoopCommonModes });
                tap.enable();
                if ready_tx.send(Ok(run_loop.clone())).is_err() || start_rx.recv() != Ok(true) {
                    return;
                }
                while !thread_stopping.load(std::sync::atomic::Ordering::Acquire) {
                    let _ = CFRunLoop::run_in_mode(unsafe { kCFRunLoopDefaultMode }, Duration::from_millis(100), false);
                }
            }) {
            Ok(thread) => thread,
            Err(error) => {
                join_worker(chord_thread, "chord");
                return Err(error.to_string());
            }
        };

        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(run_loop)) => {
                if start_tx.send(true).is_err() {
                    run_loop.stop();
                    join_worker(tap_thread, "event tap");
                    join_worker(chord_thread, "chord");
                    return Err("Could not start the macOS keyboard event run loop.".into());
                }
                Ok(Self {
                    run_loop,
                    tap_thread: Some(tap_thread),
                    chord_thread: Some(chord_thread),
                    error,
                    stopping,
                })
            }
            Ok(Err(error)) => {
                let _ = start_tx.send(false);
                join_worker(tap_thread, "event tap");
                join_worker(chord_thread, "chord");
                Err(error)
            }
            Err(RecvTimeoutError::Timeout) => {
                let _ = start_tx.send(false);
                join_worker(tap_thread, "event tap");
                join_worker(chord_thread, "chord");
                Err("The macOS keyboard event tap did not start within five seconds.".into())
            }
            Err(RecvTimeoutError::Disconnected) => {
                let _ = start_tx.send(false);
                join_worker(tap_thread, "event tap");
                join_worker(chord_thread, "chord");
                Err("The macOS keyboard event tap stopped before it was ready.".into())
            }
        }
    }
}

impl Drop for Hook {
    fn drop(&mut self) {
        self.stopping
            .store(true, std::sync::atomic::Ordering::Release);
        self.run_loop.stop();
        if let Some(thread) = self.tap_thread.take() {
            join_worker(thread, "event tap");
        }
        if let Some(thread) = self.chord_thread.take() {
            join_worker(thread, "chord");
        }
    }
}

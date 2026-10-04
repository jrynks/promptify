use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xinput::{self, ConnectionExt as _, DeviceType, XIEventMask};
use x11rb::protocol::xproto::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;

use super::*;

const POLL: Duration = Duration::from_millis(20);

enum Layout {
    X11(BTreeMap<u32, ChordKey>),
}

impl Layout {
    fn classify(&self, key: u32) -> ChordKey {
        match self {
            Self::X11(classes) => classes.get(&key).copied().unwrap_or(ChordKey::Other),
        }
    }
}

struct Chord {
    app: AppHandle,
    chord: ModifierChord,
    pressed: bool,
    down: BTreeMap<u16, BTreeSet<u32>>,
    layout: Layout,
}

impl Chord {
    fn new(app: AppHandle, layout: Layout) -> Self {
        Self {
            app,
            chord: ModifierChord::default(),
            pressed: false,
            down: BTreeMap::new(),
            layout,
        }
    }

    fn modifiers(&self) -> (bool, bool, bool) {
        let mut flags = (false, false, false);
        for key in self.down.values().flat_map(|keys| keys.iter()) {
            match self.layout.classify(*key) {
                ChordKey::Ctrl => flags.0 = true,
                ChordKey::Shift => flags.1 = true,
                ChordKey::Other => flags.2 = true,
            }
        }
        flags
    }

    fn changed(&mut self, previous: (bool, bool, bool), now: Instant) {
        let current = self.modifiers();
        for (key, before, after) in [
            (ChordKey::Ctrl, previous.0, current.0),
            (ChordKey::Shift, previous.1, current.1),
        ] {
            let action = if !before && after {
                self.chord.key_down(key, now)
            } else if before && !after {
                self.chord.key_up(key)
            } else {
                ChordAction::None
            };
            dispatch(&self.app, action, &mut self.pressed);
        }
        if current.2 {
            self.chord.key_down(ChordKey::Other, now);
        }
    }

    fn key(&mut self, device: u16, key: u32, down: bool) {
        let previous = self.modifiers();
        let keys = self.down.entry(device).or_default();
        if down {
            keys.insert(key);
        } else {
            keys.remove(&key);
        }
        self.changed(previous, Instant::now());
    }

    fn snapshot(&mut self, device: u16, keys: BTreeSet<u32>) {
        let previous = self.modifiers();
        self.down.insert(device, keys);
        self.changed(previous, Instant::now());
    }

    fn spoil(&mut self) {
        self.chord.key_down(ChordKey::Other, Instant::now());
    }

    fn tick(&mut self) {
        let (ctrl, shift, other) = self.modifiers();
        let action = self.chord.reconcile(Instant::now(), ctrl, shift);
        dispatch(&self.app, action, &mut self.pressed);
        if other {
            self.spoil();
        }
        let action = self.chord.tick(Instant::now());
        dispatch(&self.app, action, &mut self.pressed);
    }
}

impl Drop for Chord {
    fn drop(&mut self) {
        dispatch(&self.app, ChordAction::Release, &mut self.pressed);
    }
}

struct X11 {
    connection: RustConnection,
    classes: BTreeMap<u32, ChordKey>,
    sources: BTreeSet<u16>,
}

fn x11_error(error: impl std::fmt::Display) -> String {
    format!("Ctrl+Shift keyboard monitoring could not connect to X11: {error}")
}

impl X11 {
    fn start() -> Result<Self, String> {
        let (connection, screen) = x11rb::connect(None).map_err(x11_error)?;
        connection
            .xinput_xi_query_version(2, 1)
            .map_err(x11_error)?
            .reply()
            .map_err(x11_error)?;
        let setup = connection.setup();
        let mapping = connection
            .get_keyboard_mapping(setup.min_keycode, setup.max_keycode - setup.min_keycode + 1)
            .map_err(x11_error)?
            .reply()
            .map_err(x11_error)?;
        let mut classes = BTreeMap::new();
        if mapping.keysyms_per_keycode == 0 {
            return Err("X11 returned an empty keyboard mapping.".into());
        }
        for (index, symbols) in mapping
            .keysyms
            .chunks(usize::from(mapping.keysyms_per_keycode))
            .enumerate()
        {
            let class = if symbols
                .iter()
                .any(|symbol| [0xffe3, 0xffe4].contains(symbol))
            {
                ChordKey::Ctrl
            } else if symbols
                .iter()
                .any(|symbol| [0xffe1, 0xffe2].contains(symbol))
            {
                ChordKey::Shift
            } else {
                ChordKey::Other
            };
            classes.insert(u32::from(setup.min_keycode) + index as u32, class);
        }
        connection
            .xinput_xi_select_events(
                setup.roots[screen].root,
                &[
                    xinput::EventMask {
                        deviceid: 1,
                        mask: vec![XIEventMask::RAW_KEY_PRESS | XIEventMask::RAW_KEY_RELEASE],
                    },
                    xinput::EventMask {
                        deviceid: 0,
                        mask: vec![XIEventMask::HIERARCHY],
                    },
                ],
            )
            .map_err(x11_error)?
            .check()
            .map_err(x11_error)?;
        connection.flush().map_err(x11_error)?;
        let mut result = Self {
            connection,
            classes,
            sources: BTreeSet::new(),
        };
        result.refresh_sources()?;
        Ok(result)
    }

    fn refresh_sources(&mut self) -> Result<(), String> {
        let devices = self
            .connection
            .xinput_xi_query_device(0u16)
            .map_err(x11_error)?
            .reply()
            .map_err(x11_error)?;
        self.sources = devices
            .infos
            .into_iter()
            .filter(|device| {
                device.enabled
                    && device.type_ == DeviceType::SLAVE_KEYBOARD
                    && !is_injected(&device.name)
            })
            .map(|device| device.deviceid)
            .collect();
        Ok(())
    }

    fn run(self, app: AppHandle, stop: &AtomicBool) -> Result<(), String> {
        let mut chord = Chord::new(app, Layout::X11(self.classes.clone()));
        let initial = self
            .connection
            .query_keymap()
            .map_err(x11_error)?
            .reply()
            .map_err(x11_error)?;
        chord.snapshot(
            0,
            self.classes
                .keys()
                .copied()
                .filter(|code| x11_down(&initial.keys, *code))
                .collect(),
        );
        chord.spoil();
        while !stop.load(Ordering::SeqCst) {
            while let Some(event) = self.connection.poll_for_event().map_err(x11_error)? {
                let event = match event {
                    Event::XinputRawKeyPress(event) => Some((event.sourceid, event.detail, true)),
                    Event::XinputRawKeyRelease(event) => {
                        Some((event.sourceid, event.detail, false))
                    }
                    Event::XinputHierarchy(_) | Event::MappingNotify(_) => {
                        return Err("The X11 keyboard devices or layout changed. Retry Ctrl+Shift monitoring in Settings.".into());
                    }
                    _ => None,
                };
                if let Some((source, code, down)) = event
                    && self.sources.contains(&source)
                {
                    chord.key(source, code, down);
                }
            }
            let state = self
                .connection
                .query_keymap()
                .map_err(x11_error)?
                .reply()
                .map_err(x11_error)?;
            let previous = chord.modifiers();
            for keys in chord.down.values_mut() {
                keys.retain(|code| x11_down(&state.keys, *code));
            }
            chord.changed(previous, Instant::now());
            chord.tick();
            std::thread::sleep(POLL);
        }
        Ok(())
    }
}

fn x11_down(state: &[u8; 32], code: u32) -> bool {
    state
        .get(code as usize / 8)
        .is_some_and(|byte| byte & (1 << (code % 8)) != 0)
}

fn is_injected(name: &[u8]) -> bool {
    String::from_utf8_lossy(name)
        .to_ascii_uppercase()
        .contains("XTEST")
}

#[cfg(test)]
fn aggregate(
    down: &BTreeMap<u16, BTreeSet<u32>>,
    classes: &BTreeMap<u32, ChordKey>,
) -> (bool, bool, bool) {
    let mut flags = (false, false, false);
    for key in down.values().flat_map(|keys| keys.iter()) {
        match classes.get(key).copied().unwrap_or(ChordKey::Other) {
            ChordKey::Ctrl => flags.0 = true,
            ChordKey::Shift => flags.1 = true,
            ChordKey::Other => flags.2 = true,
        }
    }
    flags
}

pub struct Hook {
    stop: Arc<AtomicBool>,
    error: Arc<Mutex<Option<String>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Hook {
    pub fn start(app: AppHandle) -> Result<Self, String> {
        if crate::wayland_paste::applies() {
            return Err("Modifier-only monitoring is unavailable in this session. Use the configured Prompt shortcut through desktop integration.".into());
        }
        let backend = X11::start()?;
        let stop = Arc::new(AtomicBool::new(false));
        let error = Arc::new(Mutex::new(None));
        let flag = stop.clone();
        let failure = error.clone();
        let thread = std::thread::Builder::new()
            .name("modifier-chord".into())
            .spawn(move || {
                let result = backend.run(app.clone(), &flag);
                if let Err(error) = result {
                    log::warn!("Ctrl+Shift monitoring stopped: {error}");
                    *failure.lock().unwrap() = Some(error);
                    crate::onboarding::notify(&app);
                }
            })
            .map_err(|e| format!("Could not start Ctrl+Shift monitoring: {e}"))?;
        Ok(Self {
            stop,
            error,
            thread: Some(thread),
        })
    }

    pub fn error(&self) -> Option<String> {
        self.error.lock().unwrap().clone()
    }
}

impl Drop for Hook {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            log::error!("Ctrl+Shift monitoring thread panicked");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn injected_devices_are_classified() {
        assert!(is_injected(b"Virtual core XTEST keyboard"));
        assert!(!is_injected(b"USB keyboard"));
    }

    #[test]
    fn opposite_modifiers_and_multiple_keyboards_are_aggregated() {
        let classes = [
            (29, ChordKey::Ctrl),
            (97, ChordKey::Ctrl),
            (42, ChordKey::Shift),
            (54, ChordKey::Shift),
        ]
        .into_iter()
        .collect();
        let mut keys: BTreeMap<_, BTreeSet<_>> = [
            (1, [29, 97].into_iter().collect()),
            (2, [54].into_iter().collect()),
        ]
        .into_iter()
        .collect();
        assert_eq!(aggregate(&keys, &classes), (true, true, false));
        keys.get_mut(&1).unwrap().remove(&29);
        assert_eq!(aggregate(&keys, &classes), (true, true, false));
        keys.get_mut(&2).unwrap().insert(30);
        assert_eq!(aggregate(&keys, &classes), (true, true, true));
    }

}

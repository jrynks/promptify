use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use evdev::{Device, EventSummary};
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xinput::{self, ConnectionExt as _, DeviceType, XIEventMask};
use x11rb::protocol::xproto::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;

use super::*;

const POLL: Duration = Duration::from_millis(20);

enum Layout {
    Evdev,
    X11(BTreeMap<u32, ChordKey>),
}

impl Layout {
    fn classify(&self, key: u32) -> ChordKey {
        match self {
            Self::Evdev => classify_evdev(key),
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

fn classify_evdev(key: u32) -> ChordKey {
    match key {
        29 | 97 => ChordKey::Ctrl,
        42 | 54 => ChordKey::Shift,
        _ => ChordKey::Other,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Keyboard {
    path: PathBuf,
    name: String,
}

fn capability(bitmap: &str, key: usize) -> Result<bool, String> {
    let words: Vec<_> = bitmap.split_whitespace().rev().collect();
    let index = key / usize::BITS as usize;
    match words.get(index) {
        Some(word) => {
            let word = usize::from_str_radix(word, 16)
                .map_err(|e| format!("Invalid keyboard capabilities: {e}"))?;
            Ok(word & (1usize << (key % usize::BITS as usize)) != 0)
        }
        None => Ok(false),
    }
}

fn keyboards(root: &Path) -> Result<Vec<Keyboard>, String> {
    let entries =
        std::fs::read_dir(root).map_err(|e| format!("Could not enumerate keyboards: {e}"))?;
    let mut result = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("Could not enumerate keyboards: {e}"))?;
        let filename = entry.file_name();
        let name = filename.to_string_lossy();
        if !name.strip_prefix("event").is_some_and(|suffix| {
            !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
        }) {
            continue;
        }
        let device = entry.path().join("device");
        let physical = std::fs::canonicalize(&device)
            .map_err(|e| format!("Could not identify keyboard {name}: {e}"))?;
        if physical.starts_with("/sys/devices/virtual/input") {
            continue;
        }
        let bitmap = std::fs::read_to_string(device.join("capabilities/key"))
            .map_err(|e| format!("Could not read keyboard capabilities for {name}: {e}"))?;
        // Ignore mice, media controls and gamepads that expose only a few keyboard buttons.
        if capability(&bitmap, 30)?
            && capability(&bitmap, 44)?
            && (capability(&bitmap, 29)? || capability(&bitmap, 97)?)
            && (capability(&bitmap, 42)? || capability(&bitmap, 54)?)
        {
            let label = std::fs::read_to_string(device.join("name"))
                .map_err(|e| format!("Could not read keyboard name for {name}: {e}"))?;
            let path = PathBuf::from("/dev/input").join(name.as_ref());
            result.push(Keyboard {
                path: stable_keyboard_path(&path)?,
                name: label.trim().to_owned(),
            });
        }
    }
    result.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(result)
}

fn stable_keyboard_path(device: &Path) -> Result<PathBuf, String> {
    let canonical = std::fs::canonicalize(device)
        .map_err(|e| format!("Could not identify keyboard {}: {e}", device.display()))?;
    match std::fs::read_dir("/dev/input/by-id") {
        Ok(entries) => {
            for entry in entries {
                let entry =
                    entry.map_err(|e| format!("Could not enumerate keyboard identifiers: {e}"))?;
                if entry.file_name().to_string_lossy().ends_with("-event-kbd")
                    && std::fs::canonicalize(entry.path()).is_ok_and(|path| path == canonical)
                {
                    return Ok(entry.path());
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("Could not enumerate keyboard identifiers: {e}")),
    }
    Ok(device.to_owned())
}

pub(super) fn available_keyboards() -> Result<Vec<KeyboardDevice>, String> {
    keyboards(Path::new("/sys/class/input")).map(|keyboards| {
        keyboards
            .into_iter()
            .map(|keyboard| KeyboardDevice {
                path: keyboard.path.to_string_lossy().into_owned(),
                name: keyboard.name,
            })
            .collect()
    })
}

struct PhysicalKeyboard {
    description: Keyboard,
    id: u16,
    device: Device,
}

fn open_keyboard(description: Keyboard, id: u16) -> Result<PhysicalKeyboard, String> {
    let device = Device::open(&description.path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            format!(
                "Ctrl+Shift on Wayland needs read permission for keyboard '{}' ({}). \
                 Grant access only to this keyboard, then retry. Keyboard access can expose all keystrokes; \
                 Promptify only classifies modifiers and never saves typed text. See the README's modifier-hold setup.",
                description.name, description.path.display(),
            )
        } else {
            format!("Could not open keyboard {}: {e}", description.path.display())
        }
    })?;
    device.set_nonblocking(true).map_err(|e| {
        format!(
            "Could not monitor keyboard {}: {e}",
            description.path.display()
        )
    })?;
    Ok(PhysicalKeyboard {
        description,
        id,
        device,
    })
}

struct Evdev {
    devices: Vec<PhysicalKeyboard>,
    session: DesktopSession,
}

struct DesktopSession {
    connection: zbus::blocking::Connection,
    path: zbus::zvariant::OwnedObjectPath,
}

impl DesktopSession {
    fn start() -> Result<Self, String> {
        let failed = |error| {
            format!("Could not verify the active desktop session for keyboard monitoring: {error}")
        };
        let connection = zbus::blocking::Connection::system().map_err(failed)?;
        let manager = zbus::blocking::Proxy::new(
            &connection,
            "org.freedesktop.login1",
            "/org/freedesktop/login1",
            "org.freedesktop.login1.Manager",
        )
        .map_err(failed)?;
        let path = match std::env::var("XDG_SESSION_ID") {
            Ok(session) => manager.call("GetSession", &(session,)),
            Err(_) => manager.call("GetSessionByPID", &(std::process::id(),)),
        }
        .map_err(failed)?;
        drop(manager);
        Ok(Self { connection, path })
    }

    fn available(&self) -> Result<bool, String> {
        let proxy = zbus::blocking::Proxy::new(
            &self.connection,
            "org.freedesktop.login1",
            &self.path,
            "org.freedesktop.login1.Session",
        )
        .map_err(|error| format!("Could not check keyboard-monitoring session: {error}"))?;
        let active: bool = proxy
            .get_property("Active")
            .map_err(|error| format!("Could not check desktop activity: {error}"))?;
        let locked: bool = proxy
            .get_property("LockedHint")
            .map_err(|error| format!("Could not check desktop lock state: {error}"))?;
        Ok(active && !locked)
    }
}

impl Evdev {
    fn start(selected: &str) -> Result<Self, String> {
        let descriptions = keyboards(Path::new("/sys/class/input"))?;
        let keyboard = descriptions.into_iter().find(|keyboard| keyboard.path == Path::new(selected))
            .ok_or("The selected Ctrl+Shift keyboard is unavailable. Select a connected physical keyboard in Settings.")?;
        let devices = vec![open_keyboard(keyboard, 0)?];
        let session = DesktopSession::start()?;
        session.available()?;
        Ok(Self { devices, session })
    }

    fn run(mut self, app: AppHandle, stop: &AtomicBool) -> Result<(), String> {
        let mut chord = Chord::new(app, Layout::Evdev);
        for keyboard in &self.devices {
            chord.snapshot(
                keyboard.id,
                keyboard
                    .device
                    .get_key_state()
                    .map_err(|e| e.to_string())?
                    .iter()
                    .map(|key| u32::from(key.code()))
                    .collect(),
            );
        }
        chord.spoil();
        let mut last_scan = Instant::now();
        while !stop.load(Ordering::SeqCst) {
            for keyboard in &mut self.devices {
                match keyboard.device.fetch_events() {
                    Ok(events) => {
                        for event in events {
                            if let EventSummary::Key(_, key, value) = event.destructure() {
                                if value == 0 || value == 1 {
                                    chord.key(keyboard.id, u32::from(key.code()), value == 1);
                                }
                            }
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(e) => {
                        return Err(format!(
                            "Keyboard {} disconnected or access was lost: {e}. Retry Ctrl+Shift monitoring in Settings.",
                            keyboard.description.path.display()
                        ));
                    }
                }
                let keys = keyboard.device.get_key_state().map_err(|e| {
                    format!(
                        "Could not check keyboard {}: {e}",
                        keyboard.description.path.display()
                    )
                })?;
                chord.snapshot(
                    keyboard.id,
                    keys.iter().map(|key| u32::from(key.code())).collect(),
                );
            }
            if last_scan.elapsed() >= Duration::from_secs(1) {
                let descriptions = keyboards(Path::new("/sys/class/input"))?;
                if self
                    .devices
                    .iter()
                    .any(|keyboard| !descriptions.contains(&keyboard.description))
                {
                    // Release any active gesture before changing the physical device set.
                    return Err("The keyboard devices changed. Retry Ctrl+Shift monitoring in Settings to verify keyboard access.".into());
                }
                last_scan = Instant::now();
            }
            if self.session.available()? {
                chord.tick();
            } else {
                if chord.pressed {
                    if let Some(state) = chord.app.try_state::<AppState>() {
                        state.controller.send(Command::Cancel);
                    }
                    chord.pressed = false;
                }
                chord.chord = ModifierChord::default();
                let previous = (false, false, false);
                chord.changed(previous, Instant::now());
                chord.spoil();
            }
            std::thread::sleep(POLL);
        }
        Ok(())
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

enum Backend {
    Evdev(Evdev),
    X11(X11),
}

pub struct Hook {
    stop: Arc<AtomicBool>,
    error: Arc<Mutex<Option<String>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Hook {
    pub fn start(app: AppHandle) -> Result<Self, String> {
        let backend = if crate::wayland_paste::applies() {
            let state = app.state::<AppState>();
            let selected = state.settings.read().unwrap().modifier_keyboard.clone()
                .ok_or("Select a physical keyboard in Settings before enabling Ctrl+Shift hold on Wayland.")?;
            Backend::Evdev(Evdev::start(&selected)?)
        } else {
            Backend::X11(X11::start()?)
        };
        let stop = Arc::new(AtomicBool::new(false));
        let error = Arc::new(Mutex::new(None));
        let flag = stop.clone();
        let failure = error.clone();
        let thread = std::thread::Builder::new()
            .name("modifier-chord".into())
            .spawn(move || {
                let result = match backend {
                    Backend::Evdev(backend) => backend.run(app.clone(), &flag),
                    Backend::X11(backend) => backend.run(app.clone(), &flag),
                };
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
    fn linux_modifiers_and_injected_devices_are_classified() {
        for key in [29, 97] {
            assert_eq!(classify_evdev(key), ChordKey::Ctrl);
        }
        for key in [42, 54] {
            assert_eq!(classify_evdev(key), ChordKey::Shift);
        }
        for key in [30, 56, 125] {
            assert_eq!(classify_evdev(key), ChordKey::Other);
        }
        assert!(is_injected(b"Virtual core XTEST keyboard"));
        assert!(!is_injected(b"USB keyboard"));
    }

    #[test]
    fn kernel_capabilities_use_high_words_first() {
        assert!(capability("1 0", usize::BITS as usize).unwrap());
        assert!(!capability("1 0", 0).unwrap());
        assert!(capability("20000000", 29).unwrap());
        assert!(capability("nothex", 0).is_err());
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

    #[test]
    #[ignore = "requires explicit read access to physical keyboard devices"]
    fn live_physical_keyboards_are_readable() {
        let selected = std::env::var("PROMPTIFY_TEST_KEYBOARD")
            .expect("Set PROMPTIFY_TEST_KEYBOARD to the approved keyboard's by-id path");
        let backend = Evdev::start(&selected).unwrap();
        assert_eq!(backend.devices.len(), 1);
        assert_eq!(backend.devices[0].description.path, Path::new(&selected));
        for keyboard in backend.devices {
            keyboard.device.get_key_state().unwrap();
        }
    }
}

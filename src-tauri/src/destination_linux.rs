//! Bounded, metadata-only AT-SPI destination inspection.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, OnceLock};
use std::time::{Duration, Instant};

use promptify_core::context::WindowIdentity;
use promptify_core::delivery::Destination;
use promptify_core::pipeline::BackendError;
use zbus::blocking::connection::Builder as ConnectionBuilder;
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::OwnedObjectPath;

const REGISTRY: &str = "org.a11y.atspi.Registry";
const DESKTOP_PATH: &str = "/org/a11y/atspi/accessible/root";
const ACCESSIBLE: &str = "org.a11y.atspi.Accessible";
const MAX_APPS: usize = 32;
const MAX_WINDOWS: usize = 24;
const MAX_NODES: usize = 96;
const MAX_DEPTH: usize = 12;
const MAX_CHILDREN: usize = 32;
const CALL_TIMEOUT: Duration = Duration::from_millis(50);
const QUERY_BUDGET: Duration = Duration::from_millis(650);
const CALLER_WAIT: Duration = Duration::from_millis(800);

const STATE_ACTIVE: u32 = 1;
const STATE_DEFUNCT: u32 = 6;
const STATE_EDITABLE: u32 = 7;
const STATE_ENABLED: u32 = 8;
const STATE_FOCUSED: u32 = 12;
const STATE_SENSITIVE: u32 = 24;
const STATE_STALE: u32 = 27;
#[cfg(test)]
const STATE_HAS_POPUP: u32 = 42;
const STATE_READ_ONLY: u32 = 43;
const ROLE_DIALOG: u32 = 16;
const ROLE_FRAME: u32 = 23;
const ROLE_PASSWORD_TEXT: u32 = 40;
const ROLE_WINDOW: u32 = 69;

type Reply = mpsc::SyncSender<Result<Snapshot, String>>;
type Request = (Instant, Reply, Admission);

static WORKER: OnceLock<Result<Worker, String>> = OnceLock::new();

pub fn inspect(window: &WindowIdentity) -> Result<Destination, BackendError> {
    if window.handle == 0 || window.process_id == 0 {
        log::warn!(
            "AT-SPI destination metadata is unknown for invalid window identity {} (PID {})",
            window.handle,
            window.process_id
        );
        return Ok(Destination::default());
    }

    let snapshot = match worker().and_then(Worker::capture) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            log::warn!("AT-SPI destination metadata is unavailable: {error}");
            return Ok(Destination::default());
        }
    };
    if snapshot.context.window != *window {
        log::warn!(
            "AT-SPI focused destination no longer matches synthetic active-window identity {} (PID {})",
            window.handle,
            window.process_id
        );
        return Ok(Destination::default());
    }
    Ok(snapshot.destination)
}

/// Captures the unique live AT-SPI active window and its focused descendant.
///
/// The returned handle is a deterministic synthetic hash of the active
/// accessible's bus name and object path, not a native window handle.
pub fn active_context() -> Result<promptify_core::context::ActiveContext, BackendError> {
    worker()?.capture().map(|snapshot| snapshot.context)
}

fn worker() -> Result<&'static Worker, BackendError> {
    WORKER
        .get_or_init(Worker::start)
        .as_ref()
        .map_err(|error| BackendError(format!("AT-SPI destination metadata unavailable: {error}")))
}

struct Admission(Arc<AtomicBool>);

impl Admission {
    fn acquire(busy: &Arc<AtomicBool>) -> Option<Self> {
        busy.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Self(Arc::clone(busy)))
    }
}

impl Drop for Admission {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

struct Worker {
    requests: mpsc::SyncSender<Request>,
    busy: Arc<AtomicBool>,
}

impl Worker {
    fn start() -> Result<Self, String> {
        let (requests, receiver) = mpsc::sync_channel::<Request>(1);
        std::thread::Builder::new()
            .name("destination-atspi".into())
            .spawn(move || {
                while let Ok((deadline, reply, admission)) = receiver.recv() {
                    let result = inspect_accessible(deadline);
                    drop(admission);
                    let _ = reply.send(result);
                }
            })
            .map_err(|error| format!("could not start bounded AT-SPI worker: {error}"))?;
        Ok(Self {
            requests,
            busy: Arc::new(AtomicBool::new(false)),
        })
    }

    fn capture(&self) -> Result<Snapshot, BackendError> {
        let Some(admission) = Admission::acquire(&self.busy) else {
            return Err(BackendError(
                "AT-SPI destination worker is busy; no fresh focus snapshot is available".into(),
            ));
        };
        let (reply, receiver) = mpsc::sync_channel(1);
        let request = (Instant::now() + QUERY_BUDGET, reply, admission);
        if let Err(error) = self.requests.try_send(request) {
            return Err(BackendError(format!(
                "AT-SPI destination worker did not accept a fresh focus scan: {error}"
            )));
        }
        match receiver.recv_timeout(CALLER_WAIT) {
            Ok(Ok(snapshot)) => Ok(snapshot),
            Ok(Err(error)) => Err(BackendError(format!(
                "AT-SPI fresh focus scan failed: {error}"
            ))),
            Err(error) => Err(BackendError(format!(
                "AT-SPI fresh focus scan did not complete: {error}"
            ))),
        }
    }
}

struct Snapshot {
    context: promptify_core::context::ActiveContext,
    window_token: String,
    destination: Destination,
}

fn inspect_accessible(deadline: Instant) -> Result<Snapshot, String> {
    let session = ConnectionBuilder::session()
        .map_err(|error| format!("session bus unavailable: {error}"))?
        .method_timeout(CALL_TIMEOUT)
        .build()
        .map_err(|error| format!("session bus unavailable: {error}"))?;
    let bus_proxy = proxy(&session, "org.a11y.Bus", "/org/a11y/bus", "org.a11y.Bus")?;
    let address: String = call(&bus_proxy, "GetAddress", &())?;
    drop(bus_proxy);
    drop(session);
    check_deadline(deadline)?;

    let connection = ConnectionBuilder::address(address.as_str())
        .map_err(|error| format!("invalid accessibility bus address: {error}"))?
        .method_timeout(CALL_TIMEOUT)
        .build()
        .map_err(|error| format!("accessibility bus unavailable: {error}"))?;
    let active = locate_active_window(&connection, deadline)?;
    let snapshot = focused_snapshot(&connection, active, deadline)?;
    let rechecked = locate_active_window(&connection, deadline)?;
    if object_token(&rechecked.object) != snapshot.window_token
        || rechecked.process_id != snapshot.context.window.process_id
    {
        return Err("active AT-SPI window changed during focus inspection".into());
    }
    Ok(snapshot)
}

#[derive(Clone)]
struct ObjectRef {
    service: String,
    path: OwnedObjectPath,
}

fn proxy(
    connection: &Connection,
    service: &str,
    path: &str,
    interface: &str,
) -> Result<Proxy<'static>, String> {
    Proxy::<'static>::new_owned(
        connection.clone(),
        service.to_owned(),
        path.to_owned(),
        interface.to_owned(),
    )
    .map_err(|error| error.to_string())
}

fn accessible_proxy(connection: &Connection, object: &ObjectRef) -> Result<Proxy<'static>, String> {
    proxy(
        connection,
        &object.service,
        object.path.as_str(),
        ACCESSIBLE,
    )
}

fn call<R, B>(proxy: &Proxy<'_>, method: &str, body: &B) -> Result<R, String>
where
    R: for<'d> zbus::zvariant::DynamicDeserialize<'d>,
    B: serde::ser::Serialize + zbus::zvariant::DynamicType,
{
    proxy
        .call(method, body)
        .map_err(|error| format!("{method}: {error}"))
}

fn children(
    connection: &Connection,
    object: &ObjectRef,
    deadline: Instant,
) -> Result<Vec<ObjectRef>, String> {
    check_deadline(deadline)?;
    let proxy = accessible_proxy(connection, object)?;
    let child_count: i32 = proxy
        .get_property("ChildCount")
        .map_err(|error| format!("ChildCount: {error}"))?;
    if child_count < 0 || child_count as usize > MAX_CHILDREN {
        return Err(format!(
            "AT-SPI child count {child_count} exceeds the per-node traversal bound"
        ));
    }
    let mut children = Vec::with_capacity(child_count as usize);
    for index in 0..child_count {
        check_deadline(deadline)?;
        let (service, path): (String, OwnedObjectPath) = call(&proxy, "GetChildAtIndex", &index)?;
        children.push(ObjectRef { service, path });
    }
    Ok(children)
}

fn process_id(
    connection: &Connection,
    object: &ObjectRef,
    deadline: Instant,
) -> Result<i32, String> {
    check_deadline(deadline)?;
    let proxy = accessible_proxy(connection, object)?;
    call(&proxy, "GetProcessId", &())
}

fn states(
    connection: &Connection,
    object: &ObjectRef,
    deadline: Instant,
) -> Result<Vec<u32>, String> {
    check_deadline(deadline)?;
    let proxy = accessible_proxy(connection, object)?;
    let states: Vec<u32> = call(&proxy, "GetState", &())?;
    if !complete_state_bitset(&states) {
        return Err("AT-SPI returned an incomplete state bitset".into());
    }
    Ok(states)
}

fn complete_state_bitset(words: &[u32]) -> bool {
    words.len() >= 2
}

#[derive(Clone)]
struct ActiveWindow {
    object: ObjectRef,
    process_id: u32,
}

fn locate_active_window(
    connection: &Connection,
    deadline: Instant,
) -> Result<ActiveWindow, String> {
    let desktop = ObjectRef {
        service: REGISTRY.to_owned(),
        path: OwnedObjectPath::try_from(DESKTOP_PATH).map_err(|error| error.to_string())?,
    };
    let applications = children(connection, &desktop, deadline)?;
    if applications.len() > MAX_APPS {
        return Err("AT-SPI application count exceeds the desktop scan bound".into());
    }
    let mut active_window = None;
    for application in applications {
        check_deadline(deadline)?;
        let app_pid = process_id(connection, &application, deadline)?;
        if app_pid <= 0 {
            return Err("AT-SPI application has no verifiable process ID".into());
        }
        let windows = children(connection, &application, deadline)?;
        if windows.len() > MAX_WINDOWS {
            return Err("AT-SPI application window count exceeds the scan bound".into());
        }
        for window in windows {
            check_deadline(deadline)?;
            let accessible = accessible_proxy(connection, &window)?;
            let role: u32 = call(&accessible, "GetRole", &())?;
            if !is_top_level_window_role(role) {
                continue;
            }
            let current_states = states(connection, &window, deadline)?;
            if state(&current_states, STATE_ACTIVE) {
                let window_pid = process_id(connection, &window, deadline)?;
                if !matches_pid(window_pid, app_pid as u32) {
                    return Err(format!(
                        "active window PID {window_pid} does not match its application PID {app_pid}"
                    ));
                }
                record_unique(
                    &mut active_window,
                    ActiveWindow {
                        object: window,
                        process_id: app_pid as u32,
                    },
                    "active top-level window",
                )?;
            }
        }
    }
    active_window.ok_or_else(|| "AT-SPI reports no unique active top-level window".into())
}

fn is_top_level_window_role(role: u32) -> bool {
    matches!(role, ROLE_DIALOG | ROLE_FRAME | ROLE_WINDOW)
}

fn focused_snapshot(
    connection: &Connection,
    active: ActiveWindow,
    deadline: Instant,
) -> Result<Snapshot, String> {
    let window_token = object_token(&active.object);
    let mut pending = VecDeque::new();
    for child in children(connection, &active.object, deadline)? {
        pending.push_back((child, 1usize));
    }
    let mut visited = 0usize;
    let mut focused: Option<(ObjectRef, Vec<u32>, u32)> = None;
    while let Some((object, depth)) = pending.pop_front() {
        check_deadline(deadline)?;
        if visited == MAX_NODES {
            return Err("AT-SPI tree node limit reached before focus was verified".into());
        }
        visited += 1;
        let current_states = states(connection, &object, deadline)?;
        if state(&current_states, STATE_FOCUSED) {
            let actual_pid = process_id(connection, &object, deadline)?;
            if !matches_pid(actual_pid, active.process_id) {
                return Err(format!(
                    "focused accessible PID {actual_pid} does not match active application PID {}",
                    active.process_id
                ));
            }
            let accessible = accessible_proxy(connection, &object)?;
            let role = match call(&accessible, "GetRole", &()) {
                Ok(role) if role != 0 => role,
                Ok(_) => {
                    return Err("AT-SPI focused object has an invalid role".into());
                }
                Err(error) => return Err(format!("focused object GetRole: {error}")),
            };
            record_unique(
                &mut focused,
                (object.clone(), current_states, role),
                "focused descendant in the active window",
            )?;
        }
        let object_children = children(connection, &object, deadline)?;
        if depth >= MAX_DEPTH && !object_children.is_empty() {
            return Err("AT-SPI focus scan reached its maximum tree depth".into());
        }
        for child in object_children {
            if pending.len() + visited >= MAX_NODES {
                return Err("AT-SPI tree node limit prevents proving focus uniqueness".into());
            }
            pending.push_back((child, depth + 1));
        }
    }
    let (focused_object, _, role) =
        focused.ok_or_else(|| "active AT-SPI window has no focused descendant".to_owned())?;

    let focused_states = states(connection, &focused_object, deadline)?;
    if !state(&focused_states, STATE_FOCUSED)
        || state(&focused_states, STATE_DEFUNCT)
        || state(&focused_states, STATE_STALE)
    {
        return Err("focused AT-SPI object became unfocused or invalid during inspection".into());
    }
    let focused_pid = process_id(connection, &focused_object, deadline)?;
    if !matches_pid(focused_pid, active.process_id) {
        return Err("focused AT-SPI object process association changed during inspection".into());
    }
    let active_states = states(connection, &active.object, deadline)?;
    if !state(&active_states, STATE_ACTIVE)
        || state(&active_states, STATE_DEFUNCT)
        || state(&active_states, STATE_STALE)
    {
        return Err("active AT-SPI window became inactive or invalid during inspection".into());
    }

    let window = synthetic_identity(&window_token, active.process_id);
    let process_name = process_name(active.process_id);
    if process_name.is_empty() {
        log::warn!(
            "AT-SPI verified active PID {} but /proc metadata is unavailable; using generic process routing",
            active.process_id
        );
    }
    let token = object_token(&focused_object);
    Ok(Snapshot {
        context: promptify_core::context::ActiveContext {
            window,
            process_name,
            window_title: String::new(),
            url: None,
        },
        window_token,
        destination: destination(&token, &focused_states, Some(role)),
    })
}

fn check_deadline(deadline: Instant) -> Result<(), String> {
    if Instant::now() >= deadline {
        Err("AT-SPI inspection deadline expired".into())
    } else {
        Ok(())
    }
}

fn matches_pid(actual: i32, expected: u32) -> bool {
    actual > 0 && actual as u32 == expected
}

fn state(words: &[u32], state_id: u32) -> bool {
    let word = (state_id / u32::BITS) as usize;
    let bit = state_id % u32::BITS;
    words
        .get(word)
        .is_some_and(|value| value & (1u32 << bit) != 0)
}

fn object_token(object: &ObjectRef) -> String {
    format!("atspi:{}:{}", object.service, object.path.as_str())
}

fn synthetic_handle(window_token: &str) -> u64 {
    let hash = window_token
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
        });
    if hash == 0 {
        1
    } else {
        hash
    }
}

fn synthetic_identity(window_token: &str, process_id: u32) -> WindowIdentity {
    WindowIdentity {
        handle: synthetic_handle(window_token),
        process_id,
    }
}

fn record_unique<T>(slot: &mut Option<T>, value: T, description: &str) -> Result<(), String> {
    if slot.is_some() {
        return Err(format!("AT-SPI reports multiple {description}s"));
    }
    *slot = Some(value);
    Ok(())
}

fn process_name(pid: u32) -> String {
    let executable = std::fs::read_link(format!("/proc/{pid}/exe"))
        .ok()
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .filter(|name| !name.is_empty());
    executable.unwrap_or_default()
}

fn destination(token: &str, words: &[u32], role: Option<u32>) -> Destination {
    let secure = role.map(|role| role == ROLE_PASSWORD_TEXT);
    let invalid = state(words, STATE_DEFUNCT) || state(words, STATE_STALE);
    let positive_writable_states = state(words, STATE_EDITABLE)
        && state(words, STATE_ENABLED)
        && state(words, STATE_SENSITIVE);
    let writable = Some(
        positive_writable_states
            && !state(words, STATE_READ_ONLY)
            && !invalid
            && secure != Some(true),
    );
    Destination {
        token: Some(token.to_owned()),
        writable,
        secure,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_words(ids: &[u32]) -> Vec<u32> {
        let mut words = vec![0; 2];
        for id in ids {
            words[(id / u32::BITS) as usize] |= 1u32 << (id % u32::BITS);
        }
        words
    }

    #[test]
    fn state_bitsets_decode_low_and_extended_states() {
        let words = state_words(&[
            STATE_EDITABLE,
            STATE_ENABLED,
            STATE_FOCUSED,
            STATE_SENSITIVE,
            STATE_READ_ONLY,
        ]);

        assert!(state(&words, STATE_EDITABLE));
        assert!(state(&words, STATE_ENABLED));
        assert!(state(&words, STATE_FOCUSED));
        assert!(state(&words, STATE_SENSITIVE));
        assert!(state(&words, STATE_READ_ONLY));
        assert!(!state(&words, STATE_HAS_POPUP));
        let popup_only = state_words(&[STATE_HAS_POPUP]);
        assert!(state(&popup_only, STATE_HAS_POPUP));
        assert!(!state(&popup_only, STATE_READ_ONLY));
    }

    #[test]
    fn writable_requires_all_positive_states_and_known_role() {
        let eligible = state_words(&[STATE_EDITABLE, STATE_ENABLED, STATE_SENSITIVE]);
        assert_eq!(
            destination("atspi::1.2:/field/1", &eligible, Some(1)),
            Destination {
                token: Some("atspi::1.2:/field/1".into()),
                writable: Some(true),
                secure: Some(false),
            }
        );
        for missing in [STATE_EDITABLE, STATE_ENABLED, STATE_SENSITIVE] {
            let words = state_words(
                &[STATE_EDITABLE, STATE_ENABLED, STATE_SENSITIVE]
                    .into_iter()
                    .filter(|state| *state != missing)
                    .collect::<Vec<_>>(),
            );
            assert_eq!(destination("field", &words, Some(1)).writable, Some(false));
        }
        let role_unknown = destination("field", &eligible, None);
        assert_eq!(role_unknown.writable, Some(true));
        assert_eq!(role_unknown.secure, None);
    }

    #[test]
    fn password_role_and_read_only_state_never_confirm_writable() {
        let editable = state_words(&[STATE_EDITABLE, STATE_ENABLED, STATE_SENSITIVE]);
        let read_only = state_words(&[
            STATE_EDITABLE,
            STATE_ENABLED,
            STATE_SENSITIVE,
            STATE_READ_ONLY,
        ]);

        let password_role = destination("field", &editable, Some(ROLE_PASSWORD_TEXT));
        assert_eq!(password_role.writable, Some(false));
        assert_eq!(password_role.secure, Some(true));

        let read_only_destination = destination("field", &read_only, Some(1));
        assert_eq!(read_only_destination.writable, Some(false));
        assert_eq!(read_only_destination.secure, Some(false));
    }

    #[test]
    fn password_role_is_the_only_secure_evidence_and_requires_known_role() {
        assert_eq!(ROLE_PASSWORD_TEXT, 40);
        let editable = state_words(&[STATE_EDITABLE, STATE_ENABLED, STATE_SENSITIVE]);

        assert_eq!(
            destination("field", &editable, Some(ROLE_PASSWORD_TEXT)).secure,
            Some(true)
        );
        assert_eq!(destination("field", &editable, Some(1)).secure, Some(false));
        assert_eq!(destination("field", &editable, None).secure, None);
    }

    #[test]
    fn defunct_and_stale_focused_objects_are_not_writable() {
        let editable = state_words(&[STATE_EDITABLE, STATE_ENABLED, STATE_SENSITIVE]);
        for invalid_state in [STATE_DEFUNCT, STATE_STALE] {
            let words = state_words(&[
                STATE_EDITABLE,
                STATE_ENABLED,
                STATE_SENSITIVE,
                invalid_state,
            ]);
            let destination = destination("field", &words, Some(1));
            assert_eq!(destination.writable, Some(false));
            assert_eq!(destination.secure, Some(false));
        }
        assert_eq!(
            destination("field", &editable, Some(1)).writable,
            Some(true)
        );
    }

    #[test]
    fn incomplete_state_bitset_is_not_interpreted_as_safe() {
        assert!(!complete_state_bitset(&[u32::MAX]));
        assert!(complete_state_bitset(&[0, 0]));
        assert!(!state(&[], STATE_EDITABLE));
    }

    #[test]
    fn process_association_rejects_mismatched_or_invalid_pid() {
        assert!(matches_pid(314, 314));
        assert!(!matches_pid(315, 314));
        assert!(!matches_pid(-1, 314));
        assert!(!matches_pid(0, 314));
    }

    #[test]
    fn token_is_stable_for_the_same_accessible_reference() {
        let object = ObjectRef {
            service: ":1.42".into(),
            path: OwnedObjectPath::try_from("/org/a11y/atspi/accessible/7").unwrap(),
        };

        assert_eq!(object_token(&object), object_token(&object),);
        assert_eq!(
            object_token(&object),
            "atspi::1.42:/org/a11y/atspi/accessible/7"
        );
    }

    #[test]
    fn synthetic_window_identity_is_stable_and_scoped_to_active_window_reference() {
        let first = synthetic_identity("atspi::1.42:/window/7", 314);
        assert_eq!(first, synthetic_identity("atspi::1.42:/window/7", 314));
        assert_ne!(first.handle, 0);
        assert_ne!(first, synthetic_identity("atspi::1.42:/window/8", 314));
        assert_ne!(first, synthetic_identity("atspi::1.42:/window/7", 315));
    }

    #[test]
    fn active_focus_scan_rejects_ambiguous_results() {
        let mut active = Some("first");
        assert!(record_unique(&mut active, "second", "active top-level window").is_err());
        assert_eq!(active, Some("first"));

        let mut focused = None;
        record_unique(&mut focused, "only", "focused descendant").unwrap();
        assert!(record_unique(&mut focused, "another", "focused descendant").is_err());
        assert_eq!(focused, Some("only"));
    }

    #[test]
    fn only_window_frame_and_dialog_roles_are_active_window_candidates() {
        assert!(is_top_level_window_role(ROLE_DIALOG));
        assert!(is_top_level_window_role(ROLE_FRAME));
        assert!(is_top_level_window_role(ROLE_WINDOW));
        assert!(!is_top_level_window_role(ROLE_PASSWORD_TEXT));
        assert!(!is_top_level_window_role(0));
    }
}

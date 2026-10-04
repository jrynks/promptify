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
const MAX_SCOPED_REGISTRY_APPS: usize = 4096;
const MAX_WINDOWS: usize = 24;
const MAX_NODES: usize = 512;
const MAX_DEPTH: usize = 32;
const MAX_CHILDREN: usize = 256;
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
type Request = (Instant, Option<u32>, Reply, Admission);

static WORKER: OnceLock<Result<Worker, String>> = OnceLock::new();

pub fn inspect(window: &WindowIdentity) -> Result<Destination, BackendError> {
    inspect_for_session(window, crate::wayland_paste::applies())
}

fn inspect_for_session(window: &WindowIdentity, wayland: bool) -> Result<Destination, BackendError> {
    // AT-SPI identities are synthetic. A PID alone cannot distinguish two
    // windows of the same X11 process, so never weaken native focus evidence.
    if !wayland {
        return Ok(Destination::default());
    }
    if window.handle == 0 || window.process_id == 0 {
        log::warn!(
            "AT-SPI destination metadata is unknown for invalid window identity {} (PID {})",
            window.handle,
            window.process_id
        );
        return Ok(Destination::default());
    }

    let native = crate::kwin::identify();
    let scoped = match native {
        Some(Ok(context)) if context.window == *window => true,
        Some(Ok(_)) => {
            log::warn!("native active window changed before AT-SPI destination inspection");
            return Ok(Destination::default());
        }
        Some(Err(error)) => {
            log::warn!("native focus could not be verified before AT-SPI inspection: {error}");
            return Ok(Destination::default());
        }
        None => false,
    };
    let snapshot =
        match worker().and_then(|worker| worker.capture(scoped.then_some(window.process_id))) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                log::warn!("AT-SPI destination metadata is unavailable: {error}");
                return Ok(Destination::default());
            }
        };
    let still_focused = if scoped {
        match crate::kwin::identify() {
            Some(Ok(context)) => context.window == *window,
            Some(Err(error)) => {
                log::warn!("native focus recheck failed: {error}");
                false
            }
            _ => false,
        }
    } else {
        snapshot.context.window == *window
    };
    if !still_focused || snapshot.context.window.process_id != window.process_id {
        log::warn!(
            "AT-SPI focused destination no longer matches active-window identity {} (PID {})",
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
    worker()?.capture(None).map(|snapshot| snapshot.context)
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
                while let Ok((deadline, target_pid, reply, admission)) = receiver.recv() {
                    let result = inspect_accessible(deadline, target_pid);
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

    fn capture(&self, target_pid: Option<u32>) -> Result<Snapshot, BackendError> {
        let Some(admission) = Admission::acquire(&self.busy) else {
            return Err(BackendError(
                "AT-SPI destination worker is busy; no fresh focus snapshot is available".into(),
            ));
        };
        let (reply, receiver) = mpsc::sync_channel(1);
        let request = (Instant::now() + QUERY_BUDGET, target_pid, reply, admission);
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

fn inspect_accessible(deadline: Instant, target_pid: Option<u32>) -> Result<Snapshot, String> {
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
    let active = locate_active_window(&connection, deadline, target_pid)?;
    let snapshot = focused_snapshot(&connection, active, deadline)?;
    let rechecked = locate_active_window(&connection, deadline, target_pid)?;
    if object_token(&rechecked.object) != snapshot.window_token
        || rechecked.process_id != snapshot.context.window.process_id
    {
        return Err("active AT-SPI window changed during focus inspection".into());
    }
    Ok(snapshot)
}

#[derive(Clone, Debug)]
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
    // The AT-SPI registry may answer Get but return an empty body for GetAll.
    zbus::blocking::proxy::Builder::<Proxy<'static>>::new(connection)
        .destination(service.to_owned())
        .and_then(|builder| builder.path(path.to_owned()))
        .and_then(|builder| builder.interface(interface.to_owned()))
        .and_then(|builder| {
            builder
                .cache_properties(zbus::proxy::CacheProperties::No)
                .build()
        })
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
    let references: Vec<(String, OwnedObjectPath)> = call(&proxy, "GetChildren", &())?;
    check_deadline(deadline)?;
    bounded_children(references)
}

fn bounded_children(references: Vec<(String, OwnedObjectPath)>) -> Result<Vec<ObjectRef>, String> {
    if references.len() > MAX_CHILDREN {
        return Err(format!(
            "AT-SPI child count {} exceeds the per-node traversal bound",
            references.len()
        ));
    }
    Ok(references
        .into_iter()
        .map(|(service, path)| ObjectRef { service, path })
        .collect())
}

fn process_id(
    connection: &Connection,
    object: &ObjectRef,
    deadline: Instant,
) -> Result<i32, String> {
    check_deadline(deadline)?;
    if !object.service.starts_with(':') {
        return Err("AT-SPI process identity requires a unique D-Bus connection name".into());
    }
    // GTK/Qt providers need not implement GetProcessId; the bus authenticates the owner.
    let proxy = proxy(
        connection,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
    )?;
    let pid: u32 = call(&proxy, "GetConnectionUnixProcessID", &object.service)?;
    i32::try_from(pid)
        .map_err(|_| "AT-SPI connection process ID exceeds the supported range".into())
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
    target_pid: Option<u32>,
) -> Result<ActiveWindow, String> {
    let desktop = ObjectRef {
        service: REGISTRY.to_owned(),
        path: OwnedObjectPath::try_from(DESKTOP_PATH).map_err(|error| error.to_string())?,
    };
    // The registry is not an application tree. Bound its enumeration separately
    // so unrelated registrations do not consume a target's traversal allowance.
    check_deadline(deadline)?;
    let registry = accessible_proxy(connection, &desktop)?;
    let references: Vec<(String, OwnedObjectPath)> = call(&registry, "GetChildren", &())?;
    check_deadline(deadline)?;
    registry_count_allowed(references.len(), target_pid)?;
    let applications = references.into_iter().map(|(service, path)| ObjectRef { service, path });
    let mut active_window = None;
    for application in applications {
        check_deadline(deadline)?;
        let Some(app_pid) = scoped_application(process_id(connection, &application, deadline), target_pid)? else { continue };
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

fn registry_count_allowed(count: usize, target_pid: Option<u32>) -> Result<(), String> {
    let limit = if target_pid.is_some() { MAX_SCOPED_REGISTRY_APPS } else { MAX_APPS };
    if count > limit {
        Err("AT-SPI application count exceeds the desktop scan bound".into())
    } else {
        Ok(())
    }
}

fn scoped_application(pid: Result<i32, String>, target_pid: Option<u32>) -> Result<Option<i32>, String> {
    match pid {
        Ok(pid) if pid > 0 => Ok(matches_target(pid, target_pid).then_some(pid)),
        _ if target_pid.is_some() => Ok(None),
        Ok(_) => Err("AT-SPI application has no verifiable process ID".into()),
        Err(error) => Err(error),
    }
}

fn matches_target(app_pid: i32, target_pid: Option<u32>) -> bool {
    target_pid.is_none_or(|expected| matches_pid(app_pid, expected))
}

fn is_top_level_window_role(role: u32) -> bool {
    matches!(role, ROLE_DIALOG | ROLE_FRAME | ROLE_WINDOW)
}

struct FocusNode {
    states: Vec<u32>,
    role: Option<u32>,
    children: Vec<ObjectRef>,
}

fn scan_focused(
    children: Vec<ObjectRef>,
    deadline: Instant,
    mut read: impl FnMut(&ObjectRef) -> Result<FocusNode, String>,
) -> Result<(ObjectRef, Vec<u32>, u32), String> {
    let mut pending: VecDeque<_> = children
        .into_iter()
        .map(|object| (object, 1usize))
        .collect();
    let mut visited = 0;
    let mut focused = None;
    while let Some((object, depth)) = pending.pop_front() {
        check_deadline(deadline)?;
        if visited == MAX_NODES {
            return Err("AT-SPI tree node limit reached before focus was verified".into());
        }
        visited += 1;
        let node = read(&object)?;
        if state(&node.states, STATE_FOCUSED) {
            let role = node
                .role
                .filter(|role| *role != 0)
                .ok_or("AT-SPI focused object has an invalid role")?;
            record_unique(
                &mut focused,
                (object, node.states, role),
                "focused descendant in the active window",
            )?;
        }
        if depth >= MAX_DEPTH && !node.children.is_empty() {
            return Err("AT-SPI focus scan reached its maximum tree depth".into());
        }
        for child in node.children {
            if pending.len() + visited >= MAX_NODES {
                return Err("AT-SPI tree node limit prevents proving focus uniqueness".into());
            }
            pending.push_back((child, depth + 1));
        }
    }
    focused.ok_or_else(|| "active AT-SPI window has no focused descendant".to_owned())
}

fn focused_snapshot(
    connection: &Connection,
    active: ActiveWindow,
    deadline: Instant,
) -> Result<Snapshot, String> {
    let window_token = object_token(&active.object);
    let (focused_object, _, role) = scan_focused(
        children(connection, &active.object, deadline)?,
        deadline,
        |object| {
            let current_states = states(connection, object, deadline)?;
            let role = if state(&current_states, STATE_FOCUSED) {
                let actual_pid = process_id(connection, object, deadline)?;
                if !matches_pid(actual_pid, active.process_id) {
                    return Err(format!(
                    "focused accessible PID {actual_pid} does not match active application PID {}",
                    active.process_id
                ));
                }
                let accessible = accessible_proxy(connection, object)?;
                let role = match call(&accessible, "GetRole", &()) {
                    Ok(role) if role != 0 => role,
                    Ok(_) => {
                        return Err("AT-SPI focused object has an invalid role".into());
                    }
                    Err(error) => return Err(format!("focused object GetRole: {error}")),
                };
                Some(role)
            } else {
                None
            };
            Ok(FocusNode {
                states: current_states,
                role,
                children: children(connection, object, deadline)?,
            })
        },
    )?;

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

    #[test]
    fn scoped_registry_tolerates_unrelated_counts_and_pid_failures() {
        assert!(registry_count_allowed(33, Some(13362)).is_ok());
        assert!(registry_count_allowed(300, Some(13362)).is_ok());
        assert!(registry_count_allowed(33, None).is_err());
        assert!(registry_count_allowed(MAX_SCOPED_REGISTRY_APPS + 1, Some(13362)).is_err());
        assert_eq!(scoped_application(Err("exited".into()), Some(13362)).unwrap(), None);
        assert_eq!(scoped_application(Ok(0), Some(13362)).unwrap(), None);
        assert_eq!(scoped_application(Ok(9948), Some(13362)).unwrap(), None);
        assert_eq!(scoped_application(Ok(13362), Some(13362)).unwrap(), Some(13362));
        assert!(scoped_application(Err("exited".into()), None).is_err());
    }

    #[test]
    fn electron_child_batches_preserve_all_references_and_remain_bounded() {
        let references = |count| {
            (0..count)
                .map(|index| {
                    (
                        ":1.26".to_owned(),
                        OwnedObjectPath::try_from(format!("/accessible/{index}")).unwrap(),
                    )
                })
                .collect()
        };
        let children = bounded_children(references(41)).unwrap();
        assert_eq!(children.len(), 41);
        assert_eq!(children[40].path.as_str(), "/accessible/40");
        assert_eq!(
            bounded_children(references(MAX_CHILDREN)).unwrap().len(),
            MAX_CHILDREN
        );
        assert!(bounded_children(references(MAX_CHILDREN + 1)).is_err());
    }

    #[test]
    fn native_target_scope_excludes_other_applications_but_not_same_process_windows() {
        assert!(!matches_target(9948, Some(13362)));
        assert!(matches_target(13362, Some(13362)));
        assert!(!matches_target(0, Some(13362)));
        assert!(matches_target(9948, None));
    }

    #[test]
    fn large_application_trees_find_fields_without_application_names() {
        for service in [":1.26", ":1.900", ":2.42"] {
            let references = (0..41)
                .map(|index| {
                    (
                        service.to_owned(),
                        OwnedObjectPath::try_from(format!("/field/{index}")).unwrap(),
                    )
                })
                .collect();
            let roots = bounded_children(references).unwrap();
            let mut visited = 0;
            let (object, words, role) =
                scan_focused(roots, Instant::now() + QUERY_BUDGET, |object| {
                    visited += 1;
                    let focused = object.path.as_str() == "/field/40";
                    Ok(FocusNode {
                        states: state_words(if focused {
                            &[
                                STATE_FOCUSED,
                                STATE_EDITABLE,
                                STATE_ENABLED,
                                STATE_SENSITIVE,
                            ]
                        } else {
                            &[STATE_ENABLED]
                        }),
                        role: focused.then_some(1),
                        children: vec![],
                    })
                })
                .unwrap();
            assert_eq!(visited, 41);
            assert_eq!(object.service, service);
            assert!(destination(&object_token(&object), &words, Some(role)).confirmed());
        }
    }

    #[test]
    fn universal_focus_scan_rejects_ambiguity_provider_errors_and_expired_budget() {
        let roots = || {
            (0..2)
                .map(|index| ObjectRef {
                    service: ":1.900".into(),
                    path: OwnedObjectPath::try_from(format!("/field/{index}")).unwrap(),
                })
                .collect()
        };
        assert!(
            scan_focused(roots(), Instant::now() + QUERY_BUDGET, |_| Ok(FocusNode {
                states: state_words(&[STATE_FOCUSED]),
                role: Some(1),
                children: vec![],
            }))
            .unwrap_err()
            .contains("multiple")
        );
        assert!(
            scan_focused(roots(), Instant::now() + QUERY_BUDGET, |_| Err(
                "provider unavailable".into()
            ))
            .unwrap_err()
            .contains("provider unavailable")
        );
        assert!(scan_focused(
            roots(),
            Instant::now() - Duration::from_millis(1),
            |_| panic!("expired scans must not query providers")
        )
        .unwrap_err()
        .contains("deadline"));
    }

    #[test]
    fn universal_focus_scan_keeps_node_depth_and_protected_field_limits() {
        let object = || ObjectRef {
            service: ":1.900".into(),
            path: OwnedObjectPath::try_from("/field").unwrap(),
        };
        assert!(
            scan_focused(vec![object()], Instant::now() + QUERY_BUDGET, |_| Ok(
                FocusNode {
                    states: state_words(&[]),
                    role: None,
                    children: vec![object()],
                }
            ))
            .unwrap_err()
            .contains("depth")
        );
        let roots = (0..MAX_NODES + 1).map(|_| object()).collect();
        let mut queries = 0;
        assert!(scan_focused(roots, Instant::now() + QUERY_BUDGET, |_| {
            queries += 1;
            Ok(FocusNode {
                states: state_words(&[]),
                role: None,
                children: vec![],
            })
        })
        .unwrap_err()
        .contains("node limit"));
        assert_eq!(queries, MAX_NODES);
        let words = state_words(&[
            STATE_FOCUSED,
            STATE_EDITABLE,
            STATE_ENABLED,
            STATE_SENSITIVE,
        ]);
        assert!(!destination("field", &words, Some(ROLE_PASSWORD_TEXT)).confirmed());
        let readonly = state_words(&[
            STATE_FOCUSED,
            STATE_EDITABLE,
            STATE_ENABLED,
            STATE_SENSITIVE,
            STATE_READ_ONLY,
        ]);
        assert!(!destination("field", &readonly, Some(1)).confirmed());
    }

    #[test]
    #[ignore = "requires a focused accessible input in a live KDE session"]
    fn live_native_target_inspection() {
        let context = crate::kwin::identify().expect("KDE required").unwrap();
        eprintln!(
            "native target PID={} process={}",
            context.window.process_id, context.process_name
        );
        let snapshot = inspect_accessible(
            Instant::now() + QUERY_BUDGET,
            Some(context.window.process_id),
        )
        .unwrap();
        assert_eq!(
            snapshot.context.window.process_id,
            context.window.process_id
        );
        eprintln!(
            "native target PID={} destination confirmed={} writable={:?} secure={:?}",
            context.window.process_id,
            snapshot.destination.confirmed(),
            snapshot.destination.writable,
            snapshot.destination.secure
        );
        assert!(
            snapshot.destination.confirmed(),
            "focus an editable non-protected input before running"
        );
    }

    #[test]
    #[ignore = "requires a live desktop accessibility bus"]
    fn live_registry_child_count_does_not_require_bulk_properties() {
        let session = ConnectionBuilder::session()
            .unwrap()
            .method_timeout(CALL_TIMEOUT)
            .build()
            .unwrap();
        let bus = proxy(&session, "org.a11y.Bus", "/org/a11y/bus", "org.a11y.Bus").unwrap();
        let address: String = call(&bus, "GetAddress", &()).unwrap();
        let connection = ConnectionBuilder::address(address.as_str())
            .unwrap()
            .method_timeout(CALL_TIMEOUT)
            .build()
            .unwrap();
        let registry = proxy(&connection, REGISTRY, DESKTOP_PATH, ACCESSIBLE).unwrap();
        let count: i32 = registry.get_property("ChildCount").unwrap();
        assert!(count >= 0);
        let next: i32 = registry.get_property("ChildCount").unwrap();
        assert!(next >= 0);
        let desktop = ObjectRef {
            service: REGISTRY.to_owned(),
            path: OwnedObjectPath::try_from(DESKTOP_PATH).unwrap(),
        };
        let deadline = Instant::now() + QUERY_BUDGET;
        let applications = children(&connection, &desktop, deadline).unwrap();
        assert!(!applications.is_empty());
        for application in applications {
            assert!(process_id(&connection, &application, deadline).unwrap() > 0);
        }
    }

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
    #[test]
    fn x11_native_handles_are_not_compared_to_synthetic_accessible_handles() {
        let native = WindowIdentity { handle: 42, process_id: 13362 };
        let destination = inspect_for_session(&native, false).unwrap();
        assert_eq!(destination, Destination::default());
        assert!(!destination.confirmed(), "PID-only evidence cannot confirm same-process windows");
    }

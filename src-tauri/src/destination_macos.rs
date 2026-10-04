//! Metadata-only macOS destination inspection.
//!
//! Integration: declare this module on macOS and call `inspect` from the
//! destination provider. This file deliberately does not wire that provider.
//! Permission readiness uses `trusted`; the explicit Grant desktop integration
//! action may call `request_permission`. A native prompt is asynchronous, not
//! immediate consent. Neither function reads destination metadata.
//! No AX value, selected text, title, description, or document content is read,
//! and no accessibility value or focus is written.
//!
//! Tokens are process-local, scoped to the supplied window identity, and backed
//! by retained AX objects compared using CFEqual, not pointers or CFHash. The
//! 128-entry LRU can forget an old token; its ID is never reused, so eviction
//! fails equality closed. Public AX APIs do not reliably map an AX window to a
//! CGWindowID: the caller must still revalidate `WindowIdentity` before delivery.
//! AX metadata is application-provided evidence, not an authorization to insert.

use std::collections::VecDeque;
use std::sync::Arc;
#[cfg(any(target_os = "macos", test))]
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender, TrySendError};
use std::time::{Duration, Instant};

use promptify_core::context::WindowIdentity;
use promptify_core::delivery::Destination;
use promptify_core::pipeline::BackendError;

const WAIT_LIMIT: Duration = Duration::from_millis(400);
const QUERY_BUDGET: Duration = Duration::from_millis(350);
const RETAIN_LIMIT: usize = 128;

fn error(message: impl std::fmt::Display) -> BackendError {
    BackendError(format!("macOS destination inspection: {message}"))
}

/// Checks current Accessibility trust without showing a permission prompt.
#[cfg(target_os = "macos")]
pub fn trusted() -> bool {
    native::trusted()
}

/// Requests the native Accessibility prompt. Success means currently trusted,
/// not merely that the asynchronous permission prompt was requested.
#[cfg(target_os = "macos")]
pub fn request_permission() -> Result<(), BackendError> {
    native::request_permission()
}

fn permission_request_result(granted: bool) -> Result<(), BackendError> {
    if granted {
        Ok(())
    } else {
        Err(error(
            "Accessibility permission is not yet granted. The native permission prompt is asynchronous and does not grant consent immediately. Enable Promptify in System Settings > Privacy & Security > Accessibility, then retry Grant desktop integration.",
        ))
    }
}

fn validate_window(window: &WindowIdentity) -> Result<(), BackendError> {
    if window.handle == 0 || window.process_id == 0 || window.process_id > i32::MAX as u32 {
        return Err(error("invalid window identity or process ID"));
    }
    Ok(())
}

fn check_owner(actual: i32, expected: u32) -> Result<(), BackendError> {
    if actual <= 0 || actual as u32 != expected {
        Err(error("focused AX object belongs to a different process"))
    } else {
        Ok(())
    }
}

/// Errors (including denied permission, timeout, and a busy worker) must be
/// surfaced by the caller. Missing/unsupported metadata is logged and remains
/// unknown; it never authorizes delivery.
pub fn inspect(window: &WindowIdentity) -> Result<Destination, BackendError> {
    validate_window(window)?;
    #[cfg(target_os = "macos")]
    {
        inspect_ax(*window)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(error("AX metadata inspection is available only on macOS"))
    }
}

#[cfg(any(target_os = "macos", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn inspect_ax(window: WindowIdentity) -> Result<Destination, BackendError> {
    static WORKER: OnceLock<Result<Worker, BackendError>> = OnceLock::new();
    let worker = WORKER.get_or_init(|| {
        Worker::start(|| {
            let mut tokens = Tokens::new(RETAIN_LIMIT);
            move |window, deadline| native::inspect(window, deadline, &mut tokens)
        })
    });
    match worker {
        Ok(worker) => worker.inspect(window, WAIT_LIMIT),
        Err(failure) => Err(failure.clone()),
    }
}

struct Admission(Arc<AtomicBool>);

impl Admission {
    fn acquire(busy: &Arc<AtomicBool>) -> Result<Self, BackendError> {
        busy.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| error("worker busy; a previous AX inspection has not finished"))?;
        Ok(Self(Arc::clone(busy)))
    }
}

impl Drop for Admission {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

struct Request {
    window: WindowIdentity,
    deadline: Instant,
    reply: SyncSender<Result<Destination, BackendError>>,
    admission: Admission,
}

struct Worker {
    requests: SyncSender<Request>,
    busy: Arc<AtomicBool>,
}

impl Worker {
    fn start<F, H>(factory: F) -> Result<Self, BackendError>
    where
        F: FnOnce() -> H + Send + 'static,
        H: FnMut(WindowIdentity, Instant) -> Result<Destination, BackendError> + 'static,
    {
        let (requests, receiver) = mpsc::sync_channel::<Request>(1);
        // Construct and keep all CF/AX objects on this thread: no unsafe Send.
        std::thread::Builder::new()
            .name("destination-macos".into())
            .spawn(move || {
                let mut inspect = factory();
                while let Ok(request) = receiver.recv() {
                    let result = check_deadline(request.deadline)
                        .and_then(|()| inspect(request.window, request.deadline))
                        .and_then(|destination| {
                            check_deadline(request.deadline)?;
                            Ok(destination)
                        });
                    // Release admission before replying, so a completed request
                    // cannot spuriously reject the caller's next inspection.
                    drop(request.admission);
                    // A timed-out caller drops its receiver; there is no result
                    // to deliver, but the worker must remain usable.
                    if request.reply.send(result).is_err() {
                        log::debug!("macOS destination inspection: caller stopped waiting; AX result discarded");
                    }
                }
            })
            .map_err(|failure| error(format!("could not start AX worker: {failure}")))?;
        Ok(Self {
            requests,
            busy: Arc::new(AtomicBool::new(false)),
        })
    }

    fn inspect(&self, window: WindowIdentity, wait: Duration) -> Result<Destination, BackendError> {
        let admission = Admission::acquire(&self.busy)?;
        let (reply, receiver) = mpsc::sync_channel(1);
        let request = Request {
            window,
            deadline: Instant::now() + QUERY_BUDGET.min(wait),
            reply,
            admission,
        };
        self.requests
            .try_send(request)
            .map_err(|failure| match failure {
                TrySendError::Full(_) => error("AX worker queue is full"),
                TrySendError::Disconnected(_) => error("AX worker stopped"),
            })?;
        receiver
            .recv_timeout(wait)
            .map_err(|failure| match failure {
                RecvTimeoutError::Timeout => {
                    error("AX inspection timed out; destination is not confirmed")
                }
                RecvTimeoutError::Disconnected => error("AX worker stopped without a result"),
            })?
    }
}

fn check_deadline(deadline: Instant) -> Result<(), BackendError> {
    if Instant::now() >= deadline {
        Err(error(
            "AX query budget exhausted; destination is not confirmed",
        ))
    } else {
        Ok(())
    }
}

struct Entry<T> {
    window: WindowIdentity,
    object: T,
    id: u64,
}

struct Tokens<T> {
    entries: VecDeque<Entry<T>>,
    next: u64,
    capacity: usize,
}

impl<T> Tokens<T> {
    fn new(capacity: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            next: 1,
            capacity,
        }
    }

    fn intern(
        &mut self,
        window: WindowIdentity,
        object: T,
        equal: impl Fn(&T, &T) -> bool,
    ) -> Result<String, BackendError> {
        let id = if let Some(index) = self
            .entries
            .iter()
            .position(|entry| entry.window == window && equal(&entry.object, &object))
        {
            let entry = self
                .entries
                .remove(index)
                .ok_or_else(|| error("AX token cache inconsistency"))?;
            let id = entry.id;
            self.entries.push_back(entry);
            id
        } else {
            if self.capacity == 0 {
                return Err(error("AX token cache has no capacity"));
            }
            let id = self.next;
            self.next = self
                .next
                .checked_add(1)
                .ok_or_else(|| error("AX token IDs exhausted"))?;
            if self.entries.len() == self.capacity {
                self.entries.pop_front();
            }
            self.entries.push_back(Entry { window, object, id });
            id
        };
        Ok(format!(
            "macos-ax:{}:{}:{id}",
            window.process_id, window.handle
        ))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    TextField,
    TextArea,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Subrole {
    Secure,
    Search,
    Unknown,
    Other,
}

fn security(role: Option<Role>, subrole: Option<Subrole>) -> Option<bool> {
    match (role, subrole) {
        (_, Some(Subrole::Secure)) => Some(true),
        (Some(Role::TextField), Some(Subrole::Search | Subrole::Unknown))
        | (Some(Role::TextArea), Some(Subrole::Unknown)) => Some(false),
        _ => None,
    }
}

fn writable(role: Option<Role>, enabled: Option<bool>, settable: Option<bool>) -> Option<bool> {
    if enabled == Some(false) || settable == Some(false) {
        Some(false)
    } else if matches!(role, Some(Role::TextField | Role::TextArea))
        && enabled == Some(true)
        && settable == Some(true)
    {
        Some(true)
    } else {
        None
    }
}

fn optional_ax_status(operation: &str, status: i32) -> Result<bool, BackendError> {
    match status {
        0 => Ok(true),
        -25205 | -25208 | -25212 => {
            log::warn!(
                "macOS destination inspection: {operation} unavailable (AX error {status}); metadata unknown"
            );
            Ok(false)
        }
        -25211 => Err(error(format!(
            "{operation}: Accessibility permission denied or API disabled (AX error {status})"
        ))),
        _ => Err(error(format!("{operation} failed (AX error {status})"))),
    }
}

// Compile the FFI implementation in host tests too, but never invoke it there.
// This catches Rust type/ownership errors without claiming AX device coverage.
#[cfg(any(target_os = "macos", test))]
#[cfg_attr(all(test, not(target_os = "macos")), allow(dead_code))]
mod native {
    use std::ffi::{c_int, c_void};
    use std::ptr::{self, NonNull};

    use super::*;

    type CfRef = *const c_void;
    type AxRef = CfRef;
    // CoreFoundation Boolean is an unsigned byte, not Rust's bool.
    type Boolean = u8;

    #[repr(C)]
    struct CfDictionary {
        _private: [u8; 0],
    }

    #[repr(C)]
    struct CfString {
        _private: [u8; 0],
    }

    #[repr(C)]
    struct CfBoolean {
        _private: [u8; 0],
    }

    type DictionaryRef = *const CfDictionary;
    type StringRef = *const CfString;
    type BooleanRef = *const CfBoolean;
    type RetainCallback = Option<unsafe extern "C" fn(CfRef, CfRef) -> CfRef>;
    type ReleaseCallback = Option<unsafe extern "C" fn(CfRef, CfRef)>;
    type DescriptionCallback = Option<unsafe extern "C" fn(CfRef) -> StringRef>;
    type EqualCallback = Option<unsafe extern "C" fn(CfRef, CfRef) -> Boolean>;

    #[repr(C)]
    struct DictionaryKeyCallbacks {
        version: isize,
        retain: RetainCallback,
        release: ReleaseCallback,
        copy_description: DescriptionCallback,
        equal: EqualCallback,
        hash: Option<unsafe extern "C" fn(CfRef) -> usize>,
    }

    #[repr(C)]
    struct DictionaryValueCallbacks {
        version: isize,
        retain: RetainCallback,
        release: ReleaseCallback,
        copy_description: DescriptionCallback,
        equal: EqualCallback,
    }

    #[cfg_attr(
        target_os = "macos",
        link(name = "ApplicationServices", kind = "framework")
    )]
    unsafe extern "C" {
        fn AXIsProcessTrusted() -> Boolean;
        fn AXIsProcessTrustedWithOptions(options: DictionaryRef) -> Boolean;
        static kAXTrustedCheckOptionPrompt: StringRef;
        fn AXUIElementCreateApplication(pid: c_int) -> AxRef;
        fn AXUIElementGetTypeID() -> usize;
        fn AXUIElementGetPid(element: AxRef, pid: *mut c_int) -> c_int;
        fn AXUIElementSetMessagingTimeout(element: AxRef, seconds: f32) -> c_int;
        fn AXUIElementCopyAttributeValue(
            element: AxRef,
            attribute: CfRef,
            value: *mut CfRef,
        ) -> c_int;
        fn AXUIElementIsAttributeSettable(
            element: AxRef,
            attribute: CfRef,
            settable: *mut Boolean,
        ) -> c_int;
    }

    #[cfg_attr(target_os = "macos", link(name = "CoreFoundation", kind = "framework"))]
    unsafe extern "C" {
        static kCFBooleanTrue: BooleanRef;
        static kCFTypeDictionaryKeyCallBacks: DictionaryKeyCallbacks;
        static kCFTypeDictionaryValueCallBacks: DictionaryValueCallbacks;
        fn CFDictionaryCreate(
            allocator: CfRef,
            keys: *const CfRef,
            values: *const CfRef,
            count: isize,
            key_callbacks: *const DictionaryKeyCallbacks,
            value_callbacks: *const DictionaryValueCallbacks,
        ) -> DictionaryRef;
        fn CFRelease(value: CfRef);
        fn CFEqual(left: CfRef, right: CfRef) -> Boolean;
        fn CFGetTypeID(value: CfRef) -> usize;
        fn CFStringGetTypeID() -> usize;
        fn CFBooleanGetTypeID() -> usize;
        fn CFBooleanGetValue(value: CfRef) -> Boolean;
        fn CFStringCreateWithBytes(
            allocator: CfRef,
            bytes: *const u8,
            length: isize,
            encoding: u32,
            external: Boolean,
        ) -> CfRef;
    }

    struct Owned(NonNull<c_void>);

    impl Owned {
        fn take(raw: CfRef) -> Result<Self, BackendError> {
            NonNull::new(raw.cast_mut())
                .map(Self)
                .ok_or_else(|| error("AX/CF creation or copy returned a null object"))
        }

        fn raw(&self) -> CfRef {
            self.0.as_ptr().cast_const()
        }

        fn string(literal: &'static str) -> Result<Self, BackendError> {
            // All strings constructed here are short static metadata names.
            unsafe {
                Self::take(CFStringCreateWithBytes(
                    ptr::null(),
                    literal.as_ptr(),
                    literal.len() as isize,
                    0x0800_0100,
                    0,
                ))
            }
        }

        fn is_type(&self, type_id: usize) -> bool {
            unsafe { CFGetTypeID(self.raw()) == type_id }
        }

        fn equal(&self, other: &Self) -> bool {
            unsafe { CFEqual(self.raw(), other.raw()) != 0 }
        }

        fn matches(&self, literal: &'static str) -> Result<bool, BackendError> {
            Ok(self.equal(&Self::string(literal)?))
        }
    }

    impl Drop for Owned {
        fn drop(&mut self) {
            unsafe { CFRelease(self.raw()) }
        }
    }

    pub(super) fn trusted() -> bool {
        unsafe { AXIsProcessTrusted() != 0 }
    }

    pub(super) fn request_permission() -> Result<(), BackendError> {
        unsafe {
            let key = kAXTrustedCheckOptionPrompt;
            let value = kCFBooleanTrue;
            if key.is_null() || value.is_null() {
                return Err(error(
                    "native Accessibility permission option constants are unavailable",
                ));
            }
            let keys = [key.cast::<c_void>()];
            let values = [value.cast::<c_void>()];
            let raw = CFDictionaryCreate(
                ptr::null(),
                keys.as_ptr(),
                values.as_ptr(),
                1,
                &kCFTypeDictionaryKeyCallBacks,
                &kCFTypeDictionaryValueCallBacks,
            );
            // Create rule: retain the dictionary for the entire AX call and
            // release it on every return path. CFType callbacks retain entries.
            let options = Owned::take(raw.cast())?;
            let granted = AXIsProcessTrustedWithOptions(options.raw().cast()) != 0;
            permission_request_result(granted)
        }
    }

    pub(super) struct Element(Owned);

    impl Element {
        fn take(object: Owned) -> Result<Self, BackendError> {
            if !object.is_type(unsafe { AXUIElementGetTypeID() }) {
                return Err(error(
                    "focused application/control metadata is not an AXUIElement",
                ));
            }
            let element = Self(object);
            // Per-object only: using a system-wide element would change the
            // global AX timeout for other integrations in this process.
            let status = unsafe { AXUIElementSetMessagingTimeout(element.raw(), 0.04) };
            required("setting 40ms per-object AX timeout", status)?;
            Ok(element)
        }

        fn raw(&self) -> AxRef {
            self.0.raw()
        }

        fn owner(&self, expected: u32, deadline: Instant) -> Result<(), BackendError> {
            check_deadline(deadline)?;
            let mut pid = 0;
            required("reading AX process ownership", unsafe {
                AXUIElementGetPid(self.raw(), &mut pid)
            })?;
            check_owner(pid, expected)
        }

        fn copy(
            &self,
            attribute: &'static str,
            deadline: Instant,
        ) -> Result<Option<Owned>, BackendError> {
            check_deadline(deadline)?;
            let name = Owned::string(attribute)?;
            let mut raw = ptr::null();
            let status = unsafe { AXUIElementCopyAttributeValue(self.raw(), name.raw(), &mut raw) };
            // Even a malformed API response must not leak a returned +1 object.
            let value = if raw.is_null() {
                None
            } else {
                Some(Owned::take(raw)?)
            };
            if !optional_ax_status(attribute, status)? {
                return Ok(None);
            }
            value
                .map(Some)
                .ok_or_else(|| error(format!("{attribute} succeeded with no object")))
        }

        fn boolean(
            &self,
            name: &'static str,
            deadline: Instant,
        ) -> Result<Option<bool>, BackendError> {
            let Some(value) = self.copy(name, deadline)? else {
                return Ok(None);
            };
            if !value.is_type(unsafe { CFBooleanGetTypeID() }) {
                log::warn!(
                    "macOS destination inspection: {name} is not CFBoolean; metadata unknown"
                );
                return Ok(None);
            }
            Ok(Some(unsafe { CFBooleanGetValue(value.raw()) != 0 }))
        }

        fn focused(&self, deadline: Instant) -> Result<Option<Self>, BackendError> {
            self.copy("AXFocusedUIElement", deadline)?
                .map(Self::take)
                .transpose()
        }

        fn value_settable(&self, deadline: Instant) -> Result<Option<bool>, BackendError> {
            check_deadline(deadline)?;
            let attribute = Owned::string("AXValue")?;
            let mut settable = 0;
            let status = unsafe {
                AXUIElementIsAttributeSettable(self.raw(), attribute.raw(), &mut settable)
            };
            // This is only a writability query. AXValue is never copied.
            Ok(optional_ax_status("AXValue writability", status)?.then_some(settable != 0))
        }

        fn role(&self, deadline: Instant) -> Result<Option<Role>, BackendError> {
            let Some(value) = self.copy("AXRole", deadline)? else {
                return Ok(None);
            };
            if !value.is_type(unsafe { CFStringGetTypeID() }) {
                log::warn!(
                    "macOS destination inspection: AXRole is not CFString; metadata unknown"
                );
                return Ok(None);
            }
            Ok(Some(if value.matches("AXTextField")? {
                Role::TextField
            } else if value.matches("AXTextArea")? {
                Role::TextArea
            } else {
                Role::Other
            }))
        }

        fn subrole(&self, deadline: Instant) -> Result<Option<Subrole>, BackendError> {
            let Some(value) = self.copy("AXSubrole", deadline)? else {
                return Ok(None);
            };
            if !value.is_type(unsafe { CFStringGetTypeID() }) {
                log::warn!(
                    "macOS destination inspection: AXSubrole is not CFString; metadata unknown"
                );
                return Ok(None);
            }
            Ok(Some(if value.matches("AXSecureTextField")? {
                Subrole::Secure
            } else if value.matches("AXSearchField")? {
                Subrole::Search
            } else if value.matches("AXUnknown")? {
                Subrole::Unknown
            } else {
                Subrole::Other
            }))
        }
    }

    fn required(operation: &str, status: i32) -> Result<(), BackendError> {
        if optional_ax_status(operation, status)? {
            Ok(())
        } else {
            Err(error(format!(
                "{operation} is required but unavailable (AX error {status})"
            )))
        }
    }

    fn frontmost(app: &Element, deadline: Instant) -> Result<(), BackendError> {
        match app.boolean("AXFrontmost", deadline)? {
            Some(true) => Ok(()),
            Some(false) => Err(error("target application is no longer frontmost")),
            None => Err(error(
                "cannot establish that the target application is frontmost",
            )),
        }
    }

    pub(super) fn inspect(
        window: WindowIdentity,
        deadline: Instant,
        tokens: &mut Tokens<Element>,
    ) -> Result<Destination, BackendError> {
        check_deadline(deadline)?;
        if !trusted() {
            return Err(error(
                "Accessibility permission denied; enable Promptify in System Settings > Privacy & Security > Accessibility",
            ));
        }
        let app = Element::take(Owned::take(unsafe {
            AXUIElementCreateApplication(window.process_id as c_int)
        })?)?;
        app.owner(window.process_id, deadline)?;
        frontmost(&app, deadline)?;
        let Some(element) = app.focused(deadline)? else {
            log::warn!("macOS destination inspection: no focused AX control; destination unknown");
            return Ok(Destination::default());
        };
        element.owner(window.process_id, deadline)?;
        let role = element.role(deadline)?;
        let subrole = element.subrole(deadline)?;
        let enabled = element.boolean("AXEnabled", deadline)?;
        let settable = element.value_settable(deadline)?;
        let secure = security(role, subrole);
        let writable = writable(role, enabled, settable);
        if secure.is_none() {
            log::warn!(
                "macOS destination inspection: safe non-password text role/subrole could not be established; security unknown"
            );
        }
        if writable.is_none() {
            log::warn!(
                "macOS destination inspection: enabled, writable text control could not be established; writability unknown"
            );
        }
        // Reject a focus race, including another control in the same process.
        frontmost(&app, deadline)?;
        let current = app
            .focused(deadline)?
            .ok_or_else(|| error("focused AX control disappeared during inspection"))?;
        current.owner(window.process_id, deadline)?;
        if !element.0.equal(&current.0) {
            return Err(error("focused AX control changed during inspection"));
        }
        if !trusted() {
            return Err(error(
                "Accessibility permission was revoked during inspection",
            ));
        }
        check_deadline(deadline)?;
        let token = tokens.intern(window, element, |left, right| left.0.equal(&right.0))?;
        Ok(Destination {
            token: Some(token),
            writable,
            secure,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_prompt_without_trust_is_not_success() {
        let failure = permission_request_result(false).unwrap_err();
        assert!(failure.0.contains("not yet granted"));
        assert!(failure.0.contains("asynchronous"));
        assert!(
            failure
                .0
                .contains("System Settings > Privacy & Security > Accessibility")
        );
        assert!(failure.0.contains("retry Grant desktop integration"));
        assert!(permission_request_result(true).is_ok());
    }

    fn window() -> WindowIdentity {
        WindowIdentity {
            handle: 42,
            process_id: 123,
        }
    }

    #[test]
    fn invalid_identity_is_an_error() {
        for identity in [
            WindowIdentity {
                handle: 0,
                ..window()
            },
            WindowIdentity {
                process_id: 0,
                ..window()
            },
            WindowIdentity {
                process_id: u32::MAX,
                ..window()
            },
        ] {
            assert!(validate_window(&identity).is_err());
        }
        assert!(validate_window(&window()).is_ok());
    }

    #[test]
    fn process_ownership_must_match_positive_target_pid() {
        assert!(check_owner(123, window().process_id).is_ok());
        for actual in [-1, 0, 124] {
            assert!(check_owner(actual, window().process_id).is_err());
        }
    }

    #[test]
    fn shared_destination_contract_fails_closed() {
        let mut tokens = Tokens::new(RETAIN_LIMIT);
        let token = tokens.intern(window(), 1, |a, b| a == b).unwrap();
        let confirmed = Destination {
            token: Some(token),
            writable: writable(Some(Role::TextField), Some(true), Some(true)),
            secure: security(Some(Role::TextField), Some(Subrole::Unknown)),
        };
        assert!(confirmed.confirmed());
        assert!(confirmed.unchanged(&confirmed));
        let unknown = Destination {
            secure: security(Some(Role::TextField), None),
            ..confirmed.clone()
        };
        assert!(!unknown.confirmed());
        assert!(!confirmed.unchanged(&unknown));
        let password = Destination {
            secure: security(Some(Role::TextField), Some(Subrole::Secure)),
            ..confirmed.clone()
        };
        assert!(password.protected());
        assert!(!confirmed.unchanged(&password));
    }

    #[test]
    fn eviction_releases_owned_objects_and_hits_release_the_new_alias() {
        struct Object(Arc<std::sync::atomic::AtomicUsize>);
        impl Drop for Object {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let drops = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut tokens = Tokens::new(1);
        tokens
            .intern(window(), Object(Arc::clone(&drops)), |_, _| false)
            .unwrap();
        tokens
            .intern(window(), Object(Arc::clone(&drops)), |_, _| true)
            .unwrap();
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        tokens
            .intern(window(), Object(Arc::clone(&drops)), |_, _| false)
            .unwrap();
        assert_eq!(drops.load(Ordering::SeqCst), 2);
        drop(tokens);
        assert_eq!(drops.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn password_evidence_wins_over_missing_or_unexpected_role() {
        for role in [
            None,
            Some(Role::Other),
            Some(Role::TextField),
            Some(Role::TextArea),
        ] {
            assert_eq!(security(role, Some(Subrole::Secure)), Some(true));
        }
    }

    #[test]
    fn only_known_text_role_and_explicit_nonsecure_subrole_are_safe() {
        assert_eq!(
            security(Some(Role::TextField), Some(Subrole::Search)),
            Some(false)
        );
        assert_eq!(
            security(Some(Role::TextArea), Some(Subrole::Unknown)),
            Some(false)
        );
        for role in [
            None,
            Some(Role::Other),
            Some(Role::TextField),
            Some(Role::TextArea),
        ] {
            assert_eq!(security(role, None), None);
            assert_eq!(security(role, Some(Subrole::Other)), None);
        }
        assert_eq!(security(Some(Role::Other), Some(Subrole::Unknown)), None);
        assert_eq!(security(Some(Role::TextArea), Some(Subrole::Search)), None);
    }

    #[test]
    fn writable_requires_positive_enabled_settable_text_evidence() {
        for role in [
            None,
            Some(Role::Other),
            Some(Role::TextField),
            Some(Role::TextArea),
        ] {
            for enabled in [None, Some(false), Some(true)] {
                for settable in [None, Some(false), Some(true)] {
                    let expected = if enabled == Some(false) || settable == Some(false) {
                        Some(false)
                    } else if matches!(role, Some(Role::TextField | Role::TextArea))
                        && enabled == Some(true)
                        && settable == Some(true)
                    {
                        Some(true)
                    } else {
                        None
                    };
                    assert_eq!(writable(role, enabled, settable), expected);
                }
            }
        }
    }

    #[test]
    fn unsupported_is_unknown_but_permission_and_transport_failures_are_errors() {
        assert_eq!(optional_ax_status("test", 0).unwrap(), true);
        for status in [-25205, -25208, -25212] {
            assert_eq!(optional_ax_status("test", status).unwrap(), false);
        }
        for status in [-25211, -25204, -25202, -25201, -25200, 123] {
            assert!(optional_ax_status("test", status).is_err());
        }
        assert!(
            optional_ax_status("test", -25211)
                .unwrap_err()
                .0
                .contains("permission denied")
        );
    }

    #[test]
    fn exact_identity_not_hash_or_address_controls_token_equality() {
        #[derive(Clone)]
        struct Object {
            identity: u64,
            hash: u64,
            address: u64,
        }
        let a = Object {
            identity: 1,
            hash: 99,
            address: 10,
        };
        let alias = Object {
            address: 20,
            ..a.clone()
        };
        let collision = Object {
            identity: 2,
            ..a.clone()
        };
        assert_eq!(a.hash, collision.hash);
        assert_eq!(a.address, collision.address);
        let mut tokens = Tokens::new(RETAIN_LIMIT);
        let same = |a: &Object, b: &Object| a.identity == b.identity;
        let first = tokens.intern(window(), a, same).unwrap();
        assert_eq!(first, tokens.intern(window(), alias.clone(), same).unwrap());
        assert_ne!(first, tokens.intern(window(), collision, same).unwrap());
        assert_ne!(
            first,
            tokens
                .intern(
                    WindowIdentity {
                        process_id: 124,
                        ..window()
                    },
                    alias.clone(),
                    same
                )
                .unwrap()
        );
        assert_ne!(
            first,
            tokens
                .intern(
                    WindowIdentity {
                        handle: 43,
                        ..window()
                    },
                    alias,
                    same
                )
                .unwrap()
        );
        assert!(first.starts_with("macos-ax:123:42:"));
    }

    #[test]
    fn retention_is_bounded_lru_and_evicted_ids_are_never_reused() {
        let mut tokens = Tokens::new(2);
        let a = tokens.intern(window(), 1, |a, b| a == b).unwrap();
        let b = tokens.intern(window(), 2, |a, b| a == b).unwrap();
        assert_eq!(a, tokens.intern(window(), 1, |a, b| a == b).unwrap());
        tokens.intern(window(), 3, |a, b| a == b).unwrap();
        assert_eq!(tokens.entries.len(), 2);
        assert_ne!(b, tokens.intern(window(), 2, |a, b| a == b).unwrap());
        assert_eq!(tokens.entries.len(), 2);
        let mut exhausted = Tokens::new(1);
        exhausted.next = u64::MAX;
        assert!(exhausted.intern(window(), 1, |a, b| a == b).is_err());
        assert!(exhausted.entries.is_empty());
    }

    #[test]
    fn deadline_is_checked() {
        assert!(check_deadline(Instant::now()).is_err());
        assert!(check_deadline(Instant::now() + WAIT_LIMIT).is_ok());
    }

    #[test]
    fn worker_propagates_errors_and_can_be_reused() {
        let worker = Worker::start(|| |_, _| Err(error("test failure"))).unwrap();
        for _ in 0..2 {
            assert!(
                worker
                    .inspect(window(), WAIT_LIMIT)
                    .unwrap_err()
                    .0
                    .contains("test failure")
            );
        }
    }

    #[test]
    fn timeout_does_not_spawn_or_queue_more_inspections() {
        let (entered_tx, entered_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let worker = Arc::new(
            Worker::start(move || {
                move |_, _| {
                    entered_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Ok(Destination::default())
                }
            })
            .unwrap(),
        );
        let caller = Arc::clone(&worker);
        let result = std::thread::spawn(move || {
            let started = Instant::now();
            let result = caller.inspect(window(), WAIT_LIMIT);
            (result, started.elapsed())
        });
        entered_rx.recv_timeout(WAIT_LIMIT).unwrap();
        assert!(
            worker
                .inspect(window(), WAIT_LIMIT)
                .unwrap_err()
                .0
                .contains("busy")
        );
        let (result, elapsed) = result.join().unwrap();
        assert!(result.unwrap_err().0.contains("timed out"));
        assert!(elapsed >= WAIT_LIMIT);
        assert!(elapsed < WAIT_LIMIT + Duration::from_secs(1), "{elapsed:?}");
        assert!(
            worker
                .inspect(window(), WAIT_LIMIT)
                .unwrap_err()
                .0
                .contains("busy")
        );
        release_tx.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while worker.busy.load(Ordering::Acquire) {
            assert!(
                Instant::now() < deadline,
                "worker did not release admission"
            );
            std::thread::yield_now();
        }
    }

    #[test]
    fn admission_is_released_on_drop() {
        let busy = Arc::new(AtomicBool::new(false));
        let admission = Admission::acquire(&busy).unwrap();
        assert!(Admission::acquire(&busy).is_err());
        drop(admission);
        assert!(Admission::acquire(&busy).is_ok());
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn non_macos_inspection_is_explicitly_unsupported() {
        assert!(inspect(&window()).unwrap_err().0.contains("only on macOS"));
    }
}

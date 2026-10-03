//! Queries KWin for a fresh focus snapshot instead of trusting asynchronously cached activations.

use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use promptify_core::context::{ActiveContext, WindowIdentity};
use promptify_core::pipeline::BackendError;
use serde::Deserialize;
use zbus::blocking::Connection;

const KWIN: &str = "org.kde.KWin";
const OBJECT_PATH: &str = "/dev/promptify/Focus";
const SCRIPT: &str = include_str!("kwin_focus.js");
const REPORT_WAIT: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct Report {
    id: String,
    pid: u32,
    class: String,
    caption: String,
}

#[derive(Default)]
struct Pending {
    owner: String,
    request: String,
    reply: Option<Result<Report, String>>,
}

#[derive(Clone, Default)]
struct Shared(Arc<(Mutex<Pending>, Condvar)>);

struct FocusSink(Shared);

#[zbus::interface(name = "dev.promptify.Focus")]
impl FocusSink {
    fn snapshot(&self, #[zbus(header)] header: zbus::message::Header<'_>, request: String, payload: String) {
        let (lock, changed) = &*self.0.0;
        let mut pending = lock.lock().unwrap();
        if !header.sender().is_some_and(|sender| sender.as_str() == pending.owner) || request != pending.request {
            log::warn!("ignored an unsolicited KWin focus snapshot");
            return;
        }
        pending.reply = Some(parse(&payload));
        changed.notify_one();
    }
}

struct Tracker {
    connection: Connection,
    shared: Shared,
    next_request: u64,
}

static TRACKER: Mutex<Option<Tracker>> = Mutex::new(None);

pub fn applies() -> bool {
    crate::wayland_paste::applies()
        && std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|desktop| is_kde(&desktop))
}

fn is_kde(desktop: &str) -> bool {
    desktop.split(':').any(|part| part.eq_ignore_ascii_case("KDE"))
}

fn parse(payload: &str) -> Result<Report, String> {
    let report: Report = serde_json::from_str(payload).map_err(|e| format!("invalid KWin focus snapshot: {e}"))?;
    if report.id.is_empty() {
        return Err("No window is focused. Click into the app you want to paste into and try again.".into());
    }
    if report.pid == 0 || report.class.trim().is_empty() {
        return Err("KWin could not identify the focused application. Focus an application window and try again.".into());
    }
    Ok(report)
}

fn handle(id: &str) -> u64 {
    id.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3))
}

fn process_name(pid: u32, class: &str) -> String {
    std::fs::read_link(format!("/proc/{pid}/exe"))
        .ok()
        .and_then(|path| path.file_name().map(|name| name.to_string_lossy().trim_end_matches(" (deleted)").to_owned()))
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| class.to_owned())
}

fn context(report: Report) -> ActiveContext {
    ActiveContext {
        window: WindowIdentity { handle: handle(&report.id), process_id: report.pid },
        process_name: process_name(report.pid, &report.class),
        window_title: report.caption,
        url: None,
    }
}

fn error(message: impl std::fmt::Display) -> BackendError {
    BackendError(format!("KDE window tracking unavailable: {message}"))
}

fn kwin_owner(connection: &Connection) -> Result<String, BackendError> {
    let dbus = zbus::blocking::fdo::DBusProxy::new(connection).map_err(error)?;
    let name = zbus::names::BusName::try_from(KWIN).map_err(error)?;
    dbus.get_name_owner(name).map(|owner| owner.to_string()).map_err(error)
}

fn call<B>(connection: &Connection, owner: &str, path: &str, interface: &str, method: &str, body: &B) -> Result<zbus::message::Message, BackendError>
where
    B: serde::Serialize + zbus::zvariant::DynamicType,
{
    connection.call_method(Some(owner), path, Some(interface), method, body).map_err(error)
}

impl Tracker {
    fn start() -> Result<Self, BackendError> {
        let shared = Shared::default();
        let connection = zbus::blocking::connection::Builder::session()
            .map_err(error)?
            .serve_at(OBJECT_PATH, FocusSink(shared.clone()))
            .map_err(error)?
            .build()
            .map_err(error)?;
        Ok(Self { connection, shared, next_request: 0 })
    }

    fn current(&mut self) -> Result<ActiveContext, BackendError> {
        let owner = kwin_owner(&self.connection)?;
        self.next_request += 1;
        let request = self.next_request.to_string();
        let service = self.connection.unique_name().ok_or_else(|| error("no D-Bus name"))?.to_string();
        let plugin = format!("promptify-focus-{}-{request}", std::process::id());
        let directory = std::env::var_os("XDG_RUNTIME_DIR").ok_or_else(|| error("XDG_RUNTIME_DIR is not set"))?;
        let path = std::path::PathBuf::from(directory).join(format!("{plugin}.js"));
        {
            let mut pending = self.shared.0.0.lock().unwrap();
            *pending = Pending { owner: owner.clone(), request: request.clone(), reply: None };
        }
        let script = SCRIPT.replace("%SERVICE%", &service).replace("%REQUEST%", &request);
        // Never follow a pre-existing file or symlink in the runtime directory.
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path).map_err(error)?;
        let result = (|| {
            file.write_all(script.as_bytes()).map_err(error)?;
            let id: i32 = call(&self.connection, &owner, "/Scripting", "org.kde.kwin.Scripting", "loadScript", &(path.to_string_lossy().as_ref(), &plugin))?
                .body().deserialize().map_err(error)?;
            if id < 0 {
                return Err(error("KWin refused the focus script"));
            }
            call(&self.connection, &owner, &format!("/Scripting/Script{id}"), "org.kde.kwin.Script", "run", &())?;
            let (lock, changed) = &*self.shared.0;
            let (mut pending, _) = changed.wait_timeout_while(lock.lock().unwrap(), REPORT_WAIT, |pending| pending.reply.is_none()).unwrap();
            pending.reply.take().ok_or_else(|| error("KWin did not return a focus snapshot"))?
                .map(context).map_err(error)
        })();
        if let Err(e) = call(&self.connection, &owner, "/Scripting", "org.kde.kwin.Scripting", "unloadScript", &(&plugin,)) {
            log::warn!("could not unload KWin focus snapshot: {e}");
        }
        if let Err(e) = std::fs::remove_file(&path) {
            log::warn!("could not remove KWin focus script {}: {e}", path.display());
        }
        result
    }
}

pub fn init() {
    if applies() && let Err(e) = current() {
        log::warn!("{e}");
    }
}

fn current() -> Result<ActiveContext, BackendError> {
    let mut tracker = TRACKER.lock().unwrap();
    if tracker.is_none() {
        *tracker = Some(Tracker::start()?);
    }
    tracker.as_mut().expect("tracker started").current()
}

pub fn identify() -> Option<Result<ActiveContext, BackendError>> {
    applies().then(current)
}

pub fn shutdown() {
    TRACKER.lock().unwrap().take();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_reports_and_rejects_unknown_focus() {
        assert_eq!(parse(r#"{"id":"{abc}","pid":42,"class":"konsole","caption":"Shell"}"#).unwrap().pid, 42);
        for payload in [
            r#"{"id":"","pid":0,"class":"","caption":""}"#,
            r#"{"id":"{abc}","pid":0,"class":"konsole","caption":"Shell"}"#,
            r#"{"id":"{abc}","pid":42,"class":"","caption":"Shell"}"#,
            "not json",
        ] {
            assert!(parse(payload).is_err());
        }
    }

    #[test]
    fn desktop_selection_is_exact_and_case_insensitive() {
        assert!(is_kde("KDE"));
        assert!(is_kde("kde:PLASMA"));
        assert!(!is_kde("GNOME"));
        assert!(!is_kde("notKDE"));
    }

    #[test]
    fn window_handles_are_stable_and_distinct() {
        assert_eq!(handle("{a}"), handle("{a}"));
        assert_ne!(handle("{a}"), handle("{b}"));
    }

    #[test]
    fn process_name_falls_back_to_window_class() {
        assert_eq!(process_name(u32::MAX, "firefox"), "firefox");
        assert!(!process_name(std::process::id(), "unused").is_empty());
    }

    #[test]
    #[ignore = "requires an interactive KDE Wayland session"]
    fn live_snapshots_are_fresh_and_cleaned_up() {
        let mut tracker = Tracker::start().unwrap();
        for _ in 0..3 {
            let context = tracker.current().unwrap();
            assert_ne!(context.window.process_id, 0);
            assert!(!context.process_name.is_empty());
        }
    }
}

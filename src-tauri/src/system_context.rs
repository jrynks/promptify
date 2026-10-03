use promptify_core::context::{ActiveContext, FocusedText, WindowIdentity};
use promptify_core::pipeline::{BackendError, ContextProvider};

/// Foreground-window identity via KWin on KDE Wayland, otherwise x-win.
pub struct SystemContext;

/// Releases desktop window-tracking connections.
pub fn shutdown() {
    #[cfg(target_os = "linux")]
    crate::kwin::shutdown();
}

fn active_window() -> Result<x_win::WindowInfo, BackendError> {
    let window = x_win::get_active_window().map_err(|e| BackendError(format!("active window unavailable: {}", friendly(&format!("{e:?}")))))?;
    if window.id == 0 || window.info.process_id == 0 {
        return Err(BackendError("The focused window could not be identified. Focus an application window and try again.".into()));
    }
    Ok(window)
}

/// x-win's Wayland backend only exists on GNOME; explain that instead of naming its extension.
fn friendly(raw: &str) -> String {
    #[cfg(target_os = "linux")]
    let wayland = crate::wayland_paste::applies();
    #[cfg(not(target_os = "linux"))]
    let wayland = false;
    let gnome = std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| d.to_ascii_uppercase().contains("GNOME"));
    if cfg!(target_os = "linux") && wayland && !gnome {
        let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_else(|_| "this desktop".into());
        format!("Promptify has no focused-window integration for {desktop} on Wayland. Use KDE Plasma, GNOME with the x-win extension, or an X11 session.")
    } else {
        raw.to_owned()
    }
}

fn identity(window: &x_win::WindowInfo) -> WindowIdentity {
    WindowIdentity { handle: u64::from(window.id), process_id: window.info.process_id }
}

#[cfg(target_os = "linux")]
fn kde() -> Option<Result<ActiveContext, BackendError>> {
    crate::kwin::identify()
}

#[cfg(not(target_os = "linux"))]
fn kde() -> Option<Result<ActiveContext, BackendError>> {
    None
}

impl ContextProvider for SystemContext {
    fn identify(&self) -> Result<ActiveContext, BackendError> {
        if let Some(result) = kde() {
            return result;
        }
        let window = active_window()?;
        let url = x_win::get_browser_url(&window).ok().filter(|url| !url.trim().is_empty());
        let process_name = if window.info.exec_name.is_empty() { window.info.name.clone() } else { window.info.exec_name.clone() };
        Ok(ActiveContext { window: identity(&window), process_name, window_title: window.title, url })
    }

    /// Reads the focused control's text with UI Automation. Only called for apps the user opted in;
    /// password fields are reported as secure so the policy drops them.
    fn focused_text(&self, window: &WindowIdentity) -> Result<Option<FocusedText>, BackendError> {
        focused::read(window.process_id)
    }

    fn foreground(&self) -> Result<WindowIdentity, BackendError> {
        if let Some(result) = kde() {
            return result.map(|context| context.window);
        }
        Ok(identity(&active_window()?))
    }
}

#[cfg(windows)]
mod focused {
    use std::time::Duration;

    use promptify_core::context::FocusedText;
    use promptify_core::pipeline::BackendError;
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize};
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, IUIAutomation, IUIAutomationTextPattern, IUIAutomationValuePattern, UIA_TextPatternId, UIA_ValuePatternId,
    };

    /// A hung target app can block UI Automation; the hotkey never waits longer than this.
    const TIMEOUT: Duration = Duration::from_millis(300);
    /// Upper bound read from a document; the policy then keeps only the most recent part.
    const MAX_READ_CHARS: i32 = 20_000;

    pub fn read(process_id: u32) -> Result<Option<FocusedText>, BackendError> {
        use std::sync::atomic::{AtomicBool, Ordering};
        // A hung app keeps its reader thread blocked; never stack up more readers behind it.
        static IN_FLIGHT: AtomicBool = AtomicBool::new(false);
        if IN_FLIGHT.swap(true, Ordering::SeqCst) {
            return Ok(None);
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new().name("focused-text".into()).spawn(move || {
            let _ = tx.send(read_on_this_thread(process_id));
            IN_FLIGHT.store(false, Ordering::SeqCst);
        });
        if let Err(e) = spawned {
            IN_FLIGHT.store(false, Ordering::SeqCst);
            return Err(BackendError(e.to_string()));
        }
        // Screen text is optional context: a slow app simply contributes nothing.
        Ok(rx.recv_timeout(TIMEOUT).ok().and_then(Result::ok).flatten())
    }

    fn read_on_this_thread(process_id: u32) -> windows::core::Result<Option<FocusedText>> {
        unsafe {
            let initialized = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
            let result = (|| {
                let automation: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
                let element = automation.GetFocusedElement()?;
                // Focus may have moved to another app since the hotkey was pressed.
                if element.CurrentProcessId()? as u32 != process_id {
                    return Ok(None);
                }
                if element.CurrentIsPassword()?.as_bool() {
                    return Ok(Some(FocusedText { text: String::new(), is_secure: true }));
                }
                if let Ok(pattern) = element.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId) {
                    let value = pattern.CurrentValue()?.to_string();
                    if !value.trim().is_empty() {
                        return Ok(Some(FocusedText { text: value, is_secure: false }));
                    }
                }
                if let Ok(pattern) = element.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId) {
                    let text = pattern.DocumentRange()?.GetText(MAX_READ_CHARS)?.to_string();
                    if !text.trim().is_empty() {
                        return Ok(Some(FocusedText { text, is_secure: false }));
                    }
                }
                Ok(None)
            })();
            if initialized {
                CoUninitialize();
            }
            result
        }
    }
}

#[cfg(not(windows))]
mod focused {
    use promptify_core::context::FocusedText;
    use promptify_core::pipeline::BackendError;

    pub fn read(_process_id: u32) -> Result<Option<FocusedText>, BackendError> {
        Ok(None)
    }
}

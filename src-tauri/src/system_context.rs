use promptify_core::context::{ActiveContext, FocusedText, WindowIdentity};
use promptify_core::pipeline::{BackendError, ContextProvider};

/// Foreground-window identity via x-win: window id, process, title and (Windows/macOS) browser URL.
pub struct SystemContext;

fn active_window() -> Result<x_win::WindowInfo, BackendError> {
    x_win::get_active_window().map_err(|e| BackendError(format!("active window unavailable: {e:?}")))
}

fn identity(window: &x_win::WindowInfo) -> WindowIdentity {
    WindowIdentity { handle: u64::from(window.id), process_id: window.info.process_id }
}

impl ContextProvider for SystemContext {
    fn identify(&self) -> Result<ActiveContext, BackendError> {
        let window = active_window()?;
        let url = x_win::get_browser_url(&window).ok().filter(|url| !url.trim().is_empty());
        let process_name = if window.info.exec_name.is_empty() { window.info.name.clone() } else { window.info.exec_name.clone() };
        Ok(ActiveContext { window: identity(&window), process_name, window_title: window.title, url })
    }

    // Accessibility text capture is not implemented yet, so nothing is ever read.
    fn focused_text(&self, _window: &WindowIdentity) -> Result<Option<FocusedText>, BackendError> {
        Ok(None)
    }

    fn foreground(&self) -> Result<WindowIdentity, BackendError> {
        Ok(identity(&active_window()?))
    }
}

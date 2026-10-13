//! Linux desktop integration helpers.
//! Ensures desktop entry files exist for host registration with xdg-desktop-portal
//! and registers the host application ID.

use std::fs;
use std::path::PathBuf;

pub const APP_ID: &str = "dev.promptify.app";
pub const LEGACY_APP_ID: &str = "Promptify";

/// Returns true if a desktop entry for the given ID exists in XDG data dirs.
pub fn has_desktop_entry(app_id: &str) -> bool {
    let desktop_filename = format!("{app_id}.desktop");
    let mut dirs = Vec::new();
    if let Ok(data_home) = std::env::var("XDG_DATA_HOME") {
        dirs.push(PathBuf::from(data_home).join("applications"));
    } else if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".local/share/applications"));
    }
    if let Ok(data_dirs) = std::env::var("XDG_DATA_DIRS") {
        for dir in data_dirs.split(':') {
            dirs.push(PathBuf::from(dir).join("applications"));
        }
    } else {
        dirs.push(PathBuf::from("/usr/local/share/applications"));
        dirs.push(PathBuf::from("/usr/share/applications"));
    }

    dirs.iter().any(|dir| dir.join(&desktop_filename).is_file())
}

/// Ensures desktop entry files matching both dev.promptify.app and Promptify exist
/// in the user's applications directory (`~/.local/share/applications`).
/// When running from an AppImage or an uninstalled development build,
/// the Exec path is pointed to the running AppImage or current executable.
pub fn ensure_desktop_entries() -> Result<(), String> {
    let app_dir = if let Ok(data_home) = std::env::var("XDG_DATA_HOME") {
        PathBuf::from(data_home).join("applications")
    } else if let Some(home) = dirs::home_dir() {
        home.join(".local/share/applications")
    } else {
        return Err("Could not determine user applications directory.".into());
    };

    fs::create_dir_all(&app_dir).map_err(|e| format!("Could not create applications directory {}: {e}", app_dir.display()))?;

    // Determine executable path: prefer APPIMAGE env var if running as AppImage
    let exec_target = if let Ok(appimage) = std::env::var("APPIMAGE") && !appimage.trim().is_empty() {
        appimage
    } else if let Ok(exe) = std::env::current_exe() {
        exe.to_string_lossy().to_string()
    } else {
        "promptify".to_string()
    };

    let content = format!(
        "[Desktop Entry]\n\
        Categories=Utility;\n\
        Comment=Local voice-to-prompt desktop app\n\
        Exec=\"{}\"\n\
        StartupWMClass=promptify\n\
        Icon=promptify\n\
        Name=Promptify\n\
        Terminal=false\n\
        Type=Application\n",
        exec_target
    );

    for app_id in [APP_ID, LEGACY_APP_ID] {
        let target = app_dir.join(format!("{app_id}.desktop"));
        let should_write = match fs::read_to_string(&target) {
            Ok(existing) => !existing.contains(&format!("Exec=\"{}\"", exec_target))
                && !existing.contains(&format!("Exec={}", exec_target)),
            Err(_) => true,
        };
        if should_write {
            fs::write(&target, &content)
                .map_err(|e| format!("Could not write desktop entry {}: {e}", target.display()))?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ensure_desktop_entries() {
        let temp_dir = tempfile::tempdir().unwrap();
        let prev = std::env::var("XDG_DATA_HOME").ok();
        unsafe {
            std::env::set_var("XDG_DATA_HOME", temp_dir.path());
        }
        let res = ensure_desktop_entries();
        if let Some(val) = prev {
            unsafe { std::env::set_var("XDG_DATA_HOME", val); }
        } else {
            unsafe { std::env::remove_var("XDG_DATA_HOME"); }
        }
        assert!(res.is_ok(), "ensure_desktop_entries failed: {:?}", res);
        let app_dir = temp_dir.path().join("applications");
        assert!(app_dir.join("dev.promptify.app.desktop").exists());
        assert!(app_dir.join("Promptify.desktop").exists());
        let content = fs::read_to_string(app_dir.join("dev.promptify.app.desktop")).unwrap();
        assert!(content.contains("[Desktop Entry]"));
        assert!(content.contains("Name=Promptify"));
    }
}

/// Registers the application ID with the desktop portal Registry interface.
/// If already registered on this connection, ignores the AlreadyRegistered error.
pub async fn register_portal_app() -> Result<(), String> {
    let _ = ensure_desktop_entries();

    let app_id = match APP_ID.parse::<ashpd::AppID>() {
        Ok(id) => id,
        Err(_) => return Ok(()),
    };

    match ashpd::register_host_app(app_id).await {
        Ok(()) => {
            log::info!("Registered host app {APP_ID} with desktop portal");
            Ok(())
        }
        Err(ashpd::Error::Portal(ashpd::PortalError::Failed(msg)))
            if msg.contains("Connection already associated") =>
        {
            // Already registered on this dbus connection
            Ok(())
        }
        Err(e) => {
            log::warn!("register_host_app returned {e}; attempting fallback with legacy name {LEGACY_APP_ID}");
            if let Ok(legacy_id) = LEGACY_APP_ID.parse::<ashpd::AppID>() {
                match ashpd::register_host_app(legacy_id).await {
                    Ok(()) => {
                        log::info!("Registered host app {LEGACY_APP_ID} with desktop portal");
                        return Ok(());
                    }
                    Err(ashpd::Error::Portal(ashpd::PortalError::Failed(msg)))
                        if msg.contains("Connection already associated") =>
                    {
                        return Ok(());
                    }
                    Err(err) => {
                        log::warn!("register_host_app fallback failed: {err}");
                    }
                }
            }
            Err(format!("Could not register desktop application ID: {e}"))
        }
    }
}

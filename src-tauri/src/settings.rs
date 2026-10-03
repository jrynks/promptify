use std::path::{Path, PathBuf};
use std::io::Write;
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};

pub const APP_IDENTIFIER: &str = "dev.promptify.app";

/// Same location Tauri's `app_data_dir()` resolves to, so the CLI and app share models and history.
/// `PROMPTIFY_DATA_DIR` overrides it for isolated test runs.
pub fn app_data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("PROMPTIFY_DATA_DIR") {
        return PathBuf::from(dir);
    }
    dirs::data_dir().unwrap_or_else(std::env::temp_dir).join(APP_IDENTIFIER)
}

/// `PROMPTIFY_MODELS_DIR` lets isolated test data folders reuse already-downloaded models.
pub fn models_dir(data_dir: &Path) -> PathBuf {
    if let Some(dir) = std::env::var_os("PROMPTIFY_MODELS_DIR") {
        return PathBuf::from(dir);
    }
    data_dir.join("models")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AppSettings {
    pub onboarding: Option<crate::onboarding::Progress>,
    pub stt_model: Option<String>,
    pub llm_model: Option<String>,
    pub history_enabled: bool,
    pub prompt_hotkey: Option<String>,
    pub dictation_hotkey: Option<String>,
    pub answer_hotkey: Option<String>,
    pub use_gpu: bool,
    /// Lets local MCP tools use this desktop's engines. Phone access also needs the mobile-networking build feature.
    pub server_enabled: bool,
    /// Self-hosted relay for access over the internet, e.g. wss://relay.example.net.
    pub relay_url: Option<String>,
    /// Accept direct (still end-to-end encrypted) connections from the local network.
    pub lan_direct: bool,
    /// Announce this desktop on the local network (mDNS) so paired phones can find it.
    pub lan_discovery: bool,
    /// Holding Ctrl+Shift alone starts a prompt recording.
    pub modifier_hold: bool,
    /// Explicitly selected physical keyboard on Wayland; prefer a stable by-id path.
    pub modifier_keyboard: Option<String>,
    /// The prompt hotkey writes plain dictation in apps that are not AI tools.
    pub auto_mode: bool,
    pub code_chat_paste: bool,
    /// Words to expect and corrections for speech recognition.
    pub vocabulary: promptify_core::dictation::Vocabulary,
    /// App keys (e.g. "chatgpt.com", "outlook") whose focused text may be used as context.
    pub screen_text_apps: Vec<String>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            onboarding: None,
            stt_model: None,
            llm_model: None,
            history_enabled: true,
            prompt_hotkey: None,
            dictation_hotkey: None,
            answer_hotkey: None,
            use_gpu: true,
            server_enabled: false,
            relay_url: None,
            lan_direct: false,
            lan_discovery: false,
            modifier_hold: false,
            modifier_keyboard: None,
            auto_mode: false,
            code_chat_paste: false,
            vocabulary: Default::default(),
            screen_text_apps: Vec::new(),
        }
    }
}

pub type SharedSettings = Arc<RwLock<AppSettings>>;

fn settings_path(data_dir: &Path) -> PathBuf {
    data_dir.join("settings.json")
}

/// Unreadable settings fall back to defaults rather than blocking startup.
pub fn load(data_dir: &Path) -> AppSettings {
    match load_checked(data_dir) {
        Ok(settings) => settings.unwrap_or_default(),
        Err(error) => {
            log::warn!("{error}; using defaults");
            AppSettings::default()
        }
    }
}

pub fn load_checked(data_dir: &Path) -> Result<Option<AppSettings>, String> {
    let path = settings_path(data_dir);
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).map(Some).map_err(|e| format!("{} is invalid: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("could not read {}: {e}", path.display())),
    }
}

pub fn update(settings: &SharedSettings, data_dir: &Path, change: impl FnOnce(&mut AppSettings)) -> Result<(), String> {
    let mut current = settings.write().unwrap();
    let mut next = current.clone();
    change(&mut next);
    save(data_dir, &next).map_err(|e| format!("could not save settings: {e}"))?;
    *current = next;
    Ok(())
}

pub fn save(data_dir: &Path, settings: &AppSettings) -> std::io::Result<()> {
    std::fs::create_dir_all(data_dir)?;
    let path = settings_path(data_dir);
    let tmp = path.with_extension("json.tmp");
    {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(&serde_json::to_vec_pretty(settings)?)?;
        file.sync_all()?;
    }
    std::fs::rename(tmp, path)
}

#[derive(Debug, Clone, Serialize)]
pub struct RoutingState {
    pub rendering: promptify_core::routing::Rendering,
    pub error: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RoutingPreferences {
    version: u32,
    rendering: promptify_core::routing::Rendering,
}

pub fn load_routing(data_dir: &Path) -> Result<promptify_core::routing::Rendering, String> {
    let path = data_dir.join("routing-settings.json");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Default::default()),
        Err(error) => return Err(format!("could not read {}: {error}", path.display())),
    };
    let preferences: RoutingPreferences = serde_json::from_str(&text).map_err(|error| format!("{} is invalid: {error}", path.display()))?;
    if preferences.version != 1 {
        return Err(format!("unsupported routing settings version {} in {}", preferences.version, path.display()));
    }
    Ok(preferences.rendering)
}

pub fn save_routing(data_dir: &Path, rendering: promptify_core::routing::Rendering) -> Result<(), String> {
    // An unreadable or newer sidecar must be repaired explicitly, not overwritten.
    load_routing(data_dir)?;
    let path = data_dir.join("routing-settings.json");
    let tmp = path.with_extension("json.tmp");
    let preferences = RoutingPreferences { version: 1, rendering };
    let bytes = serde_json::to_vec_pretty(&preferences).map_err(|e| e.to_string())?;
    let write = || -> std::io::Result<()> {
        std::fs::create_dir_all(data_dir)?;
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, &path)
    };
    write().map_err(|error| format!("could not save {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let path = std::env::temp_dir().join(format!("promptify-settings-test-{}-{id}", std::process::id()));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            if let Err(error) = std::fs::remove_dir_all(&self.0) {
                eprintln!("could not clean test directory {}: {error}", self.0.display());
            }
        }
    }

    #[test]
    fn modifier_keyboard_defaults_to_unselected_and_survives_reload() {
        let legacy: AppSettings = serde_json::from_str(r#"{"modifier_hold":true}"#).unwrap();
        assert!(legacy.modifier_hold);
        assert!(legacy.modifier_keyboard.is_none());
        let dir = TestDir::new();
        let settings: SharedSettings = Arc::new(RwLock::new(legacy));
        update(&settings, &dir.0, |s| s.modifier_keyboard = Some("/dev/input/by-id/keyboard-event-kbd".into())).unwrap();
        let loaded = load_checked(&dir.0).unwrap().unwrap();
        assert_eq!(loaded.modifier_keyboard.as_deref(), Some("/dev/input/by-id/keyboard-event-kbd"));
        assert!(loaded.modifier_hold);
    }

    #[test]
    fn code_chat_paste_requires_explicit_persisted_consent() {
        let legacy: AppSettings = serde_json::from_str("{}").unwrap();
        assert!(!legacy.code_chat_paste);
        let dir = TestDir::new();
        let settings: SharedSettings = Arc::new(RwLock::new(legacy));
        update(&settings, &dir.0, |s| s.code_chat_paste = true).unwrap();
        assert!(load_checked(&dir.0).unwrap().unwrap().code_chat_paste);
        update(&settings, &dir.0, |s| s.code_chat_paste = false).unwrap();
        assert!(!load_checked(&dir.0).unwrap().unwrap().code_chat_paste);
    }

    #[test]
    fn onboarding_progress_survives_reload_without_changing_preferences() {
        let dir = TestDir::new();
        let settings: SharedSettings = Arc::new(RwLock::new(AppSettings { history_enabled: false, prompt_hotkey: Some("Control+Alt+P".into()), ..Default::default() }));
        update(&settings, &dir.0, |s| s.onboarding = Some(crate::onboarding::Progress::default())).unwrap();
        assert_eq!(load_checked(&dir.0).unwrap().unwrap(), *settings.read().unwrap());
        update(&settings, &dir.0, |s| s.onboarding.as_mut().unwrap().completed = true).unwrap();
        let loaded = load_checked(&dir.0).unwrap().unwrap();
        assert!(loaded.onboarding.unwrap().completed);
        assert!(!loaded.history_enabled);
        assert_eq!(loaded.prompt_hotkey.as_deref(), Some("Control+Alt+P"));
    }

    #[test]
    fn failed_progress_save_does_not_publish_completion() {
        let dir = TestDir::new();
        std::fs::create_dir(dir.0.join("settings.json")).unwrap();
        let settings: SharedSettings = Arc::new(RwLock::new(AppSettings { onboarding: Some(crate::onboarding::Progress::default()), ..Default::default() }));
        let before = settings.read().unwrap().clone();
        assert!(update(&settings, &dir.0, |s| s.onboarding.as_mut().unwrap().completed = true).is_err());
        assert_eq!(*settings.read().unwrap(), before);
    }

    #[test]
    fn invalid_settings_are_not_treated_as_a_new_profile() {
        let dir = TestDir::new();
        assert!(load_checked(&dir.0).unwrap().is_none());
        std::fs::write(dir.0.join("settings.json"), "{invalid").unwrap();
        assert!(load_checked(&dir.0).is_err());
        let (_, error) = crate::onboarding::initialize(&dir.0, &promptify_core::models::Manifest::bundled(), &dir.0.join("models"));
        assert!(error.is_some());
        assert_eq!(std::fs::read_to_string(dir.0.join("settings.json")).unwrap(), "{invalid");
    }

    #[test]
    fn fresh_launch_persists_an_incomplete_tour() {
        let dir = TestDir::new();
        let (settings, error) = crate::onboarding::initialize(&dir.0, &promptify_core::models::Manifest::bundled(), &dir.0.join("models"));
        assert!(error.is_none());
        assert!(!settings.onboarding.as_ref().unwrap().completed);
        assert_eq!(load_checked(&dir.0).unwrap().unwrap(), settings);
    }

    #[test]
    fn routing_sidecar_is_opt_in_and_does_not_change_legacy_settings() {
        let dir = TestDir::new();
        let original = AppSettings::default();
        save(&dir.0, &original).unwrap();
        let before = std::fs::read(dir.0.join("settings.json")).unwrap();
        assert_eq!(load_routing(&dir.0).unwrap(), promptify_core::routing::Rendering::Legacy);
        save_routing(&dir.0, promptify_core::routing::Rendering::Adaptive).unwrap();
        assert_eq!(load_routing(&dir.0).unwrap(), promptify_core::routing::Rendering::Adaptive);
        assert_eq!(std::fs::read(dir.0.join("settings.json")).unwrap(), before);
        assert_eq!(load_checked(&dir.0).unwrap().unwrap(), original);
        save_routing(&dir.0, promptify_core::routing::Rendering::Legacy).unwrap();
        assert_eq!(load_routing(&dir.0).unwrap(), promptify_core::routing::Rendering::Legacy);
    }

    #[test]
    fn invalid_routing_settings_are_reported_and_not_overwritten() {
        let dir = TestDir::new();
        let path = dir.0.join("routing-settings.json");
        let invalid = r#"{"version":99,"rendering":"adaptive"}"#;
        std::fs::write(&path, invalid).unwrap();
        assert!(load_routing(&dir.0).is_err());
        assert!(save_routing(&dir.0, promptify_core::routing::Rendering::Legacy).is_err());
        assert_eq!(std::fs::read_to_string(path).unwrap(), invalid);
    }
}

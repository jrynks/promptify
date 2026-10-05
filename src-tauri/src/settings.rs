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
    pub check_updates_on_startup: bool,
    pub prompt_hotkey: Option<String>,
    pub dictation_hotkey: Option<String>,
    #[serde(skip_serializing, rename = "answer_hotkey")]
    pub(crate) retired_answer_hotkey: Option<String>,
    pub use_gpu: bool,
    // Read-only migration fields; these integrations no longer exist.
    #[serde(skip_serializing, rename = "server_enabled")]
    pub(crate) retired_server_enabled: bool,
    #[serde(skip_serializing, rename = "relay_url")]
    pub(crate) retired_relay_url: Option<String>,
    #[serde(skip_serializing, rename = "lan_direct")]
    pub(crate) retired_lan_direct: bool,
    #[serde(skip_serializing, rename = "lan_discovery")]
    pub(crate) retired_lan_discovery: bool,
    /// Holding Ctrl+Shift alone starts a prompt recording.
    pub modifier_hold: bool,
    /// Read-only migration field; physical-device selection is retired.
    #[serde(skip_serializing)]
    pub modifier_keyboard: Option<String>,
    /// The prompt hotkey writes plain dictation in apps that are not AI tools.
    pub auto_mode: bool,
    pub dictation_tone: promptify_core::quality::DictationTone,
    pub desktop_integration_enabled: bool,
    pub desktop_integration_authorized: bool,
    #[serde(skip_serializing)]
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
            check_updates_on_startup: true,
            prompt_hotkey: None,
            dictation_hotkey: None,
            retired_answer_hotkey: None,
            use_gpu: true,
            retired_server_enabled: false,
            retired_relay_url: None,
            retired_lan_direct: false,
            retired_lan_discovery: false,
            modifier_hold: false,
            modifier_keyboard: None,
            auto_mode: false,
            dictation_tone: promptify_core::quality::DictationTone::Natural,
            desktop_integration_enabled: true,
            desktop_integration_authorized: false,
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
    write_routing(data_dir, rendering)
}

pub fn reset_routing(data_dir: &Path) -> Result<(), String> {
    let path = data_dir.join("routing-settings.json");
    match std::fs::read(&path) {
        Ok(bytes) => {
            // Never replace an earlier backup, including after a failed reset.
            let mut index = 0u64;
            loop {
                let backup = data_dir.join(format!("routing-settings.json.backup-{index}"));
                match std::fs::OpenOptions::new().write(true).create_new(true).open(&backup) {
                    Ok(mut file) => {
                        file.set_permissions(std::fs::metadata(&path).map_err(|error| error.to_string())?.permissions())
                            .map_err(|error| format!("could not protect routing backup: {error}"))?;
                        file.write_all(&bytes).and_then(|_| file.sync_all())
                            .map_err(|error| format!("could not back up {}: {error}", path.display()))?;
                        #[cfg(unix)]
                        std::fs::File::open(data_dir).and_then(|directory| directory.sync_all())
                            .map_err(|error| format!("could not persist routing backup: {error}"))?;
                        break;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        index = index.checked_add(1).ok_or("routing backup index exhausted")?;
                    }
                    Err(error) => return Err(format!("could not back up {}: {error}", path.display())),
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("could not read {} for backup: {error}", path.display())),
    }
    write_routing(data_dir, Default::default())
}

fn write_routing(data_dir: &Path, rendering: promptify_core::routing::Rendering) -> Result<(), String> {
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

    #[test]
    fn retired_integrations_migrate_without_resetting_preferences() {
        let dir = TestDir::new();
        let text = r#"{
            "answer_hotkey":"Ctrl+Alt+A","server_enabled":true,
            "relay_url":"wss://example.invalid","lan_direct":true,"lan_discovery":true,
            "stt_model":"whisper-small-en","llm_model":"qwen3.5-9b-q4km",
            "prompt_hotkey":"Ctrl+Alt+P","dictation_hotkey":"Ctrl+Alt+D",
            "history_enabled":false,"use_gpu":false,"auto_mode":true,
            "check_updates_on_startup":false,"modifier_hold":true,
            "onboarding":{"version":1,"step":"practice","completed":true},
            "vocabulary":{"words":["Promptify"],"replacements":[]},
            "screen_text_apps":["outlook"],"desktop_integration_enabled":false
        }"#;
        std::fs::write(dir.0.join("settings.json"), text).unwrap();
        let settings = load_checked(&dir.0).unwrap().unwrap();
        assert_eq!(settings.llm_model.as_deref(), Some("qwen3.5-9b-q4km"));
        assert_eq!(settings.stt_model.as_deref(), Some("whisper-small-en"));
        assert_eq!(settings.prompt_hotkey.as_deref(), Some("Ctrl+Alt+P"));
        assert_eq!(settings.dictation_hotkey.as_deref(), Some("Ctrl+Alt+D"));
        assert!(!settings.history_enabled && !settings.use_gpu && !settings.check_updates_on_startup);
        assert!(settings.auto_mode && settings.modifier_hold);
        assert!(!settings.desktop_integration_enabled);
        assert!(settings.onboarding.as_ref().unwrap().completed);
        assert_eq!(settings.vocabulary.words, ["Promptify"]);
        assert_eq!(settings.screen_text_apps, ["outlook"]);
        assert_eq!(settings.dictation_tone, promptify_core::quality::DictationTone::Natural);
        save(&dir.0, &settings).unwrap();
        let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.0.join("settings.json")).unwrap()).unwrap();
        for key in ["answer_hotkey", "server_enabled", "relay_url", "lan_direct", "lan_discovery"] {
            assert!(saved.get(key).is_none(), "{key} must not be written");
        }
        let reloaded = load_checked(&dir.0).unwrap().unwrap();
        assert_eq!(reloaded.vocabulary, settings.vocabulary);
        assert_eq!(reloaded.llm_model, settings.llm_model);
        assert_eq!(reloaded.onboarding, settings.onboarding);
    }

    #[test]
    fn migration_keeps_strict_settings_validation() {
        assert!(serde_json::from_str::<AppSettings>(r#"{"unknown_preference":true}"#).is_err());
        assert!(serde_json::from_str::<AppSettings>(r#"{"server_enabled":"yes"}"#).is_err());
        assert!(serde_json::from_str::<AppSettings>(r#"{"answer_hotkey":42}"#).is_err());
        assert!(serde_json::from_str::<AppSettings>(r#"{"dictation_tone":"invented"}"#).is_err());
    }

    #[test]
    fn dictation_tone_defaults_persists_and_rejects_invalid_saved_values() {
        let dir = TestDir::new();
        std::fs::write(dir.0.join("settings.json"), "{}").unwrap();
        assert_eq!(
            load_checked(&dir.0).unwrap().unwrap().dictation_tone,
            promptify_core::quality::DictationTone::Natural
        );

        let mut settings = AppSettings::default();
        for tone in promptify_core::quality::DictationTone::ALL {
            settings.dictation_tone = tone;
            save(&dir.0, &settings).unwrap();
            assert_eq!(load_checked(&dir.0).unwrap().unwrap().dictation_tone, tone);
        }
        std::fs::write(dir.0.join("settings.json"), r#"{"dictation_tone":"unknown"}"#).unwrap();
        assert!(load_checked(&dir.0).is_err(), "invalid values must surface as configuration errors");
    }

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
    fn retired_keyboard_setting_is_readable_but_not_written() {
        let legacy: AppSettings = serde_json::from_str(r#"{"modifier_hold":true,"modifier_keyboard":"/dev/input/by-id/keyboard-event-kbd"}"#).unwrap();
        assert!(legacy.modifier_hold);
        assert!(legacy.desktop_integration_enabled);
        assert_eq!(legacy.modifier_keyboard.as_deref(), Some("/dev/input/by-id/keyboard-event-kbd"));
        let dir = TestDir::new();
        let settings: SharedSettings = Arc::new(RwLock::new(legacy));
        update(&settings, &dir.0, |s| s.desktop_integration_enabled = false).unwrap();
        let loaded = load_checked(&dir.0).unwrap().unwrap();
        assert!(loaded.modifier_keyboard.is_none());
        assert!(!loaded.desktop_integration_enabled);
        assert!(!std::fs::read_to_string(dir.0.join("settings.json")).unwrap().contains("modifier_keyboard"));
        assert!(loaded.modifier_hold);
    }

    #[test]
    fn retired_code_chat_setting_is_readable_but_not_written() {
        let legacy: AppSettings = serde_json::from_str("{}").unwrap();
        assert!(!legacy.code_chat_paste);
        let dir = TestDir::new();
        let settings: SharedSettings = Arc::new(RwLock::new(legacy));
        update(&settings, &dir.0, |s| s.code_chat_paste = true).unwrap();
        assert!(!std::fs::read_to_string(dir.0.join("settings.json")).unwrap().contains("code_chat_paste"));
        assert!(!load_checked(&dir.0).unwrap().unwrap().code_chat_paste);
        update(&settings, &dir.0, |s| s.code_chat_paste = false).unwrap();
        assert!(!load_checked(&dir.0).unwrap().unwrap().code_chat_paste);
    }

    #[test]
    fn permission_related_preferences_survive_restarts_and_disable() {
        let dir = TestDir::new();
        let settings: SharedSettings = Arc::new(RwLock::new(AppSettings::default()));
        update(&settings, &dir.0, |s| {
            s.desktop_integration_enabled = true;
            s.desktop_integration_authorized = true;
            s.modifier_hold = true;
            s.screen_text_apps = vec!["approved-app".into()];
        }).unwrap();
        let loaded = load_checked(&dir.0).unwrap().unwrap();
        assert!(loaded.desktop_integration_enabled);
        assert!(loaded.desktop_integration_authorized);
        assert!(loaded.modifier_hold);
        assert_eq!(loaded.screen_text_apps, vec!["approved-app"]);
        update(&settings, &dir.0, |s| {
            s.desktop_integration_enabled = false;
            s.desktop_integration_authorized = false;
        }).unwrap();
        let disabled = load_checked(&dir.0).unwrap().unwrap();
        assert!(!disabled.desktop_integration_enabled);
        assert!(!disabled.desktop_integration_authorized);
        let legacy: AppSettings = serde_json::from_str(r#"{"desktop_integration_enabled":true}"#).unwrap();
        assert!(!legacy.desktop_integration_authorized, "default enable is not prior consent");
    }

    #[test]
    fn failed_integration_save_keeps_the_live_preference() {
        let dir = TestDir::new();
        std::fs::create_dir(dir.0.join("settings.json")).unwrap();
        let settings: SharedSettings = Arc::new(RwLock::new(AppSettings::default()));
        assert!(update(&settings, &dir.0, |s| {
            s.desktop_integration_enabled = false;
            s.desktop_integration_authorized = true;
        }).is_err());
        assert!(settings.read().unwrap().desktop_integration_enabled);
        assert!(!settings.read().unwrap().desktop_integration_authorized);
    }

    #[test]
    fn update_check_preference_migrates_and_persists() {
        let legacy: AppSettings = serde_json::from_str("{}").unwrap();
        assert!(legacy.check_updates_on_startup);
        let dir = TestDir::new();
        let settings: SharedSettings = Arc::new(RwLock::new(legacy));
        update(&settings, &dir.0, |settings| settings.check_updates_on_startup = false).unwrap();
        assert!(!load_checked(&dir.0).unwrap().unwrap().check_updates_on_startup);
        std::fs::remove_file(dir.0.join("settings.json")).unwrap();
        std::fs::create_dir(dir.0.join("settings.json")).unwrap();
        assert!(update(&settings, &dir.0, |settings| settings.check_updates_on_startup = true).is_err());
        assert!(!settings.read().unwrap().check_updates_on_startup);
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

    #[test]
    fn explicit_routing_reset_preserves_invalid_bytes_and_previous_backups() {
        let dir = TestDir::new();
        let path = dir.0.join("routing-settings.json");
        let invalid = b"\xffinvalid";
        std::fs::write(&path, invalid).unwrap();
        reset_routing(&dir.0).unwrap();
        assert_eq!(load_routing(&dir.0).unwrap(), promptify_core::routing::Rendering::Legacy);
        assert_eq!(std::fs::read(dir.0.join("routing-settings.json.backup-0")).unwrap(), invalid);
        let newer = br#"{"version":99,"rendering":"adaptive"}"#;
        std::fs::write(&path, newer).unwrap();
        reset_routing(&dir.0).unwrap();
        assert_eq!(std::fs::read(dir.0.join("routing-settings.json.backup-0")).unwrap(), invalid);
        assert_eq!(std::fs::read(dir.0.join("routing-settings.json.backup-1")).unwrap(), newer);
    }

    #[test]
    fn routing_reset_failure_never_overwrites_an_unbacked_file() {
        let dir = TestDir::new();
        let path = dir.0.join("routing-settings.json");
        std::fs::create_dir(&path).unwrap();
        assert!(reset_routing(&dir.0).is_err());
        assert!(path.is_dir());
        std::fs::remove_dir(&path).unwrap();
        std::fs::write(&path, b"invalid").unwrap();
        std::fs::create_dir(dir.0.join("routing-settings.json.tmp")).unwrap();
        assert!(reset_routing(&dir.0).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"invalid");
        assert_eq!(std::fs::read(dir.0.join("routing-settings.json.backup-0")).unwrap(), b"invalid");
    }
}

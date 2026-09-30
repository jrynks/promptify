use std::path::{Path, PathBuf};
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

pub fn models_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("models")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AppSettings {
    pub stt_model: Option<String>,
    pub llm_model: Option<String>,
    pub history_enabled: bool,
    pub prompt_hotkey: Option<String>,
    pub dictation_hotkey: Option<String>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self { stt_model: None, llm_model: None, history_enabled: true, prompt_hotkey: None, dictation_hotkey: None }
    }
}

pub type SharedSettings = Arc<RwLock<AppSettings>>;

fn settings_path(data_dir: &Path) -> PathBuf {
    data_dir.join("settings.json")
}

/// Unreadable settings fall back to defaults rather than blocking startup.
pub fn load(data_dir: &Path) -> AppSettings {
    match std::fs::read_to_string(settings_path(data_dir)) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
            log::warn!("settings.json is invalid, using defaults: {e}");
            AppSettings::default()
        }),
        Err(_) => AppSettings::default(),
    }
}

pub fn save(data_dir: &Path, settings: &AppSettings) -> std::io::Result<()> {
    std::fs::create_dir_all(data_dir)?;
    let path = settings_path(data_dir);
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(settings)?)?;
    std::fs::rename(tmp, path)
}

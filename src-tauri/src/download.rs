use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use promptify_core::models::{ModelEntry, ModelError, finalize_download, is_allowed_redirect, part_path};
use promptify_core::pipeline::CancelToken;
use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Progress {
    pub downloaded: u64,
    pub total: u64,
    pub verifying: bool,
}

fn client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 10 {
                attempt.error("too many redirects")
            } else if is_allowed_redirect(attempt.url()) {
                attempt.follow()
            } else {
                attempt.error("redirect to a host outside the download allowlist")
            }
        }))
        .connect_timeout(Duration::from_secs(20))
        // The blocking client applies this per read, so it bounds stalls, not total download time.
        .timeout(Duration::from_secs(60))
        .user_agent(concat!("Promptify/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| format!("HTTP client: {e}"))
}

/// Downloads to `<file>.part` (resuming if present), then verifies size + SHA-256 before the
/// file is moved to its final name. A cancelled download keeps its `.part` for resuming.
pub fn download(models_dir: &Path, entry: &ModelEntry, cancel: &CancelToken, on_progress: &mut dyn FnMut(Progress)) -> Result<PathBuf, String> {
    entry.validate().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(models_dir).map_err(|e| format!("cannot create models folder: {e}"))?;
    let part = part_path(models_dir, entry);
    let mut have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    if have > entry.size_bytes {
        std::fs::remove_file(&part).map_err(|e| e.to_string())?;
        have = 0;
    }

    if have < entry.size_bytes {
        let client = client()?;
        let mut request = client.get(&entry.url);
        if have > 0 {
            request = request.header(reqwest::header::RANGE, format!("bytes={have}-"));
        }
        let mut response = request.send().map_err(|e| format!("download failed: {e}"))?;
        let status = response.status();
        let mut file = if status == reqwest::StatusCode::PARTIAL_CONTENT && have > 0 {
            OpenOptions::new().append(true).open(&part).map_err(|e| e.to_string())?
        } else if status.is_success() {
            have = 0;
            File::create(&part).map_err(|e| e.to_string())?
        } else {
            return Err(format!("download failed: HTTP {status}"));
        };

        let mut buf = vec![0u8; 256 * 1024];
        let mut last_report = Instant::now() - Duration::from_secs(1);
        loop {
            if cancel.is_cancelled() {
                return Err("cancelled".into());
            }
            let read = response.read(&mut buf).map_err(|e| format!("download interrupted: {e}"))?;
            if read == 0 {
                break;
            }
            have += read as u64;
            if have > entry.size_bytes {
                drop(file);
                let _ = std::fs::remove_file(&part);
                return Err("server sent more data than expected".into());
            }
            file.write_all(&buf[..read]).map_err(|e| format!("write failed: {e}"))?;
            if last_report.elapsed() >= Duration::from_millis(200) {
                last_report = Instant::now();
                on_progress(Progress { downloaded: have, total: entry.size_bytes, verifying: false });
            }
        }
        file.sync_all().map_err(|e| e.to_string())?;
        if have < entry.size_bytes {
            return Err("download ended early; it will resume next time".into());
        }
    }

    on_progress(Progress { downloaded: entry.size_bytes, total: entry.size_bytes, verifying: true });
    finalize_download(models_dir, entry).map_err(|e| match e {
        ModelError::HashMismatch(_) => "the download failed its integrity check and was deleted".to_string(),
        other => other.to_string(),
    })
}

/// Tracks in-flight downloads so each model downloads at most once and can be cancelled.
#[derive(Default)]
pub struct Downloads {
    active: Mutex<HashMap<String, CancelToken>>,
}

impl Downloads {
    pub fn start(&self, id: &str) -> Option<CancelToken> {
        let mut active = self.active.lock().unwrap();
        if active.contains_key(id) {
            return None;
        }
        let token = CancelToken::default();
        active.insert(id.to_owned(), token.clone());
        Some(token)
    }

    pub fn finish(&self, id: &str) {
        self.active.lock().unwrap().remove(id);
    }

    pub fn cancel(&self, id: &str) -> bool {
        self.active.lock().unwrap().get(id).map(CancelToken::cancel).is_some()
    }

    pub fn active_ids(&self) -> Vec<String> {
        self.active.lock().unwrap().keys().cloned().collect()
    }
}

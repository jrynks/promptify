use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::Mutex;

use promptify_core::audio::TARGET_SAMPLE_RATE;
use promptify_core::dictation::strip_non_speech;
use promptify_core::models::{Manifest, ModelKind, final_path, is_installed};
use promptify_core::pipeline::{BackendError, CancelToken, Transcriber};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::settings::SharedSettings;

/// Shorter clips are almost always accidental taps.
const MIN_SAMPLES: usize = TARGET_SAMPLE_RATE as usize / 4;

/// `user_data` points at a `CancelToken` that outlives the `whisper_full` call.
unsafe extern "C" fn abort_trampoline(user_data: *mut c_void) -> bool {
    unsafe { &*(user_data as *const CancelToken) }.is_cancelled()
}

pub struct WhisperEngine {
    manifest: Manifest,
    models_dir: PathBuf,
    settings: SharedSettings,
    loaded: Mutex<Option<(String, WhisperContext)>>,
}

impl WhisperEngine {
    pub fn new(manifest: Manifest, models_dir: PathBuf, settings: SharedSettings) -> Self {
        whisper_rs::install_logging_hooks();
        Self { manifest, models_dir, settings, loaded: Mutex::new(None) }
    }

    fn selected(&self) -> Result<(String, PathBuf), BackendError> {
        let id = self.settings.read().unwrap().stt_model.clone();
        let entry = id
            .as_deref()
            .and_then(|id| self.manifest.get(id))
            .filter(|e| e.kind == ModelKind::Stt && is_installed(&self.models_dir, e))
            .ok_or_else(|| BackendError("No speech model is installed. Open Promptify settings → Models.".into()))?;
        Ok((entry.id.clone(), final_path(&self.models_dir, entry)))
    }

    /// Loads the selected model ahead of the first recording.
    pub fn preload(&self) -> Result<(), BackendError> {
        let (id, path) = self.selected()?;
        let mut loaded = self.loaded.lock().unwrap();
        if loaded.as_ref().is_none_or(|(current, _)| current != &id) {
            *loaded = None;
            let ctx = WhisperContext::new_with_params(&path, WhisperContextParameters::default())
                .map_err(|e| BackendError(format!("could not load speech model: {e}")))?;
            *loaded = Some((id, ctx));
        }
        Ok(())
    }
}

impl Transcriber for WhisperEngine {
    fn transcribe(&self, audio: &[f32], cancel: &CancelToken) -> Result<String, BackendError> {
        if audio.len() < MIN_SAMPLES {
            return Ok(String::new());
        }
        self.preload()?;
        let loaded = self.loaded.lock().unwrap();
        let (id, ctx) = loaded.as_ref().expect("preloaded");
        let mut state = ctx.create_state().map_err(|e| BackendError(format!("speech engine: {e}")))?;

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8) as i32;
        params.set_n_threads(threads);
        // English-only models reject other languages; multilingual models auto-detect.
        params.set_language(if id.ends_with("-en") { Some("en") } else { Some("auto") });
        params.set_no_context(true);
        params.set_no_timestamps(true);
        params.set_suppress_blank(true);
        params.set_suppress_nst(true);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        // whisper-rs 0.16's set_abort_callback_safe casts its boxed trait object to the closure
        // type and aborts every run, so use a direct trampoline over the cancel token instead.
        let token = cancel.clone();
        unsafe {
            params.set_abort_callback(Some(abort_trampoline));
            params.set_abort_callback_user_data(&token as *const CancelToken as *mut c_void);
        }

        state.full(params, audio).map_err(|e| BackendError(format!("transcription failed: {e}")))?;
        drop(token);
        let mut text = String::new();
        for segment in state.as_iter() {
            if let Ok(piece) = segment.to_str_lossy() {
                text.push_str(&piece);
            }
        }
        Ok(strip_non_speech(&text))
    }
}

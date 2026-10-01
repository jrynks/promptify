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

/// whisper.cpp numbers GPU and iGPU devices together in registry order (Vulkan often lists the
/// CPU's iGPU first). Returns that index for the best device: discrete first, then most memory.
fn preferred_gpu() -> Option<(i32, String)> {
    use whisper_rs::whisper_rs_sys as sys;
    let mut best: Option<(i32, bool, usize, String)> = None;
    let mut index = 0;
    unsafe {
        for i in 0..sys::ggml_backend_dev_count() {
            let dev = sys::ggml_backend_dev_get(i);
            let kind = sys::ggml_backend_dev_type(dev);
            let discrete = kind == sys::ggml_backend_dev_type_GGML_BACKEND_DEVICE_TYPE_GPU;
            if !discrete && kind != sys::ggml_backend_dev_type_GGML_BACKEND_DEVICE_TYPE_IGPU {
                continue;
            }
            let (mut free, mut total) = (0usize, 0usize);
            sys::ggml_backend_dev_memory(dev, &mut free, &mut total);
            let description = std::ffi::CStr::from_ptr(sys::ggml_backend_dev_description(dev)).to_string_lossy().into_owned();
            if best.as_ref().is_none_or(|(_, d, mem, _)| (discrete, total) > (*d, *mem)) {
                best = Some((index, discrete, total, description));
            }
            index += 1;
        }
    }
    best.map(|(index, _, _, description)| (index, description))
}

/// Runs one silent transcription so GPU kernels compile before the user's first recording.
fn warm_up(ctx: &WhisperContext) {
    let Ok(mut state) = ctx.create_state() else { return };
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_no_timestamps(true);
    let _ = state.full(params, &vec![0.0; TARGET_SAMPLE_RATE as usize]);
}

pub struct WhisperEngine {
    manifest: Manifest,
    models_dir: PathBuf,
    settings: SharedSettings,
    loaded: Mutex<Option<((String, bool), WhisperContext)>>,
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
        let key = (id, self.settings.read().unwrap().use_gpu);
        let mut loaded = self.loaded.lock().unwrap();
        if loaded.as_ref().is_none_or(|(current, _)| current != &key) {
            *loaded = None;
            let mut params = WhisperContextParameters::default();
            let gpu = if key.1 { preferred_gpu() } else { None };
            params.use_gpu(gpu.is_some());
            if let Some((index, _)) = &gpu {
                params.gpu_device(*index);
            }
            let ctx = WhisperContext::new_with_params(&path, params).map_err(|e| BackendError(format!("could not load speech model: {e}")))?;
            log::info!("speech model loaded on {}", gpu.as_ref().map_or("CPU", |(_, name)| name.as_str()));
            warm_up(&ctx);
            *loaded = Some((key, ctx));
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
        let ((id, _), ctx) = loaded.as_ref().expect("preloaded");
        let mut state = ctx.create_state().map_err(|e| BackendError(format!("speech engine: {e}")))?;

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(16) as i32;
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

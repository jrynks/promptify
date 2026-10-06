use std::path::Path;
use std::sync::Mutex;

use promptify_core::models::{Manifest, ModelKind, is_installed};
use promptify_core::pipeline::{CancelToken, Job, Mode, Outcome, Stage};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, Runtime, State};

use crate::AppState;
use crate::controller::{Command, OverlayEvent};
use crate::settings::{self, AppSettings};

const VERSION: u8 = 1;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    #[default]
    Models,
    Input,
    Practice,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Progress {
    pub version: u8,
    pub step: Step,
    pub completed: bool,
}

impl Default for Progress {
    fn default() -> Self {
        Self { version: VERSION, step: Step::Models, completed: false }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ModelKey {
    speech: Option<String>,
    language: Option<String>,
    gpu: bool,
    inference: Option<String>,
}

impl From<&AppSettings> for ModelKey {
    fn from(settings: &AppSettings) -> Self {
        Self { speech: settings.stt_model.clone(), language: settings.llm_model.clone(), gpu: settings.use_gpu, inference: None }
    }
}

impl ModelKey {
    fn selected(settings: &AppSettings, inference: &crate::inference::InferenceManager) -> Self {
        let mut key = Self::from(settings);
        match inference.config() {
            Ok(config) => match config.selection {
                crate::inference::InferenceSelection::BundledLocal => {}
                crate::inference::InferenceSelection::Connection { connection_id, model } => {
                    let revision = config.connections.iter().find(|connection| connection.id == connection_id).map(|connection| connection.revision);
                    key.language = Some(model.clone());
                    key.inference = Some(format!("{connection_id}:{revision:?}:{model}"));
                }
            },
            Err(error) => {
                key.language = None;
                key.inference = Some(format!("invalid:{error}"));
            }
        }
        key
    }
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum EngineStatus {
    #[default]
    Missing,
    Loading,
    Ready,
    Error { message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum PracticePhase {
    Armed,
    Recording,
    Processing,
    AwaitingPaste,
    Passed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Configuration {
    models: ModelKey,
    shortcut: String,
    microphone_id: String,
}

struct Attempt {
    id: u64,
    config: Configuration,
    phase: PracticePhase,
    job_id: Option<u64>,
    cancel: Option<CancelToken>,
    heard_speech: bool,
    generated: bool,
    inserted: Option<String>,
    error: Option<String>,
}

impl Attempt {
    fn verified_for(&self, config: &Configuration) -> bool {
        self.phase == PracticePhase::Passed && self.config == *config
    }

    fn observe(&mut self, event: &OverlayEvent) {
        match event {
            OverlayEvent::Listening { .. } => self.phase = PracticePhase::Recording,
            OverlayEvent::Stage { stage } => {
                self.phase = PracticePhase::Processing;
                self.generated |= *stage == Stage::Generating;
            }
            OverlayEvent::Transcript { text } => self.heard_speech |= !text.trim().is_empty(),
            OverlayEvent::Finished { report, .. } => {
                self.cancel = None;
                match &report.outcome {
                    Outcome::Inserted { text }
                        if self.job_id == Some(report.job_id) && self.heard_speech && self.generated && !text.trim().is_empty() =>
                    {
                        self.inserted = Some(text.clone());
                        self.phase = PracticePhase::AwaitingPaste;
                    }
                    _ => {
                        self.phase = PracticePhase::Failed;
                        self.error = Some(match &report.outcome {
                            Outcome::NoSpeech => "No speech was heard. Check the microphone and try again.".into(),
                            Outcome::Blocked { reason, detail, .. } => format!("The practice paste was blocked ({reason:?}). {}", detail.as_deref().unwrap_or("Keep the practice field focused and try again.")),
                            Outcome::Failed { reason, detail } => format!("Practice failed ({reason:?}). {}", detail.as_deref().unwrap_or("Try again or check your models.")),
                            Outcome::Cancelled => "Practice was cancelled. Try again when ready.".into(),
                            _ => "The practice did not produce a verified voice-to-prompt result. Try again.".into(),
                        });
                    }
                }
            }
            OverlayEvent::Error { message } => {
                self.phase = PracticePhase::Failed;
                self.error = Some(message.clone());
            }
            OverlayEvent::Cancelled => {
                self.phase = PracticePhase::Failed;
                self.error = Some("Practice was cancelled. Try again when ready.".into());
            }
            _ => {}
        }
    }

    fn acknowledge(&mut self, job_id: u64, text: &str) -> Result<(), String> {
        if self.job_id != Some(job_id)
            || !matches!(self.phase, PracticePhase::AwaitingPaste | PracticePhase::Passed)
            || self.inserted.as_deref().map(normalize_paste).as_deref() != Some(normalize_paste(text).as_str())
        {
            return Err("The pasted text does not match the current successful practice. Try again.".into());
        }
        self.phase = PracticePhase::Passed;
        Ok(())
    }
}

fn normalize_paste(text: &str) -> String {
    text.replace("\r\n", "\n")
}

#[derive(Default)]
struct LiveState {
    startup_error: Option<String>,
    load_generation: u64,
    model_key: Option<ModelKey>,
    speech: EngineStatus,
    language: EngineStatus,
    next_attempt: u64,
    attempt: Option<Attempt>,
    practice_error: Option<String>,
}

#[derive(Default)]
pub struct Onboarding {
    live: Mutex<LiveState>,
}

impl Onboarding {
    pub fn new(startup_error: Option<String>) -> Self {
        Self { live: Mutex::new(LiveState { startup_error, ..Default::default() }) }
    }

    pub fn ready(&self, settings: &AppSettings) -> bool {
        self.ready_key(&ModelKey::from(settings))
    }

    fn ready_key(&self, key: &ModelKey) -> bool {
        let live = self.live.lock().unwrap();
        live.model_key.as_ref() == Some(key)
            && matches!(live.speech, EngineStatus::Ready)
            && matches!(live.language, EngineStatus::Ready)
    }

    pub fn engine_error(&self) -> Option<String> {
        let live = self.live.lock().unwrap();
        [&live.speech, &live.language].into_iter().find_map(|status| match status {
            EngineStatus::Error { message } => Some(message.clone()),
            _ => None,
        })
    }
}

pub fn engines_ready(state: &AppState, settings: &AppSettings) -> bool {
    state.onboarding.ready_key(&ModelKey::selected(settings, &state.llm))
        && inference_available(state, settings)
}

fn engine_recovery(
    inference: crate::inference::InferenceStatus,
    engine_error: Option<String>,
) -> (crate::commands::RecoverySection, String) {
    use crate::commands::RecoverySection;
    use crate::inference::{InferenceSelection, InferenceState};
    if matches!(inference.selection, InferenceSelection::Connection { .. })
        && inference.state != InferenceState::Ready
    {
        let detail = inference.message.unwrap_or_else(|| "Selected inference is not ready.".into());
        return (RecoverySection::Inference, format!(
            "{detail} Open Settings > Models > Prompt writer and click Test for the selected connection before using shortcuts."
        ));
    }
    (RecoverySection::Models, engine_error.unwrap_or_else(||
        "The speech and language engines are not ready. Open Settings > Models and wait for loading or resolve the model error.".into()
    ))
}

pub fn show_engine_recovery(app: &AppHandle) {
    let state = app.state::<AppState>();
    let (section, message) = engine_recovery(state.llm.status(), state.onboarding.engine_error());
    record_error(app, &message);
    if let Err(error) = crate::commands::open_recovery_settings(app.clone(), section) {
        log::warn!("could not open engine recovery settings: {error}");
    }
}

fn migrate(settings: &mut AppSettings, existed: bool, configured: bool) -> Result<(), String> {
    if let Some(progress) = &settings.onboarding {
        if progress.version != VERSION {
            return Err("This setup record belongs to a different Promptify version. Use the matching application version.".into());
        }
    } else {
        settings.onboarding = Some(Progress { completed: existed && configured, ..Default::default() });
    }
    Ok(())
}

pub fn initialize(data_dir: &Path, manifest: &Manifest, models_dir: &Path) -> (AppSettings, Option<String>) {
    let saved = match settings::load_checked(data_dir) {
        Ok(saved) => saved,
        Err(error) => return (AppSettings::default(), Some(error)),
    };
    let existed = saved.is_some();
    let mut settings = saved.unwrap_or_default();
    let before = settings.clone();
    crate::ensure_selection(manifest, models_dir, &mut settings);
    let configured = settings.stt_model.is_some() && settings.llm_model.is_some();
    if let Err(error) = migrate(&mut settings, existed, configured) {
        return (settings, Some(error));
    }
    let error = if settings != before {
        settings::save(data_dir, &settings).err().map(|e| format!("could not save setup progress: {e}"))
    } else {
        None
    };
    if let Some(error) = &error {
        log::error!("{error}");
    }
    (settings, error)
}

pub fn required(state: &AppState) -> bool {
    let completed = state.settings.read().unwrap().onboarding.as_ref().is_some_and(|p| p.completed);
    !completed || state.onboarding.live.lock().unwrap().startup_error.is_some()
}

pub fn available(state: &AppState) -> Result<(), String> {
    match &state.onboarding.live.lock().unwrap().startup_error {
        Some(error) => Err(format!("Resolve the startup problem first: {error}")),
        None => Ok(()),
    }
}

pub fn require_complete(state: &AppState) -> Result<(), String> {
    if required(state) {
        Err("Finish the required setup before connecting optional desktop tools.".into())
    } else {
        Ok(())
    }
}

pub fn models_installed(state: &AppState, settings: &AppSettings) -> bool {
    [(ModelKind::Stt, &settings.stt_model), (ModelKind::Llm, &settings.llm_model)].into_iter().all(|(kind, id)| {
        id.as_deref().and_then(|id| state.manifest.get(id)).is_some_and(|entry| entry.kind == kind && is_installed(&state.models_dir, entry))
    })
}

fn inference_available(state: &AppState, settings: &AppSettings) -> bool {
    let speech = settings.stt_model.as_deref().and_then(|id| state.manifest.get(id))
        .is_some_and(|entry| entry.kind == ModelKind::Stt && is_installed(&state.models_dir, entry));
    if !speech {
        return false;
    }
    match state.llm.config() {
        Ok(config) => match config.selection {
            crate::inference::InferenceSelection::BundledLocal => models_installed(state, settings),
            crate::inference::InferenceSelection::Connection { .. } => state.llm.status().state == crate::inference::InferenceState::Ready,
        },
        Err(_) => false,
    }
}

fn require_models(state: &AppState) -> Result<(), String> {
    available(state)?;
    let settings = state.settings.read().unwrap().clone();
    if !engines_ready(state, &settings) {
        return Err("Load the speech model and load bundled inference or test the selected inference connection before continuing.".into());
    }
    Ok(())
}

fn configuration(state: &AppState) -> Result<Configuration, String> {
    require_models(state)?;
    if !state.settings.read().unwrap().desktop_integration_enabled {
        return Err("Enable desktop integration before continuing.".into());
    }
    #[cfg(target_os = "linux")]
    crate::wayland_paste::require_ready()?;
    use promptify_core::pipeline::ContextProvider;
    crate::system_context::SystemContext.identify().map_err(|e| e.0)?;
    let models = ModelKey::selected(&state.settings.read().unwrap(), &state.llm);
    let hotkeys = state.hotkeys.read().unwrap();
    if hotkeys.paused {
        return Err("Resume the Prompt shortcut before continuing.".into());
    }
    if let Some(error) = &hotkeys.prompt_error {
        return Err(error.clone());
    }
    let shortcut = hotkeys.config.prompt.clone();
    drop(hotkeys);
    let microphone_id = crate::audio::default_input_id()?;
    Ok(Configuration { models, shortcut, microphone_id })
}

#[derive(Serialize)]
pub struct PracticeStatus {
    phase: Option<PracticePhase>,
    attempt_id: Option<u64>,
    job_id: Option<u64>,
    error: Option<String>,
}

#[derive(Serialize)]
pub struct Status {
    required: bool,
    step: Step,
    speech: EngineStatus,
    language: EngineStatus,
    startup_error: Option<String>,
    practice: PracticeStatus,
}

fn snapshot(state: &AppState) -> Status {
    let settings = state.settings.read().unwrap().clone();
    let progress = settings.onboarding.clone().unwrap_or_default();
    let installed = inference_available(state, &settings);
    let live = state.onboarding.live.lock().unwrap();
    Status {
        required: !progress.completed || live.startup_error.is_some(),
        step: if installed { progress.step } else { Step::Models },
        speech: live.speech.clone(),
        language: live.language.clone(),
        startup_error: live.startup_error.clone(),
        practice: PracticeStatus {
            phase: live.attempt.as_ref().map(|a| a.phase),
            attempt_id: live.attempt.as_ref().map(|a| a.id),
            job_id: live.attempt.as_ref().and_then(|a| a.job_id),
            error: live.attempt.as_ref().and_then(|a| a.error.clone()).or_else(|| live.practice_error.clone()),
        },
    }
}

pub fn notify<R: Runtime>(app: &AppHandle<R>) {
    if let Err(error) = app.emit_to("settings", "onboarding-changed", ()) {
        log::warn!("could not notify settings of setup state: {error}");
    }
}

pub fn invalidate(state: &AppState, reason: Option<&str>) {
    let mut live = state.onboarding.live.lock().unwrap();
    if let Some(attempt) = live.attempt.take() {
        if let Some(cancel) = attempt.cancel {
            cancel.cancel();
        }
        live.practice_error = reason.map(str::to_owned);
        state.controller.send(Command::Cancel);
    }
}

pub fn interrupt(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else { return };
    let active = state.onboarding.live.lock().unwrap().attempt.as_ref().is_some_and(|a| a.phase != PracticePhase::Passed);
    if active {
        invalidate(&state, Some("Practice stopped because the practice field lost focus. Focus it and try again."));
        notify(app);
    }
}

pub fn record_error(app: &AppHandle, message: &str) {
    log::warn!("setup: {message}");
    let state = app.state::<AppState>();
    let mut live = state.onboarding.live.lock().unwrap();
    if let Some(attempt) = &mut live.attempt {
        attempt.error = Some(message.to_owned());
        attempt.phase = PracticePhase::Failed;
    } else {
        live.practice_error = Some(message.to_owned());
    }
    drop(live);
    notify(app);
}

pub fn load_engines(app: &AppHandle) {
    let state = app.state::<AppState>();
    let settings_snapshot = state.settings.read().unwrap().clone();
    if engines_ready(&state, &settings_snapshot) {
        return;
    }
    invalidate(&state, Some("The model configuration changed. Run practice again after loading finishes."));
    let key = ModelKey::selected(&state.settings.read().unwrap(), &state.llm);
    let generation = {
        let mut live = state.onboarding.live.lock().unwrap();
        live.load_generation += 1;
        live.model_key = Some(key.clone());
        live.speech = if key.speech.is_some() { EngineStatus::Loading } else { EngineStatus::Missing };
        live.language = if key.language.is_some() { EngineStatus::Loading } else { EngineStatus::Missing };
        live.load_generation
    };
    notify(app);
    let (stt, llm, onboarding, settings) = (state.stt.clone(), state.llm.clone(), state.onboarding.clone(), state.settings.clone());
    let handle = app.clone();
    let result = std::thread::Builder::new().name("setup-model-loading".into()).spawn(move || {
        for speech in [true, false] {
            if ModelKey::selected(&settings.read().unwrap(), &llm) != key {
                return;
            }
            let selected = if speech { key.speech.is_some() } else { key.language.is_some() };
            if !selected {
                continue;
            }
            let result = if speech { stt.preload() } else { llm.preload() };
            let status = match result {
                Ok(()) => EngineStatus::Ready,
                Err(error) => {
                    log::warn!("model loading failed: {error}");
                    EngineStatus::Error { message: error.to_string() }
                }
            };
            let mut live = onboarding.live.lock().unwrap();
            if live.load_generation != generation {
                return;
            }
            if speech { live.speech = status } else { live.language = status };
            drop(live);
            notify(&handle);
        }
    });
    if let Err(error) = result {
        let message = format!("could not start model loading: {error}");
        log::error!("{message}");
        let mut live = state.onboarding.live.lock().unwrap();
        live.speech = EngineStatus::Error { message: message.clone() };
        live.language = EngineStatus::Error { message };
        drop(live);
        notify(app);
    }
}

pub fn claim_practice(app: &AppHandle, mode: Mode) -> Result<Option<u64>, String> {
    let state = app.state::<AppState>();
    if !required(&state) {
        return Ok(None);
    }
    let config = configuration(&state)?;
    let focused = app.get_webview_window("settings").is_some_and(|w| w.is_focused().unwrap_or(false));
    let mut live = state.onboarding.live.lock().unwrap();
    let attempt = live.attempt.as_mut().ok_or("Finish setup in Promptify and arm the practice field before using a shortcut.")?;
    if mode != Mode::Prompt || !focused || attempt.phase != PracticePhase::Armed || attempt.config != config {
        return Err("Use the configured Prompt shortcut while the armed practice field is focused.".into());
    }
    attempt.phase = PracticePhase::Recording;
    Ok(Some(attempt.id))
}

pub fn attach_job(app: &AppHandle, attempt_id: u64, job: &Job, microphone_id: Option<&str>) -> Result<(), String> {
    let state = app.state::<AppState>();
    let mut live = state.onboarding.live.lock().unwrap();
    let attempt = live.attempt.as_mut().filter(|a| a.id == attempt_id).ok_or("Practice was interrupted. Try again.")?;
    if microphone_id != Some(attempt.config.microphone_id.as_str()) {
        return Err("The default microphone changed or could not be identified. Refresh and try practice again.".into());
    }
    attempt.job_id = Some(job.id);
    attempt.cancel = Some(job.cancel_token());
    Ok(())
}

#[derive(Clone, Serialize)]
struct PracticeEvent {
    attempt_id: u64,
    event: OverlayEvent,
}

pub fn emit_practice(app: &AppHandle, attempt_id: u64, event: OverlayEvent) {
    let state = app.state::<AppState>();
    let mut live = state.onboarding.live.lock().unwrap();
    let Some(attempt) = live.attempt.as_mut().filter(|a| a.id == attempt_id) else { return };
    attempt.observe(&event);
    let changed = matches!(event, OverlayEvent::Listening { .. } | OverlayEvent::Stage { .. } | OverlayEvent::Finished { .. } | OverlayEvent::Error { .. } | OverlayEvent::Cancelled);
    drop(live);
    if let Err(error) = app.emit_to("settings", "onboarding-practice", PracticeEvent { attempt_id, event }) {
        log::warn!("could not notify settings of practice result: {error}");
    }
    if changed {
        notify(app);
    }
}

#[tauri::command]
pub fn onboarding_status(state: State<'_, AppState>) -> Status {
    snapshot(&state)
}

#[tauri::command]
pub fn retry_onboarding_startup(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let (_, error) = initialize(&state.data_dir, &state.manifest, &state.models_dir);
    if let Some(error) = error {
        return Err(error);
    }
    app.restart();
}

#[tauri::command]
pub fn retry_model_loading(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    available(&state)?;
    load_engines(&app);
    Ok(())
}

#[tauri::command]
pub fn set_onboarding_step(app: AppHandle, state: State<'_, AppState>, step: Step) -> Result<Status, String> {
    available(&state)?;
    if !required(&state) {
        return Err("Setup is already complete.".into());
    }
    let current = state.settings.read().unwrap().onboarding.as_ref().map(|p| p.step).unwrap_or_default();
    if current == Step::Models && step == Step::Practice {
        return Err("Check your microphone and shortcut before practice.".into());
    }
    if step >= Step::Input {
        require_models(&state)?;
    }
    if step == Step::Practice {
        configuration(&state)?;
    }
    settings::update(&state.settings, &state.data_dir, |s| {
        s.onboarding.as_mut().expect("initialized setup").step = step;
    })?;
    invalidate(&state, None);
    notify(&app);
    Ok(snapshot(&state))
}

#[tauri::command]
pub fn arm_onboarding_practice(app: AppHandle, state: State<'_, AppState>) -> Result<Status, String> {
    if !required(&state) || snapshot(&state).step != Step::Practice {
        return Err("Open the required practice step first.".into());
    }
    if !app.get_webview_window("settings").is_some_and(|w| w.is_focused().unwrap_or(false)) {
        return Err("Focus the practice field in Promptify first.".into());
    }
    let config = configuration(&state)?;
    invalidate(&state, None);
    let mut live = state.onboarding.live.lock().unwrap();
    live.next_attempt += 1;
    live.practice_error = None;
    live.attempt = Some(Attempt {
        id: live.next_attempt,
        config,
        phase: PracticePhase::Armed,
        job_id: None,
        cancel: None,
        heard_speech: false,
        generated: false,
        inserted: None,
        error: None,
    });
    drop(live);
    notify(&app);
    Ok(snapshot(&state))
}

#[tauri::command]
pub fn disarm_onboarding_practice(app: AppHandle) {
    interrupt(&app);
}

#[tauri::command]
pub fn confirm_onboarding_paste(app: AppHandle, state: State<'_, AppState>, attempt_id: u64, job_id: u64, text: String) -> Result<Status, String> {
    if text.len() > 32_000 {
        return Err("The practice paste is too large.".into());
    }
    let config = configuration(&state)?;
    let mut live = state.onboarding.live.lock().unwrap();
    let attempt = live.attempt.as_mut().filter(|a| a.id == attempt_id && a.config == config).ok_or("This practice attempt is no longer current. Try again.")?;
    attempt.acknowledge(job_id, &text)?;
    drop(live);
    notify(&app);
    Ok(snapshot(&state))
}

#[tauri::command]
pub fn complete_onboarding(app: AppHandle, state: State<'_, AppState>) -> Result<Status, String> {
    let config = configuration(&state)?;
    let mut live = state.onboarding.live.lock().unwrap();
    if !live.attempt.as_ref().is_some_and(|a| a.verified_for(&config)) {
        return Err("Complete a successful voice-to-prompt practice and verify its paste first.".into());
    }
    settings::update(&state.settings, &state.data_dir, |s| {
        s.onboarding = Some(Progress { version: VERSION, step: Step::Practice, completed: true });
    })?;
    live.attempt = None;
    live.practice_error = None;
    drop(live);
    crate::tray::refresh(&app);
    notify(&app);
    Ok(snapshot(&state))
}

#[tauri::command]
pub fn resume_onboarding_hotkeys(app: AppHandle, state: State<'_, AppState>) -> Result<Status, String> {
    available(&state)?;
    crate::hotkeys::set_paused(&app, false);
    crate::tray::refresh(&app);
    notify(&app);
    if let Some(error) = &state.hotkeys.read().unwrap().prompt_error {
        return Err(error.clone());
    }
    Ok(snapshot(&state))
}

#[cfg(test)]
mod tests {
    use super::*;
    use promptify_core::pipeline::JobReport;

    fn attempt() -> Attempt {
        Attempt {
            id: 1,
            config: Configuration {
                models: ModelKey::from(&AppSettings::default()),
                shortcut: "Control+Alt+Space".into(),
                microphone_id: "test-device-1".into(),
            },
            phase: PracticePhase::Recording,
            job_id: Some(7),
            cancel: None,
            heard_speech: false,
            generated: false,
            inserted: None,
            error: None,
        }
    }

    fn finished(job_id: u64, outcome: Outcome) -> OverlayEvent {
        OverlayEvent::Finished {
            report: JobReport { job_id, profile_id: "generic".into(), outcome, elapsed_ms: 1, history_saved: false, structure: None, routing: None, delivery: None },
            capped: false,
        }
    }

    #[test]
    fn migration_only_exempts_configured_legacy_profiles() {
        for (existed, configured, completed) in [(false, false, false), (false, true, false), (true, false, false), (true, true, true)] {
            let mut settings = AppSettings { history_enabled: false, ..Default::default() };
            migrate(&mut settings, existed, configured).unwrap();
            assert_eq!(settings.onboarding.unwrap().completed, completed);
            assert!(!settings.history_enabled);
        }
    }

    #[test]
    fn interrupted_and_completed_progress_are_not_reclassified() {
        for completed in [false, true] {
            let progress = Progress { step: Step::Practice, completed, ..Default::default() };
            let mut settings = AppSettings { onboarding: Some(progress.clone()), ..Default::default() };
            migrate(&mut settings, true, !completed).unwrap();
            assert_eq!(settings.onboarding, Some(progress));
        }
    }

    #[test]
    fn unknown_progress_versions_require_recovery() {
        let mut settings = AppSettings { onboarding: Some(Progress { version: VERSION + 1, ..Default::default() }), ..Default::default() };
        assert!(migrate(&mut settings, true, true).is_err());
    }

    #[test]
    fn practice_requires_speech_generation_and_matching_native_paste() {
        let mut attempt = attempt();
        assert!(attempt.acknowledge(7, "typed text").is_err());
        attempt.observe(&OverlayEvent::Transcript { text: "write a greeting".into() });
        attempt.observe(&OverlayEvent::Stage { stage: Stage::Generating });
        attempt.observe(&finished(7, Outcome::Inserted { text: "Write a greeting.\nKeep it brief.".into() }));
        assert_eq!(attempt.phase, PracticePhase::AwaitingPaste);
        assert!(attempt.acknowledge(8, "Write a greeting.\nKeep it brief.").is_err());
        assert!(attempt.acknowledge(7, "manually edited").is_err());
        attempt.acknowledge(7, "Write a greeting.\r\nKeep it brief.").unwrap();
        assert_eq!(attempt.phase, PracticePhase::Passed);
    }

    #[test]
    fn dictation_and_stale_jobs_cannot_satisfy_practice() {
        let mut dictation = attempt();
        dictation.observe(&OverlayEvent::Transcript { text: "hello".into() });
        dictation.observe(&finished(7, Outcome::Inserted { text: "hello".into() }));
        assert_eq!(dictation.phase, PracticePhase::Failed);
        let mut stale = attempt();
        stale.heard_speech = true;
        stale.generated = true;
        stale.observe(&finished(8, Outcome::Inserted { text: "a generated prompt".into() }));
        assert_eq!(stale.phase, PracticePhase::Failed);
    }

    #[test]
    fn cancellation_silence_and_blocked_insertion_cannot_complete() {
        for outcome in [
            Outcome::Cancelled,
            Outcome::NoSpeech,
            Outcome::Blocked { text: "text".into(), reason: promptify_core::pipeline::BlockReason::FocusChanged, detail: None },
        ] {
            let mut attempt = attempt();
            attempt.heard_speech = true;
            attempt.generated = true;
            attempt.observe(&finished(7, outcome));
            assert_eq!(attempt.phase, PracticePhase::Failed);
            assert!(attempt.error.is_some());
            assert!(attempt.acknowledge(7, "text").is_err());
        }
    }

    #[test]
    fn ready_status_is_bound_to_the_loaded_configuration() {
        let settings = AppSettings::default();
        let onboarding = Onboarding::default();
        assert!(!onboarding.ready(&settings));
        {
            let mut live = onboarding.live.lock().unwrap();
            live.model_key = Some(ModelKey::from(&settings));
            live.speech = EngineStatus::Ready;
            live.language = EngineStatus::Ready;
        }
        assert!(onboarding.ready(&settings));
        assert!(!onboarding.ready(&AppSettings { use_gpu: !settings.use_gpu, ..settings }));
    }

    #[test]
    fn external_inference_identity_invalidates_stale_readiness() {
        let settings = AppSettings::default();
        let onboarding = Onboarding::default();
        let key = ModelKey {
            speech: Some("speech".into()),
            language: Some("server-model".into()),
            gpu: settings.use_gpu,
            inference: Some("lm-studio:1:server-model".into()),
        };
        {
            let mut live = onboarding.live.lock().unwrap();
            live.model_key = Some(key.clone());
            live.speech = EngineStatus::Ready;
            live.language = EngineStatus::Ready;
        }
        assert!(onboarding.ready_key(&key));
        let mut changed = key.clone();
        changed.inference = Some("lm-studio:2:server-model".into());
        assert!(!onboarding.ready_key(&changed));
        changed = key;
        changed.language = Some("another-model".into());
        assert!(!onboarding.ready_key(&changed));
    }

    #[test]
    fn untested_inference_shortcut_recovery_opens_inference_not_models() {
        use crate::commands::RecoverySection;
        use crate::inference::{InferenceSelection, InferenceState, InferenceStatus};
        let mut status = InferenceStatus {
            state: InferenceState::Configured,
            selection: InferenceSelection::Connection {
                connection_id: "maestro".into(), model: "text-model".into(),
            },
            revision: 1,
            message: Some("Configured but not tested.".into()),
        };
        let (section, message) = engine_recovery(status.clone(), Some("Generic loading error".into()));
        assert!(matches!(section, RecoverySection::Inference));
        assert!(message.contains("click Test"));
        assert!(message.contains("Configured but not tested."));
        status.state = InferenceState::Error;
        status.message = Some("API credential missing.".into());
        let (section, message) = engine_recovery(status.clone(), None);
        assert!(matches!(section, RecoverySection::Inference));
        assert!(message.contains("API credential missing."));
        status.state = InferenceState::Ready;
        let (section, message) = engine_recovery(status.clone(), Some("Speech failed to load.".into()));
        assert!(matches!(section, RecoverySection::Models));
        assert_eq!(message, "Speech failed to load.");
        status.selection = InferenceSelection::BundledLocal;
        status.state = InferenceState::Error;
        let (section, message) = engine_recovery(status, None);
        assert!(matches!(section, RecoverySection::Models));
        assert!(message.contains("Settings > Models"));
    }

    #[test]
    fn completed_practice_is_invalid_after_any_required_configuration_changes() {
        let mut attempt = attempt();
        assert!(!attempt.verified_for(&attempt.config));
        attempt.phase = PracticePhase::Passed;
        assert!(attempt.verified_for(&attempt.config));
        let mut changed = attempt.config.clone();
        changed.microphone_id = "test-device-2-with-the-same-display-name".into();
        assert!(!attempt.verified_for(&changed));
        changed = attempt.config.clone();
        changed.models.gpu = !changed.models.gpu;
        assert!(!attempt.verified_for(&changed));
        changed = attempt.config.clone();
        changed.shortcut = "Control+Alt+P".into();
        assert!(!attempt.verified_for(&changed));
        changed = attempt.config.clone();
        changed.models.language = Some("a-different-model".into());
        assert!(!attempt.verified_for(&changed));
    }
}

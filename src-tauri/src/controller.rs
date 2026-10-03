use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use promptify_core::hotkey::{Gesture, GestureAction};
use promptify_core::live::{ChunkPolicy, LiveTranscript, next_cut};
use promptify_core::pipeline::{BeginError, CancelToken, Job, JobEvent, JobOptions, JobReport, Mode, Orchestrator, Outcome, Stage};
use promptify_core::transform::TransformService;
use promptify_core::routing::ResolvedPromptPolicy;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::audio::{LiveAudio, Recording};

pub enum Command {
    Press(Mode),
    Release(Mode),
    HoldPress(Mode),
    HoldRelease(Mode),
    Cancel,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OverlayEvent {
    Listening { mode: Mode, profile: String, target: String, latched: bool },
    Level { level: f32 },
    /// Speech transcribed so far while still recording.
    Partial { text: String },
    Stage { stage: Stage },
    Transcript { text: String },
    Token { text: String },
    Routing { routing: ResolvedPromptPolicy },
    Finished { report: JobReport, capped: bool },
    Error { message: String },
    Cancelled,
}

pub fn emit(app: &AppHandle, event: OverlayEvent) {
    let status = match &event {
        OverlayEvent::Listening { .. } => Some("listening\u{2026}"),
        OverlayEvent::Stage { stage: Stage::Transcribing } => Some("transcribing\u{2026}"),
        OverlayEvent::Stage { stage: Stage::Generating } => Some("writing prompt\u{2026}"),
        OverlayEvent::Stage { stage: Stage::Researching } => Some("looking up context\u{2026}"),
        OverlayEvent::Stage { stage: Stage::Revising } => Some("revising prompt\u{2026}"),
        OverlayEvent::Stage { stage: Stage::Inserting } => Some("pasting\u{2026}"),
        OverlayEvent::Finished { .. } | OverlayEvent::Error { .. } | OverlayEvent::Cancelled => Some("ready"),
        OverlayEvent::Level { .. } | OverlayEvent::Partial { .. } | OverlayEvent::Transcript { .. } | OverlayEvent::Token { .. } | OverlayEvent::Routing { .. } => None,
    };
    if let Some(status) = status {
        crate::tray::set_status(app, status);
    }
    let _ = app.emit_to("overlay", "overlay-event", event);
}

fn emit_for(app: &AppHandle, practice: Option<u64>, event: OverlayEvent) {
    if let Some(attempt) = practice {
        crate::onboarding::emit_practice(app, attempt, event);
    } else {
        emit(app, event);
    }
}

enum Phase {
    Idle,
    Recording { job: Box<Job>, recording: Recording, live: LiveLoop, practice: Option<u64> },
    Processing { cancel: CancelToken, practice: Option<u64> },
}

/// How often the live loop looks for a finished chunk, and how long a chunk may wait for the engines.
const LIVE_POLL: Duration = Duration::from_millis(200);
const LIVE_ENGINE_WAIT: Duration = Duration::from_millis(150);

/// Transcribes finished chunks in the background while the user is still speaking.
struct LiveLoop {
    stop: Arc<AtomicBool>,
    thread: JoinHandle<LiveTranscript>,
}

impl LiveLoop {
    fn spawn(app: AppHandle, service: Arc<TransformService>, audio: LiveAudio, cancel: CancelToken, practice: Option<u64>) -> std::io::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::Builder::new().name("live-transcription".into()).spawn(move || {
            let policy = ChunkPolicy::default();
            let mut live = LiveTranscript::default();
            while !flag.load(Ordering::SeqCst) && !cancel.is_cancelled() {
                std::thread::sleep(LIVE_POLL);
                let tail = audio.copy_from(live.committed_samples());
                let Some(cut) = next_cut(&tail, &policy) else { continue };
                // A chunk that could not run now is transcribed with the rest at the end.
                if let Some(text) = service.transcribe_chunk(&tail[..cut], &cancel, LIVE_ENGINE_WAIT) {
                    live.commit(cut, &text);
                    emit_for(&app, practice, OverlayEvent::Partial { text: live.text() });
                }
            }
            live
        })?;
        Ok(Self { stop, thread })
    }

    /// Stops looking for new chunks; a chunk already being transcribed still completes.
    fn finish(self) -> LiveTranscript {
        self.stop.store(true, Ordering::SeqCst);
        self.thread.join().unwrap_or_default()
    }
}

/// Serializes hotkey commands on one worker thread; processing runs on its own thread so
/// cancellation stays responsive.
pub struct Controller {
    tx: Mutex<mpsc::Sender<Command>>,
    pub last_result: Arc<Mutex<Option<String>>>,
}

impl Controller {
    pub fn spawn(app: AppHandle, orchestrator: Arc<Orchestrator>, on_active: impl Fn(bool) + Send + Sync + 'static) -> Self {
        let (tx, rx) = mpsc::channel();
        let last_result = Arc::new(Mutex::new(None));
        let mut worker = Worker {
            app,
            orchestrator,
            phase: Arc::new(Mutex::new(Phase::Idle)),
            gesture: Gesture::default(),
            last_result: last_result.clone(),
            on_active: Arc::new(on_active),
        };
        std::thread::Builder::new()
            .name("hotkey-controller".into())
            .spawn(move || {
                for command in rx {
                    worker.handle(command);
                }
            })
            .expect("spawn controller thread");
        Self { tx: Mutex::new(tx), last_result }
    }

    pub fn send(&self, command: Command) {
        let _ = self.tx.lock().unwrap().send(command);
    }
}

struct Worker {
    app: AppHandle,
    orchestrator: Arc<Orchestrator>,
    phase: Arc<Mutex<Phase>>,
    gesture: Gesture,
    last_result: Arc<Mutex<Option<String>>>,
    on_active: Arc<dyn Fn(bool) + Send + Sync>,
}

impl Worker {
    fn handle(&mut self, command: Command) {
        let now = Instant::now();
        match command {
            Command::HoldPress(mode) => {
                if let GestureAction::Start(mode) = self.gesture.hold_press(mode) {
                    self.start(mode);
                }
            }
            Command::HoldRelease(mode) => {
                if self.gesture.hold_release(mode) == GestureAction::Stop {
                    self.stop();
                }
            }
            Command::Press(mode) => match self.gesture.press(mode, now) {
                GestureAction::Start(mode) => self.start(mode),
                GestureAction::Stop => self.stop(),
                GestureAction::Latch | GestureAction::Ignore => {}
            },
            Command::Release(mode) => match self.gesture.release(mode, now) {
                GestureAction::Stop => self.stop(),
                GestureAction::Latch => self.relisten(true),
                GestureAction::Start(_) | GestureAction::Ignore => {}
            },
            Command::Cancel => self.cancel(),
        }
    }

    fn start(&mut self, mode: Mode) {
        let mut phase = self.phase.lock().unwrap();
        if !matches!(*phase, Phase::Idle) {
            self.gesture.reset();
            return;
        }
        #[cfg(target_os = "linux")]
        if mode != Mode::Answer && let Err(message) = crate::wayland_paste::require_ready() {
            self.gesture.reset();
            if crate::onboarding::required(&self.app.state::<crate::AppState>()) {
                crate::onboarding::record_error(&self.app, &message);
                crate::tray::show_settings(&self.app);
            } else {
                self.show_overlay(None);
                emit(&self.app, OverlayEvent::Error { message });
            }
            return;
        }
        let practice = match crate::onboarding::claim_practice(&self.app, mode) {
            Ok(practice) => practice,
            Err(message) => {
                self.gesture.reset();
                crate::onboarding::record_error(&self.app, &message);
                crate::tray::show_settings(&self.app);
                return;
            }
        };
        let started = if practice.is_some() {
            self.orchestrator.begin_with_options(mode, JobOptions { use_personal_context: false, use_tools: false, auto_mode: false })
        } else {
            self.orchestrator.begin(mode)
        };
        let job = match started {
            Ok(job) => job,
            Err(BeginError::Busy) => {
                self.gesture.reset();
                if practice.is_some() {
                    emit_for(&self.app, practice, OverlayEvent::Error { message: "The engines are still busy. Try practice again.".into() });
                }
                return;
            }
            Err(err) => {
                self.gesture.reset();
                self.show_overlay(practice);
                emit_for(&self.app, practice, OverlayEvent::Error { message: err.to_string() });
                return;
            }
        };
        *self.last_result.lock().unwrap() = None;
        let app = self.app.clone();
        let last_emit = Mutex::new(Instant::now() - Duration::from_secs(1));
        let on_level = move |level: f32| {
            let mut last = last_emit.lock().unwrap();
            if last.elapsed() >= Duration::from_millis(50) {
                *last = Instant::now();
                emit_for(&app, practice, OverlayEvent::Level { level });
            }
        };
        let recording = match Recording::start(self.orchestrator.limits().max_audio_samples, on_level) {
            Ok(recording) => recording,
            Err(message) => {
                drop(job);
                self.gesture.reset();
                self.show_overlay(practice);
                emit_for(&self.app, practice, OverlayEvent::Error { message });
                return;
            }
        };
        if let Some(attempt) = practice
            && let Err(message) = crate::onboarding::attach_job(&self.app, attempt, &job, recording.device_id.as_deref())
        {
            job.cancel_token().cancel();
            if let Err(error) = recording.stop() {
                log::warn!("could not stop interrupted practice capture: {error}");
            }
            self.gesture.reset();
            emit_for(&self.app, practice, OverlayEvent::Error { message });
            return;
        }
        (self.on_active)(true);
        self.show_overlay(practice);
        emit_for(&self.app, practice, listening(&self.orchestrator, &job, false));
        let live = match LiveLoop::spawn(self.app.clone(), self.orchestrator.service().clone(), recording.live(), job.cancel_token(), practice) {
            Ok(live) => live,
            Err(e) => {
                let _ = recording.stop();
                drop(job);
                self.gesture.reset();
                (self.on_active)(false);
                emit_for(&self.app, practice, OverlayEvent::Error { message: format!("could not start live transcription: {e}") });
                return;
            }
        };
        *phase = Phase::Recording { job: Box::new(job), recording, live, practice };
    }

    fn relisten(&self, latched: bool) {
        if let Phase::Recording { job, practice, .. } = &*self.phase.lock().unwrap() {
            emit_for(&self.app, *practice, listening(&self.orchestrator, job, latched));
        }
    }

    fn stop(&mut self) {
        let mut phase = self.phase.lock().unwrap();
        let Phase::Recording { job, recording, live, practice } = std::mem::replace(&mut *phase, Phase::Idle) else {
            return;
        };
        let audio = match recording.stop() {
            Ok(audio) => audio,
            Err(message) => {
                job.cancel_token().cancel();
                let _ = live.finish();
                drop(job);
                (self.on_active)(false);
                emit_for(&self.app, practice, OverlayEvent::Error { message });
                return;
            }
        };
        *phase = Phase::Processing { cancel: job.cancel_token(), practice };
        drop(phase);

        let app = self.app.clone();
        let orchestrator = self.orchestrator.clone();
        let phase = self.phase.clone();
        let last_result = self.last_result.clone();
        let on_active = self.on_active.clone();
        std::thread::Builder::new()
            .name("job-processing".into())
            .spawn(move || {
                let live = live.finish();
                let tail = audio.samples.get(live.committed_samples()..).unwrap_or_default();
                let report = orchestrator.finish_live(*job, &live.text(), tail, &mut |event| {
                    let payload = match event {
                        JobEvent::Stage(stage) => OverlayEvent::Stage { stage },
                        JobEvent::Transcript(text) => OverlayEvent::Transcript { text: text.to_owned() },
                        JobEvent::Token(text) => OverlayEvent::Token { text: text.to_owned() },
                        JobEvent::Routing(routing) => OverlayEvent::Routing { routing: routing.clone() },
                    };
                    emit_for(&app, practice, payload);
                });
                log::info!(
                    "job {} profile={} outcome={} elapsed_ms={} history_saved={}",
                    report.job_id,
                    report.profile_id,
                    report.outcome.kind(),
                    report.elapsed_ms,
                    report.history_saved
                );
                if let Outcome::Blocked { text, .. } | Outcome::Answered { text } = &report.outcome {
                    *last_result.lock().unwrap() = Some(text.clone());
                }
                if report.history_saved {
                    let _ = app.emit_to("settings", "history-changed", ());
                }
                *phase.lock().unwrap() = Phase::Idle;
                on_active(false);
                emit_for(&app, practice, OverlayEvent::Finished { report, capped: audio.capped });
            })
            .expect("spawn processing thread");
    }

    fn cancel(&mut self) {
        let mut phase = self.phase.lock().unwrap();
        match std::mem::replace(&mut *phase, Phase::Idle) {
            Phase::Recording { job, recording, live, practice } => {
                job.cancel_token().cancel();
                let _ = recording.stop();
                drop(live);
                drop(job);
                self.gesture.reset();
                (self.on_active)(false);
                emit_for(&self.app, practice, OverlayEvent::Cancelled);
            }
            Phase::Processing { cancel, practice } => {
                cancel.cancel();
                *phase = Phase::Processing { cancel, practice };
            }
            Phase::Idle => {}
        }
    }

    fn show_overlay(&self, practice: Option<u64>) {
        if practice.is_none() && let Some(overlay) = self.app.get_webview_window("overlay") {
            let _ = overlay.show();
        }
    }
}

fn listening(orchestrator: &Orchestrator, job: &Job, latched: bool) -> OverlayEvent {
    let profile = orchestrator.profiles().get(&job.profile_id).map_or_else(|| job.profile_id.clone(), |p| p.name.clone());
    let target = job.target.url_host().unwrap_or_else(|| job.target.normalized_process());
    OverlayEvent::Listening { mode: job.mode, profile, target, latched }
}

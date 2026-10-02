use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::context::{ActiveContext, AdmittedText, ContextPolicy, FocusedText, WindowIdentity};
use crate::history::{HistoryContext, HistoryLog, NewHistoryEntry};
use crate::profiles::{PasteChord, Profile, ProfileSet};
use crate::prompt::ChatMessage;
use crate::scheduler::{AdmitError, Priority, SchedulerLimits};
use crate::transform::{Input, Transform, TransformOutcome, TransformService};

#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Prompt,
    Dictation,
    /// Answer the spoken question with the local model and show it; nothing is pasted.
    Answer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Transcribing,
    /// Fetching reference text with the user's MCP tools.
    Researching,
    Generating,
    /// Rewriting a draft whose task graph was malformed.
    Revising,
    Inserting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobEvent<'a> {
    Stage(Stage),
    Transcript(&'a str),
    Token(&'a str),
}

#[derive(Debug, Clone)]
pub struct Limits {
    pub max_audio_samples: usize,
    pub max_new_tokens: u32,
    pub generation_timeout: Duration,
    pub max_output_chars: usize,
    /// Extra generations allowed to fix a malformed task graph; all share `generation_timeout`.
    pub max_structure_repairs: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_audio_samples: crate::audio::TARGET_SAMPLE_RATE as usize * 120,
            max_new_tokens: 768,
            generation_timeout: Duration::from_secs(60),
            max_output_chars: 6000,
            max_structure_repairs: 1,
        }
    }
}

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
#[error("{0}")]
pub struct BackendError(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinishReason {
    Stop,
    /// Hit the token limit; the text is incomplete.
    Length,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Generation {
    pub text: String,
    pub finish: FinishReason,
}

pub struct GenerationRequest<'a> {
    pub messages: &'a [ChatMessage],
    /// Leading messages that repeat across requests for this profile; backends may cache them.
    pub stable_prefix: usize,
    pub max_new_tokens: u32,
    pub deadline: Instant,
}

pub trait ContextProvider: Send + Sync {
    /// Identity of the foreground window, without reading any of its text.
    fn identify(&self) -> Result<ActiveContext, BackendError>;
    /// Text of the focused element. Only called for apps the user opted in.
    fn focused_text(&self, window: &WindowIdentity) -> Result<Option<FocusedText>, BackendError>;
    fn foreground(&self) -> Result<WindowIdentity, BackendError>;
}

pub trait Transcriber: Send + Sync {
    fn transcribe(&self, audio: &[f32], cancel: &CancelToken) -> Result<String, BackendError>;
}

pub trait Generator: Send + Sync {
    fn generate(
        &self,
        request: &GenerationRequest<'_>,
        cancel: &CancelToken,
        on_token: &mut dyn FnMut(&str),
    ) -> Result<Generation, BackendError>;
}

pub trait Inserter: Send + Sync {
    fn insert(&self, target: &WindowIdentity, text: &str, chord: PasteChord) -> Result<(), BackendError>;
}

pub trait History: Send + Sync {
    fn context(&self, profile_id: &str, app_key: &str) -> HistoryContext;
    /// Returns false when history is turned off.
    fn record(&self, entry: NewHistoryEntry) -> Result<bool, BackendError>;
}

impl History for HistoryLog {
    fn context(&self, profile_id: &str, app_key: &str) -> HistoryContext {
        HistoryLog::context(self, profile_id, app_key)
    }

    fn record(&self, entry: NewHistoryEntry) -> Result<bool, BackendError> {
        HistoryLog::record(self, entry).map(|saved| saved.is_some()).map_err(|e| BackendError(format!("history write failed: {e}")))
    }
}

#[derive(Clone)]
pub struct Backends {
    pub context: Arc<dyn ContextProvider>,
    pub transcriber: Arc<dyn Transcriber>,
    pub generator: Arc<dyn Generator>,
    pub inserter: Arc<dyn Inserter>,
    pub history: Arc<dyn History>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BeginError {
    #[error("a job is already running")]
    Busy,
    #[error("could not read the active window: {0}")]
    Context(BackendError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockReason {
    FocusChanged,
    FocusUnknown,
    OutputTruncated,
    InsertFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailReason {
    RecordingTooLong,
    TranscriptionFailed,
    GenerationFailed,
    TimedOut,
    EmptyOutput,
    /// The engines stayed busy with other clients' requests past the wait limit.
    EngineBusy,
}

/// Terminal state of a job. `Blocked` carries the text so the UI can offer a copy button.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Outcome {
    Inserted { text: String },
    /// An answer to show the user; answer jobs never paste.
    Answered { text: String },
    Blocked { text: String, reason: BlockReason, detail: Option<String> },
    NoSpeech,
    Cancelled,
    Failed { reason: FailReason, detail: Option<String> },
}

impl Outcome {
    /// Content-free label for logs.
    pub fn kind(&self) -> &'static str {
        match self {
            Outcome::Inserted { .. } => "inserted",
            Outcome::Answered { .. } => "answered",
            Outcome::Blocked { .. } => "blocked",
            Outcome::NoSpeech => "no_speech",
            Outcome::Cancelled => "cancelled",
            Outcome::Failed { .. } => "failed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StructureCheck {
    Valid,
    /// The first draft was not a well-formed task graph with a loop, and a repair passed validation.
    Repaired,
    /// No repair passed validation; the first draft was kept.
    KeptOriginal,
}

#[derive(Debug, Clone, Serialize)]
pub struct JobReport {
    pub job_id: u64,
    pub profile_id: String,
    pub outcome: Outcome,
    pub elapsed_ms: u64,
    pub history_saved: bool,
    pub structure: Option<StructureCheck>,
}

struct BusyGuard(Arc<AtomicBool>);

impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// A job started at hotkey press. Dropping it releases the single-job slot.
pub struct Job {
    pub id: u64,
    pub mode: Mode,
    pub target: ActiveContext,
    pub profile_id: String,
    surrounding: Option<AdmittedText>,
    cancel: CancelToken,
    _busy: BusyGuard,
}

impl Job {
    pub fn cancel_token(&self) -> CancelToken {
        self.cancel.clone()
    }

    pub fn has_surrounding_text(&self) -> bool {
        self.surrounding.is_some()
    }
}

pub struct Orchestrator {
    context: Arc<dyn ContextProvider>,
    inserter: Arc<dyn Inserter>,
    service: Arc<TransformService>,
    policy: std::sync::RwLock<ContextPolicy>,
    busy: Arc<AtomicBool>,
    next_id: AtomicU64,
    auto_mode: AtomicBool,
}

impl Orchestrator {
    pub fn new(backends: Backends, profiles: ProfileSet, policy: ContextPolicy, limits: Limits) -> Self {
        Self::with_scheduler(backends, profiles, policy, limits, SchedulerLimits::default())
    }

    pub fn with_scheduler(backends: Backends, profiles: ProfileSet, policy: ContextPolicy, limits: Limits, scheduler: SchedulerLimits) -> Self {
        let service = TransformService::new(backends.transcriber, backends.generator, backends.history, profiles, limits, scheduler);
        Self {
            context: backends.context,
            inserter: backends.inserter,
            service: Arc::new(service),
            policy: std::sync::RwLock::new(policy),
            busy: Arc::default(),
            next_id: AtomicU64::new(1),
            auto_mode: AtomicBool::new(false),
        }
    }

    /// With automatic mode on, the prompt hotkey writes plain dictation outside AI apps.
    pub fn set_auto_mode(&self, enabled: bool) {
        self.auto_mode.store(enabled, Ordering::SeqCst);
    }

    /// Which apps may share their on-screen text; applies from the next hotkey press.
    pub fn set_policy(&self, policy: ContextPolicy) {
        *self.policy.write().unwrap_or_else(|p| p.into_inner()) = policy;
    }

    /// The engine-side service, shared with the local API and remote devices.
    pub fn service(&self) -> &Arc<TransformService> {
        &self.service
    }

    pub fn limits(&self) -> &Limits {
        self.service.limits()
    }

    pub fn profiles(&self) -> &ProfileSet {
        self.service.profiles()
    }

    /// Captures the target window (and opted-in surrounding text) at hotkey press.
    pub fn begin(&self, mode: Mode) -> Result<Job, BeginError> {
        if self.busy.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
            return Err(BeginError::Busy);
        }
        let busy = BusyGuard(self.busy.clone());
        let target = self.context.identify().map_err(BeginError::Context)?;
        let profile_id = self.profiles().resolve(&target).id.clone();
        let policy = self.policy.read().unwrap_or_else(|p| p.into_inner()).clone();
        let surrounding = if mode != Mode::Dictation && policy.allows_surrounding_text(&target) {
            // Surrounding text is optional context; a read failure must not block the job.
            match self.context.focused_text(&target.window) {
                Ok(Some(focused)) => policy.admit(&target, focused),
                Ok(None) | Err(_) => None,
            }
        } else {
            None
        };
        Ok(Job {
            id: self.next_id.fetch_add(1, Ordering::SeqCst),
            mode,
            target,
            profile_id,
            surrounding,
            cancel: CancelToken::default(),
            _busy: busy,
        })
    }

    pub fn finish(&self, job: Job, audio: &[f32], on_event: &mut dyn FnMut(JobEvent<'_>)) -> JobReport {
        self.finish_input(job, Input::Audio(audio), on_event)
    }

    /// Finishes a recording whose start was already transcribed while the user spoke.
    pub fn finish_live(&self, job: Job, committed: &str, tail: &[f32], on_event: &mut dyn FnMut(JobEvent<'_>)) -> JobReport {
        self.finish_input(job, Input::Live { committed, tail }, on_event)
    }

    fn finish_input(&self, job: Job, input: Input<'_>, on_event: &mut dyn FnMut(JobEvent<'_>)) -> JobReport {
        let started = Instant::now();
        let profiles = self.profiles();
        let profile = profiles.get(&job.profile_id).unwrap_or_else(|| profiles.resolve(&job.target));
        let transform = Transform {
            input,
            mode: job.mode,
            profile,
            target: &job.target,
            surrounding: job.surrounding.as_ref(),
            use_history: true,
            use_tools: true,
            auto_mode: self.auto_mode.load(Ordering::SeqCst),
        };
        // Local jobs go first, but may still wait for a remote job that already holds the engines.
        let queue_wait = self.limits().generation_timeout;
        let report = self.service.run_scheduled(Priority::Local, LOCAL_CLIENT, &transform, &job.cancel, queue_wait, on_event);
        let (outcome, transcript, structure, mode) = match report {
            Ok(report) => {
                let outcome = match report.outcome {
                    // Answers are only shown; they never reach the target app.
                    TransformOutcome::Ready { text } | TransformOutcome::Truncated { text } if report.mode == Mode::Answer => Outcome::Answered { text },
                    TransformOutcome::Ready { text } => self.insert(&job, profile, text, on_event),
                    TransformOutcome::Truncated { text } => Outcome::Blocked { text, reason: BlockReason::OutputTruncated, detail: None },
                    TransformOutcome::NoSpeech => Outcome::NoSpeech,
                    TransformOutcome::Cancelled => Outcome::Cancelled,
                    TransformOutcome::Failed { reason, detail } => Outcome::Failed { reason, detail },
                };
                (outcome, report.transcript, report.structure, report.mode)
            }
            Err(AdmitError::Cancelled) => (Outcome::Cancelled, None, None, job.mode),
            Err(error) => (Outcome::Failed { reason: FailReason::EngineBusy, detail: Some(error.to_string()) }, None, None, job.mode),
        };
        let history_saved = match (&outcome, transcript) {
            (Outcome::Inserted { text }, Some(transcript)) => self.record(&job, mode, transcript, text, true),
            (Outcome::Blocked { text, .. }, Some(transcript)) => self.record(&job, mode, transcript, text, false),
            _ => false,
        };
        JobReport {
            job_id: job.id,
            profile_id: job.profile_id.clone(),
            outcome,
            elapsed_ms: started.elapsed().as_millis() as u64,
            history_saved,
            structure,
        }
    }

    fn record(&self, job: &Job, mode: Mode, transcript: String, output: &str, inserted: bool) -> bool {
        let entry = NewHistoryEntry {
            mode,
            profile_id: job.profile_id.clone(),
            app_key: job.target.app_key(),
            transcript,
            output: output.to_owned(),
            inserted,
        };
        // A history failure must never change the job outcome; the report carries the receipt.
        self.service.history().record(entry).unwrap_or(false)
    }

    fn insert(&self, job: &Job, profile: &Profile, text: String, on_event: &mut dyn FnMut(JobEvent<'_>)) -> Outcome {
        on_event(JobEvent::Stage(Stage::Inserting));
        match self.context.foreground() {
            Ok(window) if window == job.target.window => {}
            Ok(_) => return Outcome::Blocked { text, reason: BlockReason::FocusChanged, detail: None },
            Err(err) => return Outcome::Blocked { text, reason: BlockReason::FocusUnknown, detail: Some(err.0) },
        }
        // Cancellation outranks insertion; this is the last check before the paste effect.
        if job.cancel.is_cancelled() {
            return Outcome::Cancelled;
        }
        match self.inserter.insert(&job.target.window, &text, profile.paste) {
            Ok(()) => Outcome::Inserted { text },
            Err(err) => Outcome::Blocked { text, reason: BlockReason::InsertFailed, detail: Some(err.0) },
        }
    }
}

pub const LOCAL_CLIENT: &str = "local";

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::AtomicUsize;

    use super::*;
    use crate::prompt::Role;

    const TARGET: WindowIdentity = WindowIdentity { handle: 42, process_id: 7 };

    #[derive(Default)]
    struct FakeContext {
        ctx: ActiveContext,
        focused: Option<FocusedText>,
        foreground: Mutex<Option<WindowIdentity>>,
        focused_calls: AtomicUsize,
        cancel_on_foreground: Mutex<Option<CancelToken>>,
    }

    impl ContextProvider for FakeContext {
        fn identify(&self) -> Result<ActiveContext, BackendError> {
            Ok(self.ctx.clone())
        }
        fn focused_text(&self, _: &WindowIdentity) -> Result<Option<FocusedText>, BackendError> {
            self.focused_calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.focused.clone())
        }
        fn foreground(&self) -> Result<WindowIdentity, BackendError> {
            if let Some(token) = self.cancel_on_foreground.lock().unwrap().as_ref() {
                token.cancel();
            }
            self.foreground.lock().unwrap().ok_or_else(|| BackendError("no foreground".into()))
        }
    }

    struct FakeTranscriber(String);

    impl Transcriber for FakeTranscriber {
        fn transcribe(&self, _: &[f32], _: &CancelToken) -> Result<String, BackendError> {
            Ok(self.0.clone())
        }
    }

    #[derive(Default)]
    struct FakeGenerator {
        output: String,
        finish: Option<FinishReason>,
        delay: Duration,
        cancel_during: bool,
        error: Option<String>,
        calls: Mutex<Vec<Vec<ChatMessage>>>,
        /// Outputs for later calls (the repair rounds), consumed in order after the first call.
        later: Mutex<Vec<String>>,
        later_delay: Duration,
        cancel_on_later: bool,
    }

    impl Generator for FakeGenerator {
        fn generate(&self, req: &GenerationRequest<'_>, cancel: &CancelToken, on_token: &mut dyn FnMut(&str)) -> Result<Generation, BackendError> {
            let call = {
                let mut calls = self.calls.lock().unwrap();
                calls.push(req.messages.to_vec());
                calls.len()
            };
            if call > 1 {
                std::thread::sleep(self.later_delay);
                if self.cancel_on_later {
                    cancel.cancel();
                    return Err(BackendError("worker killed".into()));
                }
                let mut later = self.later.lock().unwrap();
                let text = if later.is_empty() { self.output.clone() } else { later.remove(0) };
                on_token(&text);
                return Ok(Generation { text, finish: FinishReason::Stop });
            }
            std::thread::sleep(self.delay);
            if self.cancel_during {
                cancel.cancel();
            }
            if let Some(message) = &self.error {
                return Err(BackendError(message.clone()));
            }
            on_token(&self.output);
            Ok(Generation { text: self.output.clone(), finish: self.finish.unwrap_or(FinishReason::Stop) })
        }
    }

    #[derive(Default)]
    struct FakeInserter {
        calls: Mutex<Vec<(WindowIdentity, String, PasteChord)>>,
    }

    impl Inserter for FakeInserter {
        fn insert(&self, target: &WindowIdentity, text: &str, chord: PasteChord) -> Result<(), BackendError> {
            self.calls.lock().unwrap().push((*target, text.to_owned(), chord));
            Ok(())
        }
    }

    struct Harness {
        context: Arc<FakeContext>,
        generator: Arc<FakeGenerator>,
        inserter: Arc<FakeInserter>,
        history: Arc<FakeHistory>,
        orchestrator: Orchestrator,
    }

    #[derive(Default)]
    struct FakeHistory {
        context: HistoryContext,
        fail: bool,
        records: Mutex<Vec<NewHistoryEntry>>,
    }

    impl History for FakeHistory {
        fn context(&self, _: &str, _: &str) -> HistoryContext {
            self.context.clone()
        }
        fn record(&self, entry: NewHistoryEntry) -> Result<bool, BackendError> {
            if self.fail {
                return Err(BackendError("disk full".into()));
            }
            self.records.lock().unwrap().push(entry);
            Ok(true)
        }
    }

    fn harness(ctx: ActiveContext, transcript: &str, generator: FakeGenerator, policy: ContextPolicy, limits: Limits) -> Harness {
        harness_with(FakeContext { ctx, foreground: Mutex::new(Some(TARGET)), ..Default::default() }, transcript, generator, policy, limits)
    }

    fn harness_with(context: FakeContext, transcript: &str, generator: FakeGenerator, policy: ContextPolicy, limits: Limits) -> Harness {
        harness_full(context, transcript, generator, FakeHistory::default(), policy, limits)
    }

    fn harness_full(context: FakeContext, transcript: &str, generator: FakeGenerator, history: FakeHistory, policy: ContextPolicy, limits: Limits) -> Harness {
        let context = Arc::new(context);
        let generator = Arc::new(generator);
        let inserter = Arc::new(FakeInserter::default());
        let history = Arc::new(history);
        let backends = Backends {
            context: context.clone(),
            transcriber: Arc::new(FakeTranscriber(transcript.into())),
            generator: generator.clone(),
            inserter: inserter.clone(),
            history: history.clone(),
        };
        let orchestrator = Orchestrator::new(backends, ProfileSet::bundled(), policy, limits);
        Harness { context, generator, inserter, history, orchestrator }
    }

    fn chat_ctx() -> ActiveContext {
        ActiveContext {
            window: TARGET,
            process_name: "chrome.exe".into(),
            window_title: "ChatGPT".into(),
            url: Some("https://chatgpt.com/".into()),
        }
    }

    fn generator(output: &str) -> FakeGenerator {
        FakeGenerator { output: output.into(), ..Default::default() }
    }

    fn run(h: &Harness, mode: Mode) -> JobReport {
        let job = h.orchestrator.begin(mode).unwrap();
        h.orchestrator.finish(job, &[0.0; 16], &mut |_| {})
    }

    fn inserts(h: &Harness) -> usize {
        h.inserter.calls.lock().unwrap().len()
    }

    #[test]
    fn inserts_into_captured_window_with_profile_chord() {
        let h = harness(chat_ctx(), "compare pricing", generator("Compare pricing."), ContextPolicy::default(), Limits::default());
        let mut events = Vec::new();
        let job = h.orchestrator.begin(Mode::Prompt).unwrap();
        assert_eq!(job.profile_id, "chatgpt");
        let report = h.orchestrator.finish(job, &[0.0; 16], &mut |e| events.push(format!("{e:?}")));
        assert_eq!(report.outcome, Outcome::Inserted { text: "Compare pricing.".into() });
        assert_eq!(*h.inserter.calls.lock().unwrap(), vec![(TARGET, "Compare pricing.".into(), PasteChord::Standard)]);
        assert!(events.iter().any(|e| e.contains("Token")));
    }

    #[test]
    fn focus_change_blocks_insertion() {
        let h = harness(chat_ctx(), "x", generator("Prompt."), ContextPolicy::default(), Limits::default());
        *h.context.foreground.lock().unwrap() = Some(WindowIdentity { handle: 99, process_id: 7 });
        let report = run(&h, Mode::Prompt);
        assert!(matches!(report.outcome, Outcome::Blocked { reason: BlockReason::FocusChanged, ref text, .. } if text == "Prompt."));
        assert_eq!(inserts(&h), 0);
    }

    #[test]
    fn unknown_focus_blocks_insertion() {
        let h = harness(chat_ctx(), "x", generator("Prompt."), ContextPolicy::default(), Limits::default());
        *h.context.foreground.lock().unwrap() = None;
        assert!(matches!(run(&h, Mode::Prompt).outcome, Outcome::Blocked { reason: BlockReason::FocusUnknown, .. }));
        assert_eq!(inserts(&h), 0);
    }

    #[test]
    fn cancel_during_generation_prevents_insertion() {
        let h = harness(chat_ctx(), "x", FakeGenerator { output: "Prompt.".into(), cancel_during: true, ..Default::default() }, ContextPolicy::default(), Limits::default());
        assert_eq!(run(&h, Mode::Prompt).outcome, Outcome::Cancelled);
        assert_eq!(inserts(&h), 0);
    }

    #[test]
    fn cancel_arriving_after_focus_check_prevents_insertion() {
        let h = harness(chat_ctx(), "x", generator("Prompt."), ContextPolicy::default(), Limits::default());
        let job = h.orchestrator.begin(Mode::Prompt).unwrap();
        *h.context.cancel_on_foreground.lock().unwrap() = Some(job.cancel_token());
        assert_eq!(h.orchestrator.finish(job, &[0.0; 16], &mut |_| {}).outcome, Outcome::Cancelled);
        assert_eq!(inserts(&h), 0);
    }

    #[test]
    fn cancelled_before_finish_skips_all_work() {
        let h = harness(chat_ctx(), "x", generator("Prompt."), ContextPolicy::default(), Limits::default());
        let job = h.orchestrator.begin(Mode::Prompt).unwrap();
        job.cancel_token().cancel();
        assert_eq!(h.orchestrator.finish(job, &[0.0; 16], &mut |_| {}).outcome, Outcome::Cancelled);
        assert!(h.generator.calls.lock().unwrap().is_empty());
        assert_eq!(inserts(&h), 0);
    }

    #[test]
    fn only_one_job_at_a_time() {
        let h = harness(chat_ctx(), "x", generator("Prompt."), ContextPolicy::default(), Limits::default());
        let job = h.orchestrator.begin(Mode::Prompt).unwrap();
        assert_eq!(h.orchestrator.begin(Mode::Prompt).err(), Some(BeginError::Busy));
        drop(job);
        let job = h.orchestrator.begin(Mode::Prompt).unwrap();
        h.orchestrator.finish(job, &[0.0; 16], &mut |_| {});
        assert!(h.orchestrator.begin(Mode::Prompt).is_ok());
    }

    #[test]
    fn late_generation_is_discarded() {
        let limits = Limits { generation_timeout: Duration::from_millis(5), ..Limits::default() };
        let h = harness(chat_ctx(), "x", FakeGenerator { output: "Prompt.".into(), delay: Duration::from_millis(40), ..Default::default() }, ContextPolicy::default(), limits);
        assert!(matches!(run(&h, Mode::Prompt).outcome, Outcome::Failed { reason: FailReason::TimedOut, .. }));
        assert_eq!(inserts(&h), 0);
    }

    #[test]
    fn truncated_output_is_shown_not_inserted() {
        let h = harness(chat_ctx(), "x", FakeGenerator { output: "Partial".into(), finish: Some(FinishReason::Length), ..Default::default() }, ContextPolicy::default(), Limits::default());
        assert!(matches!(run(&h, Mode::Prompt).outcome, Outcome::Blocked { reason: BlockReason::OutputTruncated, .. }));
        let limits = Limits { max_output_chars: 3, ..Limits::default() };
        let h2 = harness(chat_ctx(), "x", generator("Too long"), ContextPolicy::default(), limits);
        assert!(matches!(run(&h2, Mode::Prompt).outcome, Outcome::Blocked { reason: BlockReason::OutputTruncated, .. }));
        assert_eq!(inserts(&h) + inserts(&h2), 0);
    }

    #[test]
    fn overlong_recording_and_silence_skip_generation() {
        let limits = Limits { max_audio_samples: 8, ..Limits::default() };
        let h = harness(chat_ctx(), "x", generator("Prompt."), ContextPolicy::default(), limits);
        assert!(matches!(run(&h, Mode::Prompt).outcome, Outcome::Failed { reason: FailReason::RecordingTooLong, .. }));
        let h2 = harness(chat_ctx(), "  ", generator("Prompt."), ContextPolicy::default(), Limits::default());
        assert_eq!(run(&h2, Mode::Prompt).outcome, Outcome::NoSpeech);
        assert!(h.generator.calls.lock().unwrap().is_empty() && h2.generator.calls.lock().unwrap().is_empty());
    }

    fn surrounding_policy() -> ContextPolicy {
        ContextPolicy { surrounding_text_apps: ["chatgpt.com".to_string()].into(), max_surrounding_chars: 100 }
    }

    fn with_focused(text: &str, is_secure: bool, policy: ContextPolicy) -> Harness {
        let context = FakeContext {
            ctx: chat_ctx(),
            focused: Some(FocusedText { text: text.into(), is_secure }),
            foreground: Mutex::new(Some(TARGET)),
            ..Default::default()
        };
        harness_with(context, "x", generator("Prompt."), policy, Limits::default())
    }

    fn last_user_message(h: &Harness) -> String {
        h.generator.calls.lock().unwrap()[0].last().unwrap().content.clone()
    }

    #[test]
    fn surrounding_text_is_not_read_without_opt_in() {
        let h = with_focused("SECRET-SENTINEL", false, ContextPolicy::default());
        run(&h, Mode::Prompt);
        assert_eq!(h.context.focused_calls.load(Ordering::SeqCst), 0);
        assert!(!last_user_message(&h).contains("SECRET-SENTINEL"));
    }

    #[test]
    fn secure_field_text_is_never_sent() {
        let h = with_focused("SECRET-SENTINEL", true, surrounding_policy());
        run(&h, Mode::Prompt);
        assert!(!last_user_message(&h).contains("SECRET-SENTINEL"));
    }

    #[test]
    fn opted_in_text_is_delimited() {
        let h = with_focused("earlier message", false, surrounding_policy());
        run(&h, Mode::Prompt);
        assert!(last_user_message(&h).contains("<surrounding_text>\nearlier message\n</surrounding_text>"));
    }

    #[test]
    fn dictation_skips_model_and_collapses_for_terminals() {
        let ctx = ActiveContext { window: TARGET, process_name: "WindowsTerminal.exe".into(), ..Default::default() };
        let h = harness(ctx, "um, run the tests\nthen commit", generator("unused"), surrounding_policy(), Limits::default());
        let report = run(&h, Mode::Dictation);
        assert_eq!(report.outcome, Outcome::Inserted { text: "Run the tests then commit".into() });
        assert!(h.generator.calls.lock().unwrap().is_empty());
        assert_eq!(h.inserter.calls.lock().unwrap()[0].2, PasteChord::Terminal);
    }

    #[test]
    fn terminal_prompt_is_inserted_as_one_line() {
        let ctx = ActiveContext { window: TARGET, process_name: "pwsh.exe".into(), ..Default::default() };
        let h = harness(ctx, "run tests", generator("Run the tests.\nrm -rf build\n"), ContextPolicy::default(), Limits::default());
        assert_eq!(run(&h, Mode::Prompt).outcome, Outcome::Inserted { text: "Run the tests. rm -rf build".into() });
    }

    fn records(h: &Harness) -> Vec<NewHistoryEntry> {
        h.history.records.lock().unwrap().clone()
    }

    #[test]
    fn backend_errors_from_cancel_or_deadline_are_classified() {
        let failing = |cancel_during, delay| FakeGenerator { error: Some("worker killed".into()), cancel_during, delay, ..Default::default() };
        let h = harness(chat_ctx(), "x", failing(true, Duration::ZERO), ContextPolicy::default(), Limits::default());
        assert_eq!(run(&h, Mode::Prompt).outcome, Outcome::Cancelled);
        let limits = Limits { generation_timeout: Duration::from_millis(5), ..Limits::default() };
        let h = harness(chat_ctx(), "x", failing(false, Duration::from_millis(40)), ContextPolicy::default(), limits);
        assert!(matches!(run(&h, Mode::Prompt).outcome, Outcome::Failed { reason: FailReason::TimedOut, .. }));
        let h = harness(chat_ctx(), "x", failing(false, Duration::ZERO), ContextPolicy::default(), Limits::default());
        assert!(matches!(run(&h, Mode::Prompt).outcome, Outcome::Failed { reason: FailReason::GenerationFailed, ref detail } if detail.as_deref() == Some("worker killed")));
    }

    #[test]
    fn inserted_job_is_recorded_without_surrounding_text() {
        let context = FakeContext {
            ctx: chat_ctx(),
            focused: Some(FocusedText { text: "SCREEN-SENTINEL".into(), is_secure: false }),
            foreground: Mutex::new(Some(TARGET)),
            ..Default::default()
        };
        let h = harness_with(context, "compare pricing", generator("Compare pricing."), surrounding_policy(), Limits::default());
        let report = run(&h, Mode::Prompt);
        assert!(report.history_saved);
        assert_eq!(
            records(&h),
            vec![NewHistoryEntry {
                mode: Mode::Prompt,
                profile_id: "chatgpt".into(),
                app_key: "chatgpt.com".into(),
                transcript: "compare pricing".into(),
                output: "Compare pricing.".into(),
                inserted: true,
            }]
        );
        assert!(last_user_message(&h).contains("SCREEN-SENTINEL"));
        assert!(!format!("{:?}", records(&h)).contains("SCREEN-SENTINEL"));
    }

    #[test]
    fn blocked_job_is_recorded_as_not_inserted() {
        let h = harness(chat_ctx(), "x", generator("Prompt."), ContextPolicy::default(), Limits::default());
        *h.context.foreground.lock().unwrap() = Some(WindowIdentity { handle: 1, process_id: 1 });
        run(&h, Mode::Prompt);
        assert!(!records(&h)[0].inserted);
    }

    #[test]
    fn cancelled_and_failed_jobs_are_not_recorded() {
        let h = harness(chat_ctx(), "x", FakeGenerator { output: "Prompt.".into(), cancel_during: true, ..Default::default() }, ContextPolicy::default(), Limits::default());
        assert!(!run(&h, Mode::Prompt).history_saved);
        let limits = Limits { generation_timeout: Duration::from_millis(5), ..Limits::default() };
        let h2 = harness(chat_ctx(), "x", FakeGenerator { output: "Prompt.".into(), delay: Duration::from_millis(40), ..Default::default() }, ContextPolicy::default(), limits);
        run(&h2, Mode::Prompt);
        assert!(records(&h).is_empty() && records(&h2).is_empty());
    }

    #[test]
    fn history_failure_does_not_change_outcome() {
        let context = FakeContext { ctx: chat_ctx(), foreground: Mutex::new(Some(TARGET)), ..Default::default() };
        let history = FakeHistory { fail: true, ..Default::default() };
        let h = harness_full(context, "x", generator("Prompt."), history, ContextPolicy::default(), Limits::default());
        let report = run(&h, Mode::Prompt);
        assert_eq!(report.outcome, Outcome::Inserted { text: "Prompt.".into() });
        assert!(!report.history_saved);
    }

    #[test]
    fn history_context_reaches_the_model() {
        let context = FakeContext { ctx: chat_ctx(), foreground: Mutex::new(Some(TARGET)), ..Default::default() };
        let history = FakeHistory {
            context: HistoryContext {
                examples: vec![crate::profiles::Example { said: "PAST-SAID".into(), prompt: "PAST-PROMPT".into() }],
                previous: Some(crate::history::PreviousPrompt { text: "PREVIOUS-PROMPT".into(), minutes_ago: 1 }),
            },
            ..Default::default()
        };
        let h = harness_full(context, "shorter", generator("Prompt."), history, ContextPolicy::default(), Limits::default());
        run(&h, Mode::Prompt);
        let messages = h.generator.calls.lock().unwrap()[0].clone();
        assert!(messages.iter().any(|m| m.role == crate::prompt::Role::Assistant && m.content == "PAST-PROMPT"));
        assert!(messages.last().unwrap().content.contains("<previous_prompt>\nPREVIOUS-PROMPT\n</previous_prompt>"));
    }

    const BAD_GRAPH: &str = "Step 1: Plan.\nStep 2 (after 3): Build.\nStep 3: Test.";
    const GOOD_GRAPH: &str = "Step 1: Plan.\nStep 2 (after 1): Build.\nLoop: if it fails, return to Step 2 (max 2 rounds).\nDone when: the build passes.";

    fn scripted(first: &str, later: &[&str]) -> FakeGenerator {
        FakeGenerator { output: first.into(), later: Mutex::new(later.iter().map(|s| s.to_string()).collect()), ..Default::default() }
    }

    fn calls(h: &Harness) -> usize {
        h.generator.calls.lock().unwrap().len()
    }

    #[test]
    fn valid_graph_is_inserted_without_repair() {
        let h = harness(chat_ctx(), "x", generator(GOOD_GRAPH), ContextPolicy::default(), Limits::default());
        let report = run(&h, Mode::Prompt);
        assert_eq!(report.outcome, Outcome::Inserted { text: GOOD_GRAPH.into() });
        assert_eq!(report.structure, Some(StructureCheck::Valid));
        assert_eq!(calls(&h), 1);
    }

    #[test]
    fn incomplete_drafts_are_repaired_into_graphs() {
        let no_checks = GOOD_GRAPH.split("\nDone when:").next().unwrap();
        let empty_checks = format!("{no_checks}\nDone when:");
        for (draft, problem) in [
            ("Plain prompt.", "no numbered steps"),
            ("Step 1: Plan.\nStep 2 (after 1): Build.\nDone when: built.", "no loop"),
            ("Step 1: Write it.\nLoop: if it is unclear, revise it (max 2 rounds).\nDone when: it is clear.", "no numbered return target"),
            (no_checks, "no checks"),
            (empty_checks.as_str(), "no checks"),
        ] {
            let h = harness(chat_ctx(), "x", scripted(draft, &[GOOD_GRAPH]), ContextPolicy::default(), Limits::default());
            let report = run(&h, Mode::Prompt);
            assert_eq!(report.outcome, Outcome::Inserted { text: GOOD_GRAPH.into() }, "{draft}");
            assert_eq!(report.structure, Some(StructureCheck::Repaired), "{draft}");
            let repair = h.generator.calls.lock().unwrap()[1].last().unwrap().content.clone();
            assert!(repair.contains(problem), "{repair}");
            assert!(repair.contains("at least one loop") && repair.contains("Done when:"), "{repair}");
        }
    }

    #[test]
    fn structure_checks_use_the_sanitized_draft_and_repair() {
        let hidden_good = format!("<think>{GOOD_GRAPH}</think>\nPlain prompt.");
        let hidden_bad = format!("<think>{BAD_GRAPH}</think>\n{GOOD_GRAPH}");
        for (draft, repair, expected) in [
            (hidden_good.as_str(), GOOD_GRAPH, StructureCheck::Repaired),
            (hidden_bad.as_str(), BAD_GRAPH, StructureCheck::Valid),
            (BAD_GRAPH, hidden_bad.as_str(), StructureCheck::Repaired),
            (BAD_GRAPH, hidden_good.as_str(), StructureCheck::KeptOriginal),
        ] {
            let h = harness(chat_ctx(), "x", scripted(draft, &[repair]), ContextPolicy::default(), Limits::default());
            let report = run(&h, Mode::Prompt);
            let text = if expected == StructureCheck::KeptOriginal { BAD_GRAPH } else { GOOD_GRAPH };
            assert_eq!(report.outcome, Outcome::Inserted { text: text.into() }, "{draft}");
            assert_eq!(report.structure, Some(expected), "{draft}");
            assert_eq!(calls(&h), if expected == StructureCheck::Valid { 1 } else { 2 });
        }
    }

    #[test]
    fn terminal_graphs_are_validated_after_collapsing_lines() {
        let ctx = ActiveContext { window: TARGET, process_name: "WindowsTerminal.exe".into(), window_title: "pwsh".into(), url: None };
        let draft = "Step 1: Plan\nStep 2 (after 1): Build\nLoop: if the build fails, return to Step 2 (max 2 rounds).\nDone when: the build passes.";
        let inline = GOOD_GRAPH.replace('\n', "; ");
        let h = harness(ctx, "x", scripted(draft, &[&inline]), ContextPolicy::default(), Limits::default());
        let report = run(&h, Mode::Prompt);
        assert_eq!(report.outcome, Outcome::Inserted { text: inline.clone() });
        assert_eq!(report.structure, Some(StructureCheck::Repaired));
        assert!(crate::structure::validate_graph(&inline).is_ok());
        assert!(!inline.contains('\n'));
        assert_eq!(calls(&h), 2);
    }

    #[test]
    fn oversized_repairs_keep_the_complete_original_draft() {
        let limits = Limits { max_output_chars: GOOD_GRAPH.len(), ..Limits::default() };
        let oversized = format!("{GOOD_GRAPH}\n{}", "x".repeat(50));
        let h = harness(chat_ctx(), "x", scripted(BAD_GRAPH, &[&oversized]), ContextPolicy::default(), limits);
        let report = run(&h, Mode::Prompt);
        assert_eq!(report.outcome, Outcome::Inserted { text: BAD_GRAPH.into() });
        assert_eq!(report.structure, Some(StructureCheck::KeptOriginal));
        assert_eq!(calls(&h), 2);
    }

    #[test]
    fn malformed_graph_is_repaired_once() {
        let h = harness(chat_ctx(), "x", scripted(BAD_GRAPH, &[GOOD_GRAPH]), ContextPolicy::default(), Limits::default());
        let mut stages = Vec::new();
        let job = h.orchestrator.begin(Mode::Prompt).unwrap();
        let report = h.orchestrator.finish(job, &[0.0; 16], &mut |e| if let JobEvent::Stage(s) = e { stages.push(s) });
        assert_eq!(report.outcome, Outcome::Inserted { text: GOOD_GRAPH.into() });
        assert_eq!(report.structure, Some(StructureCheck::Repaired));
        assert_eq!(stages, vec![Stage::Transcribing, Stage::Generating, Stage::Revising, Stage::Inserting]);
        let calls = h.generator.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        let repair = &calls[1];
        assert_eq!(&repair[..calls[0].len()], &calls[0][..]);
        assert_eq!(repair[repair.len() - 2], ChatMessage { role: Role::Assistant, content: BAD_GRAPH.into() });
        assert!(repair.last().unwrap().content.contains("Step 2 depends on step 3"));
    }

    #[test]
    fn failed_repair_keeps_first_draft_and_stays_bounded() {
        let h = harness(chat_ctx(), "x", scripted(BAD_GRAPH, &[BAD_GRAPH, GOOD_GRAPH]), ContextPolicy::default(), Limits::default());
        let report = run(&h, Mode::Prompt);
        assert_eq!(report.outcome, Outcome::Inserted { text: BAD_GRAPH.into() });
        assert_eq!(report.structure, Some(StructureCheck::KeptOriginal));
        assert_eq!(calls(&h), 2);
        let limits = Limits { max_structure_repairs: 0, ..Limits::default() };
        let h = harness(chat_ctx(), "x", scripted(BAD_GRAPH, &[GOOD_GRAPH]), ContextPolicy::default(), limits);
        assert_eq!(run(&h, Mode::Prompt).structure, Some(StructureCheck::KeptOriginal));
        assert_eq!(calls(&h), 1);
    }

    #[test]
    fn late_repair_is_discarded_for_the_in_time_draft() {
        let limits = Limits { generation_timeout: Duration::from_millis(30), ..Limits::default() };
        let generator = FakeGenerator { later_delay: Duration::from_millis(60), ..scripted(BAD_GRAPH, &[GOOD_GRAPH]) };
        let h = harness(chat_ctx(), "x", generator, ContextPolicy::default(), limits);
        let report = run(&h, Mode::Prompt);
        assert_eq!(report.outcome, Outcome::Inserted { text: BAD_GRAPH.into() });
        assert_eq!(report.structure, Some(StructureCheck::KeptOriginal));
    }

    #[test]
    fn cancel_during_repair_prevents_insertion() {
        let generator = FakeGenerator { cancel_on_later: true, ..scripted(BAD_GRAPH, &[GOOD_GRAPH]) };
        let h = harness(chat_ctx(), "x", generator, ContextPolicy::default(), Limits::default());
        assert_eq!(run(&h, Mode::Prompt).outcome, Outcome::Cancelled);
        assert_eq!(inserts(&h), 0);
        assert!(records(&h).is_empty());
    }

    #[test]
    fn local_job_waits_for_remote_holder_and_fails_closed_when_busy() {
        let limits = Limits { generation_timeout: Duration::from_millis(40), ..Limits::default() };
        let h = harness(chat_ctx(), "x", generator("Prompt."), ContextPolicy::default(), limits);
        let scheduler = h.orchestrator.service().scheduler();
        let held = scheduler.acquire(Priority::Device, "phone", &CancelToken::default(), Instant::now() + Duration::from_secs(5)).unwrap();
        let report = run(&h, Mode::Prompt);
        assert!(matches!(report.outcome, Outcome::Failed { reason: FailReason::EngineBusy, .. }));
        assert!(h.generator.calls.lock().unwrap().is_empty());
        assert_eq!(inserts(&h), 0);
        assert!(!report.history_saved);
        drop(held);
        assert_eq!(run(&h, Mode::Prompt).outcome, Outcome::Inserted { text: "Prompt.".into() });
    }

    #[test]
    fn media_profiles_and_dictation_skip_structure_checks() {
        let ctx = ActiveContext { window: TARGET, process_name: "chrome.exe".into(), url: Some("https://www.midjourney.com/imagine".into()), ..Default::default() };
        let h = harness(ctx, "x", scripted(BAD_GRAPH, &[GOOD_GRAPH]), ContextPolicy::default(), Limits::default());
        let report = run(&h, Mode::Prompt);
        assert_eq!(report.profile_id, "image_gen");
        assert_eq!(report.structure, None);
        assert_eq!(calls(&h), 1);
        let h = harness(chat_ctx(), "x", generator("unused"), ContextPolicy::default(), Limits::default());
        assert_eq!(run(&h, Mode::Dictation).structure, None);
    }

    fn notepad_ctx() -> ActiveContext {
        ActiveContext { window: TARGET, process_name: "notepad.exe".into(), window_title: "notes".into(), url: None }
    }

    #[test]
    fn answers_are_shown_never_pasted_or_recorded() {
        let h = harness(chat_ctx(), "what is a mutex", generator("A lock that allows one owner at a time."), ContextPolicy::default(), Limits::default());
        let report = run(&h, Mode::Answer);
        assert_eq!(report.outcome, Outcome::Answered { text: "A lock that allows one owner at a time.".into() });
        assert_eq!(inserts(&h), 0);
        assert!(!report.history_saved && records(&h).is_empty());
        assert!(last_user_message(&h).contains("<transcript>\nwhat is a mutex\n</transcript>"));
        let truncated = harness(chat_ctx(), "x", FakeGenerator { output: "Partial".into(), finish: Some(FinishReason::Length), ..Default::default() }, ContextPolicy::default(), Limits::default());
        assert!(matches!(run(&truncated, Mode::Answer).outcome, Outcome::Answered { .. }));
        assert_eq!(inserts(&truncated), 0);
        let terminal = ActiveContext { window: TARGET, process_name: "WindowsTerminal.exe".into(), window_title: "pwsh".into(), url: None };
        let h = harness(terminal, "list the steps", generator("1. Build\n2. Test"), ContextPolicy::default(), Limits::default());
        assert_eq!(run(&h, Mode::Answer).outcome, Outcome::Answered { text: "1. Build\n2. Test".into() }, "answers keep line breaks in single-line apps");
    }

    #[test]
    fn auto_mode_dictates_outside_ai_apps_unless_asked_for_a_prompt() {
        let h = harness(notepad_ctx(), "um, meeting moved to Friday.", generator("unused"), ContextPolicy::default(), Limits::default());
        h.orchestrator.set_auto_mode(true);
        let report = run(&h, Mode::Prompt);
        assert_eq!(report.outcome, Outcome::Inserted { text: "Meeting moved to Friday.".into() });
        assert_eq!(calls(&h), 0, "no prompt is written outside AI apps");
        assert_eq!(records(&h)[0].mode, Mode::Dictation, "history keeps the mode actually used");

        let cue = harness(notepad_ctx(), "Prompt: plan the launch", generator("Plan the launch."), ContextPolicy::default(), Limits::default());
        cue.orchestrator.set_auto_mode(true);
        assert_eq!(run(&cue, Mode::Prompt).outcome, Outcome::Inserted { text: "Plan the launch.".into() });
        assert!(last_user_message(&cue).contains("<transcript>\nplan the launch\n</transcript>"), "cue word removed");

        let chat = harness(chat_ctx(), "compare pricing", generator("Compare pricing."), ContextPolicy::default(), Limits::default());
        chat.orchestrator.set_auto_mode(true);
        assert_eq!(run(&chat, Mode::Prompt).outcome, Outcome::Inserted { text: "Compare pricing.".into() });
        let dictate = harness(chat_ctx(), "dictate, hello there", generator("unused"), ContextPolicy::default(), Limits::default());
        dictate.orchestrator.set_auto_mode(true);
        assert_eq!(run(&dictate, Mode::Prompt).outcome, Outcome::Inserted { text: "hello there".into() });
        assert_eq!(calls(&dictate), 0);
    }

    #[test]
    fn auto_mode_off_and_explicit_modes_are_unchanged() {
        let h = harness(notepad_ctx(), "plan the launch", generator("Plan the launch."), ContextPolicy::default(), Limits::default());
        assert_eq!(run(&h, Mode::Prompt).outcome, Outcome::Inserted { text: "Plan the launch.".into() });
        let d = harness(chat_ctx(), "Prompt: x", generator("unused"), ContextPolicy::default(), Limits::default());
        d.orchestrator.set_auto_mode(true);
        assert_eq!(run(&d, Mode::Dictation).outcome, Outcome::Inserted { text: "Prompt: x".into() }, "dictation hotkey ignores cues");
    }

    #[test]
    fn vocabulary_corrects_speech_and_spoken_commands_shape_dictation() {
        let h = harness(notepad_ctx(), "Ask prompt if I. New line. Thanks.", generator("unused"), ContextPolicy::default(), Limits::default());
        h.orchestrator.service().set_vocabulary(crate::dictation::Vocabulary {
            words: vec![],
            replacements: vec![crate::dictation::Replacement { from: "prompt if I".into(), to: "Promptify".into() }],
        });
        assert_eq!(run(&h, Mode::Dictation).outcome, Outcome::Inserted { text: "Ask Promptify.\nThanks.".into() });
    }
}

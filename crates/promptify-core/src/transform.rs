//! Speech or text in, finished prompt out. Shared by the desktop hotkey, the local API and remote devices;
//! knows nothing about windows, focus or pasting.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::context::{ActiveContext, AdmittedText, WindowIdentity};
use crate::dictation::remove_fillers;
use crate::history::HistoryContext;
use crate::pipeline::{
    CancelToken, FailReason, FinishReason, GenerationRequest, Generator, History, JobEvent, Limits, Mode, Stage,
    StructureCheck, Transcriber,
};
use crate::profiles::{Profile, ProfileSet};
use crate::prompt::{ChatMessage, PromptRequest, Role, build_prompt_messages};
use crate::sanitize::sanitize_output;
use crate::scheduler::{AdmitError, EngineScheduler, Priority, SchedulerLimits};
use crate::structure::{GraphError, MAX_LOOP_ROUNDS, Structure, validate_structure};

pub const MAX_TEXT_INPUT_CHARS: usize = 8000;

#[derive(Debug, Clone, Copy)]
pub enum Input<'a> {
    Audio(&'a [f32]),
    /// Typed or client-transcribed text; skips speech recognition.
    Text(&'a str),
}

/// Where a remote client's text will go, declared by the client itself.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientContext {
    /// Process name, Android package or iOS bundle ID, e.g. "com.openai.chatgpt".
    #[serde(default)]
    pub app: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub title: String,
}

impl ClientContext {
    pub fn to_active(&self) -> ActiveContext {
        ActiveContext {
            window: WindowIdentity::default(),
            process_name: self.app.chars().take(200).collect(),
            window_title: self.title.chars().take(300).collect(),
            url: self.url.as_ref().map(|u| u.chars().take(2000).collect()),
        }
    }
}

pub struct Transform<'a> {
    pub input: Input<'a>,
    pub mode: Mode,
    pub profile: &'a Profile,
    pub target: &'a ActiveContext,
    pub surrounding: Option<&'a AdmittedText>,
    /// Use the user's past prompts as examples. Off for remote clients unless the owner allows it.
    pub use_history: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TransformOutcome {
    Ready { text: String },
    /// Cut off by the token or length limit; shown to the user but never pasted automatically.
    Truncated { text: String },
    NoSpeech,
    Cancelled,
    Failed { reason: FailReason, detail: Option<String> },
}

#[derive(Debug, Clone, Serialize)]
pub struct TransformReport {
    pub outcome: TransformOutcome,
    #[serde(skip)]
    pub transcript: Option<String>,
    pub structure: Option<StructureCheck>,
}

pub struct TransformService {
    transcriber: Arc<dyn Transcriber>,
    generator: Arc<dyn Generator>,
    history: Arc<dyn History>,
    profiles: ProfileSet,
    limits: Limits,
    scheduler: EngineScheduler,
}

impl TransformService {
    pub fn new(
        transcriber: Arc<dyn Transcriber>,
        generator: Arc<dyn Generator>,
        history: Arc<dyn History>,
        profiles: ProfileSet,
        limits: Limits,
        scheduler: SchedulerLimits,
    ) -> Self {
        Self { transcriber, generator, history, profiles, limits, scheduler: EngineScheduler::new(scheduler) }
    }

    pub fn profiles(&self) -> &ProfileSet {
        &self.profiles
    }

    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    pub fn scheduler(&self) -> &EngineScheduler {
        &self.scheduler
    }

    pub fn history(&self) -> &Arc<dyn History> {
        &self.history
    }

    /// Waits for the engines in priority order, then runs the transform while holding them.
    pub fn run_scheduled(
        &self,
        priority: Priority,
        client: &str,
        transform: &Transform<'_>,
        cancel: &CancelToken,
        queue_wait: Duration,
        on_event: &mut dyn FnMut(JobEvent<'_>),
    ) -> Result<TransformReport, AdmitError> {
        if let Err(reason) = self.check_input(transform) {
            return Ok(TransformReport { outcome: TransformOutcome::Failed { reason, detail: None }, transcript: None, structure: None });
        }
        let _permit = self.scheduler.acquire(priority, client, cancel, Instant::now() + queue_wait)?;
        Ok(self.run(transform, cancel, on_event))
    }

    fn check_input(&self, transform: &Transform<'_>) -> Result<(), FailReason> {
        match transform.input {
            Input::Audio(audio) if audio.len() > self.limits.max_audio_samples => Err(FailReason::RecordingTooLong),
            Input::Text(text) if text.chars().count() > MAX_TEXT_INPUT_CHARS => Err(FailReason::RecordingTooLong),
            _ => Ok(()),
        }
    }

    /// Callers must hold a scheduler permit; use [`Self::run_scheduled`] unless already holding one.
    fn run(&self, t: &Transform<'_>, cancel: &CancelToken, on_event: &mut dyn FnMut(JobEvent<'_>)) -> TransformReport {
        let mut report = TransformReport { outcome: TransformOutcome::Cancelled, transcript: None, structure: None };
        report.outcome = self.stages(t, cancel, &mut report.transcript, &mut report.structure, on_event);
        report
    }

    fn stages(
        &self,
        t: &Transform<'_>,
        cancel: &CancelToken,
        saved_transcript: &mut Option<String>,
        structure: &mut Option<StructureCheck>,
        on_event: &mut dyn FnMut(JobEvent<'_>),
    ) -> TransformOutcome {
        if cancel.is_cancelled() {
            return TransformOutcome::Cancelled;
        }
        if let Err(reason) = self.check_input(t) {
            return TransformOutcome::Failed { reason, detail: None };
        }
        let profile = t.profile;

        let transcript = match t.input {
            Input::Audio(audio) => {
                on_event(JobEvent::Stage(Stage::Transcribing));
                match self.transcriber.transcribe(audio, cancel) {
                    Ok(text) => text,
                    Err(_) if cancel.is_cancelled() => return TransformOutcome::Cancelled,
                    Err(err) => return failed(FailReason::TranscriptionFailed, err.0),
                }
            }
            Input::Text(text) => text.to_owned(),
        };
        if cancel.is_cancelled() {
            return TransformOutcome::Cancelled;
        }
        let transcript = transcript.trim();
        if transcript.is_empty() {
            return TransformOutcome::NoSpeech;
        }
        on_event(JobEvent::Transcript(transcript));
        *saved_transcript = Some(transcript.to_owned());

        let (raw, finish) = match t.mode {
            Mode::Dictation => (remove_fillers(transcript), FinishReason::Stop),
            Mode::Prompt => {
                let label = target_label(t.target);
                let history = if t.use_history { self.history.context(&profile.id, &t.target.app_key()) } else { HistoryContext::default() };
                let messages = build_prompt_messages(&PromptRequest {
                    transcript,
                    profile,
                    target_label: &label,
                    surrounding: t.surrounding,
                    history: &history,
                });
                on_event(JobEvent::Stage(Stage::Generating));
                let deadline = Instant::now() + self.limits.generation_timeout;
                let request = GenerationRequest { messages: &messages, max_new_tokens: self.limits.max_new_tokens, deadline };
                let mut forward = |token: &str| on_event(JobEvent::Token(token));
                let generation = match self.generator.generate(&request, cancel, &mut forward) {
                    Ok(generation) => generation,
                    Err(_) if cancel.is_cancelled() => return TransformOutcome::Cancelled,
                    Err(_) if Instant::now() > deadline => return TransformOutcome::Failed { reason: FailReason::TimedOut, detail: None },
                    Err(err) => return failed(FailReason::GenerationFailed, err.0),
                };
                // A result that arrives after the deadline is discarded, never used late.
                if Instant::now() > deadline {
                    return TransformOutcome::Failed { reason: FailReason::TimedOut, detail: None };
                }
                if generation.finish == FinishReason::Stop && profile.structure != Structure::Flat {
                    match self.check_structure(&messages, generation.text, deadline, cancel, on_event) {
                        Ok((text, check)) => {
                            *structure = Some(check);
                            (text, FinishReason::Stop)
                        }
                        Err(outcome) => return outcome,
                    }
                } else {
                    (generation.text, generation.finish)
                }
            }
        };

        match sanitize_output(&raw, profile.newlines, self.limits.max_output_chars) {
            Ok(sanitized) if sanitized.truncated || finish == FinishReason::Length => TransformOutcome::Truncated { text: sanitized.text },
            Ok(sanitized) => TransformOutcome::Ready { text: sanitized.text },
            Err(_) => TransformOutcome::Failed { reason: FailReason::EmptyOutput, detail: None },
        }
    }

    /// Validates the draft's task graph and runs at most `max_structure_repairs` rewrites inside the
    /// original deadline. Any repair that fails, truncates or arrives late leaves the first draft in place.
    fn check_structure(
        &self,
        messages: &[ChatMessage],
        draft: String,
        deadline: Instant,
        cancel: &CancelToken,
        on_event: &mut dyn FnMut(JobEvent<'_>),
    ) -> Result<(String, StructureCheck), TransformOutcome> {
        let mut error = match validate_structure(&draft) {
            Ok(summary) if summary.is_structured() => return Ok((draft, StructureCheck::Valid)),
            Ok(_) => return Ok((draft, StructureCheck::Unstructured)),
            Err(error) => error,
        };
        let mut latest = draft.clone();
        for _ in 0..self.limits.max_structure_repairs {
            if cancel.is_cancelled() {
                return Err(TransformOutcome::Cancelled);
            }
            if Instant::now() >= deadline {
                break;
            }
            on_event(JobEvent::Stage(Stage::Revising));
            let mut repair = messages.to_vec();
            repair.push(ChatMessage { role: Role::Assistant, content: latest.clone() });
            repair.push(ChatMessage { role: Role::User, content: repair_instruction(error) });
            let request = GenerationRequest { messages: &repair, max_new_tokens: self.limits.max_new_tokens, deadline };
            let mut forward = |token: &str| on_event(JobEvent::Token(token));
            let generation = match self.generator.generate(&request, cancel, &mut forward) {
                Ok(generation) => generation,
                Err(_) if cancel.is_cancelled() => return Err(TransformOutcome::Cancelled),
                Err(_) => break,
            };
            if Instant::now() > deadline || generation.finish != FinishReason::Stop {
                break;
            }
            match validate_structure(&generation.text) {
                Ok(_) => return Ok((generation.text, StructureCheck::Repaired)),
                Err(next) => {
                    error = next;
                    latest = generation.text;
                }
            }
        }
        if cancel.is_cancelled() {
            return Err(TransformOutcome::Cancelled);
        }
        Ok((draft, StructureCheck::KeptOriginal))
    }
}

fn failed(reason: FailReason, detail: String) -> TransformOutcome {
    TransformOutcome::Failed { reason, detail: Some(detail) }
}

fn repair_instruction(error: GraphError) -> String {
    format!(
        "Your prompt has a structural problem: {error}. Rewrite the complete prompt and fix only that problem. \
Number the steps 1, 2, 3 in order, let each step depend only on earlier steps, and give every loop a step to return to \
and a limit of at most {MAX_LOOP_ROUNDS} rounds, for example \"Loop: if the tests fail, return to Step 2 (max 3 rounds).\" \
Output only the finished prompt."
    )
}

pub fn target_label(ctx: &ActiveContext) -> String {
    let process = ctx.normalized_process();
    match ctx.url_host() {
        Some(host) if process.is_empty() => host,
        Some(host) => format!("{host} in {process}"),
        None => process,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::history::NewHistoryEntry;
    use crate::pipeline::{BackendError, Generation};

    #[derive(Default)]
    struct CountingTranscriber(AtomicUsize);

    impl Transcriber for CountingTranscriber {
        fn transcribe(&self, _: &[f32], _: &CancelToken) -> Result<String, BackendError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok("spoken words".into())
        }
    }

    #[derive(Default)]
    struct EchoGenerator {
        delay: Duration,
        calls: Mutex<Vec<Vec<ChatMessage>>>,
        active: AtomicUsize,
        peak: AtomicUsize,
    }

    impl Generator for EchoGenerator {
        fn generate(&self, req: &GenerationRequest<'_>, _: &CancelToken, _: &mut dyn FnMut(&str)) -> Result<Generation, BackendError> {
            let now = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(now, Ordering::SeqCst);
            self.calls.lock().unwrap().push(req.messages.to_vec());
            std::thread::sleep(self.delay);
            self.active.fetch_sub(1, Ordering::SeqCst);
            Ok(Generation { text: "A finished prompt.".into(), finish: FinishReason::Stop })
        }
    }

    #[derive(Default)]
    struct SentinelHistory(AtomicUsize);

    impl History for SentinelHistory {
        fn context(&self, _: &str, _: &str) -> HistoryContext {
            self.0.fetch_add(1, Ordering::SeqCst);
            HistoryContext {
                examples: vec![crate::profiles::Example { said: "HISTORY-SENTINEL".into(), prompt: "HISTORY-SENTINEL".into() }],
                previous: None,
            }
        }
        fn record(&self, _: NewHistoryEntry) -> Result<bool, BackendError> {
            Ok(true)
        }
    }

    struct Fixture {
        transcriber: Arc<CountingTranscriber>,
        generator: Arc<EchoGenerator>,
        history: Arc<SentinelHistory>,
        service: TransformService,
    }

    fn fixture(delay: Duration) -> Fixture {
        let transcriber = Arc::new(CountingTranscriber::default());
        let generator = Arc::new(EchoGenerator { delay, ..Default::default() });
        let history = Arc::new(SentinelHistory::default());
        let service = TransformService::new(
            transcriber.clone(),
            generator.clone(),
            history.clone(),
            ProfileSet::bundled(),
            Limits::default(),
            SchedulerLimits::default(),
        );
        Fixture { transcriber, generator, history, service }
    }

    fn run_text(f: &Fixture, client: &ClientContext, text: &str, use_history: bool) -> TransformReport {
        let target = client.to_active();
        let profile = f.service.profiles().resolve(&target);
        let transform = Transform { input: Input::Text(text), mode: Mode::Prompt, profile, target: &target, surrounding: None, use_history };
        f.service.run_scheduled(Priority::Device, "phone", &transform, &CancelToken::default(), Duration::from_secs(5), &mut |_| {}).unwrap()
    }

    #[test]
    fn text_input_skips_speech_and_history_unless_allowed() {
        let f = fixture(Duration::ZERO);
        let client = ClientContext { app: "com.openai.chatgpt".into(), ..Default::default() };
        let report = run_text(&f, &client, "compare three crm tools", false);
        assert_eq!(report.outcome, TransformOutcome::Ready { text: "A finished prompt.".into() });
        assert_eq!(f.transcriber.0.load(Ordering::SeqCst), 0);
        assert_eq!(f.history.0.load(Ordering::SeqCst), 0);
        assert!(!format!("{:?}", f.generator.calls.lock().unwrap()[0]).contains("HISTORY-SENTINEL"));
        run_text(&f, &client, "compare three crm tools", true);
        assert!(format!("{:?}", f.generator.calls.lock().unwrap()[1]).contains("HISTORY-SENTINEL"));
    }

    #[test]
    fn client_declared_apps_resolve_profiles() {
        let services = ProfileSet::bundled();
        let id = |app: &str, url: Option<&str>| {
            let ctx = ClientContext { app: app.into(), url: url.map(Into::into), title: String::new() };
            services.resolve(&ctx.to_active()).id.clone()
        };
        assert_eq!(id("com.openai.chatgpt", None), "chatgpt");
        assert_eq!(id("com.anthropic.claude", None), "claude");
        assert_eq!(id("ai.perplexity.app.android", None), "perplexity");
        assert_eq!(id("com.google.android.apps.bard", None), "gemini");
        assert_eq!(id("ai.x.grok", None), "grok");
        assert_eq!(id("", Some("https://claude.ai/new")), "claude");
        assert_eq!(id("com.example.notes", None), "generic");
        assert_eq!(target_label(&ClientContext { url: Some("claude.ai".into()), ..Default::default() }.to_active()), "claude.ai");
    }

    #[test]
    fn oversized_input_is_rejected_before_queueing() {
        let f = fixture(Duration::ZERO);
        let held = f.service.scheduler().acquire(Priority::Device, "other", &CancelToken::default(), Instant::now() + Duration::from_secs(5)).unwrap();
        let big = "x".repeat(MAX_TEXT_INPUT_CHARS + 1);
        let started = Instant::now();
        let report = run_text(&f, &ClientContext::default(), &big, false);
        assert_eq!(report.outcome, TransformOutcome::Failed { reason: FailReason::RecordingTooLong, detail: None });
        assert!(started.elapsed() < Duration::from_secs(1), "waited for the engine");
        assert!(f.generator.calls.lock().unwrap().is_empty());
        drop(held);
    }

    #[test]
    fn engine_is_never_shared_between_concurrent_clients() {
        let f = Arc::new(fixture(Duration::from_millis(30)));
        let handles: Vec<_> = (0..4)
            .map(|i| {
                let f = f.clone();
                std::thread::spawn(move || {
                    let target = ActiveContext::default();
                    let profile = f.service.profiles().resolve(&target);
                    let transform = Transform { input: Input::Text("hello there"), mode: Mode::Prompt, profile, target: &target, surrounding: None, use_history: false };
                    let client = format!("c{i}");
                    f.service.run_scheduled(Priority::Device, &client, &transform, &CancelToken::default(), Duration::from_secs(5), &mut |_| {}).unwrap()
                })
            })
            .collect();
        for h in handles {
            assert!(matches!(h.join().unwrap().outcome, TransformOutcome::Ready { .. }));
        }
        assert_eq!(f.generator.calls.lock().unwrap().len(), 4);
        assert_eq!(f.generator.peak.load(Ordering::SeqCst), 1);
    }
}

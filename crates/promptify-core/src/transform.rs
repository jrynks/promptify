//! Speech or text in, finished prompt out. Shared by the desktop hotkey and developer CLI;
//! knows nothing about windows, focus or pasting.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::context::{ActiveContext, AdmittedText};
use crate::dictation::{Vocabulary, apply_spoken_commands, remove_fillers};
use crate::history::HistoryContext;
use crate::pipeline::{
    CancelToken, FailReason, FinishReason, GenerationRequest, Generator, History, JobEvent, Limits, Mode, Stage,
    StructureCheck, Transcriber,
};
use crate::profiles::{NewlinePolicy, Profile, ProfileSet};
use crate::prompt::{PromptRequest, adaptive_prefix_len, build_adaptive_messages, build_prompt_messages, choose_mode, stable_prefix_len};
use crate::quality::{
    self, DictationTone, QualityReport, QualityStatus, ReviewContext, ReviewResponse, Verdict,
};
use crate::routing::{self, Rendering, ResolvedPromptPolicy, RoutingOptions};
use crate::live::{ChunkPolicy, has_speech};
use crate::sanitize::sanitize_output;
use crate::scheduler::{AdmitError, EngineScheduler};
use crate::structure::{GraphError, Structure, opens_with_role, validate_graph};
pub const MAX_TEXT_INPUT_CHARS: usize = 8000;

#[derive(Debug, Clone, Copy)]
pub enum Input<'a> {
    Audio(&'a [f32]),
    /// Typed or client-transcribed text; skips speech recognition.
    Text(&'a str),
    /// Speech already transcribed while recording, plus the audio after it.
    Live { committed: &'a str, tail: &'a [f32] },
}

pub struct Transform<'a> {
    pub input: Input<'a>,
    pub mode: Mode,
    pub profile: &'a Profile,
    pub target: &'a ActiveContext,
    pub surrounding: Option<&'a AdmittedText>,
    /// Use the user's past prompts as examples.
    pub use_history: bool,
    /// For a prompt job: write plain dictation instead when the target is not an AI app, unless the
    /// user says "prompt:" first.
    pub auto_mode: bool,
    pub dictation_tone: DictationTone,
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
    /// Structurally valid text retained for explicit user review, never automatic insertion.
    ReviewOnly { text: String, status: QualityStatus },
}

#[derive(Debug, Clone, Serialize)]
pub struct TransformReport {
    pub outcome: TransformOutcome,
    #[serde(skip)]
    pub transcript: Option<String>,
    pub structure: Option<StructureCheck>,
    pub quality: Option<QualityReport>,
    pub generation_elapsed_ms: u64,
    pub structure_repair_attempts: u8,
    /// The mode actually used; differs from the request only with automatic mode.
    pub mode: Mode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub routing: Option<ResolvedPromptPolicy>,
}

pub struct TransformService {
    transcriber: Arc<dyn Transcriber>,
    generator: Arc<dyn Generator>,
    history: Arc<dyn History>,
    profiles: ProfileSet,
    limits: Limits,
    scheduler: EngineScheduler,
    vocabulary: std::sync::RwLock<Vocabulary>,
}

enum ReviewError {
    Cancelled,
    Unavailable,
    Deadline,
}

struct ReviewAttempt {
    result: Result<ReviewResponse, ReviewError>,
    elapsed_ms: u64,
}

impl TransformService {
    pub fn new(
        transcriber: Arc<dyn Transcriber>,
        generator: Arc<dyn Generator>,
        history: Arc<dyn History>,
        profiles: ProfileSet,
        limits: Limits,
    ) -> Self {
        Self {
            transcriber,
            generator,
            history,
            profiles,
            limits,
            scheduler: EngineScheduler::new(),
            vocabulary: Default::default(),
        }
    }

    /// Replacements the user set for speech recognition mistakes; applied to every spoken transcript.
    pub fn set_vocabulary(&self, vocabulary: Vocabulary) {
        *self.vocabulary.write().unwrap_or_else(|p| p.into_inner()) = vocabulary.sanitized();
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

    /// Waits for the engines in arrival order, then runs the transform while holding them.
    pub fn run_scheduled(
        &self,
        transform: &Transform<'_>,
        cancel: &CancelToken,
        queue_wait: Duration,
        on_event: &mut dyn FnMut(JobEvent<'_>),
    ) -> Result<TransformReport, AdmitError> {
        self.run_scheduled_with_options(queue_wait, transform, &RoutingOptions::default(), cancel, on_event)
    }

    pub fn run_scheduled_with_options(
        &self,
        queue_wait: Duration,
        transform: &Transform<'_>,
        options: &RoutingOptions,
        cancel: &CancelToken,
        on_event: &mut dyn FnMut(JobEvent<'_>),
    ) -> Result<TransformReport, AdmitError> {
        if let Err(reason) = self.check_input(transform) {
            return Ok(TransformReport { outcome: TransformOutcome::Failed { reason, detail: None }, transcript: None, structure: None, quality: None, generation_elapsed_ms: 0, structure_repair_attempts: 0, mode: transform.mode, routing: None });
        }
        if let Err(error) = options.validate() {
            return Ok(TransformReport { outcome: failed(FailReason::InvalidPrompt, error), transcript: None, structure: None, quality: None, generation_elapsed_ms: 0, structure_repair_attempts: 0, mode: transform.mode, routing: None });
        }
        if transform.mode != Mode::Prompt && (options.task_type.is_some() || options.surface.is_some()) {
            return Ok(TransformReport {
                outcome: failed(FailReason::InvalidPrompt, "Task and surface overrides apply only to Prompt mode.".into()),
                transcript: None, structure: None, quality: None, generation_elapsed_ms: 0, structure_repair_attempts: 0, mode: transform.mode, routing: None,
            });
        }
        let _permit = self.scheduler.acquire(cancel, Instant::now() + queue_wait)?;
        Ok(self.run(transform, options, cancel, on_event))
    }

    /// Transcribes one finished chunk while the user is still speaking. Gives up rather than wait
    /// long for the engines; the chunk is then simply transcribed with the rest at the end.
    pub fn transcribe_chunk(&self, audio: &[f32], cancel: &CancelToken, wait: Duration) -> Option<String> {
        if audio.len() > self.limits.max_audio_samples {
            return None;
        }
        let _permit = self.scheduler.acquire(cancel, Instant::now() + wait).ok()?;
        self.transcriber.transcribe(audio, cancel).ok().filter(|_| !cancel.is_cancelled())
    }

    fn check_input(&self, transform: &Transform<'_>) -> Result<(), FailReason> {
        match transform.input {
            Input::Audio(audio) if audio.len() > self.limits.max_audio_samples => Err(FailReason::RecordingTooLong),
            Input::Live { tail, .. } if tail.len() > self.limits.max_audio_samples => Err(FailReason::RecordingTooLong),
            Input::Text(text) if text.chars().count() > MAX_TEXT_INPUT_CHARS => Err(FailReason::RecordingTooLong),
            _ => Ok(()),
        }
    }

    /// Callers must hold a scheduler permit; use [`Self::run_scheduled`] unless already holding one.
    fn run(&self, t: &Transform<'_>, options: &RoutingOptions, cancel: &CancelToken, on_event: &mut dyn FnMut(JobEvent<'_>)) -> TransformReport {
        let mut report = TransformReport { outcome: TransformOutcome::Cancelled, transcript: None, structure: None, quality: None, generation_elapsed_ms: 0, structure_repair_attempts: 0, mode: t.mode, routing: None };
        report.outcome = self.stages(t, options, cancel, &mut report, on_event);
        report
    }

    fn stages(
        &self,
        t: &Transform<'_>,
        options: &RoutingOptions,
        cancel: &CancelToken,
        report: &mut TransformReport,
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
            Input::Live { committed, tail } => {
                on_event(JobEvent::Stage(Stage::Transcribing));
                let tail_text = if has_speech(tail, &ChunkPolicy::default()) { self.transcriber.transcribe(tail, cancel) } else { Ok(String::new()) };
                match tail_text {
                    Ok(text) => [committed.trim(), text.trim()].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(" "),
                    Err(_) if cancel.is_cancelled() => return TransformOutcome::Cancelled,
                    Err(err) => return failed(FailReason::TranscriptionFailed, err.0),
                }
            }
        };
        if cancel.is_cancelled() {
            return TransformOutcome::Cancelled;
        }
        // Corrections are for speech recognition mistakes; typed text is left as written.
        let transcript = match t.input {
            Input::Text(_) => transcript,
            Input::Audio(_) | Input::Live { .. } => self.vocabulary.read().unwrap_or_else(|p| p.into_inner()).apply(&transcript),
        };
        let mut mode_profile = profile.clone();
        if options.rendering == Rendering::Adaptive
            && (options.surface.is_some() || options.task_type.is_some() || routing::is_ai_target(t.target, profile))
        {
            mode_profile.id = "explicit_surface".into();
        }
        let (mode, transcript) = if t.auto_mode && t.mode == Mode::Prompt { choose_mode(&mode_profile, &transcript) } else { (t.mode, transcript.as_str()) };
        report.mode = mode;
        let transcript = transcript.trim();
        if transcript.is_empty() {
            return TransformOutcome::NoSpeech;
        }
        on_event(JobEvent::Transcript(transcript));
        report.transcript = Some(transcript.to_owned());
        let prepared = if mode == Mode::Prompt {
            match routing::prepare_profile(&self.profiles, profile, t.target, transcript, options) {
                Ok((profile, policy)) => {
                    if let Some(policy) = &policy {
                    on_event(JobEvent::Routing(policy));
                    }
                    report.routing = policy;
                    Some(profile)
                }
                Err(error) => return failed(FailReason::InvalidPrompt, error),
            }
        } else { None };
        let profile = prepared.as_ref().unwrap_or(profile);

        if mode == Mode::Dictation {
            return self.dictate(t, transcript, profile, report, cancel, on_event);
        }
                let label = target_label(t.target);
                let mut history = if !t.use_history {
                    HistoryContext::default()
                } else if let Some(policy) = &report.routing {
                    self.history.routed_context(&profile.id, &t.target.app_key(), policy, routing::is_follow_up(transcript))
                } else {
                    self.history.context(&profile.id, &t.target.app_key())
                };
                history.examples.retain(|example| validate_graph(&example.prompt).is_ok());
                let deadline = Instant::now() + self.limits.generation_timeout;
                let prompt_request = PromptRequest {
                    transcript,
                    profile,
                    target_label: &label,
                    surrounding: t.surrounding,
                    history: &history,
                };
                let mut references = t.surrounding.map_or_else(String::new, |surrounding| surrounding.text.clone());
                if let Some(previous) = &history.previous {
                    references.push_str(&format!("\n{}", previous.text));
                }
                let messages = match &report.routing {
                    Some(policy) => build_adaptive_messages(&prompt_request, policy),
                    None => build_prompt_messages(&prompt_request),
                };
                on_event(JobEvent::Stage(Stage::Generating));
                let stable_prefix = report.routing.as_ref().map_or_else(|| stable_prefix_len(profile), adaptive_prefix_len);
                let request = GenerationRequest { messages: &messages, stable_prefix, max_new_tokens: self.limits.max_new_tokens, deadline };
                let generation_started = Instant::now();
                let mut forward = |token: &str| on_event(JobEvent::Token(token));
                let generation = match self.generator.generate(&request, cancel, &mut forward) {
                    Ok(generation) => generation,
                    Err(_) if cancel.is_cancelled() => return TransformOutcome::Cancelled,
                    Err(_) if Instant::now() >= deadline => {
                        report.generation_elapsed_ms = elapsed_ms(generation_started);
                        return TransformOutcome::Failed { reason: FailReason::TimedOut, detail: None };
                    }
                    Err(err) => {
                        report.generation_elapsed_ms = elapsed_ms(generation_started);
                        return failed(FailReason::GenerationFailed, err.0);
                    }
                };
                // A result that arrives after the deadline is discarded, never used late.
                if Instant::now() > deadline {
                    report.generation_elapsed_ms = elapsed_ms(generation_started);
                    return TransformOutcome::Failed { reason: FailReason::TimedOut, detail: None };
                }
                report.generation_elapsed_ms = elapsed_ms(generation_started);
                let newlines = report.routing.as_ref().map_or(profile.newlines, |policy| policy.newlines);
                let outcome = finish_output(&generation.text, generation.finish, newlines, self.limits.max_output_chars);
                if matches!(&outcome, TransformOutcome::Truncated { text } if validate_graph(text).is_err()) {
                    return failed(FailReason::InvalidPrompt, "The incomplete draft did not satisfy the required graph and loop contract. It was not returned as a usable prompt.".into());
                }
                if profile.structure != Structure::Flat
                    && matches!(&outcome, TransformOutcome::Truncated { text } if opens_with_role(text))
                {
                    return reject_persona_draft();
                }
                let TransformOutcome::Ready { text } = outcome else {
                    return outcome;
                };
                let structure_result = self.check_structure(
                    &request,
                    newlines,
                    text.clone(),
                    report.routing.as_ref().map(|policy| (policy, transcript, references.as_str())),
                    cancel,
                    &mut report.structure_repair_attempts,
                    on_event,
                );
                report.generation_elapsed_ms = elapsed_ms(generation_started);
                let (text, check) = match structure_result {
                    Ok(result) => result,
                    Err(outcome) => return outcome,
                };
                report.structure = Some(check);
                let destination_capability = format!(
                    "profile={} can_reply={} surface={}",
                    profile.id,
                    profile.can_reply,
                    report.routing.as_ref().map_or_else(|| "profile_default".to_owned(), |policy| format!("{:?}", policy.surface)),
                );
                let context = ReviewContext {
                    mode: Mode::Prompt,
                    tone: None,
                    original_request: transcript,
                    current_request: transcript,
                    reference_context: &references,
                    candidate: &text,
                    prompt_graph_required: true,
                    destination_capability: &destination_capability,
                };
                let adaptive_policy = report.routing.clone();
                let validate = |candidate: &str| match adaptive_policy.as_ref() {
                    Some(policy) => routing::validate_rewrite_with_context(policy, transcript, &references, candidate),
                    None => validate_graph(candidate).map(|_| ()).map_err(|error| error.to_string()),
                };
                let generation_elapsed_ms = report.generation_elapsed_ms;
                return self.review_and_correct(
                    context,
                    text.clone(),
                    &request,
                    newlines,
                    cancel,
                    report,
                    on_event,
                    generation_elapsed_ms,
                    validate,
                );
    }

    fn dictate(
        &self,
        t: &Transform<'_>,
        transcript: &str,
        profile: &Profile,
        report: &mut TransformReport,
        cancel: &CancelToken,
        on_event: &mut dyn FnMut(JobEvent<'_>),
    ) -> TransformOutcome {
        let cleaned = apply_spoken_commands(&remove_fillers(transcript));
        if t.dictation_tone == DictationTone::CleanTranscript {
            return finish_output(&cleaned, FinishReason::Stop, profile.newlines, self.limits.max_output_chars);
        }
        let deadline = Instant::now() + self.limits.generation_timeout;
        let messages = match quality::build_dictation_messages(&cleaned, t.dictation_tone) {
            Ok(messages) => messages,
            Err(_) => {
                let candidate = finish_output(&cleaned, FinishReason::Stop, NewlinePolicy::Keep, self.limits.max_output_chars);
                return self.unavailable_review(candidate, report, false, 0);
            }
        };
        on_event(JobEvent::Stage(Stage::Generating));
        let request = GenerationRequest {
            messages: &messages,
            stable_prefix: 0,
            max_new_tokens: self.limits.max_new_tokens,
            deadline,
        };
        let generation_started = Instant::now();
        let mut forward = |token: &str| on_event(JobEvent::Token(token));
        let generation = match self.generator.generate(&request, cancel, &mut forward) {
            Ok(generation) if Instant::now() <= deadline => generation,
            Ok(_) => {
                report.generation_elapsed_ms = elapsed_ms(generation_started);
                let candidate = finish_output(&cleaned, FinishReason::Stop, NewlinePolicy::Keep, self.limits.max_output_chars);
                let elapsed = report.generation_elapsed_ms;
                return self.unavailable_review(candidate, report, true, elapsed);
            }
            Err(_) if cancel.is_cancelled() => return TransformOutcome::Cancelled,
            Err(_) => {
                report.generation_elapsed_ms = elapsed_ms(generation_started);
                let candidate = finish_output(&cleaned, FinishReason::Stop, NewlinePolicy::Keep, self.limits.max_output_chars);
                let deadline_exhausted = Instant::now() >= deadline;
                let elapsed = report.generation_elapsed_ms;
                return self.unavailable_review(candidate, report, deadline_exhausted, elapsed);
            }
        };
        report.generation_elapsed_ms = elapsed_ms(generation_started);
        if cancel.is_cancelled() {
            return TransformOutcome::Cancelled;
        }
        let candidate = finish_output(&generation.text, generation.finish, NewlinePolicy::Keep, self.limits.max_output_chars);
        let TransformOutcome::Ready { text } = candidate else {
            return candidate;
        };
        let destination_capability = format!("profile={} newline_policy={:?}", profile.id, profile.newlines);
        let context = ReviewContext {
            mode: Mode::Dictation,
            tone: Some(t.dictation_tone),
            original_request: transcript,
            current_request: &cleaned,
            reference_context: "",
            candidate: &text,
            prompt_graph_required: false,
            destination_capability: &destination_capability,
        };
        let request = GenerationRequest {
            messages: &messages,
            stable_prefix: 0,
            max_new_tokens: self.limits.max_new_tokens,
            deadline,
        };
        let generation_elapsed_ms = report.generation_elapsed_ms;
        let outcome = self.review_and_correct(
            context,
            text.clone(),
            &request,
            NewlinePolicy::Keep,
            cancel,
            report,
            on_event,
            generation_elapsed_ms,
            |_| Ok(()),
        );
        let (text, mut review_status) = match outcome {
            TransformOutcome::Ready { text } => (text, None),
            TransformOutcome::ReviewOnly { text, status } => (text, Some(status)),
            other => return other,
        };
        if let Err(reason) = quality::validate_dictation_fidelity(&cleaned, &text) {
            log::warn!("dictation rewrite rejected by deterministic fidelity check: {reason}");
            if let Some(quality) = &mut report.quality {
                quality.status = QualityStatus::Rejected;
            }
            review_status = Some(QualityStatus::Rejected);
        }
        match finish_output(&text, FinishReason::Stop, profile.newlines, self.limits.max_output_chars) {
            TransformOutcome::Ready { text } => match review_status {
                Some(status) => TransformOutcome::ReviewOnly { text, status },
                None => TransformOutcome::Ready { text },
            },
            other => other,
        }
    }

    fn unavailable_review(
        &self,
        candidate: TransformOutcome,
        report: &mut TransformReport,
        deadline_exhausted: bool,
        generation_elapsed_ms: u64,
    ) -> TransformOutcome {
        report.quality = Some(QualityReport {
            status: QualityStatus::Unavailable,
            generation_elapsed_ms,
            deadline_exhausted,
            ..Default::default()
        });
        match candidate {
            TransformOutcome::Ready { text } => TransformOutcome::ReviewOnly { text, status: QualityStatus::Unavailable },
            other => other,
        }
    }

    fn review_and_correct(
        &self,
        context: ReviewContext<'_>,
        candidate: String,
        request: &GenerationRequest<'_>,
        newline_policy: NewlinePolicy,
        cancel: &CancelToken,
        report: &mut TransformReport,
        on_event: &mut dyn FnMut(JobEvent<'_>),
        generation_elapsed_ms: u64,
        validate: impl Fn(&str) -> Result<(), String>,
    ) -> TransformOutcome {
        let mut metrics = QualityReport { generation_elapsed_ms, ..Default::default() };
        let first = self.review(&context, request.deadline, cancel, on_event);
        metrics.review_calls = 1;
        metrics.review_elapsed_ms = first.elapsed_ms;
        let response = match first.result {
            Ok(response) => response,
            Err(ReviewError::Cancelled) => return TransformOutcome::Cancelled,
            Err(ReviewError::Unavailable) => {
                metrics.status = QualityStatus::Unavailable;
                report.quality = Some(metrics);
                return TransformOutcome::ReviewOnly { text: candidate, status: QualityStatus::Unavailable };
            }
            Err(ReviewError::Deadline) => {
                metrics.status = QualityStatus::Unavailable;
                metrics.deadline_exhausted = true;
                report.quality = Some(metrics);
                return TransformOutcome::ReviewOnly { text: candidate, status: QualityStatus::Unavailable };
            }
        };
        if response.verdict == Verdict::Approve {
            metrics.status = QualityStatus::Checked;
            report.quality = Some(metrics);
            return TransformOutcome::Ready { text: candidate };
        }

        if cancel.is_cancelled() {
            return TransformOutcome::Cancelled;
        }
        if Instant::now() >= request.deadline {
            metrics.status = QualityStatus::Unavailable;
            metrics.deadline_exhausted = true;
            report.quality = Some(metrics);
            return TransformOutcome::ReviewOnly { text: candidate, status: QualityStatus::Unavailable };
        }
        let rewrite_messages = match quality::build_targeted_rewrite_messages(context, &response.issues) {
            Ok(messages) => messages,
            Err(_) => {
                metrics.status = QualityStatus::Unavailable;
                report.quality = Some(metrics);
                return TransformOutcome::ReviewOnly { text: candidate, status: QualityStatus::Unavailable };
            }
        };
        on_event(JobEvent::Stage(Stage::ImprovingWording));
        let rewrite_request = GenerationRequest {
            messages: &rewrite_messages,
            stable_prefix: 0,
            max_new_tokens: self.limits.max_new_tokens,
            deadline: request.deadline,
        };
        let rewrite_started = Instant::now();
        let mut forward = |token: &str| on_event(JobEvent::Token(token));
        let rewritten = self.generator.generate(&rewrite_request, cancel, &mut forward);
        metrics.rewrite_calls = 1;
        metrics.rewrite_elapsed_ms = rewrite_started.elapsed().as_millis().min(u64::MAX as u128) as u64;
        let rewritten = match rewritten {
            Ok(generation) if Instant::now() <= request.deadline => generation,
            Ok(_) => {
                metrics.status = QualityStatus::Unavailable;
                metrics.deadline_exhausted = true;
                report.quality = Some(metrics);
                return TransformOutcome::ReviewOnly { text: candidate, status: QualityStatus::Unavailable };
            }
            Err(_) if cancel.is_cancelled() => return TransformOutcome::Cancelled,
            Err(_) => {
                metrics.status = QualityStatus::Unavailable;
                metrics.deadline_exhausted = Instant::now() >= request.deadline;
                report.quality = Some(metrics);
                return TransformOutcome::ReviewOnly { text: candidate, status: QualityStatus::Unavailable };
            }
        };
        if cancel.is_cancelled() {
            return TransformOutcome::Cancelled;
        }
        if rewritten.finish == FinishReason::Length {
            metrics.status = QualityStatus::Unavailable;
            report.quality = Some(metrics);
            return TransformOutcome::ReviewOnly { text: candidate, status: QualityStatus::Unavailable };
        }
        let rewritten = match finish_output(&rewritten.text, FinishReason::Stop, newline_policy, self.limits.max_output_chars) {
            TransformOutcome::Ready { text } => text,
            _ => {
                metrics.status = QualityStatus::Unavailable;
                report.quality = Some(metrics);
                return TransformOutcome::ReviewOnly { text: candidate, status: QualityStatus::Unavailable };
            }
        };
        if let Err(error) = validate(&rewritten) {
            metrics.status = QualityStatus::Rejected;
            report.quality = Some(metrics);
            log::warn!("targeted wording correction rejected; preserving the structurally valid draft for review: {error}");
            return TransformOutcome::ReviewOnly { text: candidate, status: QualityStatus::Rejected };
        }
        let second_context = ReviewContext { candidate: &rewritten, ..context };
        let second = self.review(&second_context, request.deadline, cancel, on_event);
        metrics.review_calls = 2;
        metrics.review_elapsed_ms = metrics.review_elapsed_ms.saturating_add(second.elapsed_ms);
        match second.result {
            Err(ReviewError::Cancelled) => TransformOutcome::Cancelled,
            Err(ReviewError::Unavailable) => {
                metrics.status = QualityStatus::Unavailable;
                report.quality = Some(metrics);
                TransformOutcome::ReviewOnly { text: rewritten, status: QualityStatus::Unavailable }
            }
            Err(ReviewError::Deadline) => {
                metrics.status = QualityStatus::Unavailable;
                metrics.deadline_exhausted = true;
                report.quality = Some(metrics);
                TransformOutcome::ReviewOnly { text: rewritten, status: QualityStatus::Unavailable }
            }
            Ok(response) if response.verdict == Verdict::Approve => {
                metrics.status = QualityStatus::Corrected;
                report.quality = Some(metrics);
                TransformOutcome::Ready { text: rewritten }
            }
            Ok(_) => {
                metrics.status = QualityStatus::Rejected;
                report.quality = Some(metrics);
                TransformOutcome::ReviewOnly { text: rewritten, status: QualityStatus::Rejected }
            }
        }
    }

    fn review(
        &self,
        context: &ReviewContext<'_>,
        deadline: Instant,
        cancel: &CancelToken,
        on_event: &mut dyn FnMut(JobEvent<'_>),
    ) -> ReviewAttempt {
        let started = Instant::now();
        let result = if cancel.is_cancelled() {
            Err(ReviewError::Cancelled)
        } else if Instant::now() >= deadline {
            Err(ReviewError::Deadline)
        } else {
            match quality::build_review_messages(*context) {
                Err(_) => Err(ReviewError::Unavailable),
                Ok(messages) => {
                    on_event(JobEvent::Stage(Stage::ReviewingQuality));
                    let request = GenerationRequest {
                        messages: &messages,
                        stable_prefix: 0,
                        max_new_tokens: self.limits.max_new_tokens.min(256),
                        deadline,
                    };
                    let mut discard = |_: &str| {};
                    match self.generator.generate(&request, cancel, &mut discard) {
                        Err(_) if cancel.is_cancelled() => Err(ReviewError::Cancelled),
                        Err(_) if Instant::now() >= deadline => Err(ReviewError::Deadline),
                        Err(_) => Err(ReviewError::Unavailable),
                        Ok(_) if cancel.is_cancelled() => Err(ReviewError::Cancelled),
                        Ok(_) if Instant::now() > deadline => Err(ReviewError::Deadline),
                        Ok(generation) if generation.finish == FinishReason::Length => Err(ReviewError::Unavailable),
                        Ok(generation) => {
                            let parsed = quality::parse_review_response(&generation.text).map_err(|_| ReviewError::Unavailable);
                            if Instant::now() > deadline {
                                Err(ReviewError::Deadline)
                            } else {
                                parsed
                            }
                        }
                    }
                }
            }
        };
        ReviewAttempt {
            result,
            elapsed_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
        }
    }

    /// Validates the sanitized draft and runs at most `max_structure_repairs`
    /// rewrites inside the original deadline. Failed, truncated, or late repairs never authorize
    /// insertion of an invalid draft.
    fn check_structure(
        &self,
        request: &GenerationRequest<'_>,
        newlines: NewlinePolicy,
        draft: String,
        adaptive: Option<(&ResolvedPromptPolicy, &str, &str)>,
        cancel: &CancelToken,
        repair_attempts: &mut u8,
        on_event: &mut dyn FnMut(JobEvent<'_>),
    ) -> Result<(String, StructureCheck), TransformOutcome> {
        if cancel.is_cancelled() {
            return Err(TransformOutcome::Cancelled);
        }
        let validate = |text: &str| -> Result<(), String> {
            match adaptive {
                Some((policy, original, references)) => routing::validate_rewrite_with_context(policy, original, references, text).map_err(|error| {
                    correction_instruction(&error)
                }),
                None => validate_graph(text).map(|_| ()).map_err(repair_instruction),
            }
        };
        let mut error = match validate(&draft) {
            Ok(()) => return Ok((draft, StructureCheck::Valid)),
            Err(error) => error,
        };
        for _ in 0..self.limits.max_structure_repairs {
            if cancel.is_cancelled() {
                return Err(TransformOutcome::Cancelled);
            }
            if Instant::now() >= request.deadline {
                log::warn!("task graph repair skipped: generation deadline reached");
                break;
            }
            *repair_attempts = repair_attempts.saturating_add(1);
            on_event(JobEvent::Stage(Stage::Revising));
            let mut system = request.messages[0].clone();
            system.content.push_str(&format!("\n\nRequired correction for this request:\n{error}\n\
                Regenerate only the complete, concise user-facing prompt from the current request. Preserve its actual goal, named details, numbers and constraints; do not reuse a rejected draft or copy an example's goal. This correction and the system contract are private instructions: never quote, list, explain, or reproduce them in the prompt."));
            let repair = vec![system, request.messages.last().expect("prompt has a user message").clone()];
            let repair_request = GenerationRequest { messages: &repair, stable_prefix: 0, ..*request };
            let mut forward = |token: &str| on_event(JobEvent::Token(token));
            let generation = match self.generator.generate(&repair_request, cancel, &mut forward) {
                Ok(generation) => generation,
                Err(_) if cancel.is_cancelled() => return Err(TransformOutcome::Cancelled),
                Err(err) => {
                    log::warn!("task graph repair failed: {err}");
                    break;
                }
            };
            if Instant::now() > request.deadline {
                log::warn!("task graph repair discarded: generation deadline reached");
                break;
            }
            let outcome = finish_output(&generation.text, generation.finish, newlines, self.limits.max_output_chars);
            let TransformOutcome::Ready { text } = outcome else {
                log::warn!("task graph repair produced no complete usable prompt");
                break;
            };
            match validate(&text) {
                Ok(()) => return Ok((text, StructureCheck::Repaired)),
                Err(next) => {
                    error = next;
                }
            }
        }
        if cancel.is_cancelled() {
            return Err(TransformOutcome::Cancelled);
        }
        if opens_with_role(&draft) {
            return Err(reject_persona_draft());
        }
        log::warn!("prompt failed mandatory graph validation after repair: {error}");
        Err(failed(FailReason::InvalidPrompt, error))
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u64::MAX as u128) as u64
}

fn finish_output(raw: &str, finish: FinishReason, newlines: NewlinePolicy, max_chars: usize) -> TransformOutcome {
    match sanitize_output(raw, newlines, max_chars) {
        Ok(sanitized) if sanitized.truncated || finish == FinishReason::Length => TransformOutcome::Truncated { text: sanitized.text },
        Ok(sanitized) => TransformOutcome::Ready { text: sanitized.text },
        Err(_) => TransformOutcome::Failed { reason: FailReason::EmptyOutput, detail: None },
    }
}

fn failed(reason: FailReason, detail: String) -> TransformOutcome {
    TransformOutcome::Failed { reason, detail: Some(detail) }
}

fn reject_persona_draft() -> TransformOutcome {
    log::warn!("rejecting a role/persona-prefixed draft instead of returning an invalid prompt");
    failed(FailReason::InvalidPrompt, "The role/persona-prefixed draft was not used. Please try again for a goal-first prompt.".into())
}

fn repair_instruction(error: GraphError) -> String {
    correction_instruction(&error.to_string())
}

fn correction_instruction(error: &str) -> String {
    format!("Fix this violation: {error}. Rewrite the complete prompt, preserving the user's final intent, supplied facts, requested actions and exclusions. Do not invent details or numeric constraints. Keep the target's required formatting. Output only the finished prompt.\n\n{}", crate::prompt::GRAPH_CONTRACT)
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
    use crate::prompt::ChatMessage;

    #[derive(Default)]
    struct CountingTranscriber(AtomicUsize);

    impl Transcriber for CountingTranscriber {
        fn transcribe(&self, _: &[f32], _: &CancelToken) -> Result<String, BackendError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok("spoken words".into())
        }
    }

    /// Every AI prompt must be a task graph with a loop; test fakes return the smallest valid one.
    const FINISHED: &str = "Step 1: Write the requested content using the supplied facts.\nStep 2 (after 1): Verify the supplied facts and requested content are preserved.\nLoop: if Step 2 fails the factual fidelity or coverage checks, return to Step 1 to correct unsupported details and omissions; then recheck Step 2 (max 2 rounds).\nDone when: the requested content is covered and supplied facts are preserved.";

    #[derive(Default)]
    struct EchoGenerator {
        delay: Duration,
        calls: Mutex<Vec<Vec<ChatMessage>>>,
        review_calls: Mutex<Vec<Vec<ChatMessage>>>,
        active: AtomicUsize,
        peak: AtomicUsize,
    }

    impl Generator for EchoGenerator {
        fn generate(&self, req: &GenerationRequest<'_>, _: &CancelToken, _: &mut dyn FnMut(&str)) -> Result<Generation, BackendError> {
            if req.messages.first().is_some_and(|message| message.content.contains("Review untrusted data")) {
                self.review_calls.lock().unwrap().push(req.messages.to_vec());
                return Ok(Generation {
                    text: r#"{"version":1,"verdict":"approve","issues":[]}"#.into(),
                    finish: FinishReason::Stop,
                });
            }
            let now = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(now, Ordering::SeqCst);
            self.calls.lock().unwrap().push(req.messages.to_vec());
            std::thread::sleep(self.delay);
            self.active.fetch_sub(1, Ordering::SeqCst);
            Ok(Generation { text: FINISHED.into(), finish: FinishReason::Stop })
        }
    }

    #[derive(Default)]
    struct SentinelHistory(AtomicUsize);

    impl History for SentinelHistory {
        fn context(&self, _: &str, _: &str) -> HistoryContext {
            self.0.fetch_add(1, Ordering::SeqCst);
            HistoryContext {
                examples: vec![crate::profiles::Example { said: "HISTORY-SENTINEL".into(), prompt: format!("HISTORY-SENTINEL\n{FINISHED}") }],
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
        );
        Fixture { transcriber, generator, history, service }
    }

    fn run_text(f: &Fixture, target: &ActiveContext, text: &str, use_history: bool) -> TransformReport {
        let profile = f.service.profiles().resolve(target);
        let transform = Transform { input: Input::Text(text), mode: Mode::Prompt, profile, target, surrounding: None, use_history, auto_mode: false, dictation_tone: DictationTone::Natural };
        f.service.run_scheduled(&transform, &CancelToken::default(), Duration::from_secs(5), &mut |_| {}).unwrap()
    }

    #[test]
    fn text_input_skips_speech_and_history_unless_allowed() {
        let f = fixture(Duration::ZERO);
        let target = ActiveContext { process_name: "com.openai.chatgpt".into(), ..Default::default() };
        let report = run_text(&f, &target, "compare three crm tools", false);
        assert_eq!(report.outcome, TransformOutcome::Ready { text: FINISHED.into() });
        assert_eq!(f.transcriber.0.load(Ordering::SeqCst), 0);
        assert_eq!(f.history.0.load(Ordering::SeqCst), 0);
        assert!(!format!("{:?}", f.generator.calls.lock().unwrap()[0]).contains("HISTORY-SENTINEL"));
        run_text(&f, &target, "compare three crm tools", true);
        assert!(format!("{:?}", f.generator.calls.lock().unwrap()[1]).contains("HISTORY-SENTINEL"));
    }

    #[test]
    fn explicit_context_resolves_profiles() {
        let services = ProfileSet::bundled();
        let id = |app: &str, url: Option<&str>| {
            let ctx = ActiveContext { process_name: app.into(), url: url.map(Into::into), ..Default::default() };
            services.resolve(&ctx).id.clone()
        };
        assert_eq!(id("com.openai.chatgpt", None), "chatgpt");
        assert_eq!(id("com.anthropic.claude", None), "claude");
        assert_eq!(id("ai.perplexity.app.android", None), "perplexity");
        assert_eq!(id("com.google.android.apps.bard", None), "gemini");
        assert_eq!(id("ai.x.grok", None), "grok");
        assert_eq!(id("", Some("https://claude.ai/new")), "claude");
        assert_eq!(id("com.example.notes", None), "generic");
        assert_eq!(target_label(&ActiveContext { url: Some("claude.ai".into()), ..Default::default() }), "claude.ai");
    }

    #[test]
    fn chat_requests_for_images_or_videos_get_generator_prompts() {
        let f = fixture(Duration::ZERO);
        let chatgpt = ActiveContext { process_name: "com.openai.chatgpt".into(), ..Default::default() };
        run_text(&f, &chatgpt, "make me a picture of a fox in a snowy forest", false);
        run_text(&f, &chatgpt, "create a short cinematic video of waves at sunset", false);
        run_text(&f, &chatgpt, "write a video script for our product launch then review it", false);
        run_text(&f, &chatgpt, "design a logo for my startup then create three variations and pick the best one for the website", false);
        let calls = f.generator.calls.lock().unwrap();
        let system = |i: usize| calls[i][0].content.clone();
        let last = |i: usize| calls[i].last().unwrap().content.clone();
        for (i, what) in [(0, "an image"), (1, "a video")] {
            let system = system(i);
            assert!(system.contains(&format!("Target: ChatGPT (creating {what}).")), "{system}");
            assert!(system.contains("Required task graph:") && system.contains("Keep the required graph and loop."));
            assert!(last(i).contains("Questions: none; go ahead") && last(i).contains("Complexity: simple."));
        }
        assert!(system(2).contains("Target: ChatGPT.") && last(2).contains("Complexity: complex."), "a script is text");
        assert!(system(3).contains("Target: ChatGPT (creating an image)."));
        assert!(last(3).contains("Questions: up to 3"), "a complex image request may ask");
    }

    #[test]
    fn oversized_input_is_rejected_before_queueing() {
        let f = fixture(Duration::ZERO);
        let held = f.service.scheduler().acquire(&CancelToken::default(), Instant::now() + Duration::from_secs(5)).unwrap();
        let big = "x".repeat(MAX_TEXT_INPUT_CHARS + 1);
        let started = Instant::now();
        let report = run_text(&f, &ActiveContext::default(), &big, false);
        assert_eq!(report.outcome, TransformOutcome::Failed { reason: FailReason::RecordingTooLong, detail: None });
        assert!(started.elapsed() < Duration::from_secs(1), "waited for the engine");
        assert!(f.generator.calls.lock().unwrap().is_empty());
        drop(held);
    }

    #[test]
    fn vocabulary_never_changes_typed_text() {
        let f = fixture(Duration::ZERO);
        f.service.set_vocabulary(Vocabulary { words: vec![], replacements: vec![crate::dictation::Replacement { from: "spoken".into(), to: "CHANGED".into() }] });
        let target = ActiveContext::default();
        let profile = f.service.profiles().resolve(&target);
        let mut seen = Vec::new();
        for input in [Input::Text("typed spoken words"), Input::Audio(&[0.0; 16])] {
            let transform = Transform { input, mode: Mode::Dictation, profile, target: &target, surrounding: None, use_history: false, auto_mode: false, dictation_tone: DictationTone::CleanTranscript };
            f.service
                .run_scheduled(&transform, &CancelToken::default(), Duration::from_secs(5), &mut |e| {
                    if let JobEvent::Transcript(t) = e {
                        seen.push(t.to_owned());
                    }
                })
                .unwrap();
        }
        assert_eq!(seen, vec!["typed spoken words".to_string(), "CHANGED words".to_string()]);
    }

    #[test]
    fn live_input_transcribes_only_the_tail_after_committed_text() {
        #[derive(Default)]
        struct Lengths(Mutex<Vec<usize>>);
        impl Transcriber for Lengths {
            fn transcribe(&self, audio: &[f32], _: &CancelToken) -> Result<String, BackendError> {
                self.0.lock().unwrap().push(audio.len());
                Ok(" the tail ".into())
            }
        }
        let lengths = Arc::new(Lengths::default());
        let service = TransformService::new(
            lengths.clone(),
            Arc::new(EchoGenerator::default()),
            Arc::new(SentinelHistory::default()),
            ProfileSet::bundled(),
            Limits::default(),
        );
        let target = ActiveContext::default();
        let profile = service.profiles().resolve(&target);
        let tail: Vec<f32> = (0..1234).map(|i| (i as f32 * 0.05).sin() * 0.3).collect();
        let transform = Transform {
            input: Input::Live { committed: " already said ", tail: &tail },
            mode: Mode::Dictation,
            profile,
            target: &target,
            surrounding: None,
            use_history: false,
            auto_mode: false,
            dictation_tone: DictationTone::CleanTranscript,
        };
        let mut transcript = String::new();
        service
            .run_scheduled(&transform, &CancelToken::default(), Duration::from_secs(5), &mut |e| {
                if let JobEvent::Transcript(t) = e {
                    transcript = t.to_owned();
                }
            })
            .unwrap();
        assert_eq!(*lengths.0.lock().unwrap(), vec![1234]);
        assert_eq!(transcript, "already said the tail");

        let silent = vec![0.0; 16_000];
        let transform = Transform { input: Input::Live { committed: "only this", tail: &silent }, ..transform };
        service
            .run_scheduled(&transform, &CancelToken::default(), Duration::from_secs(5), &mut |e| {
                if let JobEvent::Transcript(t) = e {
                    transcript = t.to_owned();
                }
            })
            .unwrap();
        assert_eq!(lengths.0.lock().unwrap().len(), 1, "a silent tail is not transcribed");
        assert_eq!(transcript, "only this");

        let too_long = vec![0.5; Limits::default().max_audio_samples + 1];
        let transform = Transform { input: Input::Live { committed: "x", tail: &too_long }, ..transform };
        let report = service.run_scheduled(&transform, &CancelToken::default(), Duration::from_secs(5), &mut |_| {}).unwrap();
        assert_eq!(report.outcome, TransformOutcome::Failed { reason: FailReason::RecordingTooLong, detail: None });
        assert_eq!(lengths.0.lock().unwrap().len(), 1, "an oversized tail never reaches the engine");
    }

    #[test]
    fn live_chunks_never_wait_long_for_busy_engines_or_run_when_cancelled() {
        let f = fixture(Duration::ZERO);
        let audio = vec![0.0; 16_000];
        let held = f.service.scheduler().acquire(&CancelToken::default(), Instant::now() + Duration::from_secs(1)).unwrap();
        let started = Instant::now();
        assert_eq!(f.service.transcribe_chunk(&audio, &CancelToken::default(), Duration::from_millis(60)), None);
        assert!(started.elapsed() < Duration::from_secs(1));
        drop(held);
        let cancelled = CancelToken::default();
        cancelled.cancel();
        assert_eq!(f.service.transcribe_chunk(&audio, &cancelled, Duration::from_millis(60)), None);
        assert_eq!(f.transcriber.0.load(Ordering::SeqCst), 0, "no engine work while busy or cancelled");
        assert_eq!(f.service.transcribe_chunk(&audio, &CancelToken::default(), Duration::from_millis(60)).as_deref(), Some("spoken words"));
        let too_long = vec![0.0; Limits::default().max_audio_samples + 1];
        assert_eq!(f.service.transcribe_chunk(&too_long, &CancelToken::default(), Duration::from_millis(60)), None);
        assert_eq!(f.transcriber.0.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn engine_is_never_shared_between_concurrent_jobs() {
        let f = Arc::new(fixture(Duration::from_millis(30)));
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let f = f.clone();
                std::thread::spawn(move || {
                    let target = ActiveContext::default();
                    let profile = f.service.profiles().resolve(&target);
                    let transform = Transform { input: Input::Text("hello there"), mode: Mode::Prompt, profile, target: &target, surrounding: None, use_history: false, auto_mode: false, dictation_tone: DictationTone::Natural };
                    f.service.run_scheduled(&transform, &CancelToken::default(), Duration::from_secs(5), &mut |_| {}).unwrap()
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

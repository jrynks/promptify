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
use crate::prompt::{ChatMessage, PromptRequest, Role, adaptive_prefix_len, build_adaptive_messages, build_prompt_messages, choose_mode, stable_prefix_len};
use crate::routing::{self, Rendering, ResolvedPromptPolicy, RoutingOptions};
use crate::review::{PromptReviewer, ReviewDecision, ReviewRequest};
use crate::live::{ChunkPolicy, has_speech};
use crate::sanitize::sanitize_output;
use crate::scheduler::{AdmitError, EngineScheduler};
use crate::structure::{GraphError, MAX_LOOP_ROUNDS, Structure, opens_with_role, validate_graph};
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TransformOutcome {
    Ready { text: String },
    /// Cut off by the token or length limit; shown to the user but never pasted automatically.
    Truncated { text: String },
    /// A valid draft is available, but an opted-in quality review did not approve it.
    ReviewRequired { text: String, detail: String },
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
    reviewer: std::sync::RwLock<Option<Arc<dyn PromptReviewer>>>,
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
            reviewer: Default::default(),
        }
    }

    /// Replacements the user set for speech recognition mistakes; applied to every spoken transcript.
    pub fn set_vocabulary(&self, vocabulary: Vocabulary) {
        *self.vocabulary.write().unwrap_or_else(|p| p.into_inner()) = vocabulary.sanitized();
    }

    pub fn set_prompt_reviewer(&self, reviewer: Arc<dyn PromptReviewer>) {
        *self.reviewer.write().unwrap_or_else(|p| p.into_inner()) = Some(reviewer);
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

    pub fn generator_snapshot(&self) -> Result<Option<Arc<dyn Generator>>, crate::pipeline::BackendError> {
        self.generator.snapshot()
    }

    pub fn run_scheduled_with_options(
        &self,
        queue_wait: Duration,
        transform: &Transform<'_>,
        options: &RoutingOptions,
        cancel: &CancelToken,
        on_event: &mut dyn FnMut(JobEvent<'_>),
    ) -> Result<TransformReport, AdmitError> {
        self.run_scheduled_with_generator(queue_wait, transform, options, cancel, on_event, None)
    }

    pub fn run_scheduled_with_generator(
        &self,
        queue_wait: Duration,
        transform: &Transform<'_>,
        options: &RoutingOptions,
        cancel: &CancelToken,
        on_event: &mut dyn FnMut(JobEvent<'_>),
        pinned: Option<&dyn Generator>,
    ) -> Result<TransformReport, AdmitError> {
        if let Err(reason) = self.check_input(transform) {
            return Ok(TransformReport { outcome: TransformOutcome::Failed { reason, detail: None }, transcript: None, structure: None, mode: transform.mode, routing: None });
        }
        if let Err(error) = options.validate() {
            return Ok(TransformReport { outcome: failed(FailReason::InvalidPrompt, error), transcript: None, structure: None, mode: transform.mode, routing: None });
        }
        if transform.mode != Mode::Prompt && (options.task_type.is_some() || options.surface.is_some()) {
            return Ok(TransformReport {
                outcome: failed(FailReason::InvalidPrompt, "Task and surface overrides apply only to Prompt mode.".into()),
                transcript: None, structure: None, mode: transform.mode, routing: None,
            });
        }
        let _permit = self.scheduler.acquire(cancel, Instant::now() + queue_wait)?;
        // Dictation never calls the language model, so it must not depend on its connection.
        let snapshot = if pinned.is_some() || transform.mode == Mode::Dictation {
            None
        } else {
            match self.generator.snapshot() {
                Ok(snapshot) => snapshot,
                Err(error) => return Ok(TransformReport {
                    outcome: failed(FailReason::GenerationFailed, error.0),
                    transcript: None, structure: None, mode: transform.mode, routing: None,
                }),
            }
        };
        let generator = pinned.or(snapshot.as_deref()).unwrap_or(self.generator.as_ref());
        Ok(self.run(transform, options, cancel, on_event, generator))
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
    fn run(&self, t: &Transform<'_>, options: &RoutingOptions, cancel: &CancelToken, on_event: &mut dyn FnMut(JobEvent<'_>), generator: &dyn Generator) -> TransformReport {
        let mut report = TransformReport { outcome: TransformOutcome::Cancelled, transcript: None, structure: None, mode: t.mode, routing: None };
        report.outcome = self.stages(t, options, cancel, &mut report, on_event, generator);
        report
    }

    fn stages(
        &self,
        t: &Transform<'_>,
        options: &RoutingOptions,
        cancel: &CancelToken,
        report: &mut TransformReport,
        on_event: &mut dyn FnMut(JobEvent<'_>),
        generator: &dyn Generator,
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

        let (raw, finish) = match mode {
            Mode::Dictation => (apply_spoken_commands(&remove_fillers(transcript)), FinishReason::Stop),
            Mode::Prompt => {
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
                let mut forward = |token: &str| on_event(JobEvent::Token(token));
                let generation = match generator.generate(&request, cancel, &mut forward) {
                    Ok(generation) => generation,
                    Err(_) if cancel.is_cancelled() => return TransformOutcome::Cancelled,
                    Err(_) if Instant::now() > deadline => return TransformOutcome::Failed { reason: FailReason::TimedOut, detail: None },
                    Err(err) => return failed(FailReason::GenerationFailed, err.0),
                };
                // A result that arrives after the deadline is discarded, never used late.
                if Instant::now() > deadline {
                    return TransformOutcome::Failed { reason: FailReason::TimedOut, detail: None };
                }
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
                if report.routing.is_some() || profile.structure != Structure::Flat {
                    return match self.check_structure(generator, &request, newlines, text, report.routing.as_ref().map(|policy| (policy, transcript, references.as_str())), cancel, on_event) {
                        Ok((text, check)) => {
                            report.structure = Some(check);
                            self.review_prompt(generator, &request, transcript, newlines, text, report, references.as_str(), cancel, on_event)
                        }
                        Err(outcome) => outcome,
                    };
                }
                return self.review_prompt(generator, &request, transcript, newlines, text, report, references.as_str(), cancel, on_event);
            }
        };

        finish_output(&raw, finish, profile.newlines, self.limits.max_output_chars)
    }

    fn review_prompt(
        &self,
        generator: &dyn Generator,
        generation_request: &GenerationRequest<'_>,
        transcript: &str,
        newlines: NewlinePolicy,
        mut text: String,
        report: &mut TransformReport,
        references: &str,
        cancel: &CancelToken,
        on_event: &mut dyn FnMut(JobEvent<'_>),
    ) -> TransformOutcome {
        let reviewer = self.reviewer.read().unwrap_or_else(|p| p.into_inner()).clone();
        let Some(reviewer) = reviewer.filter(|reviewer| reviewer.enabled()) else {
            return TransformOutcome::Ready { text };
        };
        let deadline = generation_request.deadline;
        for round in 0..=1 {
            if cancel.is_cancelled() {
                return TransformOutcome::Cancelled;
            }
            if Instant::now() >= deadline {
                return TransformOutcome::ReviewRequired { text, detail: "The quality review deadline was reached. Review or copy the draft; it was not pasted.".into() };
            }
            on_event(JobEvent::Stage(Stage::Reviewing));
            let decision = reviewer.review(&ReviewRequest { transcript, draft: &text, deadline }, cancel);
            if cancel.is_cancelled() {
                return TransformOutcome::Cancelled;
            }
            if Instant::now() >= deadline {
                return TransformOutcome::ReviewRequired { text, detail: "The quality review deadline was reached. Review or copy the draft; it was not pasted.".into() };
            }
            let issues = match decision {
                Ok(ReviewDecision::Approved) => return TransformOutcome::Ready { text },
                Ok(ReviewDecision::Skipped) => return TransformOutcome::ReviewRequired {
                    text, detail: "Quality review was disabled during this request. Review or copy this draft, or start a new request.".into(),
                },
                Ok(ReviewDecision::Uncertain { detail }) => return TransformOutcome::ReviewRequired { text, detail },
                Err(error) => return TransformOutcome::ReviewRequired { text, detail: error.0 },
                Ok(ReviewDecision::Revise { issues }) => issues,
            };
            if round == 1 || issues.is_empty() {
                return TransformOutcome::ReviewRequired {
                    text, detail: format!("Quality review did not approve this draft. Review or copy it; nothing was pasted. {}", issues.join(" ")),
                };
            }
            on_event(JobEvent::Stage(Stage::Revising));
            let mut messages = generation_request.messages.to_vec();
            messages.push(ChatMessage { role: Role::Assistant, content: text.clone() });
            messages.push(ChatMessage {
                role: Role::User,
                content: format!(
                    "Revise the complete prompt to address these review findings:\n{}\nPreserve the original request, all supplied details, and the mandatory task graph, explicit dependencies, bounded refinement loop and verifiable completion criteria. Add useful methods and checks, not new restrictions. Output only the revised prompt.",
                    issues.join("\n")
                ),
            });
            let request = GenerationRequest { messages: &messages, ..*generation_request };
            let generation = generator.generate(&request, cancel, &mut |token| on_event(JobEvent::Token(token)));
            if cancel.is_cancelled() {
                return TransformOutcome::Cancelled;
            }
            if Instant::now() >= deadline {
                return TransformOutcome::ReviewRequired { text, detail: "The quality rewrite deadline was reached. The previous draft is available for manual review only.".into() };
            }
            let generation = match generation {
                Ok(generation) => generation,
                Err(error) => return TransformOutcome::ReviewRequired { text, detail: format!("Quality rewrite failed: {}. The previous draft is available for manual review only.", error.0) },
            };
            let outcome = finish_output(&generation.text, generation.finish, newlines, self.limits.max_output_chars);
            let TransformOutcome::Ready { text: revised } = outcome else {
                return TransformOutcome::ReviewRequired { text, detail: "Quality rewrite was empty or incomplete. The previous draft is available for manual review only.".into() };
            };
            // A quality rewrite must pass the same hard checks as the original generation.
            let validation = match &report.routing {
                Some(policy) => routing::validate_rewrite_with_context(policy, transcript, references, &revised),
                None => validate_graph(&revised).map(|_| ()).map_err(|error| error.to_string()),
            };
            if validation.is_err() {
                return TransformOutcome::ReviewRequired { text, detail: "Quality rewrite failed prompt validation. The previous valid graph is available for manual review only.".into() };
            }
            text = revised;
        }
        unreachable!("the second review always returns")
    }

    /// Validates the sanitized draft and runs at most `max_structure_repairs`
    /// rewrites inside the original deadline. Failed, truncated, or late repairs never authorize
    /// insertion of an invalid draft.
    fn check_structure(
        &self,
        generator: &dyn Generator,
        request: &GenerationRequest<'_>,
        newlines: NewlinePolicy,
        draft: String,
        adaptive: Option<(&ResolvedPromptPolicy, &str, &str)>,
        cancel: &CancelToken,
        on_event: &mut dyn FnMut(JobEvent<'_>),
    ) -> Result<(String, StructureCheck), TransformOutcome> {
        if cancel.is_cancelled() {
            return Err(TransformOutcome::Cancelled);
        }
        let validate = |text: &str| -> Result<(), String> {
            match adaptive {
                Some((policy, original, references)) => routing::validate_rewrite_with_context(policy, original, references, text).map_err(|error| {
                    format!("Rewrite only the finished prompt, preserving the user's final intent and supplied facts. Fix this violation: {error}\n\
                        Graph syntax is mandatory. Step 1 has NO dependencies. Step 2 can depend only on Step 1. Step 3 can depend only on Steps 1 and 2. Never list the current step or a later step in an after clause; omit a dependency rather than inventing one. \
                        Dependency clauses must contain only comma-separated earlier step numbers, for example (after 1, 2). Never put conditions or words such as 'or loop exit' inside a dependency clause; retry conditions belong on the separate Loop: line. \
                        Put a separate Loop: line with an existing return step and a limit of 1 to 8 rounds, then non-empty Done when: criteria. Keep the graph compact, retain every requested action, and include no explanation of your rewrite.")
                }),
                None => validate_graph(text).map(|_| ()).map_err(repair_instruction),
            }
        };
        let mut error = match validate(&draft) {
            Ok(()) => return Ok((draft, StructureCheck::Valid)),
            Err(error) => error,
        };
        let mut latest = draft.clone();
        for _ in 0..self.limits.max_structure_repairs {
            if cancel.is_cancelled() {
                return Err(TransformOutcome::Cancelled);
            }
            if Instant::now() >= request.deadline {
                log::warn!("task graph repair skipped: generation deadline reached");
                break;
            }
            on_event(JobEvent::Stage(Stage::Revising));
            let mut repair = if adaptive.is_some() {
                let mut system = request.messages[0].clone();
                system.content.push_str(&format!("\n\nRequired correction for this request:\n{error}\n\
                    Regenerate the complete prompt from the current request. Preserve its actual goal, named details, numbers and constraints; do not reuse a rejected draft or copy an example's goal."));
                vec![system, request.messages.last().expect("prompt has a user message").clone()]
            } else { request.messages.to_vec() };
            if adaptive.is_none() {
                repair.push(ChatMessage { role: Role::Assistant, content: latest.clone() });
                repair.push(ChatMessage { role: Role::User, content: error.clone() });
            }
            let repair_request = GenerationRequest { messages: &repair, stable_prefix: if adaptive.is_some() { 0 } else { request.stable_prefix }, ..*request };
            let mut forward = |token: &str| on_event(JobEvent::Token(token));
            let generation = match generator.generate(&repair_request, cancel, &mut forward) {
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
                    latest = text;
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
    format!(
        "Your prompt has a problem: {error}. Rewrite the complete prompt as a valid task graph, preserving the user's intent and details. \
Number the steps 1, 2, 3 in order and let each step depend only on earlier steps. Include at least one loop with a failure condition, \
an existing step to return to and a limit of 1 to {MAX_LOOP_ROUNDS} rounds, for example \"Loop: if the tests fail, return to Step 2 (max 3 rounds).\" \
Include a non-empty \"Done when:\" section with verifiable success criteria for the loop to check. Stop when the checks pass; \
if the round limit is reached, report what still fails instead of claiming success. Keep the target's required formatting. \
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

    /// Every AI prompt must be a task graph with a loop; test fakes return the smallest valid one.
    const FINISHED: &str = "Step 1: Write it.\nLoop: if it misses a requirement, return to Step 1 (max 2 rounds).\nDone when: all stated requirements are met.";

    #[derive(Default)]
    struct EchoGenerator {
        delay: Duration,
        calls: Mutex<Vec<Vec<ChatMessage>>>,
        outputs: Mutex<std::collections::VecDeque<Result<Generation, BackendError>>>,
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
            self.outputs.lock().unwrap().pop_front().unwrap_or_else(|| Ok(Generation { text: FINISHED.into(), finish: FinishReason::Stop }))
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
        let transform = Transform { input: Input::Text(text), mode: Mode::Prompt, profile, target, surrounding: None, use_history, auto_mode: false };
        f.service.run_scheduled(&transform, &CancelToken::default(), Duration::from_secs(5), &mut |_| {}).unwrap()
    }

    struct ScriptedReviewer {
        enabled: bool,
        decisions: Mutex<std::collections::VecDeque<Result<ReviewDecision, BackendError>>>,
        requests: Mutex<Vec<(String, String, Instant)>>,
        delay: Duration,
        cancel: bool,
    }

    impl PromptReviewer for ScriptedReviewer {
        fn enabled(&self) -> bool { self.enabled }

        fn review(&self, request: &ReviewRequest<'_>, cancel: &CancelToken) -> Result<ReviewDecision, BackendError> {
            self.requests.lock().unwrap().push((request.transcript.into(), request.draft.into(), request.deadline));
            std::thread::sleep(self.delay);
            if self.cancel { cancel.cancel(); }
            self.decisions.lock().unwrap().pop_front().expect("unexpected extra quality review")
        }
    }

    fn reviewer(decisions: Vec<Result<ReviewDecision, BackendError>>) -> ScriptedReviewer {
        ScriptedReviewer { enabled: true, decisions: Mutex::new(decisions.into()), requests: Mutex::default(), delay: Duration::ZERO, cancel: false }
    }

    #[test]
    fn review_is_optional_and_never_used_for_dictation() {
        let f = fixture(Duration::ZERO);
        let target = ActiveContext::default();
        let disabled = Arc::new(ScriptedReviewer { enabled: false, ..reviewer(vec![]) });
        f.service.set_prompt_reviewer(disabled.clone());
        assert!(matches!(run_text(&f, &target, "suggest improvements", false).outcome, TransformOutcome::Ready { .. }));
        assert!(disabled.requests.lock().unwrap().is_empty());

        let enabled = Arc::new(reviewer(vec![]));
        f.service.set_prompt_reviewer(enabled.clone());
        for auto_mode in [false, true] {
            let transform = Transform {
                input: Input::Text("plain speech"), mode: if auto_mode { Mode::Prompt } else { Mode::Dictation },
                profile: f.service.profiles().resolve(&target), target: &target,
                surrounding: None, use_history: false, auto_mode,
            };
            let result = f.service.run_scheduled(&transform, &CancelToken::default(), Duration::from_secs(1), &mut |_| {}).unwrap();
            assert_eq!(result.mode, Mode::Dictation);
            assert!(matches!(result.outcome, TransformOutcome::Ready { .. }));
        }
        assert!(enabled.requests.lock().unwrap().is_empty());
    }

    #[test]
    fn review_approval_preserves_graph_and_only_shares_request_and_draft() {
        let f = fixture(Duration::ZERO);
        let backend = Arc::new(reviewer(vec![Ok(ReviewDecision::Approved)]));
        f.service.set_prompt_reviewer(backend.clone());
        let target = ActiveContext::default();
        let result = run_text(&f, &target, "suggest improvements", true);
        assert_eq!(result.outcome, TransformOutcome::Ready { text: FINISHED.into() });
        let requests = backend.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!((&*requests[0].0, &*requests[0].1), ("suggest improvements", FINISHED));
        assert_eq!(f.generator.calls.lock().unwrap().len(), 1);
    }

    #[test]
    fn review_rewrites_once_then_requires_approval() {
        for second in [
            ReviewDecision::Approved,
            ReviewDecision::Revise { issues: vec!["Preserve the request.".into()] },
        ] {
            let f = fixture(Duration::ZERO);
            let backend = Arc::new(reviewer(vec![
                Ok(ReviewDecision::Revise { issues: vec!["Remove invented restrictions.".into()] }),
                Ok(second.clone()),
            ]));
            f.service.set_prompt_reviewer(backend.clone());
            let result = run_text(&f, &ActiveContext::default(), "suggest improvements", false);
            assert_eq!(matches!(result.outcome, TransformOutcome::Ready { .. }), second == ReviewDecision::Approved);
            if second != ReviewDecision::Approved {
                assert!(matches!(result.outcome, TransformOutcome::ReviewRequired { .. }));
            }
            let requests = backend.requests.lock().unwrap();
            assert_eq!(requests.len(), 2);
            assert_eq!(requests[0].2, requests[1].2, "all passes share the original deadline");
            let calls = f.generator.calls.lock().unwrap();
            assert_eq!(calls.len(), 2);
            assert!(calls[1].last().unwrap().content.contains("Remove invented restrictions."));
            assert!(calls[1].last().unwrap().content.contains("mandatory task graph"));
        }
    }

    #[test]
    fn unavailable_uncertain_or_disabled_review_never_approves_or_retries() {
        for decision in [
            Err(BackendError("Jev authentication failed.".into())),
            Ok(ReviewDecision::Uncertain { detail: "Jev is uncertain.".into() }),
            Ok(ReviewDecision::Skipped),
            Ok(ReviewDecision::Revise { issues: vec![] }),
        ] {
            let f = fixture(Duration::ZERO);
            f.service.set_prompt_reviewer(Arc::new(reviewer(vec![decision])));
            assert!(matches!(run_text(&f, &ActiveContext::default(), "suggest improvements", false).outcome,
                TransformOutcome::ReviewRequired { ref text, ref detail } if text == FINISHED && !detail.is_empty()));
            assert_eq!(f.generator.calls.lock().unwrap().len(), 1);
        }
    }

    #[test]
    fn failed_quality_rewrite_keeps_previous_draft_for_manual_review_only() {
        for output in [
            Ok(Generation { text: "Not a graph".into(), finish: FinishReason::Stop }),
            Ok(Generation { text: FINISHED.into(), finish: FinishReason::Length }),
            Err(BackendError("worker unavailable".into())),
        ] {
            let f = fixture(Duration::ZERO);
            *f.generator.outputs.lock().unwrap() = [
                Ok(Generation { text: FINISHED.into(), finish: FinishReason::Stop }), output,
            ].into();
            let backend = Arc::new(reviewer(vec![Ok(ReviewDecision::Revise { issues: vec!["Improve checks.".into()] })]));
            f.service.set_prompt_reviewer(backend.clone());
            assert!(matches!(run_text(&f, &ActiveContext::default(), "suggest improvements", false).outcome,
                TransformOutcome::ReviewRequired { ref text, .. } if text == FINISHED));
            assert_eq!(backend.requests.lock().unwrap().len(), 1);
        }
    }

    #[test]
    fn cancelled_and_late_reviews_cannot_authorize_output() {
        let f = fixture(Duration::ZERO);
        f.service.set_prompt_reviewer(Arc::new(ScriptedReviewer { cancel: true, ..reviewer(vec![Ok(ReviewDecision::Approved)]) }));
        assert_eq!(run_text(&f, &ActiveContext::default(), "suggest improvements", false).outcome, TransformOutcome::Cancelled);

        let mut f = fixture(Duration::ZERO);
        f.service.limits.generation_timeout = Duration::from_millis(20);
        f.service.set_prompt_reviewer(Arc::new(ScriptedReviewer { delay: Duration::from_millis(30), ..reviewer(vec![Ok(ReviewDecision::Approved)]) }));
        assert!(matches!(run_text(&f, &ActiveContext::default(), "suggest improvements", false).outcome, TransformOutcome::ReviewRequired { .. }));
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
            assert!(system.contains("Task structure") && !system.contains("Describe, do not instruct"));
            assert!(last(i).contains("Questions: none; go ahead") && last(i).contains("steps"));
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
            let transform = Transform { input, mode: Mode::Dictation, profile, target: &target, surrounding: None, use_history: false, auto_mode: false };
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
                    let transform = Transform { input: Input::Text("hello there"), mode: Mode::Prompt, profile, target: &target, surrounding: None, use_history: false, auto_mode: false };
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

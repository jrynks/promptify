//! Developer CLI: download models and run the real speech + prompt pipeline on a WAV file.
//! Uses the same data folder, models and settings as the desktop app.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use serde::Serialize;

use promptify_core::audio::{CaptureBuffer, TARGET_SAMPLE_RATE};
use promptify_core::context::{ActiveContext, ContextPolicy, FocusedText, WindowIdentity};
use promptify_core::eval::{self, Expect};
use promptify_core::history::{HistoryContext, HistoryLimits, HistoryLog, NewHistoryEntry};
use promptify_core::models::{Manifest, ModelKind, is_installed};
use promptify_core::pipeline::{
    BackendError, Backends, CancelToken, ContextProvider, Generator, History, Inserter, JobEvent, Limits, Mode, Orchestrator, Outcome,
    Transcriber,
};
use promptify_core::profiles::{PasteChord, ProfileSet};
use promptify_core::quality::DictationTone;
use promptify_core::routing::{self, Rendering, RoutingOptions, Surface, TaskId};
use promptify_lib::llm_client::{LlmWorker, worker_exe};
use promptify_lib::inference::InferenceManager;
use promptify_lib::settings::{self, SharedSettings};
use promptify_lib::stt::WhisperEngine;
use promptify_lib::{download, ensure_selection};

const USAGE: &str = "usage:
  promptify-cli models
  promptify-cli inference   (read-only inference selection/status; never prints secrets or endpoint URLs)
  promptify-cli download <model-id>...
  promptify-cli transcribe <file.wav>
  promptify-cli live-sim <file.wav>   (replays the file as if spoken; compares live chunks with one full pass)
  promptify-cli run <file.wav> [--model ID] [--mode prompt|dictation] [--dictation-tone clean_transcript|natural|casual|formal|concise|unhinged] [--process NAME] [--url URL] [--title TITLE] [--no-history]
  promptify-cli rewrite <text> [--model ID] [--process NAME] [--url URL] [--title TITLE] [--mode prompt|dictation] [--dictation-tone clean_transcript|natural|casual|formal|concise|unhinged] [--auto]
      (run/rewrite use saved inference: configured API/local-server providers may transmit text and incur billing;
       --model ID is an installed local-model override for this invocation only; clean_transcript needs no inference)
  promptify-cli screen-text   (reads the focused text box of the foreground app after 3 s, as the app would)
  promptify-cli paste-smoke-test <unique-window-title> <text>   (pastes into the matching focused test window after 3 s)
  promptify-cli generated-paste-smoke-test <unique-window-title> <request>   (generates an adaptive prompt and pastes into the matching test input)
  promptify-cli eval <cases.toml> [--model ID]   (local-only evaluation; never uses configured API/local-server providers)
  promptify-cli eval-adaptive <cases.toml> [--model ID]   (local model output contracts)
  promptify-cli eval-routing <cases.toml>   (classification only; no model needed)
  promptify-cli eval-quality <cases.toml> [--samples 3] [--heldout-samples 3] [--dictation-samples 3] [--prompt-only] [--deadline-seconds 60|120] [--fresh-heldout PATH] [--output PATH]
      (synthetic local quality benchmark; requires the already-selected qwen3.5-9b-q4km; never reads history or pastes)
  promptify-cli prompt-types   (list the bundled taxonomy and activation status)
  promptify-cli route <text> [--process NAME] [--url URL] [--surface SURFACE] [--prompt-type ID]
";

fn routing_flags(args: &[String]) -> Result<RoutingOptions, String> {
    let args = args.get(2..).unwrap_or_default();
    let rendering = match checked_flag(args, "--rendering")?.as_deref() {
        None | Some("legacy") => Rendering::Legacy,
        Some("adaptive") => Rendering::Adaptive,
        Some(other) => return Err(format!("unknown rendering policy {other}; use legacy or adaptive")),
    };
    let task_type = checked_flag(args, "--prompt-type")?.map(TaskId::try_from).transpose()?;
    let surface = checked_flag(args, "--surface")?.map(|value| {
        serde_json::from_value::<Surface>(serde_json::Value::String(value)).map_err(|e| format!("invalid surface: {e}"))
    }).transpose()?;
    let options = RoutingOptions { rendering, task_type, surface };
    options.validate()?;
    Ok(options)
}

fn checked_flag(args: &[String], name: &str) -> Result<Option<String>, String> {
    let positions: Vec<_> = args.iter().enumerate().filter(|(_, arg)| arg.as_str() == name).map(|(index, _)| index).collect();
    if positions.len() > 1 { return Err(format!("{name} may only be specified once")); }
    let Some(index) = positions.first() else { return Ok(None); };
    args.get(index + 1).filter(|value| !value.starts_with("--")).cloned()
        .map(Some).ok_or_else(|| format!("{name} requires a value"))
}

fn dictation_tone_flag(args: &[String], default: DictationTone) -> Result<DictationTone, String> {
    checked_flag(args, "--dictation-tone")?
        .map(|value| DictationTone::parse(&value))
        .transpose()
        .map(|tone| tone.unwrap_or(default))
}

fn needs_inference(mode: Mode, tone: DictationTone) -> bool {
    mode == Mode::Prompt || tone != DictationTone::CleanTranscript
}

fn local_model_override(
    manifest: &Manifest,
    models_dir: &Path,
    app_settings: &mut settings::AppSettings,
    id: &str,
) -> Result<(), String> {
    let entry = manifest.get(id).filter(|entry| entry.kind == ModelKind::Llm)
        .ok_or_else(|| format!("unknown language model: {id}"))?;
    if !is_installed(models_dir, entry) {
        return Err(format!("language model {id} is not installed"));
    }
    app_settings.llm_model = Some(id.to_owned());
    Ok(())
}

fn ordinary_generator(
    data_dir: PathBuf,
    manifest: Manifest,
    models_dir: PathBuf,
    shared: SharedSettings,
    local_override: Option<&str>,
    inference_needed: bool,
) -> Result<Arc<dyn Generator>, String> {
    // Clean Transcript never consults provider configuration, models or credentials.
    if !inference_needed {
        return Ok(Arc::new(LlmWorker::new(worker_exe(), manifest, models_dir, shared)));
    }
    if let Some(id) = local_override {
        local_model_override(&manifest, &models_dir, &mut shared.write().unwrap(), id)?;
        let worker = Arc::new(LlmWorker::new(worker_exe(), manifest, models_dir, shared));
        worker.preload().map_err(|error| error.0)?;
        return Ok(worker);
    }
    let manager = Arc::new(InferenceManager::new(data_dir, manifest, models_dir, shared));
    manager.preload().map_err(|error| error.0)?;
    Ok(manager)
}

/// Feeds typed text through the pipeline in place of speech.
struct TextTranscriber(String);

impl Transcriber for TextTranscriber {
    fn transcribe(&self, _: &[f32], _: &CancelToken) -> Result<String, BackendError> {
        Ok(self.0.clone())
    }
}

/// Evaluation and typed rewrites must neither read nor write the user's history.
#[derive(Default)]
struct NoHistory {
    previous: Option<promptify_core::history::PreviousPrompt>,
}

impl History for NoHistory {
    fn context(&self, _: &str, _: &str) -> HistoryContext {
        HistoryContext { previous: self.previous.clone(), ..HistoryContext::default() }
    }
    fn record(&self, _: NewHistoryEntry) -> Result<bool, BackendError> {
        Ok(false)
    }
}

fn text_context(process: String, url: Option<String>, title: String) -> ActiveContext {
    ActiveContext { window: WindowIdentity { handle: 1, process_id: 1 }, process_name: process, window_title: title, url }
}

fn rewrite_text(
    llm: Arc<dyn Generator>,
    ctx: ActiveContext,
    text: &str,
) -> Result<promptify_core::pipeline::JobReport, String> {
    rewrite_with(llm, ctx, text, Mode::Prompt, false, DictationTone::Natural, RoutingOptions::default())
}

fn rewrite_with(
    llm: Arc<dyn Generator>,
    ctx: ActiveContext,
    text: &str,
    mode: Mode,
    auto_mode: bool,
    dictation_tone: DictationTone,
    routing: RoutingOptions,
) -> Result<promptify_core::pipeline::JobReport, String> {
    rewrite_with_deadline(
        llm,
        ctx,
        text,
        mode,
        auto_mode,
        dictation_tone,
        routing,
        Duration::from_secs(120),
    )
}

fn rewrite_with_deadline(
    llm: Arc<dyn Generator>,
    ctx: ActiveContext,
    text: &str,
    mode: Mode,
    auto_mode: bool,
    dictation_tone: DictationTone,
    routing: RoutingOptions,
    deadline: Duration,
) -> Result<promptify_core::pipeline::JobReport, String> {
    rewrite_with_fixture(llm, ctx, text, mode, auto_mode, dictation_tone, routing, deadline, None)
}

fn rewrite_with_fixture(
    llm: Arc<dyn Generator>,
    ctx: ActiveContext,
    text: &str,
    mode: Mode,
    auto_mode: bool,
    dictation_tone: DictationTone,
    routing: RoutingOptions,
    deadline: Duration,
    previous_prompt: Option<&str>,
) -> Result<promptify_core::pipeline::JobReport, String> {
    if mode != Mode::Prompt && (routing.task_type.is_some() || routing.surface.is_some()) {
        return Err("Task and surface overrides apply only to Prompt mode.".into());
    }
    let backends = Backends {
        context: Arc::new(FixedContext(ctx)),
        transcriber: Arc::new(TextTranscriber(text.to_owned())),
        generator: llm,
        inserter: Arc::new(PrintInserter),
        history: Arc::new(NoHistory {
            previous: previous_prompt.map(|text| promptify_core::history::PreviousPrompt { text: text.to_owned(), minutes_ago: 1 }),
        }),
    };
    let limits = Limits { generation_timeout: deadline, ..Limits::default() };
    let orchestrator = Orchestrator::new(backends, ProfileSet::bundled(), ContextPolicy::default(), limits);
    orchestrator.set_auto_mode(auto_mode);
    orchestrator.set_dictation_tone(dictation_tone);
    orchestrator.queue_routing(routing)?;
    let job = orchestrator.begin(mode).map_err(|e| e.to_string())?;
    let show = std::env::var_os("PROMPTIFY_EVAL_SHOW").is_some();
    let mut revising = false;
    let mut revised = String::new();
    let report = orchestrator.finish(job, &[], &mut |event| match event {
        JobEvent::Stage(promptify_core::pipeline::Stage::Revising) if show => {
            revising = true;
            revised.clear();
        }
        JobEvent::Token(text) if show && revising => revised.push_str(text),
        _ => {}
    });
    if show
        && (report.structure == Some(promptify_core::pipeline::StructureCheck::KeptOriginal) || matches!(&report.outcome, Outcome::Failed { .. }))
        && !revised.trim().is_empty()
    {
        eprintln!("Rejected repair:\n{revised}\n---");
    }
    Ok(report)
}

#[derive(Serialize)]
struct QualityEvalSample {
    suite: &'static str,
    case_id: String,
    category: String,
    sample: u32,
    mode: &'static str,
    policy: Option<&'static str>,
    tone: Option<&'static str>,
    request: String,
    synthetic_previous_prompt: Option<String>,
    output: Option<String>,
    outcome: &'static str,
    block_reason: Option<String>,
    failure_reason: Option<String>,
    failure_detail: Option<String>,
    structure: Option<String>,
    quality: Option<promptify_core::quality::QualityReport>,
    structure_repair_attempts: u8,
    usable: bool,
    auto_paste_eligible: bool,
    output_chars: usize,
    generation_elapsed_ms: u64,
    wall_elapsed_ms: u64,
    reported_elapsed_ms: u64,
    history_saved: bool,
}

#[derive(Default, Serialize)]
struct QualityEvalSummary {
    sample_count: usize,
    usable_count: usize,
    auto_paste_eligible_count: usize,
    inserted_count: usize,
    blocked_count: usize,
    failed_count: usize,
    cancelled_count: usize,
    no_speech_count: usize,
    unusable_count: usize,
    review_only_count: usize,
    rejected_count: usize,
    unavailable_count: usize,
    deadline_exhausted_count: usize,
    structure_repair_attempts: u64,
    rewrite_calls: u64,
    review_calls: u64,
    total_wall_elapsed_ms: u64,
    total_generation_elapsed_ms: u64,
    total_review_elapsed_ms: u64,
    total_rewrite_elapsed_ms: u64,
    total_reported_elapsed_ms: u64,
    total_output_chars: usize,
    mean_wall_elapsed_ms: f64,
    mean_reported_elapsed_ms: f64,
    mean_generation_elapsed_ms: f64,
    mean_review_elapsed_ms: f64,
    mean_rewrite_elapsed_ms: f64,
    mean_output_chars: f64,
}

impl QualityEvalSummary {
    fn add(&mut self, sample: &QualityEvalSample) {
        self.sample_count += 1;
        self.usable_count += usize::from(sample.usable);
        self.auto_paste_eligible_count += usize::from(sample.auto_paste_eligible);
        self.inserted_count += usize::from(sample.outcome == "inserted");
        self.blocked_count += usize::from(sample.outcome == "blocked");
        self.failed_count += usize::from(sample.outcome == "failed");
        self.cancelled_count += usize::from(sample.outcome == "cancelled");
        self.no_speech_count += usize::from(sample.outcome == "no_speech");
        self.review_only_count += usize::from(sample.block_reason.as_deref() == Some("QualityReview"));
        self.total_reported_elapsed_ms += sample.reported_elapsed_ms;
        self.total_output_chars += sample.output_chars;
        if let Some(quality) = sample.quality {
            self.rejected_count += usize::from(quality.status == promptify_core::quality::QualityStatus::Rejected);
            self.unavailable_count += usize::from(quality.status == promptify_core::quality::QualityStatus::Unavailable);
            self.rewrite_calls += u64::from(quality.rewrite_calls);
            self.review_calls += u64::from(quality.review_calls);
            self.total_review_elapsed_ms += quality.review_elapsed_ms;
            self.total_rewrite_elapsed_ms += quality.rewrite_elapsed_ms;
        }
        self.total_wall_elapsed_ms += sample.wall_elapsed_ms;
        self.total_generation_elapsed_ms += sample.generation_elapsed_ms;
        self.deadline_exhausted_count += usize::from(
            sample.quality.is_some_and(|quality| quality.deadline_exhausted)
                || matches!(sample.failure_reason.as_deref(), Some("TimedOut" | "EngineBusy")),
        );
        self.structure_repair_attempts += u64::from(sample.structure_repair_attempts);
    }

    fn finalize(&mut self) {
        let count = self.sample_count.max(1) as f64;
        self.unusable_count = self.sample_count.saturating_sub(self.usable_count);
        self.mean_wall_elapsed_ms = self.total_wall_elapsed_ms as f64 / count;
        self.mean_reported_elapsed_ms = self.total_reported_elapsed_ms as f64 / count;
        self.mean_generation_elapsed_ms = self.total_generation_elapsed_ms as f64 / count;
        self.mean_review_elapsed_ms = self.total_review_elapsed_ms as f64 / count;
        self.mean_rewrite_elapsed_ms = self.total_rewrite_elapsed_ms as f64 / count;
        self.mean_output_chars = self.total_output_chars as f64 / count;
    }
}

fn append_quality_sample(
    samples: &mut Vec<QualityEvalSample>,
    summaries: &mut std::collections::BTreeMap<String, QualityEvalSummary>,
    suite: &'static str,
    id: &str,
    category: &str,
    sample_index: u32,
    mode: Mode,
    policy: Option<Rendering>,
    tone: Option<DictationTone>,
    request: &str,
    previous_prompt: Option<&str>,
    report: promptify_core::pipeline::JobReport,
    wall_elapsed_ms: u64,
) {
    let output = outcome_text(&report.outcome).map(str::to_owned);
    let block_reason = match &report.outcome {
        Outcome::Blocked { reason, .. } => Some(format!("{reason:?}")),
        _ => None,
    };
    let failure_reason = match &report.outcome {
        Outcome::Failed { reason, .. } => Some(format!("{reason:?}")),
        _ => None,
    };
    let failure_detail = match &report.outcome {
        Outcome::Failed { detail, .. } => detail.clone(),
        _ => None,
    };
    let quality = report.quality;
    let usable = output.as_deref().is_some_and(|text| !text.trim().is_empty())
        && !matches!(&report.outcome, Outcome::Blocked { reason: promptify_core::pipeline::BlockReason::OutputTruncated, .. });
    let policy_name = policy.map(|rendering| match rendering {
        Rendering::Legacy => "legacy",
        Rendering::Adaptive => "adaptive",
    });
    let tone_name = tone.map(DictationTone::as_str);
    let group_key = format!(
        "{suite}:{}:{}",
        policy_name.unwrap_or("none"),
        tone_name.unwrap_or("none")
    );
    let sample = QualityEvalSample {
        suite,
        case_id: id.to_owned(),
        category: category.to_owned(),
        sample: sample_index,
        mode: match mode {
            Mode::Prompt => "prompt",
            Mode::Dictation => "dictation",
        },
        policy: policy_name,
        tone: tone_name,
        request: request.to_owned(),
        synthetic_previous_prompt: previous_prompt.map(str::to_owned),
        output_chars: output.as_deref().map_or(0, |text| text.chars().count()),
        output,
        outcome: report.outcome.kind(),
        block_reason,
        failure_reason,
        failure_detail,
        structure: report.structure.map(|structure| format!("{structure:?}")),
        quality,
        structure_repair_attempts: report.structure_repair_attempts,
        usable,
        auto_paste_eligible: matches!(report.outcome, Outcome::Inserted { .. }),
        wall_elapsed_ms,
        generation_elapsed_ms: report.generation_elapsed_ms,
        reported_elapsed_ms: report.elapsed_ms,
        history_saved: report.history_saved,
    };
    summaries.entry(group_key).or_default().add(&sample);
    if std::env::var_os("PROMPTIFY_EVAL_SHOW").is_some() {
        if let Ok(record) = serde_json::to_string(&sample) {
            eprintln!("sample-json: {record}");
        }
    }
    samples.push(sample);
}

fn quality_eval_count(args: &[String], flag: &str, default: u32) -> Result<u32, String> {
    let Some(value) = checked_flag(args, flag)? else { return Ok(default); };
    let count = value.parse::<u32>().map_err(|_| format!("{flag} must be a positive integer"))?;
    if !(1..=100).contains(&count) {
        return Err(format!("{flag} must be between 1 and 100"));
    }
    Ok(count)
}

fn quality_eval_deadline(args: &[String]) -> Result<u64, String> {
    match checked_flag(args, "--deadline-seconds")?.as_deref() {
        None | Some("120") => Ok(120),
        Some("60") => Ok(60),
        Some(other) => Err(format!("--deadline-seconds must be 60 (desktop) or 120 (CLI), not {other}")),
    }
}

fn run_quality_eval(
    args: &[String],
    manifest: Manifest,
    models_dir: PathBuf,
    app_settings: settings::AppSettings,
) -> Result<(), String> {
    const REQUIRED_MODEL: &str = "qwen3.5-9b-q4km";
    if app_settings.llm_model.as_deref() != Some(REQUIRED_MODEL) {
        return Err(format!(
            "quality evaluation requires the already-selected {REQUIRED_MODEL}; current selection is {:?}. No model was changed or downloaded.",
            app_settings.llm_model
        ));
    }
    let model = manifest.get(REQUIRED_MODEL).filter(|entry| entry.kind == ModelKind::Llm)
        .ok_or_else(|| format!("{REQUIRED_MODEL} is not in the bundled model manifest"))?;
    if !is_installed(&models_dir, model) {
        return Err(format!("{REQUIRED_MODEL} is not installed; quality evaluation will not download it"));
    }
    let path = PathBuf::from(args.get(1).ok_or(USAGE)?);
    let mut source = std::fs::read_to_string(&path).map_err(|error| format!("cannot read {path:?}: {error}"))?;
    let base_suite = eval::load_quality_suite(&source)?;
    let base_heldout_count = base_suite.heldout_prompt.len();
    let fresh_heldout_path = checked_flag(args.get(2..).unwrap_or_default(), "--fresh-heldout")?;
    if let Some(fresh_path) = &fresh_heldout_path {
        let fragment = std::fs::read_to_string(fresh_path)
            .map_err(|error| format!("cannot read fresh held-out cases {fresh_path:?}: {error}"))?;
        source.push_str("\n\n");
        source.push_str(&fragment);
    }
    let suite = eval::load_quality_suite(&source)?;
    let fresh_heldout_count = suite.heldout_prompt.len().saturating_sub(base_heldout_count);
    if fresh_heldout_path.is_some() && fresh_heldout_count == 0 {
        return Err("--fresh-heldout must add at least one new [[heldout_prompt]] case".into());
    }
    let output_path = PathBuf::from(
        checked_flag(args.get(2..).unwrap_or_default(), "--output")?
            .unwrap_or_else(|| "eval/quality-review-results.json".into()),
    );
    if output_path.exists() {
        return Err(format!("refusing to overwrite existing evaluation evidence at {}", output_path.display()));
    }
    let original_samples = quality_eval_count(args.get(2..).unwrap_or_default(), "--samples", 3)?;
    let heldout_samples = quality_eval_count(args.get(2..).unwrap_or_default(), "--heldout-samples", 3)?;
    let dictation_samples = quality_eval_count(args.get(2..).unwrap_or_default(), "--dictation-samples", 3)?;
    let prompt_only = args.iter().any(|arg| arg == "--prompt-only");
    if args.iter().filter(|arg| arg.as_str() == "--prompt-only").count() > 1 {
        return Err("--prompt-only may only be specified once".into());
    }
    let deadline_seconds = quality_eval_deadline(args.get(2..).unwrap_or_default())?;
    let deadline = Duration::from_secs(deadline_seconds);

    let shared: SharedSettings = Arc::new(RwLock::new(app_settings));
    let llm = Arc::new(LlmWorker::new(worker_exe(), manifest, models_dir, shared));
    let preload_started = Instant::now();
    llm.preload().map_err(|error| error.0)?;
    let preload_elapsed_ms = preload_started.elapsed().as_millis() as u64;
    let mut samples = Vec::new();
    let mut summaries = std::collections::BTreeMap::new();
    let policies = [Rendering::Legacy, Rendering::Adaptive];
    for (suite_name, cases, sample_count) in [
        ("original", suite.prompt.as_slice(), original_samples),
        ("heldout", suite.heldout_prompt.as_slice(), heldout_samples),
    ] {
        for case in cases {
            let context = text_context(case.process.clone(), case.url.clone(), case.title.clone());
            for policy in policies {
                for sample_index in 1..=sample_count {
                    let started = Instant::now();
                    let report = rewrite_with_fixture(
                        llm.clone(),
                        context.clone(),
                        &case.said,
                        Mode::Prompt,
                        false,
                        DictationTone::Natural,
                        RoutingOptions { rendering: policy, ..RoutingOptions::default() },
                        deadline,
                        case.previous_prompt.as_deref(),
                    )?;
                    append_quality_sample(
                        &mut samples,
                        &mut summaries,
                        suite_name,
                        &case.id,
                        &case.category,
                        sample_index,
                        Mode::Prompt,
                        Some(policy),
                        None,
                        &case.said,
                        case.previous_prompt.as_deref(),
                        report,
                        started.elapsed().as_millis() as u64,
                    );
                    let latest = samples.last().expect("sample was appended");
                    eprintln!(
                        "{} {} {} sample {sample_index}/{sample_count}: outcome={} quality={:?} {}ms",
                        suite_name,
                        case.id,
                        if policy == Rendering::Legacy { "legacy" } else { "adaptive" },
                        latest.outcome,
                        latest.quality.map(|quality| quality.status),
                        latest.wall_elapsed_ms
                    );
                }
            }
        }
    }

    let rewrite_tones = [
        DictationTone::CleanTranscript,
        DictationTone::Natural,
        DictationTone::Casual,
        DictationTone::Formal,
        DictationTone::Concise,
        DictationTone::Unhinged,
    ];
    for case in suite.dictation.iter().filter(|_| !prompt_only) {
        let context = text_context("notepad.exe".into(), None, String::new());
        for tone in rewrite_tones {
            for sample_index in 1..=dictation_samples {
                let started = Instant::now();
                let report = rewrite_with_deadline(
                    llm.clone(),
                    context.clone(),
                    &case.said,
                    Mode::Dictation,
                    true,
                    tone,
                    RoutingOptions::default(),
                    deadline,
                )?;
                append_quality_sample(
                    &mut samples,
                    &mut summaries,
                    "dictation",
                    &case.id,
                    &case.category,
                    sample_index,
                    Mode::Dictation,
                    None,
                    Some(tone),
                    &case.said,
                    None,
                    report,
                    started.elapsed().as_millis() as u64,
                );
                let latest = samples.last().expect("sample was appended");
                eprintln!(
                    "dictation {} {} sample {sample_index}/{dictation_samples}: outcome={} quality={:?} {}ms",
                    case.id,
                    tone.as_str(),
                    latest.outcome,
                    latest.quality.map(|quality| quality.status),
                    latest.wall_elapsed_ms
                );
            }
        }
    }
    if samples.iter().any(|sample| sample.history_saved) {
        return Err("quality evaluation unexpectedly wrote a history entry".into());
    }
    summaries.values_mut().for_each(QualityEvalSummary::finalize);
    let document = serde_json::json!({
        "format_version": 1,
        "model_id": REQUIRED_MODEL,
        "evaluated_at_unix_seconds": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_secs(),
        "execution": {
            "preload_elapsed_ms": preload_elapsed_ms,
            "original_samples_per_request": original_samples,
            "heldout_samples_per_request": heldout_samples,
            "dictation_samples_per_input_and_tone": dictation_samples,
            "prompt_only": prompt_only,
            "synthetic_previous_context_case_count": suite.prompt.iter().chain(&suite.heldout_prompt)
                .filter(|case| case.previous_prompt.is_some()).count(),
            "original_case_count": suite.prompt.len(),
            "heldout_case_count": suite.heldout_prompt.len(),
            "fresh_heldout_case_count": fresh_heldout_count,
            "fresh_heldout_path": fresh_heldout_path,
            "heldout_case_ids": suite.heldout_prompt.iter().map(|case| case.id.as_str()).collect::<Vec<_>>(),
            "policy_deadline_seconds": deadline_seconds,
            "deadline_profile": if deadline_seconds == 60 { "desktop" } else { "cli" },
            "sampling": {
                "temperature": 0.3,
                "top_k": 40,
                "top_p": 0.9,
                "min_p": 0.05,
                "seed": "worker request ID, incremented per generation"
            },
            "max_new_tokens": 768,
            "max_structure_repairs": 2,
            "max_output_characters": 6000,
            "review_schema_version": 1,
            "history_accessed": false,
            "native_paste": false,
            "model_changed_or_downloaded": false,
            "model_reused_for_all_samples": true
        },
        "summary": summaries,
        "samples": samples
    });
    let encoded = serde_json::to_vec_pretty(&document).map_err(|error| error.to_string())?;
    let parent = output_path.parent().filter(|parent| !parent.as_os_str().is_empty()).unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&output_path)
        .map_err(|error| format!("cannot create {} without overwriting evidence: {error}", output_path.display()))?;
    std::io::Write::write_all(&mut file, &encoded).map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    println!(
        "Saved {} synthetic samples to {}; model={REQUIRED_MODEL}; preload={}ms; history/paste disabled.",
        samples.len(),
        output_path.display(),
        preload_elapsed_ms
    );
    for (group, summary) in summaries {
        let mean_wall = summary.total_wall_elapsed_ms / summary.sample_count.max(1) as u64;
        let mean_reviews = summary.review_calls as f64 / summary.sample_count.max(1) as f64;
        println!(
            "{group}: usable {}/{}, auto-eligible {}/{}, rejected={}, unavailable={}, deadline={}, mean wall={}ms, reviews/sample={mean_reviews:.2}",
            summary.usable_count,
            summary.sample_count,
            summary.auto_paste_eligible_count,
            summary.sample_count,
            summary.rejected_count,
            summary.unavailable_count,
            summary.deadline_exhausted_count,
            mean_wall
        );
    }
    Ok(())
}

fn outcome_text(outcome: &Outcome) -> Option<&str> {
    match outcome {
        Outcome::Inserted { text } | Outcome::Blocked { text, .. } => Some(text),
        _ => None,
    }
}

struct FixedContext(ActiveContext);

impl ContextProvider for FixedContext {
    fn identify(&self) -> Result<ActiveContext, BackendError> {
        Ok(self.0.clone())
    }
    fn focused_text(&self, _: &WindowIdentity) -> Result<Option<FocusedText>, BackendError> {
        Ok(None)
    }
    fn foreground(&self) -> Result<WindowIdentity, BackendError> {
        Ok(self.0.window)
    }
}

struct PrintInserter;

impl Inserter for PrintInserter {
    fn insert(&self, _: &WindowIdentity, _: &str, _: PasteChord) -> Result<(), BackendError> {
        Ok(())
    }
}

fn read_wav(path: &Path) -> Result<Vec<f32>, String> {
    let mut reader = hound::WavReader::open(path).map_err(|e| format!("cannot read {path:?}: {e}"))?;
    let spec = reader.spec();
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<_, _>>().map_err(|e| e.to_string())?,
        hound::SampleFormat::Int => {
            let scale = (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader.samples::<i32>().map(|s| s.map(|v| v as f32 / scale)).collect::<Result<_, _>>().map_err(|e| e.to_string())?
        }
    };
    let mut buffer = CaptureBuffer::new(spec.channels, spec.sample_rate, TARGET_SAMPLE_RATE as usize * 600);
    buffer.push_interleaved(&samples);
    Ok(buffer.finish().samples)
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

struct StderrLogger;

impl log::Log for StderrLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Info
    }
    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            eprintln!("[{}] {}", record.level(), record.args());
        }
    }
    fn flush(&self) {}
}

fn main() {
    if std::env::var_os("PROMPTIFY_LOG").is_some() {
        let _ = log::set_logger(&StderrLogger).map(|()| log::set_max_level(log::LevelFilter::Info));
    }
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn prepare_native_paste_test() -> Result<(), String> {
    #[cfg(target_os = "linux")]
    if promptify_lib::wayland_paste::applies() {
        promptify_lib::wayland_paste::init(&settings::app_data_dir());
        if !promptify_lib::wayland_paste::has_saved_permission()? {
            return Err("Grant desktop integration permission in Settings before running the Wayland paste test.".into());
        }
        promptify_lib::wayland_paste::grant(|| {})?;
    }
    Ok(())
}

fn finish_native_paste_test() -> Result<(), String> {
    if std::env::var_os("PROMPTIFY_NATIVE_TEST_ACK").is_none() {
        return Ok(());
    }
    use std::io::Write;
    std::io::stdout().flush().map_err(|error| error.to_string())?;
    let mut acknowledgement = String::new();
    std::io::stdin().read_line(&mut acknowledgement).map_err(|error| error.to_string())?;
    if acknowledgement.trim() != "observed" {
        return Err("The native harness did not acknowledge exact destination contents. Clipboard restoration was not attempted.".into());
    }
    promptify_lib::insert::restore_previous().map_err(|error| error.0)
}

fn validate_retired_options(args: &[String]) -> Result<(), String> {
    for option in ["--api-key", "--token", "--access-token", "--refresh-token", "--client-secret", "--authorization"] {
        if args.iter().skip(1).any(|arg| arg == option || arg.starts_with(&format!("{option}="))) {
            return Err("Credentials are not accepted on the command line. Configure inference securely in Settings.".into());
        }
    }
    if args.first().is_some_and(|command| ["mcp", "serve", "remote"].contains(&command.as_str())) {
        return Err("MCP and remote-control commands have been removed. Use rewrite or run with inference configured in Settings.".into());
    }
    for option in ["--mcp", "--api", "--relay", "--listen", "--advertise", "--offer-file", "--discoverable", "--identity", "--direct"] {
        if args.iter().skip(2).any(|arg| arg == option || arg.starts_with(&format!("{option}="))) {
            return Err(format!("{option} is no longer supported; configure inference in Settings instead."));
        }
    }
    if checked_flag(args.get(2..).unwrap_or_default(), "--mode")?.as_deref() == Some("answer")
        || args.iter().skip(2).any(|arg| arg == "--mode=answer")
    {
        return Err("Answer mode has been removed. Use prompt or dictation.".into());
    }
    Ok(())
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    validate_retired_options(&args)?;
    if args.first().is_some_and(|arg| arg == "generated-paste-smoke-test") {
        if args.len() != 3 || !args[1].starts_with("Promptify paste test ") || args[2].trim().is_empty() {
            return Err("Use generated-paste-smoke-test with a unique test-window title and a nonempty request.".into());
        }
        prepare_native_paste_test()?;
        let data_dir = settings::app_data_dir();
        let saved = settings::load_checked(&data_dir)?.ok_or("Configure models in Promptify before running the generated native test.")?;
        let manifest = Manifest::bundled();
        let shared = Arc::new(RwLock::new(saved));
        let worker = Arc::new(LlmWorker::new(worker_exe(), manifest, settings::models_dir(&data_dir), shared));
        std::thread::sleep(Duration::from_secs(3));
        let context = promptify_lib::system_context::SystemContext;
        let target = context.identify().map_err(|error| error.0)?;
        if !target.window_title.contains(&args[1]) {
            return Err("The unique test window is not focused. No generation or paste was attempted.".into());
        }
        if !context.destination(&target.window).map_err(|error| error.0)?.confirmed() {
            return Err("The native test input was not confirmed writable.".into());
        }
        let orchestrator = Orchestrator::new(Backends {
            context: Arc::new(context),
            transcriber: Arc::new(TextTranscriber(args[2].clone())),
            generator: worker,
            inserter: Arc::new(promptify_lib::insert::ClipboardPaste),
            history: Arc::new(NoHistory::default()),
        }, ProfileSet::bundled(), ContextPolicy::default(), Limits::default());
        orchestrator.set_rendering(Rendering::Adaptive);
        let job = orchestrator.begin(Mode::Prompt).map_err(|error| error.to_string())?;
        let report = orchestrator.finish(job, &[], &mut |_| {});
        if !matches!(report.outcome, Outcome::Inserted { .. }) {
            return Err(format!("Generated native test did not dispatch: {:?}", report.outcome));
        }
        println!("{}", serde_json::to_string(&report).map_err(|error| error.to_string())?);
        return finish_native_paste_test();
    }
    if args.first().is_some_and(|arg| arg == "paste-smoke-test") {
        if args.len() != 3 || !args[1].starts_with("Promptify paste test ") || args[2].is_empty() {
            return Err("Use paste-smoke-test with a unique 'Promptify paste test ...' window title and nonempty text.".into());
        }
        prepare_native_paste_test()?;
        std::thread::sleep(Duration::from_secs(3));
        let context = promptify_lib::system_context::SystemContext;
        let target = context.identify().map_err(|error| error.0)?;
        if !target.window_title.contains(&args[1]) {
            return Err("The test window is not focused. No paste was attempted.".into());
        }
        let destination = context.destination(&target.window).map_err(|error| error.0)?;
        if !destination.confirmed() {
            return Err("The native test input was not confirmed writable. No paste was attempted.".into());
        }
        promptify_lib::insert::ClipboardPaste.insert_into(&target.window, &destination, &args[2], PasteChord::Standard).map_err(|error| error.0)?;
        println!("Native paste dispatched; the test harness must verify the field contents.");
        return finish_native_paste_test();
    }
    if args.first().is_some_and(|arg| arg == "prompt-types") {
        println!("{}", serde_json::to_string_pretty(routing::catalog().all()).map_err(|e| e.to_string())?);
        return Ok(());
    }
    if args.first().is_some_and(|arg| arg == "route") {
        let text = args.get(1).ok_or(USAGE)?;
        let ctx = text_context(flag(&args, "--process").unwrap_or_default(), flag(&args, "--url"), flag(&args, "--title").unwrap_or_default());
        let mut flags = args.clone();
        if flag(&flags, "--rendering").is_none() {
            flags.extend(["--rendering".into(), "adaptive".into()]);
        }
        let options = routing_flags(&flags)?;
        let profiles = ProfileSet::bundled();
        let policy = routing::resolve(&ctx, profiles.resolve(&ctx), text, &options)?;
        println!("{}", serde_json::to_string_pretty(&policy).map_err(|e| e.to_string())?);
        return Ok(());
    }
    if args.first().is_some_and(|arg| arg == "eval-routing") {
        let path = args.get(1).ok_or(USAGE)?;
        let source = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let score = eval::score_routing(&eval::load_adaptive_cases(&source)?);
        println!("{}", serde_json::to_string_pretty(&score).map_err(|e| e.to_string())?);
        return if score.passed == score.total && score.macro_f1 >= 0.90 { Ok(()) } else { Err("routing evaluation failed".into()) };
    }
    if args.first().is_some_and(|arg| arg == "eval-quality") {
        if checked_flag(args.get(2..).unwrap_or_default(), "--model")?.is_some() {
            return Err("eval-quality uses only the already-selected model and does not accept --model".into());
        }
        let data_dir = settings::app_data_dir();
        let app_settings = settings::load_checked(&data_dir)?
            .ok_or("Configure the selected local language model in Promptify before quality evaluation.")?;
        return run_quality_eval(&args, Manifest::bundled(), settings::models_dir(&data_dir), app_settings);
    }
    if args.first().is_some_and(|arg| arg == "inference") {
        if args.len() != 1 {
            return Err("usage: promptify-cli inference (read-only; configure inference in Settings)".into());
        }
        let data_dir = settings::app_data_dir();
        let shared = Arc::new(RwLock::new(settings::load_checked(&data_dir)?.unwrap_or_default()));
        let manager = InferenceManager::new(data_dir.clone(), Manifest::bundled(), settings::models_dir(&data_dir), shared);
        println!("{}", serde_json::to_string_pretty(&manager.status()).map_err(|error| error.to_string())?);
        return Ok(());
    }
    let data_dir = settings::app_data_dir();
    std::fs::create_dir_all(&data_dir).map_err(|e| e.to_string())?;
    let models_dir = settings::models_dir(&data_dir);
    let manifest = Manifest::bundled();
    let mut app_settings = settings::load_checked(&data_dir)?.unwrap_or_default();
    let model_override = if matches!(args.first().map(String::as_str), Some("rewrite" | "run" | "eval" | "eval-adaptive")) {
        checked_flag(args.get(2..).unwrap_or_default(), "--model")?
    } else {
        None
    };
    if ensure_selection(&manifest, &models_dir, &mut app_settings) {
        if model_override.is_none() {
            settings::save(&data_dir, &app_settings).map_err(|e| e.to_string())?;
        }
    }
    if matches!(args.first().map(String::as_str), Some("eval" | "eval-adaptive"))
        && let Some(id) = model_override.as_deref()
    {
        local_model_override(&manifest, &models_dir, &mut app_settings, id)?;
    }

    match args.first().map(String::as_str) {
        Some("models") => {
            for e in &manifest.models {
                let installed = promptify_core::models::is_installed(&models_dir, e);
                let selected = [&app_settings.stt_model, &app_settings.llm_model].iter().any(|s| s.as_deref() == Some(e.id.as_str()));
                println!("{:<28} {:?}/{:?} installed={installed} selected={selected}  {}", e.id, e.kind, e.tier, e.display_name);
            }
            println!("models folder: {}", models_dir.display());
        }
        Some("download") => {
            for id in &args[1..] {
                let entry = manifest.get(id).ok_or(format!("unknown model {id}"))?;
                let started = Instant::now();
                let mut last = 0u64;
                let path = download::download(&models_dir, entry, &CancelToken::default(), &mut |p| {
                    let pct = p.downloaded * 100 / p.total.max(1);
                    if p.verifying {
                        eprintln!("{id}: verifying SHA-256");
                    } else if pct >= last + 10 {
                        last = pct;
                        eprintln!("{id}: {pct}%");
                    }
                })?;
                println!("{id}: installed at {} in {:.0?}", path.display(), started.elapsed());
            }
            if ensure_selection(&manifest, &models_dir, &mut app_settings) {
                settings::save(&data_dir, &app_settings).map_err(|e| e.to_string())?;
            }
        }
        Some("live-sim") => {
            use promptify_core::live::{ChunkPolicy, LiveTranscript, next_cut};
            use promptify_core::transform::{Input, Transform, TransformService};
            let audio = read_wav(Path::new(args.get(1).ok_or(USAGE)?))?;
            let shared: SharedSettings = Arc::new(RwLock::new(app_settings));
            let stt = Arc::new(WhisperEngine::new(manifest.clone(), models_dir.clone(), shared.clone()));
            stt.preload().map_err(|e| e.0)?;
            let llm = Arc::new(LlmWorker::new(worker_exe(), manifest, models_dir, shared));
            let service = TransformService::new(stt.clone(), llm, Arc::new(NoHistory::default()), ProfileSet::bundled(), Limits::default());
            let cancel = CancelToken::default();
            let policy = ChunkPolicy::default();
            let mut live = LiveTranscript::default();
            let step = TARGET_SAMPLE_RATE as usize / 5;
            let mut end = 0;
            while end < audio.len() {
                end = (end + step).min(audio.len());
                let tail = &audio[live.committed_samples()..end];
                if let Some(cut) = next_cut(tail, &policy) {
                    let started = Instant::now();
                    let text = service.transcribe_chunk(&tail[..cut], &cancel, Duration::from_secs(1)).unwrap_or_default();
                    eprintln!("chunk at {:.1}s: {:.1}s of audio in {:.0?}: {text}", end as f32 / 16000.0, cut as f32 / 16000.0, started.elapsed());
                    live.commit(cut, &text);
                }
            }
            let committed = live.text();
            let tail = &audio[live.committed_samples()..];
            let target = text_context("notepad.exe".into(), None, String::new());
            let profile = service.profiles().resolve(&target);
            let transform = Transform { input: Input::Live { committed: &committed, tail }, mode: Mode::Dictation, profile, target: &target, surrounding: None, use_history: false, auto_mode: false, dictation_tone: DictationTone::CleanTranscript };
            let started = Instant::now();
            let mut live_text = String::new();
            service.run_scheduled(&transform, &cancel, Duration::from_secs(5), &mut |e| {
                if let JobEvent::Transcript(t) = e {
                    live_text = t.to_owned();
                }
            }).map_err(|e| e.to_string())?;
            let after_stop = started.elapsed();
            let started = Instant::now();
            let full = stt.transcribe(&audio, &cancel).map_err(|e| e.0)?;
            println!("live: {live_text}\nfull: {}", full.trim());
            println!(
                "audio {:.1}s; tail {:.1}s; after stop: live {after_stop:.0?} vs full {:.0?}",
                audio.len() as f32 / 16000.0,
                tail.len() as f32 / 16000.0,
                started.elapsed()
            );
        }
        Some("screen-text") => {
            use promptify_core::pipeline::ContextProvider;
            std::thread::sleep(Duration::from_secs(3));
            let context = promptify_lib::system_context::SystemContext;
            let target = context.identify().map_err(|e| e.0)?;
            promptify_lib::system_context::shutdown();
            let started = Instant::now();
            let focused = context.focused_text(&target.window).map_err(|e| e.0)?;
            println!("app={} in {:.0?}", target.app_key(), started.elapsed());
            match focused {
                Some(f) if f.is_secure => println!("password field: nothing read"),
                Some(f) => println!("{} chars: {}", f.text.chars().count(), f.text.chars().take(60).collect::<String>()),
                None => println!("no readable text"),
            }
        }
        Some("transcribe") => {
            let wav = PathBuf::from(args.get(1).ok_or(USAGE)?);
            let audio = read_wav(&wav)?;
            let shared: SharedSettings = Arc::new(RwLock::new(app_settings));
            let stt = WhisperEngine::new(manifest, models_dir, shared);
            let started = Instant::now();
            stt.preload().map_err(|e| e.0)?;
            let loaded = started.elapsed();
            let started = Instant::now();
            let text = stt.transcribe(&audio, &CancelToken::default()).map_err(|e| e.0)?;
            println!("{text}");
            eprintln!("load {loaded:.1?}, transcribe {:.1?} for {:.1}s audio", started.elapsed(), audio.len() as f32 / TARGET_SAMPLE_RATE as f32);
        }
        Some("run") => {
            let wav = PathBuf::from(args.get(1).ok_or(USAGE)?);
            let dictation_tone = dictation_tone_flag(&args, app_settings.dictation_tone)?;
            let mode = match flag(&args, "--mode").as_deref() {
                None | Some("prompt") => Mode::Prompt,
                Some("dictation") => Mode::Dictation,
                Some(other) => return Err(format!("unknown mode {other}")),
            };
            let ctx = ActiveContext {
                window: WindowIdentity { handle: 1, process_id: 1 },
                process_name: flag(&args, "--process").unwrap_or_else(|| "chrome.exe".into()),
                window_title: flag(&args, "--title").unwrap_or_default(),
                url: flag(&args, "--url"),
            };
            let audio = read_wav(&wav)?;
            let use_history = !args.iter().any(|a| a == "--no-history");
            let (history, _) = HistoryLog::open(data_dir.join("history.jsonl"), HistoryLimits::default(), use_history && app_settings.history_enabled)
                .map_err(|e| e.to_string())?;
            let shared: SharedSettings = Arc::new(RwLock::new(app_settings));
            let stt = Arc::new(WhisperEngine::new(manifest.clone(), models_dir.clone(), shared.clone()));
            let started = Instant::now();
            stt.preload().map_err(|e| e.0)?;
            let llm = ordinary_generator(data_dir, manifest, models_dir, shared, model_override.as_deref(), needs_inference(mode, dictation_tone))?;
            eprintln!("models loaded in {:.1?}", started.elapsed());

            let backends = Backends {
                context: Arc::new(FixedContext(ctx)),
                transcriber: stt,
                generator: llm,
                inserter: Arc::new(PrintInserter),
                history: Arc::new(history),
            };
            let limits = Limits { generation_timeout: Duration::from_secs(120), ..Limits::default() };
            let orchestrator = Orchestrator::new(backends, ProfileSet::bundled(), ContextPolicy::default(), limits);
            orchestrator.set_dictation_tone(dictation_tone);
            let routing = routing_flags(&args)?;
            if mode != Mode::Prompt && (routing.task_type.is_some() || routing.surface.is_some()) {
                return Err("Task and surface overrides apply only to Prompt mode.".into());
            }
            orchestrator.queue_routing(routing)?;
            let job = orchestrator.begin(mode).map_err(|e| e.to_string())?;
            eprintln!("profile: {}", job.profile_id);
            let mut stage_started = Instant::now();
            let report = orchestrator.finish(job, &audio, &mut |event| match event {
                JobEvent::Stage(stage) => {
                    eprintln!("[{:.1?}] {stage:?}", stage_started.elapsed());
                    stage_started = Instant::now();
                }
                JobEvent::Transcript(text) => eprintln!("transcript: {text}"),
                JobEvent::Token(_) => {}
                JobEvent::Routing(policy) => eprintln!("task={} surface={:?} form={:?}", policy.task_type.as_str(), policy.surface, policy.form),
            });
            println!("{}", serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?);
        }
        Some("rewrite") => {
            let text = args.get(1).ok_or(USAGE)?.clone();
            let dictation_tone = dictation_tone_flag(&args, app_settings.dictation_tone)?;
            let ctx = text_context(
                flag(&args, "--process").unwrap_or_else(|| "chrome.exe".into()),
                flag(&args, "--url"),
                flag(&args, "--title").unwrap_or_default(),
            );
            let mode = match flag(&args, "--mode").as_deref() {
                None | Some("prompt") => Mode::Prompt,
                Some("dictation") => Mode::Dictation,
                Some(other) => return Err(format!("unknown mode {other}")),
            };
            let llm = ordinary_generator(data_dir, manifest, models_dir, Arc::new(RwLock::new(app_settings)), model_override.as_deref(), needs_inference(mode, dictation_tone))?;
            let report = rewrite_with(llm, ctx, &text, mode, args.iter().any(|a| a == "--auto"), dictation_tone, routing_flags(&args)?)?;
            println!("{}", serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?);
        }
        Some("eval-adaptive") => {
            let dictation_tone = app_settings.dictation_tone;
            let path = args.get(1).ok_or(USAGE)?;
            let source = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
            let cases = eval::load_adaptive_cases(&source)?;
            let score = eval::score_routing(&cases);
            if score.passed != score.total || score.macro_f1 < 0.90 {
                return Err(format!("routing benchmark failed: {:?}", score.failures));
            }
            let llm = Arc::new(LlmWorker::new(worker_exe(), manifest, models_dir, Arc::new(RwLock::new(app_settings))));
            llm.preload().map_err(|e| e.0)?;
            let (mut passed, mut repaired) = (0, 0);
            let mut elapsed = Vec::new();
            for case in &cases {
                if case.expect_error {
                    passed += 1;
                    println!("{} invalid or incomplete request correctly rejected before generation", case.id);
                    continue;
                }
                let ctx = text_context(case.process.clone(), case.url.clone(), case.title.clone());
                let report = rewrite_with(llm.clone(), ctx, &case.said, Mode::Prompt, false, dictation_tone, case.options())?;
                if let Outcome::Failed { reason, detail } = &report.outcome {
                    eprintln!("{} rejected ({reason:?}): {}", case.id, detail.as_deref().unwrap_or("no additional detail"));
                }
                let text = match &report.outcome {
                    Outcome::Inserted { text } | Outcome::Blocked { text, reason: promptify_core::pipeline::BlockReason::SurfaceUnconfirmed | promptify_core::pipeline::BlockReason::GraphUnsupported, .. } => Some(text),
                    _ => None,
                };
                let checked = text.zip(report.routing.as_ref()).map(|(text, policy)| eval::score_adaptive_output(case, policy, text));
                let pass = matches!(checked, Some(Ok(())));
                if let Some(Err(error)) = checked { eprintln!("{error}"); }
                passed += usize::from(pass);
                repaired += usize::from(report.structure == Some(promptify_core::pipeline::StructureCheck::Repaired));
                elapsed.push(report.elapsed_ms);
                println!("{} outcome={} valid={} structure={:?} {}ms", case.id, report.outcome.kind(), pass, report.structure, report.elapsed_ms);
                if std::env::var_os("PROMPTIFY_EVAL_SHOW").is_some() { println!("{}\n---", outcome_text(&report.outcome).unwrap_or("")); }
            }
            elapsed.sort_unstable();
            let p95 = elapsed.get((elapsed.len() * 95).div_ceil(100).saturating_sub(1)).copied().unwrap_or(0);
            println!("adaptive: {passed}/{}; repaired: {repaired}; routing macro-F1: {:.3}; p95: {p95}ms", cases.len(), score.macro_f1);
            if passed != cases.len() { return Err("adaptive output evaluation failed".into()); }
        }
        Some("eval") => {
            let path = PathBuf::from(args.get(1).ok_or(USAGE)?);
            let source = std::fs::read_to_string(&path).map_err(|e| format!("cannot read {path:?}: {e}"))?;
            let cases = eval::load_cases(&source)?;
            let llm = Arc::new(LlmWorker::new(worker_exe(), manifest, models_dir, Arc::new(RwLock::new(app_settings))));
            llm.preload().map_err(|e| e.0)?;
            let (mut graph_pass, mut graph_total, mut flat_pass, mut flat_total, mut repaired, mut roles) = (0, 0, 0, 0, 0, 0);
            for case in &cases {
                let ctx = text_context(case.process.clone(), case.url.clone(), case.title.clone());
                let report = rewrite_text(llm.clone(), ctx, &case.said)?;
                let score = eval::score(case.expect, outcome_text(&report.outcome));
                if report.structure == Some(promptify_core::pipeline::StructureCheck::Repaired) {
                    repaired += 1;
                }

                roles += usize::from(outcome_text(&report.outcome).is_some_and(eval::opens_with_role));
                match case.expect {
                    Expect::Graph => (graph_total, graph_pass) = (graph_total + 1, graph_pass + usize::from(score.pass)),
                    Expect::Flat => (flat_total, flat_pass) = (flat_total + 1, flat_pass + usize::from(score.pass)),
                }
                println!(
                    "{} profile={} outcome={} structure={:?} valid={} pass={} {}ms",
                    case.id, report.profile_id, report.outcome.kind(), report.structure, score.valid, score.pass, report.elapsed_ms
                );
                if std::env::var_os("PROMPTIFY_EVAL_SHOW").is_some() {
                    println!("{}\n---", outcome_text(&report.outcome).unwrap_or(""));
                }
            }
            println!("graph: {graph_pass}/{graph_total}  flat: {flat_pass}/{flat_total}  repaired: {repaired}  role openers: {roles}");
            if graph_pass != graph_total || flat_pass != flat_total {
                let failed = cases.len() - graph_pass - flat_pass;
                return Err(format!("prompt structure evaluation failed: {failed} of {} cases did not pass", cases.len()));
            }
        }
        _ => return Err(USAGE.into()),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_transcript_needs_no_model_or_credentials() {
        assert!(!needs_inference(Mode::Dictation, DictationTone::CleanTranscript));
        assert!(needs_inference(Mode::Prompt, DictationTone::CleanTranscript));
        for tone in DictationTone::ALL {
            assert_eq!(needs_inference(Mode::Dictation, tone), tone != DictationTone::CleanTranscript);
        }
        let shared = Arc::new(RwLock::new(settings::AppSettings::default()));
        let generator = ordinary_generator(
            PathBuf::from("unused-inference-data"),
            Manifest::bundled(),
            PathBuf::from("unused-inference-models"),
            shared.clone(),
            Some("not-an-installed-model"),
            false,
        ).unwrap();
        let report = rewrite_with(
            generator,
            text_context("notepad.exe".into(), None, String::new()),
            "Hello there.",
            Mode::Dictation,
            false,
            DictationTone::CleanTranscript,
            RoutingOptions::default(),
        ).unwrap();
        assert_eq!(outcome_text(&report.outcome), Some("Hello there."));
        assert!(shared.read().unwrap().llm_model.is_none());
    }

    #[test]
    fn cli_credentials_are_rejected_without_echoing_values() {
        for option in ["--api-key", "--token", "--access-token", "--refresh-token", "--client-secret", "--authorization"] {
            for args in [
                vec!["rewrite".into(), "hello".into(), option.into(), "secret-sentinel".into()],
                vec!["inference".into(), format!("{option}=secret-sentinel")],
            ] {
                let error = validate_retired_options(&args).unwrap_err();
                assert!(!error.contains("secret-sentinel"));
            }
        }
    }

    #[test]
    fn no_history_only_returns_explicit_synthetic_reference() {
        assert!(NoHistory::default().context("unused", "unused").previous.is_none());
        let fixture = NoHistory {
            previous: Some(promptify_core::history::PreviousPrompt { text: "synthetic reference".into(), minutes_ago: 1 }),
        };
        let context = fixture.context("unused", "unused");
        assert!(context.examples.is_empty());
        assert_eq!(context.previous.unwrap().text, "synthetic reference");
    }

    #[test]
    fn quality_deadline_is_counted_once_per_sample() {
        let mut sample = QualityEvalSample {
            suite: "test", case_id: "deadline".into(), category: "test".into(),
            sample: 1, mode: "prompt", policy: None, tone: None,
            request: "test".into(), synthetic_previous_prompt: None, output: None, outcome: "failed",
            block_reason: None, failure_reason: Some("TimedOut".into()),
            failure_detail: None, structure: None,
            quality: Some(promptify_core::quality::QualityReport {
                deadline_exhausted: true, ..Default::default()
            }),
            structure_repair_attempts: 0, usable: false, auto_paste_eligible: false,
            output_chars: 0, generation_elapsed_ms: 0, wall_elapsed_ms: 0,
            reported_elapsed_ms: 0, history_saved: false,
        };
        let mut summary = QualityEvalSummary::default();
        summary.add(&sample);
        assert_eq!(summary.deadline_exhausted_count, 1);
        sample.quality = None;
        summary.add(&sample);
        assert_eq!(summary.deadline_exhausted_count, 2);
        sample.failure_reason = None;
        summary.add(&sample);
        assert_eq!(summary.deadline_exhausted_count, 2);
    }

    #[test]
    fn retired_features_fail_before_model_loading() {
        for args in [
            vec!["mcp"], vec!["serve"], vec!["remote", "pair", "unused"],
            vec!["rewrite", "hello", "--mcp", "unused"],
            vec!["rewrite", "hello", "--mcp=unused"],
            vec!["rewrite", "hello", "--mode", "answer"],
            vec!["rewrite", "hello", "--mode=answer"],
        ] {
            assert!(validate_retired_options(&args.into_iter().map(String::from).collect::<Vec<_>>()).is_err());
        }
        assert!(validate_retired_options(&["rewrite".into(), "Explain MCP".into(), "--mode".into(), "prompt".into()]).is_ok());
    }

    #[test]
    fn dictation_tone_flags_accept_each_value_and_reject_invalid_or_duplicate_values() {
        for tone in DictationTone::ALL {
            assert_eq!(dictation_tone_flag(&["rewrite".into(), "text".into(), "--dictation-tone".into(), tone.as_str().into()], DictationTone::Natural).unwrap(), tone);
        }
        assert!(dictation_tone_flag(&["rewrite".into(), "text".into(), "--dictation-tone".into(), "invented".into()], DictationTone::Natural).is_err());
        assert!(dictation_tone_flag(&["rewrite".into(), "text".into(), "--dictation-tone".into(), "natural".into(), "--dictation-tone".into(), "formal".into()], DictationTone::Natural).is_err());
        assert!(dictation_tone_flag(&["rewrite".into(), "text".into(), "--dictation-tone".into()], DictationTone::Natural).is_err());
    }

    #[test]
    fn quality_eval_sample_counts_are_positive_bounded_and_unique_flags() {
        assert_eq!(quality_eval_count(&[], "--samples", 3).unwrap(), 3);
        assert_eq!(quality_eval_count(&["--samples".into(), "1".into()], "--samples", 3).unwrap(), 1);
        assert!(quality_eval_count(&["--samples".into(), "0".into()], "--samples", 3).is_err());
        assert!(quality_eval_count(&["--samples".into(), "101".into()], "--samples", 3).is_err());
        assert!(quality_eval_count(&["--samples".into(), "many".into()], "--samples", 3).is_err());
        assert!(quality_eval_count(&["--samples".into(), "2".into(), "--samples".into(), "3".into()], "--samples", 3).is_err());
    }

    #[test]
    fn quality_eval_deadline_only_allows_real_product_deadlines() {
        assert_eq!(quality_eval_deadline(&[]).unwrap(), 120);
        assert_eq!(quality_eval_deadline(&["--deadline-seconds".into(), "60".into()]).unwrap(), 60);
        assert_eq!(quality_eval_deadline(&["--deadline-seconds".into(), "120".into()]).unwrap(), 120);
        assert!(quality_eval_deadline(&["--deadline-seconds".into(), "0".into()]).is_err());
        assert!(quality_eval_deadline(&["--deadline-seconds".into(), "61".into()]).is_err());
        assert!(quality_eval_deadline(&["--deadline-seconds".into(), "infinite".into()]).is_err());
        assert!(
            quality_eval_deadline(&[
                "--deadline-seconds".into(),
                "60".into(),
                "--deadline-seconds".into(),
                "120".into(),
            ])
            .is_err()
        );
    }
}

//! Developer CLI: download models and run the real speech + prompt pipeline on a WAV file.
//! Uses the same data folder, models and settings as the desktop app.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use promptify_core::audio::{CaptureBuffer, TARGET_SAMPLE_RATE};
use promptify_core::context::{ActiveContext, ContextPolicy, FocusedText, WindowIdentity};
use promptify_core::eval::{self, Expect};
use promptify_core::history::{HistoryContext, HistoryLimits, HistoryLog, NewHistoryEntry};
use promptify_core::models::Manifest;
use promptify_core::pipeline::{
    BackendError, Backends, CancelToken, ContextProvider, History, Inserter, JobEvent, Limits, Mode, Orchestrator, Outcome,
    Transcriber,
};
use promptify_core::profiles::{PasteChord, ProfileSet};
use promptify_lib::llm_client::{LlmWorker, worker_exe};
use promptify_lib::settings::{self, SharedSettings};
use promptify_lib::stt::WhisperEngine;
use promptify_lib::{download, ensure_selection};

const USAGE: &str = "usage:
  promptify-cli models
  promptify-cli download <model-id>...
  promptify-cli transcribe <file.wav>
  promptify-cli run <file.wav> [--mode prompt|dictation] [--process NAME] [--url URL] [--title TITLE] [--no-history]
  promptify-cli rewrite <text> [--process NAME] [--url URL] [--title TITLE]
  promptify-cli eval <cases.toml>";

/// Feeds typed text through the pipeline in place of speech.
struct TextTranscriber(String);

impl Transcriber for TextTranscriber {
    fn transcribe(&self, _: &[f32], _: &CancelToken) -> Result<String, BackendError> {
        Ok(self.0.clone())
    }
}

/// Evaluation and typed rewrites must neither read nor write the user's history.
struct NoHistory;

impl History for NoHistory {
    fn context(&self, _: &str, _: &str) -> HistoryContext {
        HistoryContext::default()
    }
    fn record(&self, _: NewHistoryEntry) -> Result<bool, BackendError> {
        Ok(false)
    }
}

fn text_context(process: String, url: Option<String>, title: String) -> ActiveContext {
    ActiveContext { window: WindowIdentity { handle: 1, process_id: 1 }, process_name: process, window_title: title, url }
}

fn rewrite_text(llm: &Arc<LlmWorker>, ctx: ActiveContext, text: &str) -> Result<promptify_core::pipeline::JobReport, String> {
    let backends = Backends {
        context: Arc::new(FixedContext(ctx)),
        transcriber: Arc::new(TextTranscriber(text.to_owned())),
        generator: llm.clone(),
        inserter: Arc::new(PrintInserter),
        history: Arc::new(NoHistory),
    };
    let limits = Limits { generation_timeout: Duration::from_secs(120), ..Limits::default() };
    let orchestrator = Orchestrator::new(backends, ProfileSet::bundled(), ContextPolicy::default(), limits);
    let job = orchestrator.begin(Mode::Prompt).map_err(|e| e.to_string())?;
    Ok(orchestrator.finish(job, &[], &mut |_| {}))
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

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let data_dir = settings::app_data_dir();
    std::fs::create_dir_all(&data_dir).map_err(|e| e.to_string())?;
    let models_dir = settings::models_dir(&data_dir);
    let manifest = Manifest::bundled();
    let mut app_settings = settings::load(&data_dir);
    if ensure_selection(&manifest, &models_dir, &mut app_settings) {
        settings::save(&data_dir, &app_settings).map_err(|e| e.to_string())?;
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
            let llm = Arc::new(LlmWorker::new(worker_exe(), manifest, models_dir, shared));
            let started = Instant::now();
            stt.preload().map_err(|e| e.0)?;
            if mode == Mode::Prompt {
                llm.preload().map_err(|e| e.0)?;
            }
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
            });
            println!("{}", serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?);
        }
        Some("rewrite") => {
            let text = args.get(1).ok_or(USAGE)?.clone();
            let ctx = text_context(
                flag(&args, "--process").unwrap_or_else(|| "chrome.exe".into()),
                flag(&args, "--url"),
                flag(&args, "--title").unwrap_or_default(),
            );
            let llm = Arc::new(LlmWorker::new(worker_exe(), manifest, models_dir, Arc::new(RwLock::new(app_settings))));
            llm.preload().map_err(|e| e.0)?;
            let report = rewrite_text(&llm, ctx, &text)?;
            println!("{}", serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?);
        }
        Some("eval") => {
            let path = PathBuf::from(args.get(1).ok_or(USAGE)?);
            let source = std::fs::read_to_string(&path).map_err(|e| format!("cannot read {path:?}: {e}"))?;
            let cases = eval::load_cases(&source)?;
            let llm = Arc::new(LlmWorker::new(worker_exe(), manifest, models_dir, Arc::new(RwLock::new(app_settings))));
            llm.preload().map_err(|e| e.0)?;
            let (mut graph_pass, mut graph_total, mut flat_pass, mut flat_total, mut repaired) = (0, 0, 0, 0, 0);
            for case in &cases {
                let ctx = text_context(case.process.clone(), case.url.clone(), case.title.clone());
                let report = rewrite_text(&llm, ctx, &case.said)?;
                let score = eval::score(case.expect, outcome_text(&report.outcome));
                if report.structure == Some(promptify_core::pipeline::StructureCheck::Repaired) {
                    repaired += 1;
                }
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
            println!("graph: {graph_pass}/{graph_total}  flat: {flat_pass}/{flat_total}  repaired: {repaired}");
        }
        _ => return Err(USAGE.into()),
    }
    Ok(())
}

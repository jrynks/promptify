//! Developer CLI: download models and run the real speech + prompt pipeline on a WAV file.
//! Uses the same data folder, models and settings as the desktop app.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use promptify_core::audio::{CaptureBuffer, TARGET_SAMPLE_RATE};
use promptify_core::context::{ActiveContext, ContextPolicy, FocusedText, WindowIdentity};
use promptify_core::eval::{self, Expect};
use promptify_core::history::{HistoryContext, HistoryLimits, HistoryLog, NewHistoryEntry};
use promptify_core::models::{Manifest, ModelKind, is_installed};
use promptify_core::pipeline::{
    BackendError, Backends, CancelToken, ContextProvider, History, Inserter, JobEvent, Limits, Mode, Orchestrator, Outcome,
    Transcriber,
};
use promptify_core::profiles::{PasteChord, ProfileSet};
use promptify_core::routing::{self, Rendering, RoutingOptions, Surface, TaskId};
use promptify_lib::llm_client::{LlmWorker, worker_exe};
use promptify_lib::settings::{self, SharedSettings};
use promptify_lib::stt::WhisperEngine;
use promptify_lib::{download, ensure_selection};

const USAGE: &str = "usage:
  promptify-cli models
  promptify-cli download <model-id>...
  promptify-cli transcribe <file.wav>
  promptify-cli live-sim <file.wav>   (replays the file as if spoken; compares live chunks with one full pass)
  promptify-cli run <file.wav> [--mode prompt|dictation] [--process NAME] [--url URL] [--title TITLE] [--no-history]
  promptify-cli rewrite <text> [--process NAME] [--url URL] [--title TITLE] [--mcp mcp.json] [--mode prompt|dictation|answer] [--auto]
  promptify-cli screen-text   (reads the focused text box of the foreground app after 3 s, as the app would)
  promptify-cli eval <cases.toml>
  promptify-cli eval-adaptive <cases.toml> [--model ID]   (local model output contracts)
  promptify-cli eval-routing <cases.toml>   (classification only; no model needed)
  promptify-cli prompt-types   (list the bundled taxonomy and activation status)
  promptify-cli route <text> [--process NAME] [--url URL] [--surface SURFACE] [--prompt-type ID]
  promptify-cli mcp [--api http://127.0.0.1:47821]   (stdio MCP server for Claude Desktop, VS Code, Cursor...)
  promptify-cli serve [--relay URL] [--listen ADDR] [--advertise HOST:PORT] [--offer-file FILE] [--discoverable]
  promptify-cli remote pair <pairing-link> [--name NAME] [--direct] [--identity FILE]
  promptify-cli remote send <text> [--app APP] [--url URL] [--dictation] [--direct] [--identity FILE]

serve starts a loopback MCP API by default. Phone options (--relay, --advertise,
--offer-file, --discoverable) and remote commands require the mobile-networking build feature.";

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

fn rewrite_text(
    llm: &Arc<LlmWorker>,
    ctx: ActiveContext,
    text: &str,
    enricher: Option<Arc<dyn promptify_core::transform::ContextEnricher>>,
) -> Result<promptify_core::pipeline::JobReport, String> {
    rewrite_with(llm, ctx, text, enricher, Mode::Prompt, false, RoutingOptions::default())
}

fn rewrite_with(
    llm: &Arc<LlmWorker>,
    ctx: ActiveContext,
    text: &str,
    enricher: Option<Arc<dyn promptify_core::transform::ContextEnricher>>,
    mode: Mode,
    auto_mode: bool,
    routing: RoutingOptions,
) -> Result<promptify_core::pipeline::JobReport, String> {
    if mode != Mode::Prompt && (routing.task_type.is_some() || routing.surface.is_some()) {
        return Err("Task and surface overrides apply only to Prompt mode.".into());
    }
    let backends = Backends {
        context: Arc::new(FixedContext(ctx)),
        transcriber: Arc::new(TextTranscriber(text.to_owned())),
        generator: llm.clone(),
        inserter: Arc::new(PrintInserter),
        history: Arc::new(NoHistory),
    };
    let limits = Limits { generation_timeout: Duration::from_secs(120), ..Limits::default() };
    let orchestrator = Orchestrator::new(backends, ProfileSet::bundled(), ContextPolicy::default(), limits);
    orchestrator.service().set_enricher(enricher);
    orchestrator.set_auto_mode(auto_mode);
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
        metadata.level() <= log::Level::Info && !(metadata.target().starts_with("rmcp") && metadata.level() > log::Level::Warn)
    }
    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            eprintln!("[{}] {}", record.level(), record.args());
        }
    }
    fn flush(&self) {}
}

fn serve_config(args: &[String], data_dir: &Path) -> Result<promptify_server::ServerConfig, String> {
    if args.iter().any(|a| ["--relay", "--advertise", "--offer-file", "--discoverable"].contains(&a.as_str())) {
        promptify_server::require_mobile_networking()?;
    }
    let listen: std::net::SocketAddr = flag(args, "--listen").unwrap_or_else(|| "127.0.0.1:47822".into()).parse().map_err(|e| format!("bad --listen: {e}"))?;
    let config = promptify_server::ServerConfig {
        data_dir: data_dir.to_path_buf(),
        relay_url: flag(args, "--relay"),
        listen: Some(listen),
        advertise_direct: if promptify_server::MOBILE_NETWORKING_AVAILABLE { Some(flag(args, "--advertise").unwrap_or_else(|| listen.to_string())) } else { None },
        discoverable: args.iter().any(|a| a == "--discoverable"),
    };
    config.validate()?;
    Ok(config)
}

/// Acts as a paired phone, for testing remote access end to end.
fn remote(args: &[String], data_dir: &Path) -> Result<(), String> {
    use promptify_protocol::messages::{ServerMessage, WireContext, WireMode};
    use promptify_server::client;

    promptify_server::require_mobile_networking()?;
    let identity_path = flag(args, "--identity").map(PathBuf::from).unwrap_or_else(|| data_dir.join("remote-test-client.json"));
    let direct = args.iter().any(|a| a == "--direct");
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
    match args.get(1).map(String::as_str) {
        Some("pair") => {
            let link = args.get(2).ok_or(USAGE)?;
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            let offer = promptify_protocol::pairing::PairingOffer::parse(link, now).map_err(|e| e.to_string())?;
            let name = flag(args, "--name").unwrap_or_else(|| "Promptify CLI".into());
            let (identity, _) = runtime.block_on(client::pair(&offer, &name, direct))?;
            identity.save(&identity_path)?;
            println!("paired as {} (identity saved to {})", identity.device_id, identity_path.display());
        }
        Some("send") => {
            let text = args.get(2).ok_or(USAGE)?;
            let identity = client::ClientIdentity::load(&identity_path)?;
            let mode = if args.iter().any(|a| a == "--dictation") { WireMode::Dictation } else { WireMode::Prompt };
            let context = WireContext { app: flag(args, "--app").unwrap_or_default(), url: flag(args, "--url"), title: String::new() };
            let started = Instant::now();
            let result = runtime.block_on(async {
                let mut connection = client::open(&identity, direct).await?;
                connection
                    .transform_text(1, mode, context, text, &mut |event| {
                        if let ServerMessage::Stage { stage, .. } = event {
                            eprintln!("[{:.1?}] {stage}", started.elapsed());
                        }
                    })
                    .await
            })?;
            match result {
                ServerMessage::Done { text, profile, structure, truncated, .. } => {
                    eprintln!("profile={profile} structure={structure:?} truncated={truncated} in {:.1?}", started.elapsed());
                    println!("{text}");
                }
                other => return Err(format!("{other:?}")),
            }
        }
        _ => return Err(USAGE.into()),
    }
    Ok(())
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
    let data_dir = settings::app_data_dir();
    std::fs::create_dir_all(&data_dir).map_err(|e| e.to_string())?;
    let models_dir = settings::models_dir(&data_dir);
    let manifest = Manifest::bundled();
    let mut app_settings = settings::load(&data_dir);
    if ensure_selection(&manifest, &models_dir, &mut app_settings) {
        settings::save(&data_dir, &app_settings).map_err(|e| e.to_string())?;
    }
    if matches!(args.first().map(String::as_str), Some("rewrite" | "run" | "eval" | "eval-adaptive"))
        && let Some(id) = checked_flag(args.get(2..).unwrap_or_default(), "--model")?
    {
            let entry = manifest.get(&id).filter(|entry| entry.kind == ModelKind::Llm).ok_or_else(|| format!("unknown language model: {id}"))?;
            if !is_installed(&models_dir, entry) { return Err(format!("language model {id} is not installed")); }
            app_settings.llm_model = Some(id);
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
            use promptify_core::scheduler::{Priority, SchedulerLimits};
            use promptify_core::transform::{Input, Transform, TransformService};
            let audio = read_wav(Path::new(args.get(1).ok_or(USAGE)?))?;
            let shared: SharedSettings = Arc::new(RwLock::new(app_settings));
            let stt = Arc::new(WhisperEngine::new(manifest.clone(), models_dir.clone(), shared.clone()));
            stt.preload().map_err(|e| e.0)?;
            let llm = Arc::new(LlmWorker::new(worker_exe(), manifest, models_dir, shared));
            let service = TransformService::new(stt.clone(), llm, Arc::new(NoHistory), ProfileSet::bundled(), Limits::default(), SchedulerLimits::default());
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
            let transform = Transform { input: Input::Live { committed: &committed, tail }, mode: Mode::Dictation, profile, target: &target, surrounding: None, use_history: false, use_tools: false, auto_mode: false };
            let started = Instant::now();
            let mut live_text = String::new();
            service.run_scheduled(Priority::Local, "local", &transform, &cancel, Duration::from_secs(5), &mut |e| {
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
            let ctx = text_context(
                flag(&args, "--process").unwrap_or_else(|| "chrome.exe".into()),
                flag(&args, "--url"),
                flag(&args, "--title").unwrap_or_default(),
            );
            let llm = Arc::new(LlmWorker::new(worker_exe(), manifest, models_dir, Arc::new(RwLock::new(app_settings))));
            let enricher: Option<Arc<dyn promptify_core::transform::ContextEnricher>> = match flag(&args, "--mcp") {
                Some(path) => {
                    let config = promptify_mcp::client::McpConfig::load(Path::new(&path))?;
                    promptify_mcp::client::McpEnricher::from_config(config)?.map(|e| Arc::new(e) as _)
                }
                None => None,
            };
            llm.preload().map_err(|e| e.0)?;
            let mode = match flag(&args, "--mode").as_deref() {
                None | Some("prompt") => Mode::Prompt,
                Some("dictation") => Mode::Dictation,
                Some("answer") => Mode::Answer,
                Some(other) => return Err(format!("unknown mode {other}")),
            };
            let report = rewrite_with(&llm, ctx, &text, enricher, mode, args.iter().any(|a| a == "--auto"), routing_flags(&args)?)?;
            println!("{}", serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?);
        }
        Some("eval-adaptive") => {
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
                let report = rewrite_with(&llm, ctx, &case.said, None, Mode::Prompt, false, case.options())?;
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
                let report = rewrite_text(&llm, ctx, &case.said, None)?;
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
        Some("remote") => remote(&args, &data_dir)?,
        Some("mcp") => {
            // stdout carries the MCP protocol; diagnostics go to stderr only.
            let base = flag(&args, "--api").unwrap_or_else(|| "http://127.0.0.1:47821".into());
            let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().map_err(|e| e.to_string())?;
            runtime.block_on(promptify_mcp::server::serve_stdio(base, promptify_server::api_token_path(&data_dir)))?;
        }
        Some("serve") => {
            use promptify_core::scheduler::SchedulerLimits;
            use promptify_core::transform::TransformService;
            let config = serve_config(&args, &data_dir)?;
            let shared: SharedSettings = Arc::new(RwLock::new(app_settings));
            let stt = Arc::new(WhisperEngine::new(manifest.clone(), models_dir.clone(), shared.clone()));
            let llm = Arc::new(LlmWorker::new(worker_exe(), manifest, models_dir, shared));
            stt.preload().map_err(|e| e.0)?;
            llm.preload().map_err(|e| e.0)?;
            let limits = Limits { generation_timeout: Duration::from_secs(120), ..Limits::default() };
            let service = Arc::new(TransformService::new(stt, llm, Arc::new(NoHistory), ProfileSet::bundled(), limits, SchedulerLimits::default()));
            let server = promptify_server::RemoteServer::start(config, service)?;
            if promptify_server::MOBILE_NETWORKING_AVAILABLE {
                let offer = server.pairing_offer(600)?;
                if let Some(path) = flag(&args, "--offer-file") {
                    std::fs::write(&path, &offer.uri).map_err(|e| e.to_string())?;
                }
                println!("{}", offer.uri);
                eprintln!("serving on {:?}; pairing link valid for 10 minutes; Ctrl+C to stop", server.listen_addr());
            } else {
                let addr = server.listen_addr().ok_or("local MCP API listener is unavailable")?;
                println!("http://{addr}");
                eprintln!("local MCP API only; token in {}; phone networking disabled; Ctrl+C to stop", promptify_server::api_token_path(&data_dir).display());
            }
            loop {
                std::thread::sleep(Duration::from_secs(10));
                if promptify_server::MOBILE_NETWORKING_AVAILABLE {
                    eprintln!("status: {}", serde_json::to_string(&server.status()).unwrap_or_default());
                }
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
    fn default_serve_config_keeps_the_desktop_api() {
        let config = serve_config(&["serve".into()], Path::new("unused")).unwrap();
        assert!(config.listen.unwrap().ip().is_loopback());
        assert!(config.relay_url.is_none() && !config.discoverable);
        assert_eq!(config.advertise_direct.is_some(), promptify_server::MOBILE_NETWORKING_AVAILABLE);
    }

    #[cfg(not(feature = "mobile-networking"))]
    #[test]
    fn phone_serve_options_fail_before_loading_models() {
        for options in [
            vec!["--relay", "wss://relay.example.test"],
            vec!["--advertise", "192.168.1.20:47822"],
            vec!["--offer-file", "unused"],
            vec!["--discoverable"],
            vec!["--listen", "0.0.0.0:47822"],
        ] {
            let args: Vec<String> = std::iter::once("serve").chain(options).map(String::from).collect();
            assert_eq!(serve_config(&args, Path::new("unused")).unwrap_err(), promptify_server::MOBILE_NETWORKING_DISABLED);
        }
    }
}

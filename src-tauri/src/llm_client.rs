use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant};

use promptify_core::llm_protocol::{WireFinish, WorkerEvent, WorkerRequest, chatml_prompt};
use promptify_core::models::{Manifest, ModelKind, final_path, is_installed};
use promptify_core::pipeline::{BackendError, CancelToken, FinishReason, Generation, GenerationRequest, Generator};

use crate::settings::SharedSettings;

const N_CTX: u32 = 8192;
const LOAD_TIMEOUT: Duration = Duration::from_secs(120);
/// How long a cooperative cancel may take before the worker is killed.
const CANCEL_GRACE: Duration = Duration::from_millis(750);

fn err(msg: impl Into<String>) -> BackendError {
    BackendError(msg.into())
}

struct Worker {
    child: Child,
    stdin: ChildStdin,
    events: mpsc::Receiver<WorkerEvent>,
    loaded: Option<(String, bool)>,
}

impl Worker {
    fn send(&mut self, request: &WorkerRequest) -> Result<(), BackendError> {
        let line = serde_json::to_string(request).expect("requests serialize");
        writeln!(self.stdin, "{line}").and_then(|_| self.stdin.flush()).map_err(|e| err(format!("language model process stopped: {e}")))
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Talks to the `promptify-llm` worker process, starting it on demand.
pub struct LlmWorker {
    exe: PathBuf,
    manifest: Manifest,
    models_dir: PathBuf,
    settings: SharedSettings,
    worker: Mutex<Option<Worker>>,
    next_id: AtomicU64,
    device: Mutex<Option<String>>,
}

pub fn worker_exe() -> PathBuf {
    if let Some(path) = std::env::var_os("PROMPTIFY_LLM_WORKER") {
        return PathBuf::from(path);
    }
    let name = format!("promptify-llm{}", std::env::consts::EXE_SUFFIX);
    std::env::current_exe().ok().and_then(|exe| exe.parent().map(|dir| dir.join(&name))).unwrap_or_else(|| PathBuf::from(name))
}

impl LlmWorker {
    pub fn new(exe: PathBuf, manifest: Manifest, models_dir: PathBuf, settings: SharedSettings) -> Self {
        Self { exe, manifest, models_dir, settings, worker: Mutex::new(None), next_id: AtomicU64::new(1), device: Mutex::new(None) }
    }

    /// The GPU the loaded model runs on, or `None` for CPU.
    pub fn device(&self) -> Option<String> {
        self.device.lock().unwrap().clone()
    }

    fn selected_path(&self) -> Result<String, BackendError> {
        let id = self.settings.read().unwrap().llm_model.clone();
        let entry = id
            .as_deref()
            .and_then(|id| self.manifest.get(id))
            .filter(|e| e.kind == ModelKind::Llm && is_installed(&self.models_dir, e))
            .ok_or_else(|| err("No language model is installed. Open Promptify settings → Models."))?;
        Ok(final_path(&self.models_dir, entry).to_string_lossy().into_owned())
    }

    fn spawn(&self) -> Result<Worker, BackendError> {
        let mut command = Command::new(&self.exe);
        command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = command.spawn().map_err(|e| err(format!("could not start the language model process {:?}: {e}", self.exe)))?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("llm-worker-reader".into())
            .spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    if let Ok(event) = serde_json::from_str::<WorkerEvent>(&line)
                        && tx.send(event).is_err()
                    {
                        break;
                    }
                }
            })
            .map_err(|e| err(e.to_string()))?;
        Ok(Worker { child, stdin, events: rx, loaded: None })
    }

    /// Returns a live worker with the selected model loaded.
    fn ready<'a>(&self, slot: &'a mut Option<Worker>, deadline: Instant) -> Result<&'a mut Worker, BackendError> {
        let path = self.selected_path()?;
        let use_gpu = self.settings.read().unwrap().use_gpu;
        if slot.as_mut().is_some_and(|w| !matches!(w.child.try_wait(), Ok(None))) {
            *slot = None;
        }
        if slot.is_none() {
            *slot = Some(self.spawn()?);
        }
        let worker = slot.as_mut().expect("spawned");
        let wanted = (path.clone(), use_gpu);
        if worker.loaded.as_ref() != Some(&wanted) {
            worker.loaded = None;
            worker.send(&WorkerRequest::Load { model_path: path.clone(), n_ctx: N_CTX, use_gpu })?;
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                match worker.events.recv_timeout(remaining) {
                    Ok(WorkerEvent::Loaded { model_path, device }) if model_path == path => {
                        log::info!("language model loaded on {}", device.as_deref().unwrap_or("CPU"));
                        *self.device.lock().unwrap() = device;
                        break;
                    }
                    Ok(WorkerEvent::Error { id: None, message }) => return Err(err(message)),
                    Ok(_) => {}
                    Err(_) => return Err(err("the language model did not load in time")),
                }
            }
            worker.loaded = Some(wanted);
        }
        Ok(worker)
    }

    pub fn preload(&self) -> Result<(), BackendError> {
        let mut slot = self.worker.lock().unwrap();
        let result = self.ready(&mut slot, Instant::now() + LOAD_TIMEOUT).map(|_| ());
        if result.is_err() {
            *slot = None;
        }
        result
    }
}

impl Generator for LlmWorker {
    fn generate(&self, request: &GenerationRequest<'_>, cancel: &CancelToken, on_token: &mut dyn FnMut(&str)) -> Result<Generation, BackendError> {
        // A background preload may hold the worker; keep Esc and the deadline responsive meanwhile.
        let mut slot = loop {
            match self.worker.try_lock() {
                Ok(slot) => break slot,
                Err(std::sync::TryLockError::Poisoned(e)) => break e.into_inner(),
                Err(std::sync::TryLockError::WouldBlock) => {}
            }
            if cancel.is_cancelled() {
                return Err(err("cancelled"));
            }
            if Instant::now() > request.deadline {
                return Err(err("the language model is still loading"));
            }
            std::thread::sleep(Duration::from_millis(25));
        };
        let result = self.run(&mut slot, request, cancel, on_token);
        if result.is_err() {
            // Kill on any failure so a stuck or cancelled generation can never keep running.
            *slot = None;
        }
        result
    }
}

impl LlmWorker {
    fn run(&self, slot: &mut Option<Worker>, request: &GenerationRequest<'_>, cancel: &CancelToken, on_token: &mut dyn FnMut(&str)) -> Result<Generation, BackendError> {
        let load_deadline = request.deadline.max(Instant::now() + LOAD_TIMEOUT);
        let worker = self.ready(slot, load_deadline)?;
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        worker.send(&WorkerRequest::Generate { id, prompt: chatml_prompt(request.messages), max_new_tokens: request.max_new_tokens })?;

        let mut text = String::new();
        let mut cancel_sent: Option<Instant> = None;
        loop {
            if cancel.is_cancelled() && cancel_sent.is_none() {
                let _ = worker.send(&WorkerRequest::Cancel { id });
                cancel_sent = Some(Instant::now());
            }
            if cancel_sent.is_some_and(|at| at.elapsed() > CANCEL_GRACE) {
                return Err(err("cancelled"));
            }
            if Instant::now() > request.deadline {
                return Err(err("the language model took too long"));
            }
            match worker.events.recv_timeout(Duration::from_millis(50)) {
                Ok(WorkerEvent::Token { id: got, text: piece }) if got == id => {
                    text.push_str(&piece);
                    if cancel_sent.is_none() {
                        on_token(&piece);
                    }
                }
                Ok(WorkerEvent::Done { id: got, finish }) if got == id => {
                    return match finish {
                        WireFinish::Stop => Ok(Generation { text, finish: FinishReason::Stop }),
                        WireFinish::Length => Ok(Generation { text, finish: FinishReason::Length }),
                        WireFinish::Cancelled => Err(err("cancelled")),
                    };
                }
                Ok(WorkerEvent::Error { id: Some(got), message }) if got == id => return Err(err(message)),
                Ok(WorkerEvent::Error { id: None, message }) => return Err(err(message)),
                Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return Err(err("the language model process exited")),
            }
        }
    }
}

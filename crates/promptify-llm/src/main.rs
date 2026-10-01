//! Runs llama.cpp in its own process: whisper.cpp in the app vendors a different ggml, and a
//! separate process lets the app enforce cancellation and timeouts by killing it.

use std::io::{self, BufRead, Write};
use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;

use llama_cpp_2::context::LlamaContext;
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;
use llama_cpp_2::token::LlamaToken;
use llama_cpp_2::{LlamaBackendDevice, LlamaBackendDeviceType, LlamaStateSeqFlags, SeqState, list_llama_ggml_backend_devices};
use promptify_core::llm_protocol::{WireFinish, WorkerEvent, WorkerRequest};
use promptify_core::prefix_cache::PrefixCache;

fn send(event: &WorkerEvent) {
    let line = serde_json::to_string(event).expect("events serialize");
    let mut out = io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

struct Loaded {
    path: String,
    n_ctx: u32,
    use_gpu: bool,
    device: Option<String>,
    model: LlamaModel,
}

/// One persistent context per loaded model, so repeated prompt prefixes can be restored, not recomputed.
struct Session<'m> {
    loaded: &'m Loaded,
    ctx: LlamaContext<'m>,
    cache: PrefixCache<LlamaToken, SeqState>,
}

/// Never equals a real request id (they start at 1) or the "nothing cancelled" value 0.
const WARMUP_ID: u64 = u64::MAX;

const WARMUP_PROMPT: &str = "<|im_start|>system\nYou rewrite spoken requests into clear prompts for an AI assistant, keeping every detail the speaker gave.<|im_end|>\n<|im_start|>user\nhelp me plan a short weekend trip with a few ideas for things to do<|im_end|>\n<|im_start|>assistant\n";

/// Host memory for cached prefix states. A Qwen3.5 9B prefix of ~1,500 tokens is roughly 100 MB.
const PREFIX_CACHE_BYTES: usize = 768 << 20;
const PREFIX_CACHE_ENTRIES: usize = 8;

/// Prefers a discrete GPU over an integrated one (e.g. an RTX card over the CPU's iGPU).
fn pick_gpu() -> Option<LlamaBackendDevice> {
    let devices = list_llama_ggml_backend_devices();
    let of = |kind: LlamaBackendDeviceType| devices.iter().filter(|d| d.device_type == kind).max_by_key(|d| d.memory_total).cloned();
    of(LlamaBackendDeviceType::Gpu).or_else(|| of(LlamaBackendDeviceType::IntegratedGpu))
}

enum Exit {
    Reload(WorkerRequest),
    Unloaded,
    Closed,
}

fn main() {
    let cancelled = Arc::new(AtomicU64::new(0));
    let (tx, rx) = mpsc::channel::<WorkerRequest>();
    let reader_cancelled = cancelled.clone();
    std::thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            match serde_json::from_str::<WorkerRequest>(&line) {
                Ok(WorkerRequest::Cancel { id }) => reader_cancelled.store(id, Ordering::SeqCst),
                Ok(request) => {
                    if tx.send(request).is_err() {
                        break;
                    }
                }
                Err(e) => send(&WorkerEvent::Error { id: None, message: format!("invalid request: {e}") }),
            }
        }
        // stdin closed means the app is gone; never outlive it.
        std::process::exit(0);
    });

    let mut backend = match LlamaBackend::init() {
        Ok(backend) => backend,
        Err(e) => {
            send(&WorkerEvent::Error { id: None, message: format!("llama.cpp init failed: {e}") });
            return;
        }
    };
    backend.void_logs();
    let use_cache = std::env::var_os("PROMPTIFY_LLM_NO_PREFIX_CACHE").is_none();

    let mut next: Option<WorkerRequest> = None;
    loop {
        let request = match next.take() {
            Some(request) => request,
            None => match rx.recv() {
                Ok(request) => request,
                Err(_) => return,
            },
        };
        match request {
            WorkerRequest::Load { model_path, n_ctx, use_gpu } => {
                let gpu = if use_gpu { pick_gpu() } else { None };
                let base = || LlamaModelParams::default().with_n_gpu_layers(if gpu.is_some() { 999 } else { 0 });
                let params = match &gpu {
                    Some(device) => base().with_devices(&[device.index]).unwrap_or_else(|_| base()),
                    None => base(),
                };
                let device = gpu.map(|d| d.description);
                let model = match LlamaModel::load_from_file(&backend, &model_path, &params) {
                    Ok(model) => model,
                    Err(e) => {
                        send(&WorkerEvent::Error { id: None, message: format!("could not load model: {e}") });
                        continue;
                    }
                };
                let loaded = Loaded { path: model_path, n_ctx, use_gpu, device, model };
                match serve(&backend, &loaded, &rx, &cancelled, use_cache) {
                    Exit::Reload(request) => next = Some(request),
                    Exit::Unloaded => {}
                    Exit::Closed => return,
                }
            }
            WorkerRequest::Generate { id, .. } => send(&WorkerEvent::Error { id: Some(id), message: "no model loaded".into() }),
            WorkerRequest::Cancel { .. } => {}
        }
    }
}

/// Answers requests for one loaded model until another model is requested.
fn serve(backend: &LlamaBackend, loaded: &Loaded, rx: &mpsc::Receiver<WorkerRequest>, cancelled: &AtomicU64, use_cache: bool) -> Exit {
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(16) as i32;
    let params = LlamaContextParams::default()
        .with_n_ctx(NonZeroU32::new(loaded.n_ctx))
        .with_n_batch(loaded.n_ctx)
        .with_n_threads(threads)
        .with_n_threads_batch(threads);
    let ctx = match loaded.model.new_context(backend, params) {
        Ok(ctx) => ctx,
        Err(e) => {
            send(&WorkerEvent::Error { id: None, message: format!("context failed: {e}") });
            return Exit::Unloaded;
        }
    };
    let capacity = if use_cache { PREFIX_CACHE_BYTES } else { 0 };
    let mut session = Session { loaded, ctx, cache: PrefixCache::new(capacity, PREFIX_CACHE_ENTRIES) };
    // Compiles GPU kernels now instead of during the user's first request.
    let _ = generate(&mut session, WARMUP_ID, WARMUP_PROMPT, None, 1, cancelled);
    send(&WorkerEvent::Loaded { model_path: loaded.path.clone(), device: loaded.device.clone() });

    for request in rx.iter() {
        match request {
            WorkerRequest::Load { ref model_path, n_ctx, use_gpu } => {
                if *model_path == loaded.path && n_ctx == loaded.n_ctx && use_gpu == loaded.use_gpu {
                    send(&WorkerEvent::Loaded { model_path: loaded.path.clone(), device: loaded.device.clone() });
                } else {
                    return Exit::Reload(request);
                }
            }
            WorkerRequest::Generate { id, prompt, max_new_tokens, prefix_bytes } => {
                match generate(&mut session, id, &prompt, prefix_bytes, max_new_tokens, cancelled) {
                    Ok(done) => send(&WorkerEvent::Done {
                        id,
                        finish: done.finish,
                        prompt_tokens: done.prompt_tokens,
                        cached_tokens: done.cached_tokens,
                        prefill_ms: done.prefill_ms,
                    }),
                    Err(message) => send(&WorkerEvent::Error { id: Some(id), message }),
                }
            }
            WorkerRequest::Cancel { .. } => {}
        }
    }
    Exit::Closed
}

struct Done {
    finish: WireFinish,
    prompt_tokens: u32,
    cached_tokens: u32,
    prefill_ms: u32,
}

fn tokenize(model: &LlamaModel, text: &str) -> Result<Vec<LlamaToken>, String> {
    model.str_to_token(text, AddBos::Never).map_err(|e| format!("tokenize failed: {e}"))
}

fn decode(ctx: &mut LlamaContext<'_>, batch: &mut LlamaBatch, tokens: &[LlamaToken], start: usize, logits_last: bool) -> Result<(), String> {
    batch.clear();
    let last = tokens.len().saturating_sub(1);
    for (i, token) in tokens.iter().enumerate() {
        batch.add(*token, (start + i) as i32, &[0], logits_last && i == last).map_err(|e| e.to_string())?;
    }
    ctx.decode(batch).map_err(|e| format!("decode failed: {e}"))
}

/// Restores the cached state for `prefix`, or computes it and caches it. Returns the tokens restored.
fn load_prefix(session: &mut Session<'_>, batch: &mut LlamaBatch, prefix: Vec<LlamaToken>) -> Result<u32, String> {
    if let Some(state) = session.cache.get(&prefix)
        && session.ctx.state_seq_set(state, 0).is_ok()
    {
        return Ok(prefix.len() as u32);
    }
    // A failed restore may leave partial state behind.
    session.ctx.clear_kv_cache();
    decode(&mut session.ctx, batch, &prefix, 0, false)?;
    if session.cache.capacity() > 0
        && let Ok(state) = session.ctx.state_seq_get(0, LlamaStateSeqFlags::empty())
    {
        let bytes = state.byte_len();
        session.cache.insert(prefix, state, bytes);
    }
    Ok(0)
}

fn generate(
    session: &mut Session<'_>,
    id: u64,
    prompt: &str,
    prefix_bytes: Option<usize>,
    max_new_tokens: u32,
    cancelled: &AtomicU64,
) -> Result<Done, String> {
    let loaded = session.loaded;
    let model = &loaded.model;
    let n_ctx = loaded.n_ctx as usize;
    let split = prefix_bytes.filter(|&n| n > 0 && n < prompt.len() && prompt.is_char_boundary(n));
    let (prefix, suffix) = match split {
        Some(n) => (tokenize(model, &prompt[..n])?, tokenize(model, &prompt[n..])?),
        None => (Vec::new(), tokenize(model, prompt)?),
    };
    if suffix.is_empty() {
        return Err("empty prompt".into());
    }
    let prompt_tokens = prefix.len() + suffix.len();
    if prompt_tokens + max_new_tokens as usize > n_ctx {
        return Err(format!("prompt of {prompt_tokens} tokens does not fit the {n_ctx}-token context"));
    }

    session.ctx.clear_kv_cache();
    let started = std::time::Instant::now();
    let mut batch = LlamaBatch::new(n_ctx, 1);
    let start = prefix.len();
    let cached_tokens = if prefix.is_empty() { 0 } else { load_prefix(session, &mut batch, prefix)? };
    if cancelled.load(Ordering::SeqCst) == id {
        return Ok(Done { finish: WireFinish::Cancelled, prompt_tokens: prompt_tokens as u32, cached_tokens, prefill_ms: 0 });
    }
    let ctx = &mut session.ctx;
    decode(ctx, &mut batch, &suffix, start, true)?;
    let prefill_ms = started.elapsed().as_millis() as u32;

    let mut sampler = LlamaSampler::chain_simple([
        LlamaSampler::top_k(40),
        LlamaSampler::top_p(0.9, 1),
        LlamaSampler::min_p(0.05, 1),
        LlamaSampler::temp(0.3),
        LlamaSampler::dist(id as u32),
    ]);
    let done = |finish| -> Result<Done, String> { Ok(Done { finish, prompt_tokens: prompt_tokens as u32, cached_tokens, prefill_ms }) };
    let mut pending: Vec<u8> = Vec::new();
    for position in (prompt_tokens as i32..).take(max_new_tokens as usize) {
        if cancelled.load(Ordering::SeqCst) == id {
            return done(WireFinish::Cancelled);
        }
        let token = sampler.sample(ctx, batch.n_tokens() - 1);
        if model.is_eog_token(token) {
            flush(id, &mut pending, true);
            return done(WireFinish::Stop);
        }
        let piece = model.token_to_piece_bytes(token, 256, false, None).map_err(|e| format!("detokenize failed: {e}"))?;
        pending.extend_from_slice(&piece);
        flush(id, &mut pending, false);
        decode(ctx, &mut batch, &[token], position as usize, true)?;
    }
    flush(id, &mut pending, true);
    done(WireFinish::Length)
}

/// Emits the longest valid UTF-8 prefix; multi-byte characters can span tokens.
fn flush(id: u64, pending: &mut Vec<u8>, final_flush: bool) {
    let valid = match std::str::from_utf8(pending) {
        Ok(_) => pending.len(),
        Err(e) => e.valid_up_to(),
    };
    let take = if final_flush { pending.len() } else { valid };
    if take == 0 {
        return;
    }
    let text = String::from_utf8_lossy(&pending[..take]).into_owned();
    pending.drain(..take);
    send(&WorkerEvent::Token { id, text });
}

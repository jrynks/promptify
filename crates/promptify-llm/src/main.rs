//! Runs llama.cpp in its own process: whisper.cpp in the app vendors a different ggml, and a
//! separate process lets the app enforce cancellation and timeouts by killing it.

use std::io::{self, BufRead, Write};
use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;

use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;
use promptify_core::llm_protocol::{WireFinish, WorkerEvent, WorkerRequest};

fn send(event: &WorkerEvent) {
    let line = serde_json::to_string(event).expect("events serialize");
    let mut out = io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

struct Loaded {
    path: String,
    n_ctx: u32,
    model: LlamaModel,
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

    let mut loaded: Option<Loaded> = None;
    for request in rx {
        match request {
            WorkerRequest::Load { model_path, n_ctx } => {
                if loaded.as_ref().is_some_and(|l| l.path == model_path && l.n_ctx == n_ctx) {
                    send(&WorkerEvent::Loaded { model_path });
                    continue;
                }
                loaded = None;
                let params = LlamaModelParams::default().with_n_gpu_layers(if cfg!(target_os = "macos") { 999 } else { 0 });
                match LlamaModel::load_from_file(&backend, &model_path, &params) {
                    Ok(model) => {
                        loaded = Some(Loaded { path: model_path.clone(), n_ctx, model });
                        send(&WorkerEvent::Loaded { model_path });
                    }
                    Err(e) => send(&WorkerEvent::Error { id: None, message: format!("could not load model: {e}") }),
                }
            }
            WorkerRequest::Generate { id, prompt, max_new_tokens } => {
                let result = match &loaded {
                    Some(l) => generate(&backend, l, id, &prompt, max_new_tokens, &cancelled),
                    None => Err("no model loaded".into()),
                };
                match result {
                    Ok(finish) => send(&WorkerEvent::Done { id, finish }),
                    Err(message) => send(&WorkerEvent::Error { id: Some(id), message }),
                }
            }
            WorkerRequest::Cancel { .. } => {}
        }
    }
}

fn generate(
    backend: &LlamaBackend,
    loaded: &Loaded,
    id: u64,
    prompt: &str,
    max_new_tokens: u32,
    cancelled: &AtomicU64,
) -> Result<WireFinish, String> {
    let model = &loaded.model;
    let tokens = model.str_to_token(prompt, AddBos::Never).map_err(|e| format!("tokenize failed: {e}"))?;
    if tokens.is_empty() {
        return Err("empty prompt".into());
    }
    if tokens.len() + max_new_tokens as usize > loaded.n_ctx as usize {
        return Err(format!("prompt of {} tokens does not fit the {}-token context", tokens.len(), loaded.n_ctx));
    }
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8) as i32;
    let params = LlamaContextParams::default()
        .with_n_ctx(NonZeroU32::new(loaded.n_ctx))
        .with_n_batch(loaded.n_ctx)
        .with_n_threads(threads)
        .with_n_threads_batch(threads);
    let mut ctx = model.new_context(backend, params).map_err(|e| format!("context failed: {e}"))?;

    let mut batch = LlamaBatch::new(loaded.n_ctx as usize, 1);
    let last = tokens.len() - 1;
    for (i, token) in tokens.iter().enumerate() {
        batch.add(*token, i as i32, &[0], i == last).map_err(|e| e.to_string())?;
    }
    ctx.decode(&mut batch).map_err(|e| format!("decode failed: {e}"))?;

    let mut sampler = LlamaSampler::chain_simple([
        LlamaSampler::top_k(40),
        LlamaSampler::top_p(0.9, 1),
        LlamaSampler::min_p(0.05, 1),
        LlamaSampler::temp(0.3),
        LlamaSampler::dist(id as u32),
    ]);
    let mut pending: Vec<u8> = Vec::new();
    for position in (tokens.len() as i32..).take(max_new_tokens as usize) {
        if cancelled.load(Ordering::SeqCst) == id {
            return Ok(WireFinish::Cancelled);
        }
        let token = sampler.sample(&ctx, batch.n_tokens() - 1);
        if model.is_eog_token(token) {
            flush(id, &mut pending, true);
            return Ok(WireFinish::Stop);
        }
        let piece = model.token_to_piece_bytes(token, 256, false, None).map_err(|e| format!("detokenize failed: {e}"))?;
        pending.extend_from_slice(&piece);
        flush(id, &mut pending, false);

        batch.clear();
        batch.add(token, position, &[0], true).map_err(|e| e.to_string())?;
        ctx.decode(&mut batch).map_err(|e| format!("decode failed: {e}"))?;
    }
    flush(id, &mut pending, true);
    Ok(WireFinish::Length)
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

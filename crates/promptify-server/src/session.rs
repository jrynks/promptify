//! One encrypted session with a phone: Noise handshake, device admission, then the job protocol.
//! Transport-agnostic: frames arrive on `inbound` and leave on `outbound`, whether via relay or direct.

use std::sync::Arc;
use std::time::Duration;

use promptify_core::pipeline::{CancelToken, JobEvent, Mode};
use promptify_core::scheduler::{AdmitError, Priority};
use promptify_core::transform::{ClientContext, Input, Transform, TransformOutcome};
use promptify_protocol::messages::{
    AUDIO_SAMPLE_RATE, ClientMessage, ErrorCode, MAX_AUDIO_SECONDS, MAX_TEXT_CHARS, ProfileInfo, ServerMessage, WireContext, WireMode,
    decode_client, decode_pcm16, encode,
};
use promptify_protocol::noise::{Handshake, SecureChannel};
use tokio::sync::mpsc;
use tokio::time::timeout;

use crate::Shared;

pub const KIND_PAIR: u8 = b'P';
pub const KIND_SESSION: u8 = b'S';
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(300);
pub const MAX_PAIRING_FAILURES: u32 = 5;
const QUEUE_WAIT: Duration = Duration::from_secs(30);

pub type Inbound = mpsc::Receiver<Vec<u8>>;
pub type Outbound = mpsc::Sender<Vec<u8>>;

struct Link {
    channel: SecureChannel,
    outbound: Outbound,
}

impl Link {
    async fn send(&mut self, message: &ServerMessage) -> bool {
        match self.channel.seal(&encode(message)) {
            Ok(frame) => self.outbound.send(frame).await.is_ok(),
            Err(_) => false,
        }
    }
}

async fn recv(inbound: &mut Inbound, within: Duration) -> Option<Vec<u8>> {
    timeout(within, inbound.recv()).await.ok().flatten()
}

/// Runs until the peer leaves, the device is revoked, the server stops or the session idles out.
pub async fn run_session(shared: Arc<Shared>, mut inbound: Inbound, outbound: Outbound) {
    let Some(first) = recv(&mut inbound, HANDSHAKE_TIMEOUT).await else { return };
    let Some((&kind, message)) = first.split_first() else { return };
    let admitted = match kind {
        KIND_PAIR => pair(&shared, message, &mut inbound, &outbound).await,
        KIND_SESSION => resume(&shared, message, &outbound).await,
        _ => None,
    };
    let Some((link, device_id)) = admitted else { return };
    shared.sessions.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    shared.devices.touch(&device_id);
    serve(&shared, link, device_id, inbound).await;
    shared.sessions.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
}

async fn pair(shared: &Shared, msg1: &[u8], inbound: &mut Inbound, outbound: &Outbound) -> Option<(Link, String)> {
    let secret = shared.pairing.active_secret()?;
    let mut hs = Handshake::pairing_responder(&shared.identity.keys, &secret).ok()?;
    hs.read(msg1).ok()?;
    outbound.send(hs.write(b"").ok()?).await.ok()?;
    let msg3 = recv(inbound, HANDSHAKE_TIMEOUT).await?;
    if hs.read(&msg3).is_err() {
        shared.pairing.record_failure(&secret, MAX_PAIRING_FAILURES);
        return None;
    }
    let mut channel = hs.into_channel().ok()?;
    let hello = recv(inbound, HANDSHAKE_TIMEOUT).await?;
    let ClientMessage::Hello { device_name } = decode_client(&channel.open(&hello).ok()?).ok()? else { return None };
    // Single use: only the first phone to finish with this secret is paired.
    if !shared.pairing.consume(&secret) {
        return None;
    }
    let device = shared.devices.add(&device_name, &channel.remote_static()).ok()?;
    log::info!("paired device {}", device.id);
    let mut link = Link { channel, outbound: outbound.clone() };
    link.send(&ServerMessage::Paired { device_id: device.id.clone() }).await.then_some((link, device.id))
}

async fn resume(shared: &Shared, msg1: &[u8], outbound: &Outbound) -> Option<(Link, String)> {
    let mut hs = Handshake::session_responder(&shared.identity.keys).ok()?;
    hs.read(msg1).ok()?;
    // IK reveals the phone's key in the first message; unknown or revoked keys get no reply at all.
    let device = shared.devices.find_by_key(&hs.remote_static()?)?;
    outbound.send(hs.write(b"").ok()?).await.ok()?;
    let channel = hs.into_channel().ok()?;
    let mut link = Link { channel, outbound: outbound.clone() };
    link.send(&ServerMessage::Ready { device_id: device.id.clone() }).await.then_some((link, device.id))
}

struct PendingJob {
    id: u64,
    mode: Mode,
    context: WireContext,
    audio: Vec<f32>,
    cancel: CancelToken,
    started: bool,
}

fn is_terminal(message: &ServerMessage) -> Option<u64> {
    match message {
        ServerMessage::Done { id, .. } | ServerMessage::NoSpeech { id } | ServerMessage::Cancelled { id } | ServerMessage::Failed { id, .. } => Some(*id),
        _ => None,
    }
}

async fn serve(shared: &Arc<Shared>, mut link: Link, device_id: String, mut inbound: Inbound) {
    let (events_tx, mut events_rx) = mpsc::channel::<ServerMessage>(256);
    let mut revisions = shared.devices.subscribe();
    let mut stopping = shared.shutdown.subscribe();
    let mut job: Option<PendingJob> = None;
    let max_samples = (AUDIO_SAMPLE_RATE * MAX_AUDIO_SECONDS) as usize;

    loop {
        tokio::select! {
            frame = timeout(IDLE_TIMEOUT, inbound.recv()) => {
                let Ok(Some(frame)) = frame else { break };
                let Ok(plain) = link.channel.open(&frame) else { break };
                let message = match decode_client(&plain) {
                    Ok(message) => message,
                    Err(_) => {
                        if !link.send(&ServerMessage::Error { code: ErrorCode::BadMessage, message: "unreadable message".into() }).await { break }
                        continue;
                    }
                };
                let reply = match message {
                    ClientMessage::Ping => Some(ServerMessage::Pong),
                    ClientMessage::Hello { .. } => None,
                    ClientMessage::ListProfiles => Some(ServerMessage::Profiles {
                        profiles: shared.service.profiles().all().iter().map(|p| ProfileInfo { id: p.id.clone(), name: p.name.clone() }).collect(),
                    }),
                    ClientMessage::Transform { id, mode, context, text } => {
                        if job.is_some() {
                            Some(ServerMessage::Error { code: ErrorCode::Busy, message: "a job is already running on this device".into() })
                        } else if text.as_ref().is_some_and(|t| t.chars().count() > MAX_TEXT_CHARS) {
                            Some(ServerMessage::Error { code: ErrorCode::TooLarge, message: "text is too long".into() })
                        } else {
                            let mode = match mode { WireMode::Prompt => Mode::Prompt, WireMode::Dictation => Mode::Dictation };
                            let mut pending = PendingJob { id, mode, context, audio: Vec::new(), cancel: CancelToken::default(), started: false };
                            if let Some(text) = text {
                                start(shared, &device_id, &mut pending, Some(text), events_tx.clone());
                            }
                            job = Some(pending);
                            None
                        }
                    }
                    ClientMessage::AudioChunk { id, pcm16 } => match job.as_mut().filter(|j| j.id == id && !j.started) {
                        None => Some(ServerMessage::Error { code: ErrorCode::UnknownJob, message: "no recording in progress for this id".into() }),
                        Some(pending) => match decode_pcm16(&pcm16) {
                            Some(samples) if pending.audio.len() + samples.len() <= max_samples => {
                                pending.audio.extend(samples);
                                None
                            }
                            Some(_) => {
                                job = None;
                                Some(ServerMessage::Error { code: ErrorCode::TooLarge, message: "recording is too long".into() })
                            }
                            None => Some(ServerMessage::Error { code: ErrorCode::BadMessage, message: "invalid audio chunk".into() }),
                        },
                    },
                    ClientMessage::AudioEnd { id } => match job.as_mut().filter(|j| j.id == id && !j.started) {
                        Some(pending) => {
                            start(shared, &device_id, pending, None, events_tx.clone());
                            None
                        }
                        None => Some(ServerMessage::Error { code: ErrorCode::UnknownJob, message: "no recording in progress for this id".into() }),
                    },
                    ClientMessage::Cancel { id } => {
                        match job.take() {
                            Some(pending) if pending.id == id && pending.started => {
                                pending.cancel.cancel();
                                job = Some(pending);
                                None
                            }
                            Some(pending) if pending.id == id => Some(ServerMessage::Cancelled { id }),
                            other => {
                                job = other;
                                None
                            }
                        }
                    }
                };
                if let Some(reply) = reply && !link.send(&reply).await {
                    break;
                }
            }
            Some(event) = events_rx.recv() => {
                if let Some(id) = is_terminal(&event) && job.as_ref().is_some_and(|j| j.id == id) {
                    job = None;
                }
                if !link.send(&event).await {
                    break;
                }
            }
            _ = revisions.changed() => {
                if !shared.devices.is_active(&device_id) {
                    break;
                }
            }
            _ = stopping.changed() => break,
        }
    }
    // Leaving for any reason stops the engine work this device started.
    if let Some(pending) = job {
        pending.cancel.cancel();
    }
}

fn start(shared: &Arc<Shared>, device_id: &str, pending: &mut PendingJob, text: Option<String>, events: mpsc::Sender<ServerMessage>) {
    pending.started = true;
    let service = shared.service.clone();
    let device_id = device_id.to_owned();
    let (id, mode, cancel) = (pending.id, pending.mode, pending.cancel.clone());
    let context = ClientContext { app: pending.context.app.clone(), url: pending.context.url.clone(), title: pending.context.title.clone() };
    let audio = std::mem::take(&mut pending.audio);
    tokio::task::spawn_blocking(move || {
        let target = context.to_active();
        let profile = service.profiles().resolve(&target);
        let input = match &text {
            Some(text) => Input::Text(text),
            None => Input::Audio(&audio),
        };
        let transform = Transform { input, mode, profile, target: &target, surrounding: None, use_history: false, use_tools: false, auto_mode: false };
        let mut on_event = |event: JobEvent<'_>| {
            let message = match event {
                JobEvent::Stage(stage) => ServerMessage::Stage { id, stage: serde_json::to_value(stage).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default() },
                JobEvent::Transcript(text) => ServerMessage::Transcript { id, text: text.to_owned() },
                JobEvent::Token(text) => ServerMessage::Token { id, text: text.to_owned() },
                JobEvent::Routing(_) => return,
            };
            let _ = events.blocking_send(message);
        };
        let result = service.run_scheduled(Priority::Device, &device_id, &transform, &cancel, QUEUE_WAIT, &mut on_event);
        let profile_id = profile.id.clone();
        let message = match result {
            Ok(report) => {
                let structure = report.structure.and_then(|s| serde_json::to_value(s).ok()).and_then(|v| v.as_str().map(str::to_owned));
                match report.outcome {
                    TransformOutcome::Ready { text } => ServerMessage::Done { id, text, truncated: false, profile: profile_id, structure },
                    TransformOutcome::Truncated { text } => ServerMessage::Done { id, text, truncated: true, profile: profile_id, structure },
                    TransformOutcome::NoSpeech => ServerMessage::NoSpeech { id },
                    TransformOutcome::Cancelled => ServerMessage::Cancelled { id },
                    TransformOutcome::Failed { reason, .. } => ServerMessage::Failed {
                        id,
                        reason: serde_json::to_value(reason).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default(),
                    },
                }
            }
            Err(AdmitError::Cancelled) => ServerMessage::Cancelled { id },
            Err(_) => ServerMessage::Failed { id, reason: "engine_busy".into() },
        };
        let _ = events.blocking_send(message);
    });
}

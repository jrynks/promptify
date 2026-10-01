use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use promptify_core::history::{HistoryContext, NewHistoryEntry};
use promptify_core::pipeline::{BackendError, CancelToken, FinishReason, Generation, GenerationRequest, Generator, History, Limits, Transcriber};
use promptify_core::profiles::ProfileSet;
use promptify_core::scheduler::SchedulerLimits;
use promptify_protocol::messages::{ClientMessage, ErrorCode, ServerMessage, WireContext, WireMode};

use super::*;
use crate::client::{self, Connection};

struct Echo;

impl Transcriber for Echo {
    fn transcribe(&self, audio: &[f32], _: &CancelToken) -> Result<String, BackendError> {
        Ok(format!("heard {} samples", audio.len()))
    }
}

#[derive(Default)]
struct SlowGenerator {
    delay_ms: AtomicUsize,
    saw_cancel: AtomicBool,
    calls: AtomicUsize,
}

impl Generator for SlowGenerator {
    fn generate(&self, req: &GenerationRequest<'_>, cancel: &CancelToken, on_token: &mut dyn FnMut(&str)) -> Result<Generation, BackendError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let until = std::time::Instant::now() + Duration::from_millis(self.delay_ms.load(Ordering::SeqCst) as u64);
        while std::time::Instant::now() < until {
            if cancel.is_cancelled() {
                self.saw_cancel.store(true, Ordering::SeqCst);
                return Err(BackendError("cancelled".into()));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let said = req.messages.last().map(|m| m.content.clone()).unwrap_or_default();
        on_token("Prompt ");
        Ok(Generation { text: format!("Prompt for: {}", said.lines().last().unwrap_or_default().len()), finish: FinishReason::Stop })
    }
}

struct NoHistory;

impl History for NoHistory {
    fn context(&self, _: &str, _: &str) -> HistoryContext {
        HistoryContext::default()
    }
    fn record(&self, _: NewHistoryEntry) -> Result<bool, BackendError> {
        Ok(false)
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    _relay_rt: tokio::runtime::Runtime,
    server: RemoteServer,
    generator: Arc<SlowGenerator>,
    rt: tokio::runtime::Runtime,
}

fn fixture() -> Fixture {
    let relay_rt = tokio::runtime::Runtime::new().unwrap();
    let relay_addr = relay_rt.block_on(async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = promptify_relay::router(promptify_relay::Relay::new(promptify_relay::Limits::default()));
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        addr
    });
    let dir = tempfile::tempdir().unwrap();
    let generator = Arc::new(SlowGenerator::default());
    let service = Arc::new(TransformService::new(Arc::new(Echo), generator.clone(), Arc::new(NoHistory), ProfileSet::bundled(), Limits::default(), SchedulerLimits::default()));
    let config = ServerConfig {
        data_dir: dir.path().to_path_buf(),
        relay_url: Some(format!("ws://{relay_addr}")),
        listen: Some("127.0.0.1:0".parse().unwrap()),
        advertise_direct: None,
    };
    let server = RemoteServer::start(config, service).unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while server.status().relay != RelayStatus::Connected {
        assert!(std::time::Instant::now() < deadline, "relay link never connected: {:?}", server.status().relay);
        std::thread::sleep(Duration::from_millis(20));
    }
    Fixture { _dir: dir, _relay_rt: relay_rt, server, generator, rt }
}

fn offer(f: &Fixture, with_direct: bool) -> PairingOffer {
    let info = f.server.pairing_offer(120).unwrap();
    let mut offer = PairingOffer::parse(&info.uri, now_unix()).unwrap();
    if with_direct {
        offer.direct = Some(f.server.listen_addr().unwrap().to_string());
    }
    offer
}

fn text_job(f: &Fixture, connection: &mut Connection, id: u64, text: &str) -> ServerMessage {
    f.rt.block_on(connection.transform_text(id, WireMode::Prompt, WireContext { app: "com.openai.chatgpt".into(), ..Default::default() }, text, &mut |_| {})).unwrap()
}

#[test]
fn pair_over_relay_then_reconnect_and_transform() {
    let f = fixture();
    let offer = offer(&f, false);
    let (identity, mut conn) = f.rt.block_on(client::pair(&offer, "Test phone", false)).unwrap();
    assert_eq!(f.server.devices().len(), 1);
    assert_eq!(f.server.devices()[0].name, "Test phone");
    match text_job(&f, &mut conn, 1, "compare three crm tools") {
        ServerMessage::Done { id: 1, profile, truncated: false, .. } => assert_eq!(profile, "chatgpt"),
        other => panic!("unexpected {other:?}"),
    }
    drop(conn);
    let mut again = f.rt.block_on(client::open(&identity, false)).unwrap();
    assert!(matches!(text_job(&f, &mut again, 2, "hello"), ServerMessage::Done { id: 2, .. }));
    assert_eq!(f.server.status().devices, 1);
}

#[test]
fn pairing_offer_is_single_use_and_must_match() {
    let f = fixture();
    let offer = offer(&f, false);
    f.rt.block_on(client::pair(&offer, "first", false)).unwrap();
    assert!(f.rt.block_on(client::pair(&offer, "second", false)).is_err(), "offer reused");
    let fresh = self::offer(&f, false);
    let mut forged = fresh.clone();
    forged.secret[0] ^= 1;
    assert!(f.rt.block_on(client::pair(&forged, "forged", false)).is_err());
    let mut impostor = fresh.clone();
    impostor.desktop_key[0] ^= 1;
    assert!(f.rt.block_on(client::pair(&impostor, "pinned", false)).is_err(), "client accepted an unpinned desktop key");
    assert_eq!(f.server.devices().len(), 1);
    f.rt.block_on(client::pair(&fresh, "late", false)).unwrap();
    assert_eq!(f.server.devices().len(), 2);
}

#[test]
fn repeated_bad_pairing_attempts_kill_the_offer() {
    let f = fixture();
    let good = offer(&f, false);
    for _ in 0..session::MAX_PAIRING_FAILURES {
        let mut forged = good.clone();
        forged.secret[5] ^= 0xff;
        assert!(f.rt.block_on(client::pair(&forged, "x", false)).is_err());
    }
    assert!(f.rt.block_on(client::pair(&good, "real", false)).is_err(), "offer survived brute force attempts");
    assert!(f.server.status().pairing_expires_unix.is_none());
}

#[test]
fn removed_device_is_disconnected_and_refused() {
    let f = fixture();
    let (identity, mut conn) = f.rt.block_on(client::pair(&offer(&f, false), "phone", false)).unwrap();
    assert!(f.server.remove_device(&identity.device_id).unwrap());
    let closed = f.rt.block_on(async { tokio::time::timeout(Duration::from_secs(5), conn.recv()).await });
    assert!(matches!(closed, Ok(Err(_))), "revoked session stayed open: {closed:?}");
    assert!(f.rt.block_on(client::open(&identity, false)).is_err());
}

#[test]
fn unknown_device_gets_no_session() {
    let f = fixture();
    let (identity, _conn) = f.rt.block_on(client::pair(&offer(&f, false), "phone", false)).unwrap();
    let stranger = identity.with_new_keys();
    assert!(f.rt.block_on(client::open(&stranger, false)).is_err());
    assert!(f.rt.block_on(client::open(&identity, false)).is_ok());
}

#[test]
fn direct_link_busy_and_cancel() {
    let f = fixture();
    let (identity, _) = f.rt.block_on(client::pair(&offer(&f, true), "lan phone", true)).unwrap();
    let mut conn = f.rt.block_on(client::open(&identity, true)).unwrap();
    f.generator.delay_ms.store(3000, Ordering::SeqCst);
    let context = WireContext::default();
    f.rt.block_on(async {
        conn.send(&ClientMessage::Transform { id: 7, mode: WireMode::Prompt, context: context.clone(), text: Some("long job".into()) }).await.unwrap();
        conn.send(&ClientMessage::Transform { id: 8, mode: WireMode::Prompt, context, text: Some("second".into()) }).await.unwrap();
        loop {
            match conn.recv().await.unwrap() {
                ServerMessage::Error { code: ErrorCode::Busy, .. } => break,
                ServerMessage::Stage { .. } | ServerMessage::Transcript { .. } => continue,
                other => panic!("unexpected {other:?}"),
            }
        }
        while f.generator.calls.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        conn.send(&ClientMessage::Cancel { id: 7 }).await.unwrap();
        loop {
            match conn.recv().await.unwrap() {
                ServerMessage::Cancelled { id: 7 } => break,
                ServerMessage::Stage { .. } | ServerMessage::Transcript { .. } | ServerMessage::Token { .. } => continue,
                other => panic!("unexpected {other:?}"),
            }
        }
    });
    assert!(f.generator.saw_cancel.load(Ordering::SeqCst));
}

#[test]
fn audio_job_and_oversized_audio() {
    use promptify_protocol::messages::{MAX_AUDIO_CHUNK_BYTES, encode_pcm16};
    let f = fixture();
    let (_, mut conn) = f.rt.block_on(client::pair(&offer(&f, true), "phone", true)).unwrap();
    f.rt.block_on(async {
        conn.send(&ClientMessage::Transform { id: 1, mode: WireMode::Dictation, context: WireContext::default(), text: None }).await.unwrap();
        conn.send(&ClientMessage::AudioChunk { id: 1, pcm16: encode_pcm16(&[0.1; 1600]) }).await.unwrap();
        conn.send(&ClientMessage::AudioEnd { id: 1 }).await.unwrap();
        loop {
            match conn.recv().await.unwrap() {
                ServerMessage::Done { id: 1, text, .. } => {
                    assert_eq!(text, "heard 1600 samples");
                    break;
                }
                ServerMessage::Stage { .. } | ServerMessage::Transcript { .. } => continue,
                other => panic!("unexpected {other:?}"),
            }
        }
        conn.send(&ClientMessage::Transform { id: 2, mode: WireMode::Dictation, context: WireContext::default(), text: None }).await.unwrap();
        let chunk = encode_pcm16(&vec![0.0; MAX_AUDIO_CHUNK_BYTES / 2]);
        let chunks = (promptify_protocol::messages::AUDIO_SAMPLE_RATE * promptify_protocol::messages::MAX_AUDIO_SECONDS) as usize / (MAX_AUDIO_CHUNK_BYTES / 2) + 2;
        for _ in 0..chunks {
            conn.send(&ClientMessage::AudioChunk { id: 2, pcm16: chunk.clone() }).await.unwrap();
        }
        loop {
            match conn.recv().await.unwrap() {
                ServerMessage::Error { code: ErrorCode::TooLarge, .. } => break,
                ServerMessage::Error { code: ErrorCode::UnknownJob, .. } => continue,
                other => panic!("unexpected {other:?}"),
            }
        }
    });
}

fn http(addr: SocketAddr, auth: Option<&str>, body: &str) -> String {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(addr).unwrap();
    let auth = auth.map(|t| format!("Authorization: Bearer {t}\r\n")).unwrap_or_default();
    write!(stream, "POST /v1/transform HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\n{auth}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
    let mut out = String::new();
    stream.read_to_string(&mut out).unwrap();
    out
}

#[test]
fn local_api_requires_the_token() {
    let f = fixture();
    let addr = f.server.listen_addr().unwrap();
    let body = r#"{"text":"plan a trip","app":"com.anthropic.claude"}"#;
    assert!(http(addr, None, body).starts_with("HTTP/1.1 401"));
    assert!(http(addr, Some("wrong-token-wrong-token-wrong-token-xx"), body).starts_with("HTTP/1.1 401"));
    let token = std::fs::read_to_string(api_token_path(&f.server.shared.config.data_dir)).unwrap();
    let ok = http(addr, Some(token.trim()), body);
    assert!(ok.starts_with("HTTP/1.1 200"), "{ok}");
    assert!(ok.contains(r#""profile":"claude""#));
}

#[test]
fn relay_urls_must_be_secure_on_the_internet() {
    assert!(validate_relay_url("wss://relay.example.net").is_ok());
    assert!(validate_relay_url("ws://127.0.0.1:8787").is_ok());
    assert!(validate_relay_url("ws://192.168.1.5:8787").is_ok());
    assert!(validate_relay_url("ws://relay.example.net").is_err());
    assert!(validate_relay_url("https://relay.example.net").is_err());
}

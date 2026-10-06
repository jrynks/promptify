use super::*;
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};
#[derive(Default)]
struct MemoryCredentials {
    values: Mutex<HashMap<String, String>>,
    fail: AtomicBool,
}
impl CredentialStore for MemoryCredentials {
    fn get(&self, id: &str) -> Result<Option<String>, String> {
        Ok(self.values.lock().unwrap().get(id).cloned())
    }
    fn set(&self, id: &str, secret: &str) -> Result<(), String> {
        if self.fail.load(Ordering::SeqCst) {
            return Err("locked credential store".into());
        }
        self.values.lock().unwrap().insert(id.into(), secret.into());
        Ok(())
    }
    fn remove(&self, id: &str) -> Result<(), String> {
        self.values.lock().unwrap().remove(id);
        Ok(())
    }
}
struct TestDir(PathBuf);
impl TestDir {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::current_dir().unwrap().join(format!(
            ".inference-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir(&dir).unwrap();
        Self(dir)
    }
}
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn manager(dir: &TestDir, store: Arc<MemoryCredentials>) -> InferenceManager {
    InferenceManager::with_credential_store(
        dir.0.clone(),
        Manifest::bundled(),
        dir.0.join("models"),
        Arc::new(RwLock::new(crate::settings::AppSettings::default())),
        store,
    )
}
fn input(url: &str) -> ConnectionInput {
    ConnectionInput {
        id: "test".into(),
        name: "Test endpoint".into(),
        provider: Provider::LmStudio,
        base_url: url.into(),
        protocol: Protocol::OpenaiChatCompletions,
        auth: AuthMode::None,
        model: "exact-model".into(),
        stream: true,
        allow_insecure_lan: false,
        consent_remote: false,
        secret: None,
        remove_secret: false,
    }
}
fn select(m: &InferenceManager) {
    m.select(InferenceSelection::Connection {
        connection_id: "test".into(),
        model: "exact-model".into(),
    })
    .unwrap();
}
fn request() -> (Vec<ChatMessage>, Instant) {
    (
        vec![ChatMessage {
            role: Role::User,
            content: "Synthetic test only.".into(),
        }],
        Instant::now() + Duration::from_secs(2),
    )
}
fn server(response: &'static str) -> (String, std::thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let thread = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut bytes = Vec::new();
        let mut buf = [0; 4096];
        let mut expected = None;
        loop {
            let n = socket.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            bytes.extend_from_slice(&buf[..n]);
            if let Some(pos) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&bytes[..pos]).to_ascii_lowercase();
                let len = headers
                    .lines()
                    .find_map(|l| {
                        l.strip_prefix("content-length:")
                            .and_then(|l| l.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                expected = Some(pos + 4 + len);
            }
            if expected.is_some_and(|n| bytes.len() >= n) {
                break;
            }
        }
        for chunk in response.as_bytes().chunks(1) {
            if socket.write_all(chunk).is_err() {
                break;
            }
        }
        String::from_utf8(bytes).unwrap()
    });
    (format!("http://{address}/v1"), thread)
}
const CHAT: &str = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"é OK\"},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
#[test]
fn absent_invalid_and_newer_sidecar() {
    let dir = TestDir::new();
    let store = Arc::new(MemoryCredentials::default());
    assert_eq!(
        manager(&dir, store.clone()).config().unwrap().selection,
        InferenceSelection::BundledLocal
    );
    for content in [
        "{bad",
        r#"{"version":99,"revision":0,"selection":{"kind":"bundled_local"},"connections":[]}"#,
    ] {
        std::fs::write(dir.0.join("inference.json"), content).unwrap();
        let m = manager(&dir, store.clone());
        assert!(m.config().is_err());
        assert!(m.snapshot().is_err());
        assert_eq!(m.status().state, StatusState::Error);
    }
}
#[test]
fn credentials_transactional_write_only_and_revocation() {
    let dir = TestDir::new();
    let store = Arc::new(MemoryCredentials::default());
    let m = manager(&dir, store.clone());
    let mut c = input("http://127.0.0.1:1234/v1");
    c.auth = AuthMode::ApiKey;
    c.secret = Some("synthetic-secret".into());
    store.fail.store(true, Ordering::SeqCst);
    assert!(m.upsert_connection(c.clone()).is_err());
    assert!(m.config().unwrap().connections.is_empty());
    store.fail.store(false, Ordering::SeqCst);
    m.upsert_connection(c.clone()).unwrap();
    select(&m);
    assert!(
        !std::fs::read_to_string(dir.0.join("inference.json"))
            .unwrap()
            .contains("synthetic-secret")
    );
    assert!(
        !serde_json::to_string(&m.config().unwrap())
            .unwrap()
            .contains("synthetic-secret")
    );
    let session = m.snapshot().unwrap().unwrap();
    c.secret = None;
    c.remove_secret = true;
    m.upsert_connection(c).unwrap();
    let (messages, deadline) = request();
    assert!(
        session
            .generate(
                &GenerationRequest {
                    messages: &messages,
                    stable_prefix: 0,
                    max_new_tokens: 10,
                    deadline
                },
                &CancelToken::default(),
                &mut |_| {}
            )
            .unwrap_err()
            .0
            .contains("revoked")
    );
    assert!(store.values.lock().unwrap().is_empty());
}
#[test]
fn failed_save_not_published_and_credential_rolled_back() {
    let dir = TestDir::new();
    let store = Arc::new(MemoryCredentials::default());
    let m = manager(&dir, store.clone());
    std::fs::create_dir(dir.0.join("inference.json.new")).unwrap();
    let mut c = input("http://127.0.0.1:1234/v1");
    c.auth = AuthMode::ApiKey;
    c.secret = Some("synthetic-secret".into());
    assert!(m.upsert_connection(c).is_err());
    assert!(m.config().unwrap().connections.is_empty());
    assert!(store.values.lock().unwrap().is_empty());
}
#[test]
fn exact_model_no_fake_auth_selection_pinned() {
    let (url, server) = server(CHAT);
    let dir = TestDir::new();
    let m = manager(&dir, Arc::new(MemoryCredentials::default()));
    m.upsert_connection(input(&url)).unwrap();
    select(&m);
    let session = m.snapshot().unwrap().unwrap();
    m.select(InferenceSelection::BundledLocal).unwrap();
    let (messages, deadline) = request();
    let mut tokens = String::new();
    let result = session
        .generate(
            &GenerationRequest {
                messages: &messages,
                stable_prefix: 0,
                max_new_tokens: 10,
                deadline,
            },
            &CancelToken::default(),
            &mut |t| tokens.push_str(t),
        )
        .unwrap();
    assert_eq!(result.text, "é OK");
    assert_eq!(tokens, result.text);
    let sent = server.join().unwrap();
    assert!(sent.starts_with("POST /v1/chat/completions "));
    assert!(sent.contains("\"model\":\"exact-model\""));
    assert!(!sent.to_ascii_lowercase().contains("authorization:"));
}
#[test]
fn discovery_not_readiness_explicit_synthetic_test() {
    let (url, thread) = server(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{\"data\":[{\"id\":\"unloaded-model\"}]}",
    );
    let dir = TestDir::new();
    let m = manager(&dir, Arc::new(MemoryCredentials::default()));
    m.upsert_connection(input(&url)).unwrap();
    select(&m);
    assert_eq!(m.status().state, StatusState::Configured);
    assert!(m.preload().is_err());
    assert_eq!(m.discover("test").unwrap()[0].id, "unloaded-model");
    assert_eq!(m.status().state, StatusState::Configured);
    assert!(thread.join().unwrap().starts_with("GET /v1/models "));
    let (url, thread) = server(CHAT);
    m.upsert_connection(input(&url)).unwrap();
    assert_eq!(
        m.test("test", "exact-model").unwrap().state,
        StatusState::Ready
    );
    assert_eq!(m.status().state, StatusState::Ready);
    m.preload().unwrap();
    assert!(
        thread
            .join()
            .unwrap()
            .contains("synthetic connectivity test")
    );
}
#[test]
fn abrupt_eof_and_redirect_fail() {
    for response in [
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: {\"choices\":[{\"delta\":{\"content\":\"partial\"},\"finish_reason\":null}]}\n\n",
        "HTTP/1.1 307 Temporary Redirect\r\nLocation: http://127.0.0.1:9/steal\r\nContent-Length: 0\r\n\r\n",
    ] {
        let (url, thread) = server(response);
        let dir = TestDir::new();
        let m = manager(&dir, Arc::new(MemoryCredentials::default()));
        m.upsert_connection(input(&url)).unwrap();
        select(&m);
        let (messages, deadline) = request();
        assert!(
            m.generate(
                &GenerationRequest {
                    messages: &messages,
                    stable_prefix: 0,
                    max_new_tokens: 10,
                    deadline
                },
                &CancelToken::default(),
                &mut |_| {}
            )
            .is_err()
        );
        thread.join().unwrap();
    }
}
#[test]
fn stalled_headers_cancel_and_deadline_promptly() {
    for cancel_request in [true, false] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let thread = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut bytes = [0; 4096];
            let _ = socket.read(&mut bytes);
            loop {
                match socket.read(&mut bytes) {
                    Ok(0) | Err(_) => break,
                    _ => {}
                }
            }
        });
        let dir = TestDir::new();
        let m = manager(&dir, Arc::new(MemoryCredentials::default()));
        m.upsert_connection(input(&format!("http://{address}/v1")))
            .unwrap();
        select(&m);
        let cancel = CancelToken::default();
        let trigger = cancel.clone();
        let helper = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(70));
            if cancel_request {
                trigger.cancel();
            }
        });
        let (messages, _) = request();
        let start = Instant::now();
        let deadline = start
            + if cancel_request {
                Duration::from_secs(2)
            } else {
                Duration::from_millis(100)
            };
        assert!(
            m.generate(
                &GenerationRequest {
                    messages: &messages,
                    stable_prefix: 0,
                    max_new_tokens: 10,
                    deadline
                },
                &cancel,
                &mut |_| {}
            )
            .is_err()
        );
        assert!(start.elapsed() < Duration::from_secs(1));
        helper.join().unwrap();
        thread.join().unwrap();
    }
}
#[test]
fn endpoint_policy_and_gateway_prefix() {
    let mut c = InferenceConnection {
        id: "test".into(),
        name: "test".into(),
        provider: Provider::Custom,
        base_url: "https://example.com/gateway/v1/".into(),
        protocol: Protocol::OpenaiChatCompletions,
        auth: AuthMode::None,
        model: "model".into(),
        stream: true,
        allow_insecure_lan: false,
        consent_remote: true,
        credential_present: false,
        revision: 1,
    };
    assert_eq!(
        config::endpoint(&c, "chat/completions").unwrap().path(),
        "/gateway/v1/chat/completions"
    );
    for url in [
        "http://example.com/v1",
        "https://key@example.com/v1",
        "https://example.com/v1?key=x",
        "https://example.com/v1#x",
        "http://192.168.1.2/v1",
    ] {
        c.base_url = url.into();
        assert!(config::endpoint(&c, "models").is_err());
    }
    c.allow_insecure_lan = true;
    assert!(config::endpoint(&c, "models").is_ok());
    c.base_url = "http://[::1]:1234/v1".into();
    c.consent_remote = false;
    assert!(config::endpoint(&c, "models").is_ok());
}
#[test]
fn responses_anthropic_streaming_with_correct_headers() {
    let fixtures = [
        (
            Protocol::OpenaiResponses,
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"OK\"}\n\ndata: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"OK\"}]}]}}\n\n",
            "/v1/responses",
        ),
        (
            Protocol::AnthropicMessages,
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: {\"type\":\"message_start\",\"message\":{\"role\":\"assistant\"}}\n\ndata: {\"type\":\"content_block_start\",\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"OK\"}}\n\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n\ndata: {\"type\":\"message_stop\"}\n\n",
            "/v1/messages",
        ),
    ];
    for (protocol, response, path) in fixtures {
        let (url, thread) = server(response);
        let dir = TestDir::new();
        let m = manager(&dir, Arc::new(MemoryCredentials::default()));
        let mut c = input(&url);
        c.protocol = protocol;
        c.auth = AuthMode::ApiKey;
        c.secret = Some("synthetic-token".into());
        m.upsert_connection(c).unwrap();
        select(&m);
        let mut messages = vec![ChatMessage {
            role: Role::System,
            content: "Synthetic system instruction.".into(),
        }];
        messages.extend(request().0);
        let result = m
            .generate(
                &GenerationRequest {
                    messages: &messages,
                    stable_prefix: 1,
                    max_new_tokens: 10,
                    deadline: Instant::now() + Duration::from_secs(2),
                },
                &CancelToken::default(),
                &mut |_| {},
            )
            .unwrap();
        assert_eq!(result.text, "OK");
        let sent = thread.join().unwrap();
        assert!(sent.starts_with(&format!("POST {path} ")));
        let sent = sent.to_ascii_lowercase();
        if protocol == Protocol::AnthropicMessages {
            assert!(sent.contains("anthropic-version: 2023-06-01"));
            assert!(sent.contains("x-api-key: synthetic-token"));
            assert!(!sent.contains("authorization:"));
            assert!(sent.contains(
                "\"system\":[{\"text\":\"synthetic system instruction.\",\"type\":\"text\"}]"
            ));
        } else {
            assert!(sent.contains("authorization: bearer synthetic-token"));
            assert!(sent.contains("\"store\":false"));
        }
    }
}
#[test]
fn nonstream_protocols_and_google_preset() {
    for (protocol, response) in [
        (
            Protocol::OpenaiChatCompletions,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{\"choices\":[{\"message\":{\"content\":\"OK\"},\"finish_reason\":\"stop\"}]}",
        ),
        (
            Protocol::OpenaiResponses,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"OK\"}]}]}",
        ),
        (
            Protocol::AnthropicMessages,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{\"content\":[{\"type\":\"thinking\",\"thinking\":\"hidden\"},{\"type\":\"text\",\"text\":\"OK\"}],\"stop_reason\":\"end_turn\"}",
        ),
    ] {
        let (url, thread) = server(response);
        let dir = TestDir::new();
        let m = manager(&dir, Arc::new(MemoryCredentials::default()));
        let mut c = input(&url);
        c.protocol = protocol;
        c.stream = false;
        m.upsert_connection(c).unwrap();
        select(&m);
        let (messages, deadline) = request();
        assert_eq!(
            m.generate(
                &GenerationRequest {
                    messages: &messages,
                    stable_prefix: 0,
                    max_new_tokens: 10,
                    deadline
                },
                &CancelToken::default(),
                &mut |_| {}
            )
            .unwrap()
            .text,
            "OK"
        );
        assert!(thread.join().unwrap().contains("\"stream\":false"));
    }
    let dir = TestDir::new();
    let m = manager(&dir, Arc::new(MemoryCredentials::default()));
    let mut c = input("");
    c.provider = Provider::Google;
    c.auth = AuthMode::ApiKey;
    c.consent_remote = true;
    c.secret = Some("synthetic-google-key".into());
    assert_eq!(
        m.upsert_connection(c).unwrap().connections[0].base_url,
        Provider::Google.default_base_url().unwrap()
    );
    select(&m);
    assert!(m.preload().is_err());
    assert_eq!(m.status().state, StatusState::Configured);
}
#[test]
fn local_worker_cache_pins_model_gpu_without_spawn() {
    let dir = TestDir::new();
    let m = manager(&dir, Arc::new(MemoryCredentials::default()));
    let first = m.snapshot().unwrap().unwrap();
    assert!(Arc::ptr_eq(&first, &m.snapshot().unwrap().unwrap()));
    let gpu = m.settings.read().unwrap().use_gpu;
    m.settings.write().unwrap().use_gpu = !gpu;
    let weak = Arc::downgrade(&first);
    let changed = m.snapshot().unwrap().unwrap();
    assert!(!Arc::ptr_eq(&first, &changed));
    drop(first);
    assert!(
        weak.upgrade().is_none(),
        "obsolete cache does not retain its worker"
    );
    m.settings.write().unwrap().use_gpu = gpu;
    let current = m.snapshot().unwrap().unwrap();
    assert!(Arc::ptr_eq(&current, &m.snapshot().unwrap().unwrap()));
}

#[test]
fn external_selection_releases_idle_local_worker_but_preserves_job_pin() {
    let dir = TestDir::new();
    let m = manager(&dir, Arc::new(MemoryCredentials::default()));
    let pinned = m.snapshot().unwrap().unwrap();
    let weak = Arc::downgrade(&pinned);
    m.upsert_connection(input("http://127.0.0.1:1234/v1"))
        .unwrap();
    select(&m);
    assert!(m.local_workers.lock().unwrap().is_empty());
    assert!(
        weak.upgrade().is_some(),
        "running local job retains its immutable worker"
    );
    drop(pinned);
    assert!(
        weak.upgrade().is_none(),
        "idle worker is released after its final job ends"
    );
}
#[test]
fn edit_revokes_and_selection_preserves_sessions_until_deletion() {
    let (url, thread) = server(CHAT);
    let dir = TestDir::new();
    let m = manager(&dir, Arc::new(MemoryCredentials::default()));
    let c = input(&url);
    m.upsert_connection(c.clone()).unwrap();
    select(&m);
    let session = m.snapshot().unwrap().unwrap();
    let (messages, deadline) = request();
    let cancel = CancelToken::default();
    session
        .generate(
            &GenerationRequest {
                messages: &messages,
                stable_prefix: 0,
                max_new_tokens: 10,
                deadline,
            },
            &cancel,
            &mut |_| {},
        )
        .unwrap();
    thread.join().unwrap();
    assert!(session.is_valid());
    let mut edited = c;
    edited.name = "Edited for next job".into();
    m.upsert_connection(edited).unwrap();
    assert!(!session.is_valid());
    assert!(cancel.is_cancelled());
    let replacement_session = m.snapshot().unwrap().unwrap();
    assert!(replacement_session.is_valid());
    assert!(m.remove_connection("test").is_err());
    m.select(InferenceSelection::BundledLocal).unwrap();
    assert!(
        replacement_session.is_valid(),
        "selection changes do not revoke pinned sessions"
    );
    m.remove_connection("test").unwrap();
    assert!(!replacement_session.is_valid());
    assert!(cancel.is_cancelled());
    assert!(
        session
            .generate(
                &GenerationRequest {
                    messages: &messages,
                    stable_prefix: 0,
                    max_new_tokens: 10,
                    deadline
                },
                &CancelToken::default(),
                &mut |_| {}
            )
            .is_err()
    );
}
#[test]
fn credential_removal_aborts_inflight_and_cancels_job() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut bytes = [0; 4096];
        let _ = socket.read(&mut bytes);
        started_tx.send(()).unwrap();
        loop {
            match socket.read(&mut bytes) {
                Ok(0) | Err(_) => break,
                _ => {}
            }
        }
    });
    let dir = TestDir::new();
    let m = manager(&dir, Arc::new(MemoryCredentials::default()));
    let mut c = input(&url);
    c.auth = AuthMode::ApiKey;
    c.secret = Some("synthetic-secret".into());
    m.upsert_connection(c.clone()).unwrap();
    select(&m);
    let session = m.snapshot().unwrap().unwrap();
    let cancel = CancelToken::default();
    let job_cancel = cancel.clone();
    let generation = std::thread::spawn(move || {
        let (messages, deadline) = request();
        session.generate(
            &GenerationRequest {
                messages: &messages,
                stable_prefix: 0,
                max_new_tokens: 10,
                deadline,
            },
            &job_cancel,
            &mut |_| {},
        )
    });
    started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let start = Instant::now();
    c.secret = None;
    c.remove_secret = true;
    m.upsert_connection(c).unwrap();
    assert!(generation.join().unwrap().is_err());
    assert!(cancel.is_cancelled());
    assert!(start.elapsed() < Duration::from_secs(1));
    thread.join().unwrap();
}
#[test]
fn recovery_preserves_backup() {
    let dir = TestDir::new();
    std::fs::write(dir.0.join("inference.json"), "{broken").unwrap();
    let m = manager(&dir, Arc::new(MemoryCredentials::default()));
    m.recover(InferenceConfig::default()).unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.0.join("inference.json.bak")).unwrap(),
        "{broken"
    );
    assert!(m.config().is_ok());
}

#[test]
fn explicit_reset_preserves_sidecar_revokes_sessions_and_removes_keys() {
    let dir = TestDir::new();
    let store = Arc::new(MemoryCredentials::default());
    let m = manager(&dir, store.clone());
    let mut c = input("http://127.0.0.1:1234/v1");
    c.auth = AuthMode::ApiKey;
    c.secret = Some("synthetic-key".into());
    m.upsert_connection(c).unwrap();
    select(&m);
    let session = m.snapshot().unwrap().unwrap();
    let previous = std::fs::read(dir.0.join("inference.json")).unwrap();
    assert_eq!(
        m.reset().unwrap().selection,
        InferenceSelection::BundledLocal
    );
    assert_eq!(
        std::fs::read(dir.0.join("inference.json.bak")).unwrap(),
        previous
    );
    assert!(store.values.lock().unwrap().is_empty());
    let (messages, deadline) = request();
    assert!(
        session
            .generate(
                &GenerationRequest {
                    messages: &messages,
                    stable_prefix: 0,
                    max_new_tokens: 10,
                    deadline,
                },
                &CancelToken::default(),
                &mut |_| {}
            )
            .is_err()
    );
}

#[test]
fn stalled_stream_is_aborted_before_deadline_and_does_not_return_partial_success() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut bytes = [0; 4096];
        let _ = socket.read(&mut bytes);
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: {\"choices\":[{\"delta\":{\"content\":\"partial\"},\"finish_reason\":null}]}\n\n").unwrap();
        loop {
            match socket.read(&mut bytes) {
                Ok(0) | Err(_) => break,
                _ => {}
            }
        }
    });
    let dir = TestDir::new();
    let m = manager(&dir, Arc::new(MemoryCredentials::default()));
    m.upsert_connection(input(&url)).unwrap();
    select(&m);
    let (messages, _) = request();
    let start = Instant::now();
    let deadline = start + Duration::from_millis(150);
    let mut text = String::new();
    let result = m.generate(
        &GenerationRequest {
            messages: &messages,
            stable_prefix: 0,
            max_new_tokens: 10,
            deadline,
        },
        &CancelToken::default(),
        &mut |token| text.push_str(token),
    );
    assert!(result.is_err());
    assert_eq!(text, "partial");
    assert!(start.elapsed() < Duration::from_secs(1));
    server.join().unwrap();
}

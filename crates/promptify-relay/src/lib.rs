//! Blind relay: desktops host a room, paired devices connect to it, and the relay forwards opaque
//! (end-to-end encrypted) frames between them. It stores nothing and logs no payloads.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use futures_util::{SinkExt, StreamExt};
use promptify_protocol::pairing::{is_room_id, room_id};
use promptify_protocol::relay::{HostFrame, HostHello, MAX_HOST_FRAME, MAX_PAYLOAD};
use tokio::sync::mpsc;
use tokio::time::timeout;

#[derive(Debug, Clone)]
pub struct Limits {
    pub max_rooms: usize,
    pub max_channels_per_room: usize,
    pub bytes_per_sec: u64,
    pub burst_bytes: u64,
    pub idle: Duration,
    pub hello_timeout: Duration,
    pub queue: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_rooms: 1024,
            max_channels_per_room: 8,
            bytes_per_sec: 512 * 1024,
            burst_bytes: 2 * 1024 * 1024,
            idle: Duration::from_secs(120),
            hello_timeout: Duration::from_secs(10),
            queue: 64,
        }
    }
}

/// Test hook that sees every forwarded payload, used to prove the relay only carries ciphertext.
pub type Observer = Arc<dyn Fn(&[u8]) + Send + Sync>;

struct Room {
    generation: u64,
    host: mpsc::Sender<Message>,
    clients: HashMap<u32, mpsc::Sender<Message>>,
    next_channel: u32,
}

pub struct Relay {
    rooms: Mutex<HashMap<String, Room>>,
    limits: Limits,
    next_generation: AtomicU64,
    observer: Option<Observer>,
}

impl Relay {
    pub fn new(limits: Limits) -> Arc<Self> {
        Self::with_observer(limits, None)
    }

    pub fn with_observer(limits: Limits, observer: Option<Observer>) -> Arc<Self> {
        Arc::new(Self { rooms: Mutex::default(), limits, next_generation: AtomicU64::new(1), observer })
    }

    fn rooms(&self) -> std::sync::MutexGuard<'_, HashMap<String, Room>> {
        self.rooms.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn room_count(&self) -> usize {
        self.rooms().len()
    }

    fn observe(&self, payload: &[u8]) {
        if let Some(observer) = &self.observer {
            observer(payload);
        }
    }
}

pub fn router(relay: Arc<Relay>) -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/v1/host", get(host_upgrade))
        .route("/v1/connect/{room}", get(client_upgrade))
        .with_state(relay)
}

struct Bucket {
    tokens: f64,
    last: Instant,
    rate: f64,
    burst: f64,
}

impl Bucket {
    fn new(limits: &Limits) -> Self {
        Self { tokens: limits.burst_bytes as f64, last: Instant::now(), rate: limits.bytes_per_sec as f64, burst: limits.burst_bytes as f64 }
    }

    fn take(&mut self, bytes: usize) -> bool {
        let now = Instant::now();
        self.tokens = (self.tokens + now.duration_since(self.last).as_secs_f64() * self.rate).min(self.burst);
        self.last = now;
        self.tokens -= bytes as f64;
        self.tokens >= 0.0
    }
}

async fn host_upgrade(State(relay): State<Arc<Relay>>, ws: WebSocketUpgrade) -> Response {
    ws.max_message_size(MAX_HOST_FRAME).max_frame_size(MAX_HOST_FRAME).on_upgrade(move |socket| host_session(relay, socket))
}

async fn client_upgrade(State(relay): State<Arc<Relay>>, Path(room): Path<String>, ws: WebSocketUpgrade) -> Response {
    if !is_room_id(&room) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    if !relay.rooms().contains_key(&room) {
        return StatusCode::NOT_FOUND.into_response();
    }
    ws.max_message_size(MAX_PAYLOAD).max_frame_size(MAX_PAYLOAD).on_upgrade(move |socket| client_session(relay, room, socket))
}

fn spawn_writer(mut sink: futures_util::stream::SplitSink<WebSocket, Message>, mut rx: mpsc::Receiver<Message>) {
    tokio::spawn(async move {
        while let Some(message) = rx.recv().await {
            if sink.send(message).await.is_err() {
                return;
            }
        }
        let _ = sink.send(Message::Close(None)).await;
        let _ = sink.close().await;
    });
}

async fn read_hello(relay: &Relay, stream: &mut futures_util::stream::SplitStream<WebSocket>) -> Option<String> {
    let Ok(Some(Ok(Message::Text(text)))) = timeout(relay.limits.hello_timeout, stream.next()).await else {
        return None;
    };
    let hello: HostHello = serde_json::from_str(text.as_str()).ok()?;
    if hello.v != promptify_protocol::PROTOCOL_VERSION {
        return None;
    }
    let secret = promptify_protocol::decode_key(&hello.host_secret)?;
    Some(room_id(&secret))
}

async fn host_session(relay: Arc<Relay>, socket: WebSocket) {
    let (sink, mut stream) = socket.split();
    let Some(room) = read_hello(&relay, &mut stream).await else {
        return;
    };
    let (tx, rx) = mpsc::channel(relay.limits.queue);
    let generation = relay.next_generation.fetch_add(1, Ordering::SeqCst);
    {
        let mut rooms = relay.rooms();
        if !rooms.contains_key(&room) && rooms.len() >= relay.limits.max_rooms {
            return;
        }
        // A reconnecting desktop replaces its previous link; that link's channels are dropped with it.
        rooms.insert(room.clone(), Room { generation, host: tx.clone(), clients: HashMap::new(), next_channel: 1 });
    }
    spawn_writer(sink, rx);
    let _ = tx.send(Message::Text(format!(r#"{{"ok":true,"room":"{room}"}}"#).into())).await;
    // The room owns the only sender, so replacing or removing the room closes this link.
    drop(tx);

    let mut bucket = Bucket::new(&relay.limits);
    loop {
        let message = match timeout(relay.limits.idle, stream.next()).await {
            Ok(Some(Ok(message))) => message,
            _ => break,
        };
        match message {
            Message::Binary(bytes) => {
                if !bucket.take(bytes.len()) {
                    break;
                }
                match HostFrame::decode(&bytes) {
                    Ok(HostFrame::Data(channel, payload)) => {
                        relay.observe(&payload);
                        let client = relay.rooms().get(&room).filter(|r| r.generation == generation).and_then(|r| r.clients.get(&channel).cloned());
                        if let Some(client) = client {
                            // A device that cannot keep up is disconnected rather than buffered without bound.
                            if client.try_send(Message::Binary(payload.into())).is_err() {
                                remove_client(&relay, &room, generation, channel);
                            }
                        }
                    }
                    Ok(HostFrame::Close(channel)) => remove_client(&relay, &room, generation, channel),
                    Ok(HostFrame::Open(_)) | Err(_) => break,
                }
            }
            Message::Ping(_) | Message::Pong(_) => {}
            Message::Text(_) | Message::Close(_) => break,
        }
        if !relay.rooms().get(&room).is_some_and(|r| r.generation == generation) {
            break;
        }
    }
    let mut rooms = relay.rooms();
    if rooms.get(&room).is_some_and(|r| r.generation == generation) {
        rooms.remove(&room);
    }
}

fn remove_client(relay: &Relay, room: &str, generation: u64, channel: u32) {
    if let Some(r) = relay.rooms().get_mut(room).filter(|r| r.generation == generation) {
        r.clients.remove(&channel);
    }
}

async fn client_session(relay: Arc<Relay>, room: String, socket: WebSocket) {
    let (sink, mut stream) = socket.split();
    let (tx, rx) = mpsc::channel(relay.limits.queue);
    let (channel, generation, host) = {
        let mut rooms = relay.rooms();
        let Some(r) = rooms.get_mut(&room) else { return };
        if r.clients.len() >= relay.limits.max_channels_per_room {
            return;
        }
        let mut channel = r.next_channel;
        while channel == 0 || r.clients.contains_key(&channel) {
            channel = channel.wrapping_add(1);
        }
        r.next_channel = channel.wrapping_add(1);
        r.clients.insert(channel, tx);
        (channel, r.generation, r.host.clone())
    };
    spawn_writer(sink, rx);
    let open = HostFrame::Open(channel).encode().expect("empty frame");
    let opened = host.send(Message::Binary(open.into())).await.is_ok();
    // Never hold the host's sender between frames: a replaced host link must be able to close.
    drop(host);
    let current_host = || relay.rooms().get(&room).filter(|r| r.generation == generation && r.clients.contains_key(&channel)).map(|r| r.host.clone());
    if opened {
        let mut bucket = Bucket::new(&relay.limits);
        loop {
            let message = match timeout(relay.limits.idle, stream.next()).await {
                Ok(Some(Ok(message))) => message,
                _ => break,
            };
            match message {
                Message::Binary(bytes) => {
                    if bytes.len() > MAX_PAYLOAD || !bucket.take(bytes.len()) {
                        break;
                    }
                    let Some(host) = current_host() else { break };
                    relay.observe(&bytes);
                    let frame = HostFrame::Data(channel, bytes.to_vec()).encode().expect("payload within limit");
                    if timeout(Duration::from_secs(5), host.send(Message::Binary(frame.into()))).await.map_or(true, |r| r.is_err()) {
                        break;
                    }
                }
                Message::Ping(_) | Message::Pong(_) => {}
                Message::Text(_) | Message::Close(_) => break,
            }
            if current_host().is_none() {
                break;
            }
        }
    }
    if let Some(host) = current_host() {
        let close = HostFrame::Close(channel).encode().expect("empty frame");
        let _ = host.try_send(Message::Binary(close.into()));
    }
    remove_client(&relay, &room, generation, channel);
}

#[cfg(test)]
mod tests;

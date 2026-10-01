//! Direct link and the local API.
//!
//! `/v1/direct` carries the same end-to-end encrypted session as the relay, so it is safe on a LAN.
//! `/v1/transform` is plain HTTP with a bearer token and only answers loopback callers.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, DefaultBodyLimit, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use futures_util::{SinkExt, StreamExt};
use promptify_core::pipeline::{CancelToken, Mode};
use promptify_core::scheduler::Priority;
use promptify_core::transform::{ClientContext, Input, Transform};
use promptify_protocol::messages::{MAX_TEXT_CHARS, WireMode};
use promptify_protocol::noise::NOISE_MAX_MESSAGE;
use serde::Deserialize;
use tokio::sync::mpsc;

use crate::Shared;

pub const LOCAL_API_CLIENT: &str = "local-api";

pub async fn serve(shared: Arc<Shared>, listener: tokio::net::TcpListener) {
    let mut stopping = shared.shutdown.subscribe();
    let app = Router::new()
        .route("/v1/health", get(|| async { Json(serde_json::json!({ "ok": true, "app": "promptify" })) }))
        .route("/v1/direct", get(direct_upgrade))
        .route("/v1/transform", post(transform))
        .route("/v1/profiles", get(profiles))
        .layer(DefaultBodyLimit::max(64 * 1024))
        .with_state(shared);
    let shutdown = async move {
        let _ = stopping.changed().await;
    };
    let _ = axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).with_graceful_shutdown(shutdown).await;
}

async fn direct_upgrade(State(shared): State<Arc<Shared>>, ws: WebSocketUpgrade) -> Response {
    let max = NOISE_MAX_MESSAGE + 1;
    ws.max_message_size(max).max_frame_size(max).on_upgrade(move |socket| direct_session(shared, socket))
}

async fn direct_session(shared: Arc<Shared>, socket: WebSocket) {
    let (mut sink, mut stream) = socket.split();
    let (in_tx, in_rx) = mpsc::channel::<Vec<u8>>(32);
    let (out_tx, mut out_rx) = mpsc::channel::<Vec<u8>>(32);
    let session = tokio::spawn(crate::session::run_session(shared, in_rx, out_tx));
    let writer = tokio::spawn(async move {
        while let Some(frame) = out_rx.recv().await {
            if sink.send(Message::Binary(frame.into())).await.is_err() {
                break;
            }
        }
        let _ = sink.send(Message::Close(None)).await;
    });
    while let Some(Ok(message)) = stream.next().await {
        match message {
            Message::Binary(bytes) => {
                if in_tx.send(bytes.to_vec()).await.is_err() {
                    break;
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }
    drop(in_tx);
    let _ = session.await;
    let _ = writer.await;
}

fn authorized(shared: &Shared, peer: &SocketAddr, headers: &HeaderMap) -> bool {
    if !peer.ip().is_loopback() {
        return false;
    }
    let Some(value) = headers.get("authorization").and_then(|v| v.to_str().ok()) else { return false };
    let Some(token) = value.strip_prefix("Bearer ") else { return false };
    constant_time_eq(token.trim().as_bytes(), shared.api_token.as_bytes())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TransformBody {
    text: String,
    #[serde(default = "default_mode")]
    mode: WireMode,
    #[serde(default)]
    app: String,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    title: String,
}

fn default_mode() -> WireMode {
    WireMode::Prompt
}

async fn profiles(State(shared): State<Arc<Shared>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap) -> Response {
    if !authorized(&shared, &peer, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let list: Vec<_> = shared.service.profiles().all().iter().map(|p| serde_json::json!({ "id": p.id, "name": p.name })).collect();
    Json(list).into_response()
}

async fn transform(State(shared): State<Arc<Shared>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap, body: Option<Json<TransformBody>>) -> Response {
    if !authorized(&shared, &peer, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Some(Json(body)) = body else { return StatusCode::BAD_REQUEST.into_response() };
    if body.text.trim().is_empty() || body.text.chars().count() > MAX_TEXT_CHARS {
        return (StatusCode::PAYLOAD_TOO_LARGE, "text must be 1 to 8000 characters").into_response();
    }
    let service = shared.service.clone();
    let result = tokio::task::spawn_blocking(move || {
        let target = ClientContext { app: body.app, url: body.url, title: body.title }.to_active();
        let profile = service.profiles().resolve(&target);
        let mode = match body.mode {
            WireMode::Prompt => Mode::Prompt,
            WireMode::Dictation => Mode::Dictation,
        };
        let transform = Transform { input: Input::Text(&body.text), mode, profile, target: &target, surrounding: None, use_history: false, use_tools: false, auto_mode: false };
        let report = service.run_scheduled(Priority::Tool, LOCAL_API_CLIENT, &transform, &CancelToken::default(), Duration::from_secs(30), &mut |_| {});
        (profile.id.clone(), report)
    })
    .await;
    match result {
        Ok((profile, Ok(report))) => Json(serde_json::json!({ "profile": profile, "outcome": report.outcome, "structure": report.structure })).into_response(),
        Ok((_, Err(error))) => (StatusCode::SERVICE_UNAVAILABLE, error.to_string()).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

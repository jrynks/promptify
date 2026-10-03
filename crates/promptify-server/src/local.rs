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
use promptify_core::routing::{self, Rendering, RoutingOptions, Surface};
use promptify_core::scheduler::Priority;
use promptify_core::transform::{ClientContext, Input, Schedule, Transform};
use promptify_protocol::messages::{MAX_TEXT_CHARS, WireMode};
use promptify_protocol::noise::NOISE_MAX_MESSAGE;
use serde::Deserialize;
use tokio::sync::mpsc;

use crate::{MOBILE_NETWORKING_AVAILABLE, Shared};

pub const LOCAL_API_CLIENT: &str = "local-api";
/// Concurrent direct links, including ones still in the handshake.
pub const MAX_DIRECT_LINKS: usize = 16;

pub async fn serve(shared: Arc<Shared>, listener: tokio::net::TcpListener) {
    let mut stopping = shared.shutdown.subscribe();
    let app = Router::new()
        .route("/v1/health", get(|| async { Json(serde_json::json!({ "ok": true, "app": "promptify" })) }))
        .route("/v1/transform", post(transform))
        .route("/v1/profiles", get(profiles))
        .route("/v2/catalog", get(catalog))
        .route("/v2/transform", post(transform_v2));
    let app = if MOBILE_NETWORKING_AVAILABLE { app.route("/v1/direct", get(direct_upgrade)) } else { app };
    let app = app.layer(DefaultBodyLimit::max(64 * 1024)).with_state(shared);
    let shutdown = async move {
        let _ = stopping.changed().await;
    };
    let _ = axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).with_graceful_shutdown(shutdown).await;
}

async fn direct_upgrade(State(shared): State<Arc<Shared>>, headers: HeaderMap, ws: WebSocketUpgrade) -> Response {
    // Browsers always send Origin and do not apply CORS to WebSockets; phones and the CLI never send it.
    if headers.contains_key(axum::http::header::ORIGIN) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Ok(slot) = shared.direct_links.clone().try_acquire_owned() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let max = NOISE_MAX_MESSAGE + 1;
    ws.max_message_size(max).max_frame_size(max).on_upgrade(move |socket| async move {
        direct_session(shared, socket).await;
        drop(slot);
    })
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

fn adaptive_options() -> RoutingOptions {
    RoutingOptions { rendering: Rendering::Adaptive, ..Default::default() }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TransformV2Body {
    text: String,
    #[serde(default = "default_mode")]
    mode: WireMode,
    #[serde(default)]
    app: String,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    title: String,
    #[serde(default = "adaptive_options")]
    routing: RoutingOptions,
}

async fn catalog(State(shared): State<Arc<Shared>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap) -> Response {
    if !authorized(&shared, &peer, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    Json(serde_json::json!({
        "version": routing::CATALOG_VERSION,
        "rendering": ["legacy", "adaptive"],
        "required_structure": { "numbered_steps": true, "bounded_loop": true, "done_when": true },
        "forms": ["graph", "inline_graph"],
        "tasks": routing::catalog().all(),
        "surfaces": Surface::all(),
        "surface_detection": "app/site only; mixed-purpose fields require per-request confirmation",
    })).into_response()
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
    execute_transform(shared, body, RoutingOptions::default(), false).await
}

async fn transform_v2(
    State(shared): State<Arc<Shared>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Result<Json<TransformV2Body>, axum::extract::rejection::JsonRejection>,
) -> Response {
    if !authorized(&shared, &peer, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let body = match body {
        Ok(Json(body)) => body,
        Err(error) => return (StatusCode::BAD_REQUEST, error.body_text()).into_response(),
    };
    if let Err(error) = body.routing.validate() {
        return (StatusCode::BAD_REQUEST, error).into_response();
    }
    if matches!(body.mode, WireMode::Dictation) && (body.routing.task_type.is_some() || body.routing.surface.is_some()) {
        return (StatusCode::BAD_REQUEST, "Task and surface overrides require Prompt mode.").into_response();
    }
    let request = TransformBody { text: body.text, mode: body.mode, app: body.app, url: body.url, title: body.title };
    execute_transform(shared, request, body.routing, true).await
}

async fn execute_transform(shared: Arc<Shared>, body: TransformBody, routing: RoutingOptions, v2: bool) -> Response {
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
        let report = service.run_scheduled_with_options(
            Schedule { priority: Priority::Tool, client: LOCAL_API_CLIENT, wait: Duration::from_secs(30) },
            &transform, &routing, &CancelToken::default(), &mut |_| {},
        );
        (profile.id.clone(), report)
    })
    .await;
    match result {
        Ok((profile, Ok(report))) if v2 => {
            let validation = match &report.outcome {
                promptify_core::transform::TransformOutcome::Ready { .. } => serde_json::json!({ "status": report.structure, "issues": [] }),
                promptify_core::transform::TransformOutcome::Failed { reason, detail } => serde_json::json!({ "status": "failed", "reason": reason, "issues": [detail] }),
                promptify_core::transform::TransformOutcome::Truncated { .. } => serde_json::json!({ "status": "failed", "issues": ["The output is incomplete; do not insert it automatically."] }),
                _ => serde_json::json!({ "status": "not_generated", "issues": [] }),
            };
            Json(serde_json::json!({
                "version": routing::CATALOG_VERSION, "profile": profile,
                "outcome": report.outcome, "structure": report.structure, "routing": report.routing,
                "validation": validation,
            })).into_response()
        }
        Ok((profile, Ok(report))) => Json(serde_json::json!({ "profile": profile, "outcome": report.outcome, "structure": report.structure })).into_response(),
        Ok((_, Err(error))) => (StatusCode::SERVICE_UNAVAILABLE, error.to_string()).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

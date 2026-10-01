//! Desktop side of the relay: hosts the room and runs one session per relay channel.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use promptify_protocol::relay::{HostFrame, HostHello, MAX_HOST_FRAME};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

use crate::{RelayStatus, Shared};

pub const MAX_CHANNELS: usize = 8;
const MAX_BACKOFF: Duration = Duration::from_secs(60);

pub(crate) fn tls_connector() -> Option<tokio_tungstenite::Connector> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
        .with_safe_default_protocol_versions()
        .ok()?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Some(tokio_tungstenite::Connector::Rustls(Arc::new(config)))
}

pub(crate) fn ws_config(max: usize) -> WebSocketConfig {
    WebSocketConfig::default().max_message_size(Some(max)).max_frame_size(Some(max))
}

pub async fn run(shared: Arc<Shared>, relay_url: String) {
    let mut stopping = shared.shutdown.subscribe();
    let mut backoff = Duration::from_secs(1);
    loop {
        if *stopping.borrow() {
            return;
        }
        shared.set_relay_status(RelayStatus::Connecting);
        match connect_once(&shared, &relay_url).await {
            Ok(()) => backoff = Duration::from_secs(1),
            Err(message) => {
                log::warn!("relay link: {message}");
                shared.set_relay_status(RelayStatus::Error { message });
            }
        }
        tokio::select! {
            _ = tokio::time::sleep(backoff) => {}
            _ = stopping.changed() => return,
        }
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

async fn connect_once(shared: &Arc<Shared>, relay_url: &str) -> Result<(), String> {
    let url = format!("{}/v1/host", relay_url.trim_end_matches('/'));
    let (mut ws, _) = tokio::time::timeout(
        Duration::from_secs(15),
        tokio_tungstenite::connect_async_tls_with_config(url, Some(ws_config(MAX_HOST_FRAME)), false, tls_connector()),
    )
    .await
    .map_err(|_| "timed out connecting to the relay".to_string())?
    .map_err(|e| format!("cannot reach the relay: {e}"))?;
    let hello = HostHello { v: promptify_protocol::PROTOCOL_VERSION, host_secret: promptify_protocol::encode_key(&shared.identity.host_secret) };
    ws.send(Message::Text(serde_json::to_string(&hello).expect("serializable").into())).await.map_err(|e| e.to_string())?;
    match tokio::time::timeout(Duration::from_secs(10), ws.next()).await {
        Ok(Some(Ok(Message::Text(text)))) if text.contains(r#""ok":true"#) => {}
        _ => return Err("the relay did not accept this desktop".into()),
    }
    shared.set_relay_status(RelayStatus::Connected);
    log::info!("relay link connected");

    let mut stopping = shared.shutdown.subscribe();
    let (out_tx, mut out_rx) = mpsc::channel::<(u32, Option<Vec<u8>>)>(64);
    let mut channels: HashMap<u32, mpsc::Sender<Vec<u8>>> = HashMap::new();
    let mut keepalive = tokio::time::interval(Duration::from_secs(30));
    loop {
        tokio::select! {
            incoming = ws.next() => {
                let Some(Ok(message)) = incoming else { return Ok(()) };
                let Message::Binary(bytes) = message else {
                    if matches!(message, Message::Close(_)) { return Ok(()) }
                    continue;
                };
                match HostFrame::decode(&bytes) {
                    Ok(HostFrame::Open(channel)) => {
                        if channels.len() >= MAX_CHANNELS || channels.contains_key(&channel) {
                            let _ = ws.send(Message::Binary(HostFrame::Close(channel).encode().expect("empty").into())).await;
                            continue;
                        }
                        let (in_tx, in_rx) = mpsc::channel(32);
                        let (session_tx, mut session_rx) = mpsc::channel::<Vec<u8>>(32);
                        channels.insert(channel, in_tx);
                        tokio::spawn(crate::session::run_session(shared.clone(), in_rx, session_tx));
                        let out = out_tx.clone();
                        tokio::spawn(async move {
                            while let Some(frame) = session_rx.recv().await {
                                if out.send((channel, Some(frame))).await.is_err() {
                                    return;
                                }
                            }
                            let _ = out.send((channel, None)).await;
                        });
                    }
                    Ok(HostFrame::Data(channel, payload)) => {
                        if let Some(tx) = channels.get(&channel)
                            && tx.try_send(payload).is_err()
                        {
                            channels.remove(&channel);
                            let _ = ws.send(Message::Binary(HostFrame::Close(channel).encode().expect("empty").into())).await;
                        }
                    }
                    Ok(HostFrame::Close(channel)) => {
                        channels.remove(&channel);
                    }
                    Err(_) => return Err("the relay sent a malformed frame".into()),
                }
            }
            Some((channel, frame)) = out_rx.recv() => {
                let frame = match frame {
                    Some(payload) => HostFrame::Data(channel, payload),
                    None => {
                        channels.remove(&channel);
                        HostFrame::Close(channel)
                    }
                };
                let Ok(encoded) = frame.encode() else { continue };
                ws.send(Message::Binary(encoded.into())).await.map_err(|e| e.to_string())?;
            }
            _ = keepalive.tick() => {
                ws.send(Message::Ping(Vec::new().into())).await.map_err(|e| e.to_string())?;
            }
            _ = stopping.changed() => {
                let _ = ws.close(None).await;
                return Ok(());
            }
        }
    }
}

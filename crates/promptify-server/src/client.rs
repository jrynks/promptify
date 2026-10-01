//! Reference client: what a phone app does, usable from the CLI and in end-to-end tests.

use std::path::Path;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use promptify_protocol::messages::{ClientMessage, ServerMessage, WireContext, WireMode, encode};
use promptify_protocol::noise::{Handshake, NOISE_MAX_MESSAGE, SecureChannel, StaticKeys};
use promptify_protocol::pairing::PairingOffer;
use promptify_protocol::{decode_key, encode_key};
use serde::{Deserialize, Serialize};
use tokio_tungstenite::tungstenite::Message;

use crate::session::{KIND_PAIR, KIND_SESSION};

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

#[derive(Clone, Serialize, Deserialize)]
pub struct ClientIdentity {
    private_key: String,
    public_key: String,
    pub desktop_key: String,
    pub relay: Option<String>,
    pub direct: Option<String>,
    pub room: String,
    pub device_id: String,
}

impl ClientIdentity {
    pub fn load(path: &Path) -> Result<Self, String> {
        serde_json::from_str(&std::fs::read_to_string(path).map_err(|e| format!("cannot read {path:?}: {e}"))?).map_err(|e| e.to_string())
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        std::fs::write(path, serde_json::to_vec_pretty(self).expect("serializable")).map_err(|e| e.to_string())
    }

    fn keys(&self) -> Result<StaticKeys, String> {
        Ok(StaticKeys {
            private: decode_key(&self.private_key).ok_or("bad client key")?,
            public: decode_key(&self.public_key).ok_or("bad client key")?,
        })
    }

    #[cfg(test)]
    pub(crate) fn with_new_keys(&self) -> Self {
        let keys = StaticKeys::generate().unwrap();
        Self { private_key: encode_key(&keys.private), public_key: encode_key(&keys.public), ..self.clone() }
    }
}

async fn connect(relay: Option<&str>, direct: Option<&str>, room: &str, prefer_direct: bool) -> Result<Ws, String> {
    let url = match (relay, direct) {
        (_, Some(d)) if prefer_direct || relay.is_none() => format!("ws://{d}/v1/direct"),
        (Some(r), _) => format!("{}/v1/connect/{room}", r.trim_end_matches('/')),
        _ => return Err("no relay or direct address".into()),
    };
    let config = crate::relay_link::ws_config(NOISE_MAX_MESSAGE + 1);
    let (ws, _) = tokio::time::timeout(Duration::from_secs(15), tokio_tungstenite::connect_async_tls_with_config(url, Some(config), false, crate::relay_link::tls_connector()))
        .await
        .map_err(|_| "timed out connecting".to_string())?
        .map_err(|e| format!("cannot connect: {e}"))?;
    Ok(ws)
}

async fn recv_frame(ws: &mut Ws) -> Result<Vec<u8>, String> {
    loop {
        match tokio::time::timeout(Duration::from_secs(150), ws.next()).await {
            Ok(Some(Ok(Message::Binary(b)))) => return Ok(b.to_vec()),
            Ok(Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Text(_)))) => continue,
            Ok(_) => return Err("connection closed".into()),
            Err(_) => return Err("timed out waiting for the desktop".into()),
        }
    }
}

pub struct Connection {
    ws: Ws,
    channel: SecureChannel,
}

impl Connection {
    pub async fn send(&mut self, message: &ClientMessage) -> Result<(), String> {
        let frame = self.channel.seal(&encode(message)).map_err(|e| e.to_string())?;
        self.ws.send(Message::Binary(frame.into())).await.map_err(|e| e.to_string())
    }

    pub async fn recv(&mut self) -> Result<ServerMessage, String> {
        let frame = recv_frame(&mut self.ws).await?;
        let plain = self.channel.open(&frame).map_err(|e| e.to_string())?;
        serde_json::from_slice(&plain).map_err(|e| e.to_string())
    }

    /// Sends text for transformation and returns the terminal message, reporting progress events.
    pub async fn transform_text(&mut self, id: u64, mode: WireMode, context: WireContext, text: &str, on_event: &mut dyn FnMut(&ServerMessage)) -> Result<ServerMessage, String> {
        self.send(&ClientMessage::Transform { id, mode, context, text: Some(text.to_owned()) }).await?;
        loop {
            let message = self.recv().await?;
            match &message {
                ServerMessage::Done { .. } | ServerMessage::NoSpeech { .. } | ServerMessage::Cancelled { .. } | ServerMessage::Failed { .. } | ServerMessage::Error { .. } => return Ok(message),
                _ => on_event(&message),
            }
        }
    }
}

pub async fn pair(offer: &PairingOffer, device_name: &str, prefer_direct: bool) -> Result<(ClientIdentity, Connection), String> {
    let keys = StaticKeys::generate().map_err(|e| e.to_string())?;
    let mut ws = connect(offer.relay.as_deref(), offer.direct.as_deref(), &offer.room, prefer_direct).await?;
    let mut hs = Handshake::pairing_initiator(&keys, &offer.secret).map_err(|e| e.to_string())?;
    let mut first = vec![KIND_PAIR];
    first.extend(hs.write(b"").map_err(|e| e.to_string())?);
    ws.send(Message::Binary(first.into())).await.map_err(|e| e.to_string())?;
    hs.read(&recv_frame(&mut ws).await?).map_err(|e| e.to_string())?;
    // Pin the desktop from the QR code before revealing our key; anything else is an impostor.
    if hs.remote_static() != Some(offer.desktop_key) {
        return Err("the desktop's key does not match the pairing code".into());
    }
    ws.send(Message::Binary(hs.write(b"").map_err(|e| e.to_string())?.into())).await.map_err(|e| e.to_string())?;
    let channel = hs.into_channel().map_err(|e| e.to_string())?;
    let mut connection = Connection { ws, channel };
    connection.send(&ClientMessage::Hello { device_name: device_name.to_owned() }).await?;
    let ServerMessage::Paired { device_id } = connection.recv().await? else {
        return Err("pairing was not accepted".into());
    };
    let identity = ClientIdentity {
        private_key: encode_key(&keys.private),
        public_key: encode_key(&keys.public),
        desktop_key: encode_key(&offer.desktop_key),
        relay: offer.relay.clone(),
        direct: offer.direct.clone(),
        room: offer.room.clone(),
        device_id,
    };
    Ok((identity, connection))
}

pub async fn open(identity: &ClientIdentity, prefer_direct: bool) -> Result<Connection, String> {
    let keys = identity.keys()?;
    let desktop = decode_key(&identity.desktop_key).ok_or("bad desktop key")?;
    let mut ws = connect(identity.relay.as_deref(), identity.direct.as_deref(), &identity.room, prefer_direct).await?;
    let mut hs = Handshake::session_initiator(&keys, &desktop).map_err(|e| e.to_string())?;
    let mut first = vec![KIND_SESSION];
    first.extend(hs.write(b"").map_err(|e| e.to_string())?);
    ws.send(Message::Binary(first.into())).await.map_err(|e| e.to_string())?;
    let reply = recv_frame(&mut ws).await.map_err(|_| "the desktop did not accept this device (it may have been removed)".to_string())?;
    hs.read(&reply).map_err(|e| e.to_string())?;
    let channel = hs.into_channel().map_err(|e| e.to_string())?;
    let mut connection = Connection { ws, channel };
    match connection.recv().await? {
        ServerMessage::Ready { .. } => Ok(connection),
        _ => Err("unexpected reply from the desktop".into()),
    }
}

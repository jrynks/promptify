use std::net::SocketAddr;

use futures_util::{SinkExt, StreamExt};
use promptify_protocol::noise::{Handshake, StaticKeys, random_secret};
use promptify_protocol::relay::{HostFrame, HostHello};
use tokio_tungstenite::tungstenite::Message as WsMessage;

use super::*;

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn start(limits: Limits, observer: Option<Observer>) -> (SocketAddr, Arc<Relay>) {
    let relay = Relay::with_observer(limits, observer);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = router(relay.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (addr, relay)
}

async fn host(addr: SocketAddr, secret: &[u8; 32]) -> Ws {
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/v1/host")).await.unwrap();
    let hello = HostHello { v: 1, host_secret: promptify_protocol::encode_key(secret) };
    ws.send(WsMessage::Text(serde_json::to_string(&hello).unwrap().into())).await.unwrap();
    match ws.next().await.unwrap().unwrap() {
        WsMessage::Text(t) => assert!(t.contains(&room_id(secret))),
        other => panic!("unexpected {other:?}"),
    }
    ws
}

async fn client(addr: SocketAddr, room: &str) -> Result<Ws, tokio_tungstenite::tungstenite::Error> {
    tokio_tungstenite::connect_async(format!("ws://{addr}/v1/connect/{room}")).await.map(|(ws, _)| ws)
}

async fn next_frame(ws: &mut Ws) -> HostFrame {
    loop {
        match tokio::time::timeout(Duration::from_secs(5), ws.next()).await.expect("frame in time").unwrap().unwrap() {
            WsMessage::Binary(b) => return HostFrame::decode(&b).unwrap(),
            WsMessage::Ping(_) | WsMessage::Pong(_) => continue,
            other => panic!("unexpected {other:?}"),
        }
    }
}

async fn next_binary(ws: &mut Ws) -> Option<Vec<u8>> {
    loop {
        match tokio::time::timeout(Duration::from_secs(5), ws.next()).await.ok()?? {
            Ok(WsMessage::Binary(b)) => return Some(b.to_vec()),
            Ok(WsMessage::Ping(_) | WsMessage::Pong(_)) => continue,
            _ => return None,
        }
    }
}

async fn closed(ws: &mut Ws) -> bool {
    loop {
        match tokio::time::timeout(Duration::from_secs(5), ws.next()).await {
            Err(_) => return false,
            Ok(None | Some(Err(_)) | Some(Ok(WsMessage::Close(_)))) => return true,
            Ok(Some(Ok(_))) => continue,
        }
    }
}

#[tokio::test]
async fn forwards_between_host_and_client_channels() {
    let (addr, _) = start(Limits::default(), None).await;
    let secret = [3; 32];
    let mut h = host(addr, &secret).await;
    let mut c = client(addr, &room_id(&secret)).await.unwrap();
    let HostFrame::Open(channel) = next_frame(&mut h).await else { panic!("expected open") };
    c.send(WsMessage::Binary(b"to host".to_vec().into())).await.unwrap();
    assert_eq!(next_frame(&mut h).await, HostFrame::Data(channel, b"to host".to_vec()));
    h.send(WsMessage::Binary(HostFrame::Data(channel, b"to phone".to_vec()).encode().unwrap().into())).await.unwrap();
    assert_eq!(next_binary(&mut c).await.unwrap(), b"to phone");
    h.send(WsMessage::Binary(HostFrame::Close(channel).encode().unwrap().into())).await.unwrap();
    assert!(closed(&mut c).await);
}

#[tokio::test]
async fn unknown_or_malformed_rooms_are_refused_before_upgrade() {
    let (addr, _) = start(Limits::default(), None).await;
    assert!(client(addr, &room_id(&[1; 32])).await.is_err());
    assert!(client(addr, "not-a-room").await.is_err());
}

#[tokio::test]
async fn bad_host_hello_does_not_create_a_room() {
    let (addr, relay) = start(Limits::default(), None).await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/v1/host")).await.unwrap();
    ws.send(WsMessage::Text(r#"{"v":1,"host_secret":"short"}"#.into())).await.unwrap();
    assert!(closed(&mut ws).await);
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/v1/host")).await.unwrap();
    ws.send(WsMessage::Binary(vec![1, 2, 3].into())).await.unwrap();
    assert!(closed(&mut ws).await);
    assert_eq!(relay.room_count(), 0);
}

#[tokio::test]
async fn reconnecting_host_replaces_the_old_link_and_its_channels() {
    let (addr, _) = start(Limits::default(), None).await;
    let secret = [4; 32];
    let mut old = host(addr, &secret).await;
    let mut c = client(addr, &room_id(&secret)).await.unwrap();
    let _ = next_frame(&mut old).await;
    let mut new = host(addr, &secret).await;
    assert!(closed(&mut old).await, "old host still connected");
    assert!(closed(&mut c).await, "old channel survived host replacement");
    let mut c2 = client(addr, &room_id(&secret)).await.unwrap();
    assert!(matches!(next_frame(&mut new).await, HostFrame::Open(_)));
    c2.send(WsMessage::Binary(b"x".to_vec().into())).await.unwrap();
    assert!(matches!(next_frame(&mut new).await, HostFrame::Data(_, _)));
}

#[tokio::test]
async fn channel_cap_and_rate_limit_disconnect_abusers() {
    let limits = Limits { max_channels_per_room: 1, bytes_per_sec: 1000, burst_bytes: 4000, ..Limits::default() };
    let (addr, _) = start(limits, None).await;
    let secret = [5; 32];
    let mut h = host(addr, &secret).await;
    let mut first = client(addr, &room_id(&secret)).await.unwrap();
    let _ = next_frame(&mut h).await;
    let mut second = client(addr, &room_id(&secret)).await.unwrap();
    assert!(closed(&mut second).await, "second channel admitted past the cap");
    for _ in 0..3 {
        let _ = first.send(WsMessage::Binary(vec![0; 3000].into())).await;
    }
    assert!(closed(&mut first).await, "flooding client not disconnected");
}

#[tokio::test]
async fn relay_only_ever_sees_ciphertext() {
    let seen = Arc::new(Mutex::new(Vec::<u8>::new()));
    let sink = seen.clone();
    let observer: Observer = Arc::new(move |bytes: &[u8]| sink.lock().unwrap().extend_from_slice(bytes));
    let (addr, _) = start(Limits::default(), Some(observer)).await;
    let host_secret = random_secret().unwrap();
    let (desktop, phone) = (StaticKeys::generate().unwrap(), StaticKeys::generate().unwrap());
    let mut h = host(addr, &host_secret).await;
    let mut c = client(addr, &room_id(&host_secret)).await.unwrap();
    let HostFrame::Open(channel) = next_frame(&mut h).await else { panic!() };

    let mut p = Handshake::session_initiator(&phone, &desktop.public).unwrap();
    let mut d = Handshake::session_responder(&desktop).unwrap();
    c.send(WsMessage::Binary(p.write(b"").unwrap().into())).await.unwrap();
    let HostFrame::Data(_, m1) = next_frame(&mut h).await else { panic!() };
    d.read(&m1).unwrap();
    h.send(WsMessage::Binary(HostFrame::Data(channel, d.write(b"").unwrap()).encode().unwrap().into())).await.unwrap();
    p.read(&next_binary(&mut c).await.unwrap()).unwrap();
    let (mut pc, mut dc) = (p.into_channel().unwrap(), d.into_channel().unwrap());

    let sentinel = b"PLAINTEXT-SENTINEL-transcript";
    c.send(WsMessage::Binary(pc.seal(sentinel).unwrap().into())).await.unwrap();
    let HostFrame::Data(_, sealed) = next_frame(&mut h).await else { panic!() };
    assert_eq!(dc.open(&sealed).unwrap(), sentinel);
    h.send(WsMessage::Binary(HostFrame::Data(channel, dc.seal(sentinel).unwrap()).encode().unwrap().into())).await.unwrap();
    assert_eq!(pc.open(&next_binary(&mut c).await.unwrap()).unwrap(), sentinel);

    let seen = seen.lock().unwrap();
    assert!(seen.len() > 2 * sentinel.len(), "observer saw no traffic");
    assert!(!seen.windows(sentinel.len()).any(|w| w == sentinel), "relay saw plaintext");
}

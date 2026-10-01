//! Promptify desktop server. Paired phones reach the desktop's engines through a blind relay or a
//! direct link; every byte between them is end-to-end encrypted. Also serves the loopback-only API.

pub mod client;
mod local;
mod relay_link;
pub mod session;
pub mod store;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use promptify_core::transform::TransformService;
use promptify_protocol::noise::random_secret;
use promptify_protocol::pairing::{MAX_OFFER_LIFETIME_SECS, PairingOffer};
use serde::Serialize;
use tokio::sync::watch;

pub use store::{Device, DeviceRegistry, Identity, now_unix};

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub data_dir: PathBuf,
    /// Self-hosted relay base URL, e.g. wss://relay.example.net. None disables internet access.
    pub relay_url: Option<String>,
    /// Address for the direct link and local API. Use a loopback address unless LAN pairing is wanted.
    pub listen: Option<SocketAddr>,
    /// host:port phones should dial for a direct link, advertised in pairing offers.
    pub advertise_direct: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RelayStatus {
    Disabled,
    Connecting,
    Connected,
    Error { message: String },
}

#[derive(Debug, Clone, Serialize)]
pub struct ServerStatus {
    pub relay: RelayStatus,
    pub relay_url: Option<String>,
    pub listen: Option<String>,
    pub sessions: usize,
    pub devices: usize,
    pub pairing_expires_unix: Option<u64>,
}

struct PendingOffer {
    secret: [u8; 32],
    expires_unix: u64,
    failures: u32,
}

/// At most one pairing offer exists; it expires, is single use, and dies after repeated failures.
#[derive(Default)]
pub struct PairingState(Mutex<Option<PendingOffer>>);

impl PairingState {
    fn lock(&self) -> std::sync::MutexGuard<'_, Option<PendingOffer>> {
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn create(&self, lifetime_secs: u64) -> Result<([u8; 32], u64), String> {
        let secret = random_secret().map_err(|e| e.to_string())?;
        let expires_unix = now_unix() + lifetime_secs.clamp(30, MAX_OFFER_LIFETIME_SECS);
        *self.lock() = Some(PendingOffer { secret, expires_unix, failures: 0 });
        Ok((secret, expires_unix))
    }

    pub fn active_secret(&self) -> Option<[u8; 32]> {
        let mut pending = self.lock();
        if pending.as_ref().is_some_and(|p| now_unix() >= p.expires_unix) {
            *pending = None;
        }
        pending.as_ref().map(|p| p.secret)
    }

    pub fn record_failure(&self, secret: &[u8; 32], max: u32) {
        let mut pending = self.lock();
        if let Some(p) = pending.as_mut().filter(|p| &p.secret == secret) {
            p.failures += 1;
            if p.failures >= max {
                *pending = None;
            }
        }
    }

    pub fn consume(&self, secret: &[u8; 32]) -> bool {
        let mut pending = self.lock();
        let valid = pending.as_ref().is_some_and(|p| &p.secret == secret && now_unix() < p.expires_unix);
        if valid {
            *pending = None;
        }
        valid
    }

    pub fn cancel(&self) {
        *self.lock() = None;
    }

    fn expires(&self) -> Option<u64> {
        self.lock().as_ref().map(|p| p.expires_unix)
    }
}

pub struct Shared {
    pub service: Arc<TransformService>,
    pub identity: Identity,
    pub devices: DeviceRegistry,
    pub pairing: PairingState,
    pub sessions: AtomicUsize,
    pub shutdown: watch::Sender<bool>,
    pub api_token: String,
    relay_status: Mutex<RelayStatus>,
    config: ServerConfig,
}

impl Shared {
    fn set_relay_status(&self, status: RelayStatus) {
        *self.relay_status.lock().unwrap_or_else(|p| p.into_inner()) = status;
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct OfferInfo {
    pub uri: String,
    pub svg: String,
    pub expires_unix: u64,
}

pub struct RemoteServer {
    shared: Arc<Shared>,
    runtime: Option<tokio::runtime::Runtime>,
    listen: Option<SocketAddr>,
}

fn load_api_token(path: &std::path::Path) -> Result<String, String> {
    if let Ok(token) = std::fs::read_to_string(path)
        && token.trim().len() >= 32
    {
        return Ok(token.trim().to_owned());
    }
    let token = promptify_protocol::encode_key(&random_secret().map_err(|e| e.to_string())?);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, &token).map_err(|e| format!("cannot save API token: {e}"))?;
    Ok(token)
}

pub fn api_token_path(data_dir: &std::path::Path) -> PathBuf {
    data_dir.join("remote").join("api-token")
}

impl RemoteServer {
    pub fn start(config: ServerConfig, service: Arc<TransformService>) -> Result<Self, String> {
        let remote_dir = config.data_dir.join("remote");
        let identity = Identity::load_or_create(&remote_dir.join("identity.json"))?;
        let devices = DeviceRegistry::open(remote_dir.join("devices.json"))?;
        let api_token = load_api_token(&api_token_path(&config.data_dir))?;
        if let Some(relay) = &config.relay_url {
            validate_relay_url(relay)?;
        }
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("promptify-server")
            .enable_all()
            .build()
            .map_err(|e| format!("cannot start server runtime: {e}"))?;
        let shared = Arc::new(Shared {
            service,
            identity,
            devices,
            pairing: PairingState::default(),
            sessions: AtomicUsize::new(0),
            shutdown: watch::channel(false).0,
            api_token,
            relay_status: Mutex::new(if config.relay_url.is_some() { RelayStatus::Connecting } else { RelayStatus::Disabled }),
            config: config.clone(),
        });
        let listen = match config.listen {
            Some(addr) => {
                let listener = runtime.block_on(tokio::net::TcpListener::bind(addr)).map_err(|e| format!("cannot listen on {addr}: {e}"))?;
                let bound = listener.local_addr().map_err(|e| e.to_string())?;
                runtime.spawn(local::serve(shared.clone(), listener));
                Some(bound)
            }
            None => None,
        };
        if let Some(relay) = config.relay_url.clone() {
            runtime.spawn(relay_link::run(shared.clone(), relay));
        }
        Ok(Self { shared, runtime: Some(runtime), listen })
    }

    pub fn listen_addr(&self) -> Option<SocketAddr> {
        self.listen
    }

    pub fn desktop_key(&self) -> [u8; 32] {
        self.shared.identity.keys.public
    }

    pub fn status(&self) -> ServerStatus {
        ServerStatus {
            relay: self.shared.relay_status.lock().unwrap_or_else(|p| p.into_inner()).clone(),
            relay_url: self.shared.config.relay_url.clone(),
            listen: self.listen.map(|a| a.to_string()),
            sessions: self.shared.sessions.load(std::sync::atomic::Ordering::SeqCst),
            devices: self.shared.devices.list().len(),
            pairing_expires_unix: self.shared.pairing.expires(),
        }
    }

    /// Creates a fresh single-use offer, replacing any previous one.
    pub fn pairing_offer(&self, lifetime_secs: u64) -> Result<OfferInfo, String> {
        let direct = self.shared.config.advertise_direct.clone();
        if self.shared.config.relay_url.is_none() && direct.is_none() {
            return Err("set a relay URL or enable direct connections before pairing".into());
        }
        let (secret, expires_unix) = self.shared.pairing.create(lifetime_secs)?;
        let offer = PairingOffer {
            relay: self.shared.config.relay_url.clone(),
            direct,
            room: self.shared.identity.room(),
            desktop_key: self.shared.identity.keys.public,
            secret,
            expires_unix,
        };
        let uri = offer.to_uri();
        let svg = qrcode::QrCode::new(uri.as_bytes())
            .map_err(|e| format!("cannot draw QR code: {e}"))?
            .render::<qrcode::render::svg::Color<'_>>()
            .min_dimensions(240, 240)
            .build();
        Ok(OfferInfo { uri, svg, expires_unix })
    }

    pub fn cancel_pairing(&self) {
        self.shared.pairing.cancel();
    }

    pub fn devices(&self) -> Vec<Device> {
        self.shared.devices.list()
    }

    pub fn remove_device(&self, id: &str) -> Result<bool, String> {
        self.shared.devices.remove(id)
    }

    pub fn rename_device(&self, id: &str, name: &str) -> Result<bool, String> {
        self.shared.devices.rename(id, name)
    }

    pub fn shutdown(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        let _ = self.shared.shutdown.send(true);
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_timeout(Duration::from_secs(2));
        }
    }
}

impl Drop for RemoteServer {
    fn drop(&mut self) {
        let _ = self.shared.shutdown.send(true);
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

pub fn validate_relay_url(raw: &str) -> Result<(), String> {
    let url = url::Url::parse(raw).map_err(|_| "relay URL is not a valid URL".to_string())?;
    match url.scheme() {
        "wss" => Ok(()),
        // Plain ws exposes only the room-hosting secret, never content; allow it for local testing.
        "ws" if url.host_str().is_some_and(|h| h == "localhost" || h.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback() || is_private(&ip))) => Ok(()),
        "ws" => Err("use wss:// for a relay on the internet".into()),
        _ => Err("relay URL must start with wss://".into()),
    }
}

fn is_private(ip: &std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => v4.is_private() || v4.is_link_local(),
        std::net::IpAddr::V6(v6) => v6.is_unique_local(),
    }
}

#[cfg(test)]
mod tests;

//! Desktop MCP API, with phone access available only in opt-in development builds.

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::Mutex;

use promptify_server::{Device, MOBILE_NETWORKING_AVAILABLE, OfferInfo, RemoteServer, ServerConfig, ServerStatus, require_mobile_networking};
use serde::Serialize;
use tauri::State;

use crate::AppState;
use crate::settings::AppSettings;

pub const DEFAULT_PORT: u16 = 47821;
const OFFER_LIFETIME_SECS: u64 = 300;

#[derive(Default)]
pub struct RemoteState {
    pub server: Mutex<Option<RemoteServer>>,
    pub error: Mutex<Option<String>>,
}

/// The address other devices on this network would use to reach us; no packet is sent.
fn lan_ip() -> Option<Ipv4Addr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    match socket.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_unspecified() => Some(ip),
        _ => None,
    }
}

pub fn config_for(settings: &AppSettings, data_dir: &std::path::Path) -> ServerConfig {
    let lan = MOBILE_NETWORKING_AVAILABLE && settings.lan_direct;
    let listen: SocketAddr = if lan { (Ipv4Addr::UNSPECIFIED, DEFAULT_PORT).into() } else { (Ipv4Addr::LOCALHOST, DEFAULT_PORT).into() };
    ServerConfig {
        data_dir: data_dir.to_path_buf(),
        relay_url: if MOBILE_NETWORKING_AVAILABLE { settings.relay_url.clone().filter(|u| !u.trim().is_empty()) } else { None },
        listen: Some(listen),
        advertise_direct: if lan { lan_ip().map(|ip| format!("{ip}:{DEFAULT_PORT}")) } else { None },
        discoverable: lan && settings.lan_discovery,
    }
}

/// Stops any running server, then starts one if the settings ask for it. Errors are kept for the UI.
pub fn apply(state: &AppState) {
    let settings = state.settings.read().unwrap().clone();
    let mut server = state.remote.server.lock().unwrap();
    if let Some(old) = server.take() {
        old.shutdown();
    }
    let mut error = state.remote.error.lock().unwrap();
    *error = None;
    if !settings.server_enabled {
        return;
    }
    match RemoteServer::start(config_for(&settings, &state.data_dir), state.orchestrator.service().clone()) {
        Ok(started) => {
            log::info!("desktop API on; phone networking available={MOBILE_NETWORKING_AVAILABLE}; relay={}", started.status().relay_url.is_some());
            *server = Some(started);
        }
        Err(e) => {
            log::warn!("desktop API failed to start: {e}");
            *error = Some(e);
        }
    }
}

#[derive(Serialize)]
pub struct RemoteInfo {
    mobile_available: bool,
    enabled: bool,
    relay_url: Option<String>,
    lan_direct: bool,
    lan_discovery: bool,
    status: Option<ServerStatus>,
    error: Option<String>,
    devices: Vec<Device>,
    api_token_path: String,
}

#[tauri::command]
pub fn remote_info(state: State<'_, AppState>) -> RemoteInfo {
    let settings = state.settings.read().unwrap().clone();
    let server = state.remote.server.lock().unwrap();
    RemoteInfo {
        mobile_available: MOBILE_NETWORKING_AVAILABLE,
        enabled: settings.server_enabled,
        relay_url: if MOBILE_NETWORKING_AVAILABLE { settings.relay_url } else { None },
        lan_direct: MOBILE_NETWORKING_AVAILABLE && settings.lan_direct,
        lan_discovery: MOBILE_NETWORKING_AVAILABLE && settings.lan_discovery,
        status: server.as_ref().map(|s| s.status()),
        error: state.remote.error.lock().unwrap().clone(),
        devices: if MOBILE_NETWORKING_AVAILABLE { server.as_ref().map(|s| s.devices()).unwrap_or_default() } else { Vec::new() },
        api_token_path: promptify_server::api_token_path(&state.data_dir).to_string_lossy().into_owned(),
    }
}

fn update_settings(settings: &mut AppSettings, enabled: bool, relay_url: Option<String>, lan_direct: bool, lan_discovery: bool) -> Result<(), String> {
    let relay_url = relay_url.map(|u| u.trim().to_owned()).filter(|u| !u.is_empty());
    if relay_url.is_some() || lan_direct || lan_discovery {
        require_mobile_networking()?;
    }
    if let Some(url) = &relay_url {
        promptify_server::validate_relay_url(url)?;
    }
    settings.server_enabled = enabled;
    if MOBILE_NETWORKING_AVAILABLE {
        settings.relay_url = relay_url;
        settings.lan_direct = lan_direct;
        settings.lan_discovery = lan_discovery;
    }
    Ok(())
}

#[tauri::command]
pub fn set_remote_settings(state: State<'_, AppState>, enabled: bool, relay_url: Option<String>, lan_direct: bool, lan_discovery: bool) -> Result<(), String> {
    {
        let mut settings = state.settings.write().unwrap();
        update_settings(&mut settings, enabled, relay_url, lan_direct, lan_discovery)?;
    }
    let snapshot = state.settings.read().unwrap().clone();
    crate::settings::save(&state.data_dir, &snapshot).map_err(|e| format!("could not save settings: {e}"))?;
    apply(&state);
    match state.remote.error.lock().unwrap().clone() {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

fn with_server<T>(state: &AppState, f: impl FnOnce(&RemoteServer) -> Result<T, String>) -> Result<T, String> {
    require_mobile_networking()?;
    let server = state.remote.server.lock().unwrap();
    f(server.as_ref().ok_or("turn on the desktop API first")?)
}

#[tauri::command]
pub fn create_pairing_offer(state: State<'_, AppState>) -> Result<OfferInfo, String> {
    with_server(&state, |s| s.pairing_offer(OFFER_LIFETIME_SECS))
}

#[tauri::command]
pub fn cancel_pairing(state: State<'_, AppState>) -> Result<(), String> {
    with_server(&state, |s| {
        s.cancel_pairing();
        Ok(())
    })
}

#[tauri::command]
pub fn remove_device(state: State<'_, AppState>, id: String) -> Result<bool, String> {
    with_server(&state, |s| s.remove_device(&id))
}

#[tauri::command]
pub fn rename_device(state: State<'_, AppState>, id: String, name: String) -> Result<bool, String> {
    with_server(&state, |s| s.rename_device(&id, &name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_api_config_stays_on_loopback() {
        let config = config_for(&AppSettings::default(), std::path::Path::new("unused"));
        assert!(config.listen.unwrap().ip().is_loopback());
        assert!(config.relay_url.is_none() && config.advertise_direct.is_none());
        assert!(!config.discoverable);
        config.validate().unwrap();
    }

    #[cfg(not(feature = "mobile-networking"))]
    #[test]
    fn saved_phone_settings_cannot_reactivate_networking() {
        let mut settings = AppSettings {
            relay_url: Some("wss://relay.example.test".into()),
            lan_direct: true,
            lan_discovery: true,
            ..AppSettings::default()
        };
        let mut expected = settings.clone();
        expected.server_enabled = true;
        update_settings(&mut settings, true, None, false, false).unwrap();
        assert_eq!(settings, expected, "desktop MCP toggles must preserve inactive phone preferences");
        let config = config_for(&settings, std::path::Path::new("unused"));
        assert!(config.listen.unwrap().ip().is_loopback());
        assert!(config.relay_url.is_none() && config.advertise_direct.is_none());
        assert!(!config.discoverable);
        config.validate().unwrap();
    }

    #[cfg(not(feature = "mobile-networking"))]
    #[test]
    fn phone_options_are_rejected_without_changing_settings() {
        for (relay, lan, discovery) in [(Some("wss://relay.example.test".into()), false, false), (None, true, false), (None, false, true)] {
            let original = AppSettings::default();
            let mut settings = original.clone();
            assert_eq!(update_settings(&mut settings, true, relay, lan, discovery).unwrap_err(), promptify_server::MOBILE_NETWORKING_DISABLED);
            assert_eq!(settings, original);
        }
    }
}

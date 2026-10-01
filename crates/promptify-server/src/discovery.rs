//! Optional LAN discovery over mDNS, so a paired phone can find its desktop after the desktop's
//! address changes. Off unless the owner turns it on.
//!
//! Only an identifier derived from the room is advertised, under a synthetic host name. Without
//! the pairing data, other devices on the network learn neither the room nor this computer's name.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use sha2::{Digest, Sha256};

pub const SERVICE_TYPE: &str = "_promptify._tcp.local.";

/// The advertised instance name for a room: 16 hex characters that only paired devices can predict.
pub fn instance_id(room: &str) -> String {
    let digest = Sha256::new().chain_update(b"promptify-mdns/1").chain_update(room.as_bytes()).finalize();
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// Advertises this desktop's direct port until dropped.
pub struct Advertiser {
    daemon: ServiceDaemon,
    fullname: String,
}

impl Advertiser {
    pub fn start(room: &str, port: u16) -> Result<Self, String> {
        let id = instance_id(room);
        let daemon = ServiceDaemon::new().map_err(|e| format!("mDNS unavailable: {e}"))?;
        let info = ServiceInfo::new(SERVICE_TYPE, &id, &format!("promptify-{id}.local."), "", port, [("v", "1")].as_slice())
            .map_err(|e| e.to_string())?
            .enable_addr_auto();
        let fullname = info.get_fullname().to_owned();
        daemon.register(info).map_err(|e| e.to_string())?;
        Ok(Self { daemon, fullname })
    }
}

impl Drop for Advertiser {
    fn drop(&mut self) {
        let _ = self.daemon.unregister(&self.fullname);
        let _ = self.daemon.shutdown();
    }
}

/// Looks for the desktop that owns `room` on the local network.
pub fn find(room: &str, timeout: Duration) -> Option<SocketAddr> {
    let wanted = format!("{}.{SERVICE_TYPE}", instance_id(room));
    let daemon = ServiceDaemon::new().ok()?;
    let events = daemon.browse(SERVICE_TYPE).ok()?;
    let deadline = Instant::now() + timeout;
    let mut found = None;
    while found.is_none() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match events.recv_timeout(remaining) {
            Ok(ServiceEvent::ServiceResolved(info)) if info.get_fullname() == wanted => {
                let mut addresses: Vec<_> = info.get_addresses_v4().into_iter().copied().collect();
                addresses.sort();
                found = addresses.first().map(|ip| SocketAddr::from((*ip, info.get_port())));
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    let _ = daemon.shutdown();
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_ids_are_stable_short_and_reveal_nothing() {
        let room = "a".repeat(64);
        let id = instance_id(&room);
        assert_eq!(id, instance_id(&room));
        assert_eq!(id.len(), 16);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(!room.contains(&id));
        assert_ne!(id, instance_id(&"b".repeat(64)));
    }

    #[test]
    fn only_network_reachable_listeners_are_advertised() {
        use crate::should_advertise;
        assert!(should_advertise(true, "0.0.0.0:47821".parse().unwrap()));
        assert!(should_advertise(true, "192.168.1.20:47821".parse().unwrap()));
        assert!(!should_advertise(true, "127.0.0.1:47821".parse().unwrap()));
        assert!(!should_advertise(true, "[::1]:47821".parse().unwrap()));
        assert!(!should_advertise(false, "0.0.0.0:47821".parse().unwrap()));
    }

    /// Uses real multicast on this machine's interfaces.
    #[test]
    fn advertised_desktop_is_found_by_its_room_only() {
        let room = format!("test-room-{}", std::process::id());
        let _advertiser = Advertiser::start(&room, 47_999).expect("advertise");
        let found = find(&room, Duration::from_secs(8)).expect("found on the local network");
        assert_eq!(found.port(), 47_999);
        assert_eq!(find("some-other-room", Duration::from_secs(2)), None);
    }
}

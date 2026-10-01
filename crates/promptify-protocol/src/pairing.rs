//! Pairing offers carried in the QR code, and relay room derivation.

use sha2::{Digest, Sha256};

use crate::{decode_key, encode_key};

pub const OFFER_SCHEME: &str = "promptify";
pub const MAX_OFFER_LIFETIME_SECS: u64 = 600;

/// Everything a phone needs to pair: where to connect, which desktop key to pin, and the one-time secret.
#[derive(Clone, PartialEq, Eq)]
pub struct PairingOffer {
    /// Self-hosted relay base URL (wss:// or ws:// for local testing); None for direct LAN only.
    pub relay: Option<String>,
    /// Direct address on the local network, host:port.
    pub direct: Option<String>,
    pub room: String,
    pub desktop_key: [u8; 32],
    pub secret: [u8; 32],
    pub expires_unix: u64,
}

impl std::fmt::Debug for PairingOffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PairingOffer")
            .field("relay", &self.relay)
            .field("direct", &self.direct)
            .field("room", &self.room)
            .field("expires_unix", &self.expires_unix)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OfferError {
    #[error("not a Promptify pairing link")]
    NotAnOffer,
    #[error("unsupported pairing version")]
    Version,
    #[error("pairing link is missing {0}")]
    Missing(&'static str),
    #[error("pairing link has an invalid {0}")]
    Invalid(&'static str),
    #[error("pairing link has expired")]
    Expired,
}

impl PairingOffer {
    pub fn to_uri(&self) -> String {
        let mut url = url::Url::parse(&format!("{OFFER_SCHEME}://pair")).expect("static base URL");
        {
            let mut q = url.query_pairs_mut();
            q.append_pair("v", &crate::PROTOCOL_VERSION.to_string());
            if let Some(relay) = &self.relay {
                q.append_pair("relay", relay);
            }
            if let Some(direct) = &self.direct {
                q.append_pair("direct", direct);
            }
            q.append_pair("room", &self.room);
            q.append_pair("pk", &encode_key(&self.desktop_key));
            q.append_pair("s", &encode_key(&self.secret));
            q.append_pair("exp", &self.expires_unix.to_string());
        }
        url.to_string()
    }

    pub fn parse(uri: &str, now_unix: u64) -> Result<Self, OfferError> {
        let url = url::Url::parse(uri.trim()).map_err(|_| OfferError::NotAnOffer)?;
        if url.scheme() != OFFER_SCHEME || url.host_str() != Some("pair") {
            return Err(OfferError::NotAnOffer);
        }
        let get = |key: &str| url.query_pairs().find(|(k, _)| k == key).map(|(_, v)| v.into_owned());
        if get("v").as_deref() != Some("1") {
            return Err(OfferError::Version);
        }
        let relay = get("relay");
        if let Some(r) = &relay {
            let parsed = url::Url::parse(r).map_err(|_| OfferError::Invalid("relay"))?;
            if !matches!(parsed.scheme(), "wss" | "ws") {
                return Err(OfferError::Invalid("relay"));
            }
        }
        let direct = get("direct");
        if relay.is_none() && direct.is_none() {
            return Err(OfferError::Missing("relay or direct address"));
        }
        let room = get("room").ok_or(OfferError::Missing("room"))?;
        if !is_room_id(&room) {
            return Err(OfferError::Invalid("room"));
        }
        let desktop_key = decode_key(&get("pk").ok_or(OfferError::Missing("pk"))?).ok_or(OfferError::Invalid("pk"))?;
        let secret = decode_key(&get("s").ok_or(OfferError::Missing("s"))?).ok_or(OfferError::Invalid("s"))?;
        let expires_unix: u64 = get("exp").ok_or(OfferError::Missing("exp"))?.parse().map_err(|_| OfferError::Invalid("exp"))?;
        if now_unix >= expires_unix || expires_unix > now_unix + MAX_OFFER_LIFETIME_SECS {
            return Err(OfferError::Expired);
        }
        Ok(Self { relay, direct, room, desktop_key, secret, expires_unix })
    }
}

/// Relay room for a desktop. Only the desktop knows `host_secret`, so only it can host the room;
/// phones know just the derived ID.
pub fn room_id(host_secret: &[u8; 32]) -> String {
    let digest = Sha256::new().chain_update(b"promptify-room/1").chain_update(host_secret).finalize();
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn is_room_id(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer() -> PairingOffer {
        PairingOffer {
            relay: Some("wss://relay.example.net".into()),
            direct: Some("192.168.1.20:47821".into()),
            room: room_id(&[7; 32]),
            desktop_key: [1; 32],
            secret: [2; 32],
            expires_unix: 1_000_300,
        }
    }

    #[test]
    fn offer_round_trips_and_hides_secret_from_debug() {
        let uri = offer().to_uri();
        assert!(uri.starts_with("promptify://pair?v=1&"));
        assert_eq!(PairingOffer::parse(&uri, 1_000_000).unwrap(), offer());
        assert!(!format!("{:?}", offer()).contains(&encode_key(&[2; 32])));
    }

    #[test]
    fn rejects_expired_far_future_and_malformed_offers() {
        let uri = offer().to_uri();
        assert_eq!(PairingOffer::parse(&uri, 1_000_300), Err(OfferError::Expired));
        assert_eq!(PairingOffer::parse(&uri, 1_000_300 - MAX_OFFER_LIFETIME_SECS - 1), Err(OfferError::Expired));
        assert_eq!(PairingOffer::parse("https://pair?v=1", 0), Err(OfferError::NotAnOffer));
        assert_eq!(PairingOffer::parse(&uri.replace("v=1", "v=2"), 1_000_000), Err(OfferError::Version));
        let bad_relay = PairingOffer { relay: Some("http://relay".into()), ..offer() }.to_uri();
        assert_eq!(PairingOffer::parse(&bad_relay, 1_000_000), Err(OfferError::Invalid("relay")));
        let nowhere = PairingOffer { relay: None, direct: None, ..offer() }.to_uri();
        assert!(matches!(PairingOffer::parse(&nowhere, 1_000_000), Err(OfferError::Missing(_))));
        let bad_room = PairingOffer { room: "ABC".into(), ..offer() }.to_uri();
        assert_eq!(PairingOffer::parse(&bad_room, 1_000_000), Err(OfferError::Invalid("room")));
    }

    #[test]
    fn room_is_derived_and_not_the_secret() {
        let room = room_id(&[9; 32]);
        assert!(is_room_id(&room));
        assert_ne!(room, room_id(&[8; 32]));
        assert!(!room.contains(&"09".repeat(32)));
    }
}

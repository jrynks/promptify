//! Wire protocol shared by the Promptify desktop server, the relay and remote clients.
//!
//! Security model: a paired phone and the desktop run a Noise handshake end to end. The relay only
//! routes opaque frames between them and never holds a key that can read or forge their traffic.

pub mod messages;
pub mod noise;
pub mod pairing;
pub mod relay;

pub const PROTOCOL_VERSION: u32 = 1;

pub(crate) fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
}

pub fn encode_key(key: &[u8; 32]) -> String {
    use base64::Engine;
    b64().encode(key)
}

pub fn decode_key(text: &str) -> Option<[u8; 32]> {
    use base64::Engine;
    b64().decode(text.trim()).ok()?.try_into().ok()
}

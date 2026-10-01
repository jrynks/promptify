//! Relay framing. The relay sees only these envelopes; payloads are Noise ciphertext.
//!
//! Host link (desktop <-> relay): every binary message is `[kind u8][channel u32 BE][payload]`.
//! Client link (phone <-> relay): binary messages are the raw payload; the relay adds or strips the channel.

use serde::{Deserialize, Serialize};

pub const MAX_PAYLOAD: usize = crate::noise::NOISE_MAX_MESSAGE;
pub const HEADER_LEN: usize = 5;
pub const MAX_HOST_FRAME: usize = MAX_PAYLOAD + HEADER_LEN;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostFrame {
    /// Relay -> host: a client connected on this channel.
    Open(u32),
    Data(u32, Vec<u8>),
    /// Either direction: the channel is gone.
    Close(u32),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FrameError {
    #[error("frame too short")]
    Short,
    #[error("frame too large")]
    TooLarge,
    #[error("unknown frame kind {0}")]
    Kind(u8),
}

impl HostFrame {
    pub fn encode(&self) -> Result<Vec<u8>, FrameError> {
        let (kind, channel, payload): (u8, u32, &[u8]) = match self {
            HostFrame::Open(c) => (1, *c, &[]),
            HostFrame::Data(c, p) => (0, *c, p),
            HostFrame::Close(c) => (2, *c, &[]),
        };
        if payload.len() > MAX_PAYLOAD {
            return Err(FrameError::TooLarge);
        }
        let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
        out.push(kind);
        out.extend_from_slice(&channel.to_be_bytes());
        out.extend_from_slice(payload);
        Ok(out)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, FrameError> {
        if bytes.len() < HEADER_LEN {
            return Err(FrameError::Short);
        }
        if bytes.len() > MAX_HOST_FRAME {
            return Err(FrameError::TooLarge);
        }
        let channel = u32::from_be_bytes(bytes[1..5].try_into().expect("4 bytes"));
        match bytes[0] {
            0 => Ok(HostFrame::Data(channel, bytes[HEADER_LEN..].to_vec())),
            1 => Ok(HostFrame::Open(channel)),
            2 => Ok(HostFrame::Close(channel)),
            other => Err(FrameError::Kind(other)),
        }
    }
}

/// First text message on a host link; proves the right to host `room_id(host_secret)`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HostHello {
    pub v: u32,
    pub host_secret: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        for frame in [HostFrame::Open(7), HostFrame::Data(u32::MAX, vec![1, 2, 3]), HostFrame::Close(0)] {
            assert_eq!(HostFrame::decode(&frame.encode().unwrap()).unwrap(), frame);
        }
    }

    #[test]
    fn rejects_short_oversized_and_unknown_frames() {
        assert_eq!(HostFrame::decode(&[0, 0, 0]), Err(FrameError::Short));
        assert_eq!(HostFrame::decode(&vec![0; MAX_HOST_FRAME + 1]), Err(FrameError::TooLarge));
        assert_eq!(HostFrame::decode(&[9, 0, 0, 0, 1]), Err(FrameError::Kind(9)));
        assert_eq!(HostFrame::Data(1, vec![0; MAX_PAYLOAD + 1]).encode(), Err(FrameError::TooLarge));
    }
}

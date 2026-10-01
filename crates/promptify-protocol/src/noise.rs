//! Noise handshakes and the encrypted channel.
//!
//! Pairing uses `XXpsk3`: both static keys are exchanged and the one-time secret from the QR code is
//! mixed in, so only the holder of the QR code can complete it. Later sessions use `IK`: the phone
//! already knows the desktop's key, and the desktop admits only paired, unrevoked phone keys.

use snow::{Builder, HandshakeState, TransportState};

pub const PAIRING_PATTERN: &str = "Noise_XXpsk3_25519_ChaChaPoly_BLAKE2s";
pub const SESSION_PATTERN: &str = "Noise_IK_25519_ChaChaPoly_BLAKE2s";
pub const NOISE_MAX_MESSAGE: usize = 65535;
const TAG_LEN: usize = 16;
pub const MAX_PLAINTEXT: usize = NOISE_MAX_MESSAGE - TAG_LEN;
const PROLOGUE: &[u8] = b"promptify/1";

#[derive(Debug, thiserror::Error)]
pub enum NoiseError {
    #[error("noise: {0}")]
    Snow(#[from] snow::Error),
    #[error("message too large")]
    TooLarge,
    #[error("handshake is not finished")]
    NotFinished,
    #[error("peer key is missing or malformed")]
    BadPeerKey,
}

#[derive(Clone)]
pub struct StaticKeys {
    pub private: [u8; 32],
    pub public: [u8; 32],
}

impl std::fmt::Debug for StaticKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StaticKeys").field("public", &crate::encode_key(&self.public)).finish_non_exhaustive()
    }
}

impl StaticKeys {
    pub fn generate() -> Result<Self, NoiseError> {
        let pair = Builder::new(SESSION_PATTERN.parse()?).generate_keypair()?;
        Ok(Self {
            private: pair.private.as_slice().try_into().map_err(|_| NoiseError::BadPeerKey)?,
            public: pair.public.as_slice().try_into().map_err(|_| NoiseError::BadPeerKey)?,
        })
    }
}

pub fn random_secret() -> Result<[u8; 32], NoiseError> {
    // A fresh X25519 private key is 32 uniformly random bytes from the OS RNG.
    Ok(StaticKeys::generate()?.private)
}

pub struct Handshake {
    state: HandshakeState,
}

impl Handshake {
    /// Phone side of pairing. Callers must check [`Self::remote_static`] against the QR key before
    /// writing the third message.
    pub fn pairing_initiator(local: &StaticKeys, secret: &[u8; 32]) -> Result<Self, NoiseError> {
        let state = Builder::new(PAIRING_PATTERN.parse()?)
            .local_private_key(&local.private)?
            .psk(3, secret)?
            .prologue(PROLOGUE)?
            .build_initiator()?;
        Ok(Self { state })
    }

    pub fn pairing_responder(local: &StaticKeys, secret: &[u8; 32]) -> Result<Self, NoiseError> {
        let state = Builder::new(PAIRING_PATTERN.parse()?)
            .local_private_key(&local.private)?
            .psk(3, secret)?
            .prologue(PROLOGUE)?
            .build_responder()?;
        Ok(Self { state })
    }

    pub fn session_initiator(local: &StaticKeys, desktop: &[u8; 32]) -> Result<Self, NoiseError> {
        let state = Builder::new(SESSION_PATTERN.parse()?)
            .local_private_key(&local.private)?
            .remote_public_key(desktop)?
            .prologue(PROLOGUE)?
            .build_initiator()?;
        Ok(Self { state })
    }

    /// Desktop side. The caller must reject the session unless [`Self::remote_static`] is a paired device.
    pub fn session_responder(local: &StaticKeys) -> Result<Self, NoiseError> {
        let state = Builder::new(SESSION_PATTERN.parse()?)
            .local_private_key(&local.private)?
            .prologue(PROLOGUE)?
            .build_responder()?;
        Ok(Self { state })
    }

    pub fn write(&mut self, payload: &[u8]) -> Result<Vec<u8>, NoiseError> {
        let mut out = vec![0u8; NOISE_MAX_MESSAGE];
        let len = self.state.write_message(payload, &mut out)?;
        out.truncate(len);
        Ok(out)
    }

    pub fn read(&mut self, message: &[u8]) -> Result<Vec<u8>, NoiseError> {
        if message.len() > NOISE_MAX_MESSAGE {
            return Err(NoiseError::TooLarge);
        }
        let mut out = vec![0u8; NOISE_MAX_MESSAGE];
        let len = self.state.read_message(message, &mut out)?;
        out.truncate(len);
        Ok(out)
    }

    pub fn is_finished(&self) -> bool {
        self.state.is_handshake_finished()
    }

    pub fn remote_static(&self) -> Option<[u8; 32]> {
        self.state.get_remote_static()?.try_into().ok()
    }

    pub fn into_channel(self) -> Result<SecureChannel, NoiseError> {
        if !self.state.is_handshake_finished() {
            return Err(NoiseError::NotFinished);
        }
        let remote = self.remote_static().ok_or(NoiseError::BadPeerKey)?;
        Ok(SecureChannel { transport: self.state.into_transport_mode()?, remote })
    }
}

/// Ordered, authenticated encryption. Replayed, reordered or altered frames fail to open.
pub struct SecureChannel {
    transport: TransportState,
    remote: [u8; 32],
}

impl SecureChannel {
    pub fn remote_static(&self) -> [u8; 32] {
        self.remote
    }

    pub fn seal(&mut self, plaintext: &[u8]) -> Result<Vec<u8>, NoiseError> {
        if plaintext.len() > MAX_PLAINTEXT {
            return Err(NoiseError::TooLarge);
        }
        let mut out = vec![0u8; plaintext.len() + TAG_LEN];
        let len = self.transport.write_message(plaintext, &mut out)?;
        out.truncate(len);
        Ok(out)
    }

    pub fn open(&mut self, ciphertext: &[u8]) -> Result<Vec<u8>, NoiseError> {
        if ciphertext.len() > NOISE_MAX_MESSAGE {
            return Err(NoiseError::TooLarge);
        }
        let mut out = vec![0u8; ciphertext.len()];
        let len = self.transport.read_message(ciphertext, &mut out)?;
        out.truncate(len);
        Ok(out)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn pair(phone: &StaticKeys, desktop: &StaticKeys, phone_secret: &[u8; 32], desktop_secret: &[u8; 32]) -> Result<(SecureChannel, SecureChannel), NoiseError> {
        let mut p = Handshake::pairing_initiator(phone, phone_secret)?;
        let mut d = Handshake::pairing_responder(desktop, desktop_secret)?;
        d.read(&p.write(b"")?)?;
        p.read(&d.write(b"")?)?;
        assert_eq!(p.remote_static(), Some(desktop.public));
        d.read(&p.write(b"")?)?;
        Ok((p.into_channel()?, d.into_channel()?))
    }

    #[test]
    fn pairing_binds_both_keys_and_the_secret() {
        let (phone, desktop) = (StaticKeys::generate().unwrap(), StaticKeys::generate().unwrap());
        let secret = random_secret().unwrap();
        let (mut p, mut d) = pair(&phone, &desktop, &secret, &secret).unwrap();
        assert_eq!(d.remote_static(), phone.public);
        assert_eq!(p.remote_static(), desktop.public);
        let sealed = p.seal(b"hello desktop").unwrap();
        assert!(!sealed.windows(5).any(|w| w == b"hello"));
        assert_eq!(d.open(&sealed).unwrap(), b"hello desktop");
        assert_eq!(p.open(&d.seal(b"hi phone").unwrap()).unwrap(), b"hi phone");
    }

    #[test]
    fn pairing_fails_without_the_qr_secret() {
        let (phone, desktop) = (StaticKeys::generate().unwrap(), StaticKeys::generate().unwrap());
        let wrong = random_secret().unwrap();
        assert!(pair(&phone, &desktop, &wrong, &random_secret().unwrap()).is_err());
    }

    fn session(phone: &StaticKeys, desktop: &StaticKeys, pinned: &[u8; 32]) -> Result<(SecureChannel, SecureChannel), NoiseError> {
        let mut p = Handshake::session_initiator(phone, pinned)?;
        let mut d = Handshake::session_responder(desktop)?;
        d.read(&p.write(b"")?)?;
        p.read(&d.write(b"")?)?;
        Ok((p.into_channel()?, d.into_channel()?))
    }

    #[test]
    fn session_authenticates_the_phone_to_the_desktop() {
        let (phone, desktop) = (StaticKeys::generate().unwrap(), StaticKeys::generate().unwrap());
        let (mut p, mut d) = session(&phone, &desktop, &desktop.public).unwrap();
        assert_eq!(d.remote_static(), phone.public);
        assert_eq!(d.open(&p.seal(b"x").unwrap()).unwrap(), b"x");
    }

    #[test]
    fn session_to_an_impostor_desktop_fails() {
        let (phone, desktop, impostor) = (StaticKeys::generate().unwrap(), StaticKeys::generate().unwrap(), StaticKeys::generate().unwrap());
        assert!(session(&phone, &impostor, &desktop.public).is_err());
    }

    #[test]
    fn tampered_replayed_and_oversized_frames_are_rejected() {
        let (phone, desktop) = (StaticKeys::generate().unwrap(), StaticKeys::generate().unwrap());
        let (mut p, mut d) = session(&phone, &desktop, &desktop.public).unwrap();
        let first = p.seal(b"one").unwrap();
        let mut tampered = first.clone();
        tampered[0] ^= 1;
        assert!(d.open(&tampered).is_err());
        let (mut p, mut d) = session(&phone, &desktop, &desktop.public).unwrap();
        let first = p.seal(b"one").unwrap();
        assert_eq!(d.open(&first).unwrap(), b"one");
        assert!(d.open(&first).is_err(), "replay accepted");
        assert!(matches!(p.seal(&vec![0; MAX_PLAINTEXT + 1]), Err(NoiseError::TooLarge)));
        assert!(matches!(d.open(&vec![0; NOISE_MAX_MESSAGE + 1]), Err(NoiseError::TooLarge)));
    }
}

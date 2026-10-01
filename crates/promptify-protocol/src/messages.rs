//! Application messages, JSON-encoded inside the encrypted channel.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const MAX_MESSAGE_BYTES: usize = crate::noise::MAX_PLAINTEXT;
/// Raw PCM16 bytes per audio chunk; leaves headroom for base64 and JSON inside one Noise frame.
pub const MAX_AUDIO_CHUNK_BYTES: usize = 32 * 1024;
pub const MAX_TEXT_CHARS: usize = 8000;
pub const MAX_AUDIO_SECONDS: u32 = 120;
pub const AUDIO_SAMPLE_RATE: u32 = 16_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WireMode {
    Prompt,
    Dictation,
}

/// Where the result will go on the client, declared by the client.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WireContext {
    #[serde(default)]
    pub app: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClientMessage {
    /// First message after pairing: a name the owner will recognise in the device list.
    Hello { device_name: String },
    /// Starts a job. With `text` the job runs immediately; otherwise audio chunks follow (16 kHz mono PCM16 LE).
    Transform { id: u64, mode: WireMode, #[serde(default)] context: WireContext, #[serde(default)] text: Option<String> },
    AudioChunk { id: u64, pcm16: String },
    AudioEnd { id: u64 },
    Cancel { id: u64 },
    ListProfiles,
    Ping,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Paired { device_id: String },
    Ready { device_id: String },
    Stage { id: u64, stage: String },
    Transcript { id: u64, text: String },
    Token { id: u64, text: String },
    Done { id: u64, text: String, truncated: bool, profile: String, structure: Option<String> },
    NoSpeech { id: u64 },
    Cancelled { id: u64 },
    Failed { id: u64, reason: String },
    Error { code: ErrorCode, message: String },
    Profiles { profiles: Vec<ProfileInfo> },
    Pong,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    BadMessage,
    Busy,
    TooLarge,
    UnknownJob,
    Unauthorized,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProfileInfo {
    pub id: String,
    pub name: String,
}

pub fn decode_client(bytes: &[u8]) -> Result<ClientMessage, String> {
    if bytes.len() > MAX_MESSAGE_BYTES {
        return Err("message too large".into());
    }
    serde_json::from_slice(bytes).map_err(|e| format!("bad message: {e}"))
}

pub fn encode<T: Serialize>(message: &T) -> Vec<u8> {
    serde_json::to_vec(message).expect("protocol messages always serialize")
}

/// JSON Schema for every message a client sends or receives, for clients written in other languages.
pub fn json_schema() -> serde_json::Value {
    let mut generator = schemars::generate::SchemaSettings::draft2020_12().into_generator();
    let client = generator.subschema_for::<ClientMessage>();
    let server = generator.subschema_for::<ServerMessage>();
    let definitions = generator.take_definitions(true);
    serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "Promptify remote protocol",
        "description": "Messages exchanged inside the end-to-end encrypted Noise channel, one JSON object per frame.",
        "protocol_version": crate::PROTOCOL_VERSION,
        "limits": {
            "max_message_bytes": MAX_MESSAGE_BYTES,
            "max_audio_chunk_bytes": MAX_AUDIO_CHUNK_BYTES,
            "max_text_chars": MAX_TEXT_CHARS,
            "max_audio_seconds": MAX_AUDIO_SECONDS,
            "audio_sample_rate": AUDIO_SAMPLE_RATE
        },
        "properties": { "client": client, "server": server },
        "$defs": definitions
    })
}

pub fn decode_pcm16(text: &str) -> Option<Vec<f32>> {
    use base64::Engine;
    let bytes = crate::b64().decode(text).ok()?;
    if bytes.len() > MAX_AUDIO_CHUNK_BYTES || bytes.len() % 2 != 0 {
        return None;
    }
    Some(bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0).collect())
}

pub fn encode_pcm16(samples: &[f32]) -> String {
    use base64::Engine;
    let bytes: Vec<u8> = samples.iter().flat_map(|s| ((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes()).collect();
    crate::b64().encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_round_trip_and_reject_unknown_fields() {
        let msg = ClientMessage::Transform { id: 3, mode: WireMode::Prompt, context: WireContext { app: "com.openai.chatgpt".into(), ..Default::default() }, text: Some("hi".into()) };
        assert_eq!(decode_client(&encode(&msg)).unwrap(), msg);
        assert!(decode_client(br#"{"type":"cancel","id":1,"extra":1}"#).is_err());
        assert!(decode_client(br#"{"type":"transform","id":1,"mode":"prompt","context":{"app":"x","secret":1}}"#).is_err());
        assert!(decode_client(&vec![b' '; MAX_MESSAGE_BYTES + 1]).is_err());
    }

    #[test]
    fn published_schema_matches_the_code() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../protocol/schema-v1.json");
        let current = serde_json::to_string_pretty(&json_schema()).unwrap() + "\n";
        if std::env::var_os("PROMPTIFY_WRITE_SCHEMA").is_some() {
            std::fs::create_dir_all(std::path::Path::new(path).parent().unwrap()).unwrap();
            std::fs::write(path, &current).unwrap();
        }
        let published = std::fs::read_to_string(path).unwrap_or_default().replace("\r\n", "\n");
        assert!(published == current, "protocol/schema-v1.json is out of date; rerun with PROMPTIFY_WRITE_SCHEMA=1");
        for name in ["hello", "transform", "audio_chunk", "audio_end", "cancel", "list_profiles", "paired", "done", "no_speech"] {
            assert!(current.contains(&format!("\"{name}\"")), "{name} missing from schema");
        }
    }

    #[test]
    fn audio_chunks_round_trip_within_limits() {
        let samples = [0.0, 0.5, -0.5, 1.0];
        let decoded = decode_pcm16(&encode_pcm16(&samples)).unwrap();
        assert!(decoded.iter().zip(samples).all(|(a, b)| (a - b).abs() < 0.001));
        assert!(decode_pcm16(&encode_pcm16(&vec![0.0; MAX_AUDIO_CHUNK_BYTES / 2 + 1])).is_none());
        assert!(decode_pcm16("not base64!").is_none());
        let max_chunk = ClientMessage::AudioChunk { id: 1, pcm16: encode_pcm16(&vec![0.1; MAX_AUDIO_CHUNK_BYTES / 2]) };
        assert!(encode(&max_chunk).len() <= MAX_MESSAGE_BYTES);
    }
}

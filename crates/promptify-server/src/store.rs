//! Desktop identity and the paired-device registry, persisted in the app data folder.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use promptify_protocol::noise::{StaticKeys, random_secret};
use promptify_protocol::{decode_key, encode_key};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

pub fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

#[derive(Serialize, Deserialize)]
struct IdentityFile {
    private_key: String,
    public_key: String,
    host_secret: String,
}

/// The desktop's long-term Noise key and its relay hosting secret. Never leaves this machine.
pub struct Identity {
    pub keys: StaticKeys,
    pub host_secret: [u8; 32],
}

impl Identity {
    pub fn load_or_create(path: &Path) -> Result<Self, String> {
        if let Ok(text) = std::fs::read_to_string(path) {
            let file: IdentityFile = serde_json::from_str(&text).map_err(|e| format!("identity file is invalid: {e}"))?;
            let private = decode_key(&file.private_key).ok_or("identity private key is invalid")?;
            let public = decode_key(&file.public_key).ok_or("identity public key is invalid")?;
            let host_secret = decode_key(&file.host_secret).ok_or("identity host secret is invalid")?;
            return Ok(Self { keys: StaticKeys { private, public }, host_secret });
        }
        let keys = StaticKeys::generate().map_err(|e| e.to_string())?;
        let host_secret = random_secret().map_err(|e| e.to_string())?;
        let file = IdentityFile { private_key: encode_key(&keys.private), public_key: encode_key(&keys.public), host_secret: encode_key(&host_secret) };
        write_atomic(path, &serde_json::to_vec_pretty(&file).expect("serializable")).map_err(|e| format!("cannot save identity: {e}"))?;
        Ok(Self { keys, host_secret })
    }

    pub fn room(&self) -> String {
        promptify_protocol::pairing::room_id(&self.host_secret)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub public_key: String,
    pub paired_unix: u64,
    #[serde(default)]
    pub last_seen_unix: Option<u64>,
}

pub const MAX_DEVICES: usize = 16;
pub const MAX_NAME_CHARS: usize = 40;

pub fn clean_name(raw: &str) -> String {
    let name: String = raw.chars().filter(|c| !c.is_control()).take(MAX_NAME_CHARS).collect();
    let name = name.trim();
    if name.is_empty() { "Unnamed device".into() } else { name.to_owned() }
}

/// Paired devices. Removing a device revokes it: its live sessions end and it can no longer connect.
pub struct DeviceRegistry {
    path: PathBuf,
    devices: Mutex<BTreeMap<String, Device>>,
    revision: watch::Sender<u64>,
}

impl DeviceRegistry {
    pub fn open(path: PathBuf) -> Result<Self, String> {
        let devices = match std::fs::read_to_string(&path) {
            Ok(text) => {
                let list: Vec<Device> = serde_json::from_str(&text).map_err(|e| format!("device list is invalid: {e}"))?;
                list.into_iter().map(|d| (d.id.clone(), d)).collect()
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(format!("cannot read device list: {e}")),
        };
        Ok(Self { path, devices: Mutex::new(devices), revision: watch::channel(0).0 })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Device>> {
        self.devices.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn save(&self, devices: &BTreeMap<String, Device>) -> Result<(), String> {
        let list: Vec<&Device> = devices.values().collect();
        write_atomic(&self.path, &serde_json::to_vec_pretty(&list).expect("serializable")).map_err(|e| format!("cannot save device list: {e}"))
    }

    pub fn list(&self) -> Vec<Device> {
        self.lock().values().cloned().collect()
    }

    pub fn find_by_key(&self, key: &[u8; 32]) -> Option<Device> {
        let encoded = encode_key(key);
        self.lock().values().find(|d| d.public_key == encoded).cloned()
    }

    pub fn is_active(&self, id: &str) -> bool {
        self.lock().contains_key(id)
    }

    /// Re-pairing the same phone key replaces its old entry instead of adding a duplicate.
    pub fn add(&self, name: &str, key: &[u8; 32]) -> Result<Device, String> {
        let mut devices = self.lock();
        let encoded = encode_key(key);
        devices.retain(|_, d| d.public_key != encoded);
        if devices.len() >= MAX_DEVICES {
            return Err("too many paired devices; remove one first".into());
        }
        let id = format!("dev-{}", encoded[..12].replace(['-', '_'], "x"));
        let device = Device { id: id.clone(), name: clean_name(name), public_key: encoded, paired_unix: now_unix(), last_seen_unix: None };
        devices.insert(id, device.clone());
        self.save(&devices)?;
        drop(devices);
        self.revision.send_modify(|r| *r += 1);
        Ok(device)
    }

    pub fn remove(&self, id: &str) -> Result<bool, String> {
        let mut devices = self.lock();
        let removed = devices.remove(id).is_some();
        if removed {
            self.save(&devices)?;
        }
        drop(devices);
        if removed {
            self.revision.send_modify(|r| *r += 1);
        }
        Ok(removed)
    }

    pub fn rename(&self, id: &str, name: &str) -> Result<bool, String> {
        let mut devices = self.lock();
        let Some(device) = devices.get_mut(id) else { return Ok(false) };
        device.name = clean_name(name);
        self.save(&devices)?;
        Ok(true)
    }

    /// Best effort; a failed write only loses the timestamp.
    pub fn touch(&self, id: &str) {
        let mut devices = self.lock();
        if let Some(device) = devices.get_mut(id) {
            device.last_seen_unix = Some(now_unix());
            let _ = self.save(&devices);
        }
    }

    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.revision.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_created_once_and_reloaded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("remote").join("identity.json");
        let first = Identity::load_or_create(&path).unwrap();
        let second = Identity::load_or_create(&path).unwrap();
        assert_eq!(first.keys.public, second.keys.public);
        assert_eq!(first.host_secret, second.host_secret);
        assert_ne!(first.keys.private, first.host_secret);
    }

    #[test]
    fn registry_persists_dedupes_revokes_and_signals() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("devices.json");
        let registry = DeviceRegistry::open(path.clone()).unwrap();
        let mut rev = registry.subscribe();
        let a = registry.add("Jason's \u{7}phone", &[1; 32]).unwrap();
        assert_eq!(a.name, "Jason's phone");
        assert!(rev.has_changed().unwrap());
        rev.mark_unchanged();
        let again = registry.add("Phone", &[1; 32]).unwrap();
        assert_eq!(registry.list().len(), 1);
        assert_eq!(again.id, a.id);
        let reopened = DeviceRegistry::open(path).unwrap();
        assert_eq!(reopened.find_by_key(&[1; 32]).unwrap().name, "Phone");
        assert!(registry.remove(&a.id).unwrap());
        assert!(rev.has_changed().unwrap());
        assert!(registry.find_by_key(&[1; 32]).is_none());
        assert!(!registry.is_active(&a.id));
    }

    #[test]
    fn registry_caps_devices() {
        let dir = tempfile::tempdir().unwrap();
        let registry = DeviceRegistry::open(dir.path().join("d.json")).unwrap();
        for i in 0..MAX_DEVICES {
            let mut key = [0u8; 32];
            key[0] = i as u8;
            key[1] = 1;
            registry.add("x", &key).unwrap();
        }
        assert!(registry.add("one too many", &[200; 32]).is_err());
    }
}

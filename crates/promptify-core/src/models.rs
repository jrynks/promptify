use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Hosts model downloads may start from. Redirect targets are checked separately by the downloader.
pub const ALLOWED_DOWNLOAD_HOSTS: &[&str] = &["huggingface.co"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelKind {
    Stt,
    Llm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelTier {
    Small,
    Balanced,
    Quality,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelEntry {
    pub id: String,
    pub kind: ModelKind,
    pub tier: ModelTier,
    pub display_name: String,
    pub url: String,
    pub file_name: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub license: String,
    pub min_ram_gb: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub version: u32,
    pub models: Vec<ModelEntry>,
}

#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    #[error("invalid manifest: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("model {0:?}: {1}")]
    Invalid(String, String),
    #[error("model {id:?}: expected {expected} bytes, found {actual}")]
    SizeMismatch { id: String, expected: u64, actual: u64 },
    #[error("model {0:?}: SHA-256 mismatch")]
    HashMismatch(String),
    #[error("model file I/O: {0}")]
    Io(#[from] io::Error),
}

impl Manifest {
    pub fn bundled() -> Self {
        Self::from_json(include_str!("../models/manifest.json")).expect("bundled manifest is validated by tests")
    }

    pub fn get(&self, id: &str) -> Option<&ModelEntry> {
        self.models.iter().find(|m| m.id == id)
    }

    pub fn from_json(source: &str) -> Result<Self, ModelError> {
        let manifest: Manifest = serde_json::from_str(source)?;
        for (index, entry) in manifest.models.iter().enumerate() {
            entry.validate()?;
            if manifest.models[..index].iter().any(|other| other.id == entry.id || other.file_name == entry.file_name) {
                return Err(ModelError::Invalid(entry.id.clone(), "duplicate id or file name".into()));
            }
        }
        Ok(manifest)
    }
}

impl ModelEntry {
    pub fn validate(&self) -> Result<(), ModelError> {
        let invalid = |msg: &str| Err(ModelError::Invalid(self.id.clone(), msg.into()));
        let url = match url::Url::parse(&self.url) {
            Ok(url) => url,
            Err(_) => return invalid("url does not parse"),
        };
        if url.scheme() != "https" {
            return invalid("url must use https");
        }
        if !url.username().is_empty() || url.password().is_some() || url.port().is_some() {
            return invalid("url must not carry credentials or a port");
        }
        if !url.host_str().is_some_and(|h| ALLOWED_DOWNLOAD_HOSTS.contains(&h)) {
            return invalid("url host is not allowlisted");
        }
        if self.sha256.len() != 64 || !self.sha256.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return invalid("sha256 must be 64 lowercase hex characters");
        }
        if !is_safe_file_name(&self.file_name) {
            return invalid("file name must be a plain file name");
        }
        if self.size_bytes == 0 {
            return invalid("size must be positive");
        }
        Ok(())
    }
}

fn is_safe_file_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 200
        && !name.starts_with('.')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

/// Hugging Face serves files through redirects to its own CDN hosts; nothing else is followed.
pub fn is_allowed_redirect(url: &url::Url) -> bool {
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() || url.port().is_some() {
        return false;
    }
    url.host_str().is_some_and(|host| {
        ALLOWED_DOWNLOAD_HOSTS.contains(&host) || host.ends_with(".huggingface.co") || host.ends_with(".hf.co")
    })
}

/// True when a finalized model file is present with the pinned size. Only [`finalize_download`]
/// creates files at final paths, after the SHA-256 check.
pub fn is_installed(models_dir: &Path, entry: &ModelEntry) -> bool {
    std::fs::metadata(final_path(models_dir, entry)).is_ok_and(|m| m.is_file() && m.len() == entry.size_bytes)
}

pub fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let read = file.read(&mut buf)?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

pub fn verify_file(path: &Path, entry: &ModelEntry) -> Result<(), ModelError> {
    let actual = std::fs::metadata(path)?.len();
    if actual != entry.size_bytes {
        return Err(ModelError::SizeMismatch { id: entry.id.clone(), expected: entry.size_bytes, actual });
    }
    if sha256_file(path)? != entry.sha256 {
        return Err(ModelError::HashMismatch(entry.id.clone()));
    }
    Ok(())
}

pub fn final_path(models_dir: &Path, entry: &ModelEntry) -> PathBuf {
    models_dir.join(&entry.file_name)
}

pub fn part_path(models_dir: &Path, entry: &ModelEntry) -> PathBuf {
    models_dir.join(format!("{}.part", entry.file_name))
}

/// Verifies a completed download and atomically moves it into place.
/// A file that fails verification is deleted so it can never be loaded.
pub fn finalize_download(models_dir: &Path, entry: &ModelEntry) -> Result<PathBuf, ModelError> {
    entry.validate()?;
    let part = part_path(models_dir, entry);
    if let Err(err) = verify_file(&part, entry) {
        if !matches!(&err, ModelError::Io(e) if e.kind() == io::ErrorKind::NotFound) {
            let _ = std::fs::remove_file(&part);
        }
        return Err(err);
    }
    let target = final_path(models_dir, entry);
    std::fs::rename(&part, &target)?;
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(bytes: &[u8]) -> ModelEntry {
        ModelEntry {
            id: "tiny".into(),
            kind: ModelKind::Llm,
            tier: ModelTier::Small,
            display_name: "Tiny".into(),
            url: "https://huggingface.co/org/repo/resolve/abc123/tiny.gguf".into(),
            file_name: "tiny.gguf".into(),
            sha256: hex::encode(Sha256::digest(bytes)),
            size_bytes: bytes.len() as u64,
            license: "apache-2.0".into(),
            min_ram_gb: 4,
        }
    }

    #[test]
    fn validation_rejects_unsafe_entries() {
        let ok = entry(b"x");
        assert!(ok.validate().is_ok());
        let cases: Vec<fn(&mut ModelEntry)> = vec![
            |e| e.url = "http://huggingface.co/a".into(),
            |e| e.url = "https://huggingface.co.evil.com/a".into(),
            |e| e.url = "https://evil.com/huggingface.co/a".into(),
            |e| e.url = "https://user@huggingface.co/a".into(),
            |e| e.url = "https://huggingface.co:8443/a".into(),
            |e| e.sha256 = "ABC".into(),
            |e| e.sha256 = e.sha256.to_uppercase(),
            |e| e.file_name = "../evil.gguf".into(),
            |e| e.file_name = r"..\evil.gguf".into(),
            |e| e.file_name = "dir/evil.gguf".into(),
            |e| e.file_name = ".hidden".into(),
            |e| e.size_bytes = 0,
        ];
        for (index, mutate) in cases.into_iter().enumerate() {
            let mut bad = ok.clone();
            mutate(&mut bad);
            assert!(bad.validate().is_err(), "case {index} accepted");
        }
    }

    #[test]
    fn manifest_rejects_duplicates_and_unknown_fields() {
        let e = serde_json::to_value(entry(b"x")).unwrap();
        let dup = serde_json::json!({ "version": 1, "models": [e, e] });
        assert!(Manifest::from_json(&dup.to_string()).is_err());
        let unknown = serde_json::json!({ "version": 1, "models": [], "extra": true });
        assert!(Manifest::from_json(&unknown.to_string()).is_err());
        let single = serde_json::json!({ "version": 1, "models": [e] });
        assert_eq!(Manifest::from_json(&single.to_string()).unwrap().models.len(), 1);
    }

    #[test]
    fn bundled_manifest_is_valid_and_pinned() {
        let manifest = Manifest::bundled();
        for kind in [ModelKind::Stt, ModelKind::Llm] {
            for tier in [ModelTier::Small, ModelTier::Balanced, ModelTier::Quality] {
                assert_eq!(manifest.models.iter().filter(|m| m.kind == kind && m.tier == tier).count(), 1, "{kind:?} {tier:?}");
            }
        }
        // Commit-pinned URLs: a moved branch can never change what a hash refers to.
        assert!(manifest.models.iter().all(|m| m.url.contains("/resolve/") && !m.url.contains("/resolve/main/")));
    }

    #[test]
    fn redirects_only_to_hugging_face_hosts() {
        let ok = |s: &str| is_allowed_redirect(&url::Url::parse(s).unwrap());
        assert!(ok("https://huggingface.co/a"));
        assert!(ok("https://cas-bridge.xethub.hf.co/x"));
        assert!(ok("https://cdn-lfs-us-1.huggingface.co/x"));
        assert!(!ok("http://cas-bridge.xethub.hf.co/x"));
        assert!(!ok("https://evilhf.co/x"));
        assert!(!ok("https://hf.co.evil.com/x"));
        assert!(!ok("https://user:pw@huggingface.co/x"));
    }

    #[test]
    fn installed_requires_final_file_with_exact_size() {
        let dir = tempfile::tempdir().unwrap();
        let e = entry(b"abc");
        assert!(!is_installed(dir.path(), &e));
        std::fs::write(part_path(dir.path(), &e), b"abc").unwrap();
        assert!(!is_installed(dir.path(), &e));
        std::fs::write(final_path(dir.path(), &e), b"ab").unwrap();
        assert!(!is_installed(dir.path(), &e));
        std::fs::write(final_path(dir.path(), &e), b"abc").unwrap();
        assert!(is_installed(dir.path(), &e));
    }

    #[test]
    fn finalize_moves_verified_download() {
        let dir = tempfile::tempdir().unwrap();
        let e = entry(b"model bytes");
        std::fs::write(part_path(dir.path(), &e), b"model bytes").unwrap();
        let path = finalize_download(dir.path(), &e).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"model bytes");
        assert!(!part_path(dir.path(), &e).exists());
    }

    #[test]
    fn finalize_deletes_tampered_or_short_download() {
        let dir = tempfile::tempdir().unwrap();
        let e = entry(b"model bytes");
        for (bytes, hash_error) in [(&b"model bytez"[..], true), (&b"model"[..], false)] {
            std::fs::write(part_path(dir.path(), &e), bytes).unwrap();
            let err = finalize_download(dir.path(), &e).unwrap_err();
            assert_eq!(matches!(err, ModelError::HashMismatch(_)), hash_error, "{err}");
            assert!(!part_path(dir.path(), &e).exists());
            assert!(!final_path(dir.path(), &e).exists());
        }
    }
}

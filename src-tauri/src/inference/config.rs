use super::types::*;
use std::{io::Write, net::IpAddr, path::Path};

pub fn endpoint(c: &InferenceConnection, path: &str) -> Result<reqwest::Url, String> {
    let mut url = reqwest::Url::parse(&c.base_url).map_err(|_| "Invalid API base URL.")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("Use an HTTP(S) API base URL without userinfo, queries, or fragments.".into());
    }
    let host = url.host_str().unwrap().trim_matches(['[', ']']);
    let ip = host.parse::<IpAddr>().ok();
    let loopback = host == "localhost" || ip.is_some_and(|ip| ip.is_loopback());
    let private = ip.is_some_and(|ip| match ip {
        IpAddr::V4(ip) => ip.is_private() || ip.is_link_local(),
        IpAddr::V6(ip) => ip.is_unique_local() || ip.is_unicast_link_local(),
    });
    if url.scheme() == "http" && !loopback && !(private && c.allow_insecure_lan) {
        return Err(
            "Public endpoints require HTTPS; private HTTP requires explicit LAN acknowledgment."
                .into(),
        );
    }
    if !loopback && !c.consent_remote {
        return Err("Confirm remote data disclosure before using this endpoint.".into());
    }
    let base = url.path().trim_end_matches('/');
    url.set_path(&format!("{base}/{path}"));
    Ok(url)
}
pub fn validate(config: &InferenceConfig) -> Result<(), String> {
    if config.version != 1 {
        return Err(
            "Unsupported inference configuration version. Restore a compatible backup.".into(),
        );
    }
    let mut ids = std::collections::HashSet::new();
    for c in &config.connections {
        if c.id.is_empty()
            || c.id.len() > 128
            || !c
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
            || !ids.insert(&c.id)
            || c.name.trim().is_empty()
            || c.model.trim().is_empty()
            || c.revision == 0
            || c.revision > config.revision
        {
            return Err(
                "Invalid or duplicate inference connection identifier/name/model/revision.".into(),
            );
        }
        endpoint(c, "models")?;
        if c.provider == Provider::Google && c.protocol != Protocol::OpenaiChatCompletions {
            return Err("Google preset uses its documented Chat Completions endpoint.".into());
        }
    }
    if let InferenceSelection::Connection {
        connection_id,
        model,
    } = &config.selection
    {
        if model.trim().is_empty() || !config.connections.iter().any(|c| &c.id == connection_id) {
            return Err(
                "Selected inference connection/model unavailable. Choose an explicit replacement."
                    .into(),
            );
        }
    }
    Ok(())
}
pub fn load(dir: &Path) -> Result<InferenceConfig, String> {
    match std::fs::read(dir.join("inference.json")) {
        Ok(bytes) => {
            let config=serde_json::from_slice(&bytes).map_err(|_|"Invalid inference.json. Restore or repair the sidecar; no fallback was selected.")?;
            validate(&config)?;
            Ok(config)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(InferenceConfig::default()),
        Err(_) => Err("Cannot read inference.json.".into()),
    }
}
pub fn save(dir: &Path, config: &InferenceConfig) -> Result<(), String> {
    validate(config)?;
    std::fs::create_dir_all(dir).map_err(|_| "Cannot create inference data directory.")?;
    let path = dir.join("inference.json");
    if path.exists() {
        std::fs::copy(&path, dir.join("inference.json.bak"))
            .map_err(|_| "Cannot preserve inference backup.")?;
    }
    let staging = dir.join("inference.json.new");
    let mut created = false;
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging)
            .map_err(
                |_| "Cannot stage inference configuration; check for a stale inference.json.new.",
            )?;
        created = true;
        file.write_all(
            &serde_json::to_vec_pretty(config)
                .map_err(|_| "Cannot encode inference configuration.")?,
        )
        .and_then(|_| file.sync_all())
        .map_err(|_| "Cannot write inference configuration.")?;
        std::fs::rename(&staging, &path).map_err(|_| "Cannot publish inference configuration.")?;
        Ok(())
    })();
    if result.is_err() && created {
        let _ = std::fs::remove_file(staging);
    }
    result
}

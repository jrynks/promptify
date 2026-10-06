//! Direct desktop inference with immutable job sessions and OS-store-only credentials.
mod config;
pub mod credentials;
mod transport;
pub mod types;
use crate::{
    llm_client::{LlmWorker, worker_exe},
    settings::SharedSettings,
};
use credentials::{CredentialStore, OsCredentialStore};
use promptify_core::{
    models::{Manifest, ModelKind, is_installed},
    pipeline::{BackendError, CancelToken, Generation, GenerationRequest, Generator},
    prompt::{ChatMessage, Role},
};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, RwLock, Weak},
    time::{Duration, Instant},
};
pub use types::*;

fn connection_base_url(provider: Provider, base_url: &str) -> String {
    if base_url.trim().is_empty() {
        provider.default_base_url().unwrap_or("").into()
    } else {
        base_url.trim().into()
    }
}

pub struct InferenceManager {
    data_dir: PathBuf,
    manifest: Manifest,
    models_dir: PathBuf,
    settings: SharedSettings,
    state: Mutex<Result<InferenceConfig, String>>,
    credentials: Arc<dyn CredentialStore>,
    revocations: Mutex<HashMap<String, Vec<Weak<transport::SessionControl>>>>,
    local_workers: Mutex<HashMap<(Option<String>, bool), Arc<LlmWorker>>>,
    test_attempts: Mutex<HashMap<(String, String, u64), u64>>,
}
impl InferenceManager {
    pub fn new(
        data_dir: PathBuf,
        manifest: Manifest,
        models_dir: PathBuf,
        settings: SharedSettings,
    ) -> Self {
        Self::with_credential_store(
            data_dir,
            manifest,
            models_dir,
            settings,
            Arc::new(OsCredentialStore),
        )
    }
    pub fn with_credential_store(
        data_dir: PathBuf,
        manifest: Manifest,
        models_dir: PathBuf,
        settings: SharedSettings,
        credentials: Arc<dyn CredentialStore>,
    ) -> Self {
        let state = config::load(&data_dir);
        Self {
            data_dir,
            manifest,
            models_dir,
            settings,
            state: Mutex::new(state),
            credentials,
            revocations: Mutex::new(HashMap::new()),
            local_workers: Mutex::new(HashMap::new()),
            test_attempts: Mutex::new(HashMap::new()),
        }
    }
    pub fn config(&self) -> Result<InferenceConfig, String> {
        self.state
            .lock()
            .map_err(|_| "Inference state unavailable.")?
            .clone()
    }
    /// Configuration readiness only; never performs an inference test.
    pub fn configured(&self) -> Result<bool, String> {
        let config = self.config()?;
        match config.selection {
            InferenceSelection::BundledLocal => {
                let settings = self
                    .settings
                    .read()
                    .map_err(|_| "Local settings unavailable.")?;
                Ok(settings
                    .llm_model
                    .as_deref()
                    .and_then(|id| self.manifest.get(id))
                    .is_some_and(|model| {
                        model.kind == ModelKind::Llm && is_installed(&self.models_dir, model)
                    }))
            }
            InferenceSelection::Connection { connection_id, .. } => {
                let connection = config
                    .connections
                    .iter()
                    .find(|connection| connection.id == connection_id)
                    .ok_or("Selected inference connection unavailable.")?;
                if connection.auth == AuthMode::ApiKey && !connection.credential_present {
                    return Ok(false);
                }
                self.secret(connection)?;
                Ok(true)
            }
        }
    }
    fn key(&self, c: &InferenceConnection) -> String {
        let namespace = Sha256::digest(self.data_dir.as_os_str().as_encoded_bytes());
        format!("{namespace:x}.{}.{}", c.id, c.revision)
    }
    fn revocation(&self, c: &InferenceConnection) -> Arc<transport::SessionControl> {
        let control = Arc::new(transport::SessionControl::default());
        let mut sessions = self.revocations.lock().unwrap();
        sessions.retain(|_, values| {
            values.retain(|value| value.strong_count() > 0);
            !values.is_empty()
        });
        sessions
            .entry(self.key(c))
            .or_default()
            .push(Arc::downgrade(&control));
        control
    }
    fn revoke(&self, c: &InferenceConnection) {
        let key = self.key(c);
        let prefix = key.rsplit_once('.').unwrap().0;
        for (key, sessions) in self.revocations.lock().unwrap().iter() {
            if key
                .rsplit_once('.')
                .is_some_and(|(candidate, _)| candidate == prefix)
            {
                for session in sessions.iter().filter_map(Weak::upgrade) {
                    session.revoke();
                }
            }
        }
    }
    fn secret(&self, c: &InferenceConnection) -> Result<Option<String>, String> {
        if c.auth == AuthMode::None {
            return Ok(None);
        }
        self.credentials
            .get(&self.key(c))?
            .filter(|s| !s.is_empty())
            .map(Some)
            .ok_or_else(|| "API credential missing. Save a key in Models > Prompt writer.".into())
    }
    pub fn upsert_connection(&self, input: ConnectionInput) -> Result<InferenceConfig, String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "Inference state unavailable.")?;
        let mut next = state.clone()?;
        if input.secret.is_some() && input.remove_secret {
            return Err("Cannot save and remove a credential together.".into());
        }
        let old = next.connections.iter().find(|c| c.id == input.id).cloned();
        let credential_changed = input.secret.is_some() || input.remove_secret;
        let revision = next
            .revision
            .checked_add(1)
            .ok_or("Inference revision overflow.")?;
        let mut c = InferenceConnection {
            id: input.id,
            name: input.name,
            provider: input.provider,
            base_url: connection_base_url(input.provider, &input.base_url),
            protocol: input.protocol,
            auth: input.auth,
            model: input.model,
            stream: input.stream,
            allow_insecure_lan: input.allow_insecure_lan,
            consent_remote: input.consent_remote,
            credential_present: false,
            revision,
        };
        if let Some(old) = &old {
            if (old.base_url != c.base_url
                || old.provider != c.provider
                || old.protocol != c.protocol)
                && old.credential_present
                && input.secret.is_none()
                && !input.remove_secret
            {
                return Err("Endpoint/provider changes require explicitly supplying a new credential or removing the old one.".into());
            }
        }
        let secret = if input.remove_secret || c.auth == AuthMode::None {
            None
        } else if let Some(secret) = input.secret {
            if secret.is_empty() || secret.contains(['\r', '\n']) {
                return Err("Credential must be nonempty and contain no newlines.".into());
            }
            Some(secret)
        } else if let Some(old) = &old {
            self.secret(old)?
        } else {
            None
        };
        c.credential_present = secret.is_some();
        let changed = old.as_ref().is_none_or(|old| {
            credential_changed
                || old.base_url != c.base_url
                || old.provider != c.provider
                || old.protocol != c.protocol
                || old.auth != c.auth
                || old.model != c.model
                || old.stream != c.stream
                || old.consent_remote != c.consent_remote
                || old.credential_present != c.credential_present
        });
        if !changed {
            c.revision = old.as_ref().unwrap().revision;
        } else {
            next.verified.retain(|tested| tested.connection_id != c.id);
        }
        next.connections.retain(|old| old.id != c.id);
        next.connections.push(c.clone());
        next.revision = revision;
        config::validate(&next)?;
        if let Some(secret) = secret.as_ref().filter(|_| changed) {
            if let Err(error) = self.credentials.set(&self.key(&c), secret) {
                if self.credentials.remove(&self.key(&c)).is_err() {
                    return Err(format!(
                        "{error} Staged credential cleanup also failed; retry OS credential cleanup before saving."
                    ));
                }
                return Err(error);
            }
        }
        if let Err(error) = config::save(&self.data_dir, &next) {
            if changed && secret.is_some() && self.credentials.remove(&self.key(&c)).is_err() {
                return Err(format!(
                    "{error} Credential rollback also failed; remove the staged OS credential before retrying."
                ));
            }
            return Err(error);
        }
        *state = Ok(next.clone());
        if let Some(old) = old {
            self.revoke(&old);
            if changed && old.credential_present {
                self.credentials.remove(&self.key(&old)).map_err(|_|"Connection saved and old sessions revoked, but old OS credential cleanup failed.")?;
            }
        }
        Ok(next)
    }
    pub fn remove_connection(&self, id: &str) -> Result<InferenceConfig, String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "Inference state unavailable.")?;
        let mut next = state.clone()?;
        if matches!(&next.selection,InferenceSelection::Connection{connection_id,..}if connection_id==id)
        {
            return Err(
                "Select an explicit replacement before deleting the active connection.".into(),
            );
        }
        let old = next
            .connections
            .iter()
            .find(|c| c.id == id)
            .cloned()
            .ok_or("Inference connection not found.")?;
        next.connections.retain(|c| c.id != id);
        next.verified.retain(|tested| tested.connection_id != id);
        next.revision = next
            .revision
            .checked_add(1)
            .ok_or("Inference revision overflow.")?;
        config::save(&self.data_dir, &next)?;
        *state = Ok(next.clone());
        self.revoke(&old);
        if old.credential_present {
            self.credentials.remove(&self.key(&old)).map_err(
                |_| "Connection deleted and sessions revoked, but OS credential cleanup failed.",
            )?;
        }
        Ok(next)
    }
    pub fn select(&self, selection: InferenceSelection) -> Result<InferenceConfig, String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "Inference state unavailable.")?;
        let mut next = state.clone()?;
        next.selection = selection;
        next.revision = next
            .revision
            .checked_add(1)
            .ok_or("Inference revision overflow.")?;
        config::validate(&next)?;
        if let InferenceSelection::Connection { connection_id, .. } = &next.selection {
            self.secret(
                next.connections
                    .iter()
                    .find(|c| &c.id == connection_id)
                    .unwrap(),
            )?;
        }
        config::save(&self.data_dir, &next)?;
        *state = Ok(next.clone());
        if matches!(next.selection, InferenceSelection::Connection { .. }) {
            self.local_workers.lock().unwrap().clear();
        }
        Ok(next)
    }
    fn local(&self) -> Arc<LlmWorker> {
        let settings = self.settings.read().unwrap().clone();
        let key = (settings.llm_model.clone(), settings.use_gpu);
        let mut workers = self.local_workers.lock().unwrap();
        workers.retain(|configuration, _| configuration == &key);
        workers
            .entry(key)
            .or_insert_with(|| {
                Arc::new(LlmWorker::new(
                    worker_exe(),
                    self.manifest.clone(),
                    self.models_dir.clone(),
                    Arc::new(RwLock::new(settings)),
                ))
            })
            .clone()
    }
    fn session(&self) -> Result<Arc<dyn Generator>, BackendError> {
        let state = self
            .state
            .lock()
            .map_err(|_| BackendError("Inference state unavailable.".into()))?;
        let config = state.as_ref().map_err(|e| BackendError(e.clone()))?;
        match &config.selection {
            InferenceSelection::BundledLocal => Ok(self.local()),
            InferenceSelection::Connection {
                connection_id,
                model,
            } => {
                let c = config
                    .connections
                    .iter()
                    .find(|c| &c.id == connection_id)
                    .ok_or_else(|| {
                        BackendError("Selected inference connection is unavailable.".into())
                    })?;
                Ok(Arc::new(transport::HttpSession {
                    connection: c.clone(),
                    model: model.clone(),
                    secret: self.secret(c).map_err(BackendError)?,
                    revoked: self.revocation(c),
                }))
            }
        }
    }
    pub fn status(&self) -> InferenceStatus {
        let config = match self.config() {
            Ok(config) => config,
            Err(message) => {
                return InferenceStatus {
                    state: StatusState::Error,
                    selection: InferenceSelection::BundledLocal,
                    revision: 0,
                    message: Some(message),
                };
            }
        };
        let (state, message) = match &config.selection {
            InferenceSelection::BundledLocal => {
                let settings = self.settings.read().unwrap();
                let installed = settings
                    .llm_model
                    .as_deref()
                    .and_then(|id| self.manifest.get(id))
                    .is_some_and(|m| m.kind == ModelKind::Llm && is_installed(&self.models_dir, m));
                if installed {
                    (StatusState::Ready, None)
                } else {
                    (
                        StatusState::Error,
                        Some(
                            "Install and select a bundled language model in Models settings."
                                .into(),
                        ),
                    )
                }
            }
            InferenceSelection::Connection {
                connection_id,
                model,
            } => {
                if let Some(c) = config.connections.iter().find(|c| &c.id == connection_id) {
                    if c.auth == AuthMode::ApiKey && !c.credential_present {
                        (StatusState::Error, Some("API credential missing.".into()))
                    } else if let Err(error) = self.secret(c) {
                        (StatusState::Error, Some(error))
                    } else if config.is_verified(c, model) {
                        (StatusState::Ready, None)
                    } else {
                        (StatusState::Configured,Some("Configured but not tested. Explicit tests may incur charges or load a model.".into()))
                    }
                } else {
                    (
                        StatusState::Error,
                        Some("Selected inference connection is unavailable.".into()),
                    )
                }
            }
        };
        InferenceStatus {
            state,
            selection: config.selection,
            revision: config.revision,
            message,
        }
    }
    pub fn discover_draft(&self, input: ConnectionInput) -> Result<Vec<DiscoveredModel>, String> {
        if input.id.is_empty()
            || input.id.len() > 128
            || !input
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
            || input.name.len() > 1024
            || input.model.len() > 1024
            || input.base_url.len() > 8192
            || input.secret.as_ref().is_some_and(|s| s.len() > 8192)
        {
            return Err("Invalid discovery input or input exceeds size limit.".into());
        }
        if input.secret.is_some() && input.remove_secret {
            return Err("Cannot supply and remove a credential together.".into());
        }
        if input
            .secret
            .as_ref()
            .is_some_and(|s| s.is_empty() || s.contains(['\r', '\n']))
        {
            return Err("Credential must be nonempty and contain no newlines.".into());
        }
        if input.provider == Provider::Google && input.protocol != Protocol::OpenaiChatCompletions {
            return Err("Google preset uses its documented Chat Completions endpoint.".into());
        }
        let c = InferenceConnection {
            id: input.id,
            name: input.name,
            provider: input.provider,
            base_url: connection_base_url(input.provider, &input.base_url),
            protocol: input.protocol,
            auth: input.auth,
            model: input.model,
            stream: input.stream,
            allow_insecure_lan: false,
            consent_remote: input.consent_remote,
            credential_present: false,
            revision: 0,
        };
        let endpoint = config::endpoint(&c, "models")?;
        let mut revoked = Arc::new(transport::SessionControl::default());
        let secret = if c.auth == AuthMode::None {
            None
        } else if let Some(secret) = input.secret {
            Some(secret)
        } else {
            let state = self
                .state
                .lock()
                .map_err(|_| "Inference state unavailable.")?;
            let old = state
                .as_ref()
                .map_err(Clone::clone)?
                .connections
                .iter()
                .find(|old| {
                    old.id == c.id
                        && old.provider == c.provider
                        && old.protocol == c.protocol
                        && old.auth == c.auth
                        && old.credential_present
                        && !input.remove_secret
                        && config::endpoint(old, "models")
                            .is_ok_and(|old_endpoint| old_endpoint == endpoint)
                })
                .ok_or("Supply a new API credential for this draft endpoint.")?;
            let secret = self.secret(old)?;
            revoked = self.revocation(old);
            secret
        };
        transport::discover(c, secret, revoked)
    }
    pub fn discover(&self, id: &str) -> Result<Vec<DiscoveredModel>, String> {
        let state = self
            .state
            .lock()
            .map_err(|_| "Inference state unavailable.")?;
        let c = state
            .as_ref()
            .map_err(Clone::clone)?
            .connections
            .iter()
            .find(|c| c.id == id)
            .cloned()
            .ok_or("Inference connection not found.")?;
        let secret = self.secret(&c)?;
        let revoked = self.revocation(&c);
        drop(state);
        transport::discover(c, secret, revoked)
    }
    pub fn test(&self, id: &str, model: &str) -> Result<InferenceStatus, String> {
        if model.trim().is_empty() {
            return Err("Choose a model to test.".into());
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_| "Inference state unavailable.")?;
        let config = state.as_ref().map_err(Clone::clone)?;
        let c = config
            .connections
            .iter()
            .find(|c| c.id == id)
            .cloned()
            .ok_or("Inference connection not found.")?;
        let key = (id.to_owned(), model.to_owned(), c.revision);
        let mut attempts = self
            .test_attempts
            .lock()
            .map_err(|_| "Inference tests unavailable.")?;
        let attempt = attempts
            .get(&key)
            .copied()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or("Inference test revision overflow.")?;
        let mut next = config.clone();
        next.verified
            .retain(|tested| !(tested.connection_id == id && tested.model == model));
        // Revoke the previous success durably before a retry can fail or be interrupted.
        if next.verified.len() != config.verified.len() {
            config::save(&self.data_dir, &next)?;
            *state = Ok(next);
        }
        attempts.insert(key.clone(), attempt);
        drop(attempts);
        let session = transport::HttpSession {
            connection: c.clone(),
            model: model.into(),
            secret: self.secret(&c)?,
            revoked: self.revocation(&c),
        };
        drop(state);
        let messages = [ChatMessage {
            role: Role::User,
            content: "Reply with the single word OK. This is a synthetic connectivity test.".into(),
        }];
        let request = GenerationRequest {
            messages: &messages,
            stable_prefix: 0,
            max_new_tokens: 32,
            deadline: Instant::now() + Duration::from_secs(30),
        };
        let result = session.generate(&request, &CancelToken::default(), &mut |_| {});
        let mut state_guard = self
            .state
            .lock()
            .map_err(|_| "Inference state unavailable.")?;
        let current = state_guard.as_ref().map_err(Clone::clone)?;
        if !current
            .connections
            .iter()
            .any(|item| item.id == id && item.revision == c.revision)
            || session.revoked.is_revoked()
            || self
                .test_attempts
                .lock()
                .map_err(|_| "Inference tests unavailable.")?
                .get(&key)
                != Some(&attempt)
        {
            return Err("Connection changed or test superseded while testing. Test the current configuration.".into());
        }
        let revision = current.revision;
        let (state, message) = match result {
            Ok(generation) if generation.finish == promptify_core::pipeline::FinishReason::Stop => {
                let mut next = current.clone();
                next.verified.push(VerifiedInference {
                    connection_id: id.into(),
                    model: model.into(),
                    revision: c.revision,
                });
                config::save(&self.data_dir, &next)?;
                *state_guard = Ok(next);
                (StatusState::Ready, None)
            }
            Ok(_) => (
                StatusState::Error,
                Some(
                    "Test output hit the token limit; generation readiness was not verified."
                        .into(),
                ),
            ),
            Err(error) => (StatusState::Error, Some(error.0)),
        };
        Ok(InferenceStatus {
            state,
            selection: InferenceSelection::Connection {
                connection_id: id.into(),
                model: model.into(),
            },
            revision,
            message,
        })
    }
    pub fn device(&self) -> Option<String> {
        if matches!(
            self.config().ok()?.selection,
            InferenceSelection::BundledLocal
        ) {
            self.local().device()
        } else {
            None
        }
    }
    pub fn preload(&self) -> Result<(), BackendError> {
        match self.config().map_err(BackendError)?.selection {
            InferenceSelection::BundledLocal => self.local().preload(),
            InferenceSelection::Connection { .. } => {
                let state = self
                    .state
                    .lock()
                    .map_err(|_| BackendError("Inference state unavailable.".into()))?;
                let config = state
                    .as_ref()
                    .map_err(|error| BackendError(error.clone()))?;
                let InferenceSelection::Connection {
                    connection_id,
                    model,
                } = &config.selection
                else {
                    return Err(BackendError(
                        "Inference selection changed while preparing. Retry.".into(),
                    ));
                };
                let connection = config
                    .connections
                    .iter()
                    .find(|connection| &connection.id == connection_id)
                    .ok_or_else(|| {
                        BackendError("Selected inference connection unavailable.".into())
                    })?;
                if !config.is_verified(connection, model) {
                    return Err(BackendError("Inference is configured but not verified. Explicitly test the selected model in Models > Prompt writer; tests may incur charges or load a model.".into()));
                }
                self.secret(connection).map_err(BackendError).map(|_| ())
            }
        }
    }
    pub fn recover(&self, replacement: InferenceConfig) -> Result<InferenceConfig, String> {
        config::validate(&replacement)?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| "Inference state unavailable.")?;
        if state.is_ok() {
            return Err("Recovery is only available for invalid inference configuration.".into());
        }
        config::save(&self.data_dir, &replacement)?;
        *state = Ok(replacement.clone());
        Ok(replacement)
    }

    /// Explicit recovery action, preserving the previous sidecar as inference.json.bak.
    pub fn reset(&self) -> Result<InferenceConfig, String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "Inference state unavailable.")?;
        let old = state.as_ref().ok().cloned();
        let mut replacement = InferenceConfig::default();
        if let Some(old) = &old {
            replacement.revision = old
                .revision
                .checked_add(1)
                .ok_or("Inference revision overflow.")?;
        }
        config::save(&self.data_dir, &replacement)?;
        *state = Ok(replacement.clone());
        for sessions in self.revocations.lock().unwrap().values() {
            for session in sessions.iter().filter_map(Weak::upgrade) {
                session.revoke();
            }
        }
        let mut cleanup_failed = false;
        if let Some(old) = old {
            for connection in old.connections.iter().filter(|c| c.credential_present) {
                cleanup_failed |= self.credentials.remove(&self.key(connection)).is_err();
            }
        }
        if cleanup_failed {
            return Err("Inference reset and sessions revoked, but OS credential cleanup failed. Retry credential cleanup before adding connections.".into());
        }
        Ok(replacement)
    }
}
impl Generator for InferenceManager {
    fn snapshot(&self) -> Result<Option<Arc<dyn Generator>>, BackendError> {
        self.session().map(Some)
    }
    fn generate(
        &self,
        request: &GenerationRequest<'_>,
        cancel: &CancelToken,
        on_token: &mut dyn FnMut(&str),
    ) -> Result<Generation, BackendError> {
        self.session()?.generate(request, cancel, on_token)
    }
}
#[cfg(test)]
mod tests;

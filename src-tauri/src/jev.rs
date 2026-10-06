//! Opt-in, experimental review. Only the request and generated draft leave the device.
use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use promptify_core::pipeline::{BackendError, CancelToken};
use promptify_core::review::{PromptReviewer, ReviewDecision, ReviewRequest};
use serde::{Deserialize, Serialize};
use tauri::Manager;

use crate::settings::{self, SharedSettings};

const ENDPOINT: &str = "https://api.typesafe.ai/v1/systemone";
const MAX_RESPONSE: u64 = 64 * 1024;
const QUESTIONS: [(&str, &str, &str); 4] = [
    (
        "fidelity",
        "Probability that the draft faithfully preserves and usefully expands the user's request without changing its goal. You see only the current request and draft, not prior conversation. Missing prior context is not proof of fabricated detail; conditional instructions to use context if available are not factual claims.",
        "Preserve the user's intent and requested outcome.",
    ),
    (
        "constraints",
        "Probability that the draft avoids invented facts, unrelated tasks, and unjustified constraints. You see no prior conversation: absent context is not evidence that a detail is fabricated. Distinguish unsupported factual claims from conditional instructions to consult context if available.",
        "Remove invented facts, unrelated tasks, and unjustified constraints.",
    ),
    (
        "steps",
        "Probability that the draft's task graph has useful actionable steps with explicit dependencies. Task graphs are mandatory even for simple questions: never penalize a draft merely for expanding a simple request.",
        "Make graph steps useful and dependencies explicit while preserving the mandatory task graph.",
    ),
    (
        "completion",
        "Probability that the draft includes a meaningful bounded refinement loop and verifiable completion criteria that improve the requested result, not arbitrary checks. Task graphs and bounded loops are mandatory even for simple requests.",
        "Make the bounded refinement loop and verifiable completion criteria meaningful for the request.",
    ),
];

trait Credentials: Send + Sync {
    fn get(&self) -> Result<Option<String>, String>;
    fn save(&self, key: &str) -> Result<(), String>;
    fn delete(&self) -> Result<(), String>;
}

struct OsCredentials;

impl OsCredentials {
    fn entry(&self) -> Result<keyring::Entry, String> {
        keyring::Entry::new(settings::APP_IDENTIFIER, "jev-api-key")
            .map_err(|_| "Could not access the OS credential store.".into())
    }
}

impl Credentials for OsCredentials {
    fn get(&self) -> Result<Option<String>, String> {
        match self.entry()?.get_password() {
            Ok(key) => Ok(Some(key)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err("Could not read the Jev key from the OS credential store. Unlock your keychain and retry.".into()),
        }
    }

    fn save(&self, key: &str) -> Result<(), String> {
        self.entry()?.set_password(key)
            .map_err(|_| "Could not save the Jev key in the OS credential store. Unlock your keychain and retry.".into())
    }

    fn delete(&self) -> Result<(), String> {
        match self.entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err("Jev is disabled, but its key could not be removed from the OS credential store. Unlock your keychain and retry.".into()),
        }
    }
}

#[derive(Clone, Serialize)]
pub struct JevStatus {
    enabled: bool,
    key_configured: Option<bool>,
    credential_error: Option<String>,
}

enum Action {
    Status,
    Save(String),
    Delete,
    Enable(bool),
}

fn manage(
    settings: &SharedSettings,
    data_dir: &Path,
    credentials: &dyn Credentials,
    action: Action,
) -> Result<JevStatus, String> {
    // The settings lock serializes credential edits and enablement with reviewer snapshots.
    let mut current = settings
        .write()
        .map_err(|_| "Jev settings lock is unavailable.")?;
    match action {
        Action::Status => {}
        Action::Save(key) => {
            if key.is_empty() || key.len() > 4096 || !key.bytes().all(|b| (33..=126).contains(&b)) {
                return Err("Enter a non-empty API key without spaces or control characters (at most 4096 bytes).".into());
            }
            credentials.save(&key)?;
        }
        Action::Delete => {
            let mut next = current.clone();
            next.jev_enabled = false;
            settings::save(data_dir, &next)
                .map_err(|_| "Could not save Jev settings. The key was not removed.")?;
            *current = next;
            credentials.delete()?;
        }
        Action::Enable(enabled) => {
            if enabled && credentials.get()?.is_none() {
                return Err("Save a Jev API key before enabling remote review.".into());
            }
            let mut next = current.clone();
            next.jev_enabled = enabled;
            settings::save(data_dir, &next)
                .map_err(|_| "Could not save Jev settings. The setting was not changed.")?;
            *current = next;
        }
    }
    let (key_configured, credential_error) = match credentials.get() {
        Ok(key) => (Some(key.is_some()), None),
        Err(message) => (None, Some(message)),
    };
    Ok(JevStatus {
        enabled: current.jev_enabled,
        key_configured,
        credential_error,
    })
}

async fn command(app: tauri::AppHandle, action: Action) -> Result<JevStatus, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<crate::AppState>();
        manage(&state.settings, &state.data_dir, &OsCredentials, action)
    })
    .await
    .map_err(|_| "Jev settings operation could not complete.".to_string())?
}

#[tauri::command]
pub async fn jev_status(app: tauri::AppHandle) -> Result<JevStatus, String> {
    command(app, Action::Status).await
}

#[tauri::command]
pub async fn save_jev_key(app: tauri::AppHandle, key: String) -> Result<JevStatus, String> {
    command(app, Action::Save(key)).await
}

#[tauri::command]
pub async fn delete_jev_key(app: tauri::AppHandle) -> Result<JevStatus, String> {
    command(app, Action::Delete).await
}

#[tauri::command]
pub async fn set_jev_enabled(app: tauri::AppHandle, enabled: bool) -> Result<JevStatus, String> {
    command(app, Action::Enable(enabled)).await
}

pub struct JevReviewer {
    settings: SharedSettings,
    credentials: Arc<dyn Credentials>,
}

impl JevReviewer {
    pub fn new(settings: SharedSettings) -> Self {
        Self {
            settings,
            credentials: Arc::new(OsCredentials),
        }
    }

    fn run(
        &self,
        request: &ReviewRequest<'_>,
        cancel: &CancelToken,
        endpoint: &str,
    ) -> Result<ReviewDecision, BackendError> {
        let key = {
            let settings = self
                .settings
                .read()
                .map_err(|_| error("Jev settings lock is unavailable."))?;
            if !settings.jev_enabled {
                return Ok(ReviewDecision::Skipped);
            }
            check_active(request.deadline, cancel)?;
            self.credentials.get().map_err(error)?.ok_or_else(|| {
                error("Jev review is enabled but no API key is stored. Save a key or disable Jev.")
            })?
        };
        check_active(request.deadline, cancel)?;
        let timeout = request
            .deadline
            .saturating_duration_since(Instant::now())
            .min(Duration::from_secs(10));
        let client = reqwest::blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(timeout)
            .build()
            .map_err(|_| error("Could not initialize the Jev HTTPS client."))?;
        let questions: BTreeMap<_, _> = QUESTIONS
            .iter()
            .map(|(key, instructions, _)| {
                (
                    *key,
                    serde_json::json!({ "type": "noul", "instructions": instructions }),
                )
            })
            .collect();
        let body = serde_json::to_vec(&serde_json::json!({
            "model": "jev-latest",
            "state": { "request": request.transcript, "draft": request.draft },
            "questions": questions,
        }))
        .map_err(|_| error("Could not encode the Jev review request."))?;
        check_active(request.deadline, cancel)?;
        let result = (|| {
            let mut response = client
                .post(endpoint)
                .bearer_auth(key)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body)
                .send()
                .map_err(network_error)?;
            match response.status().as_u16() {
                200..=299 => {}
                401 => {
                    return Err(error(
                        "Jev authentication failed (HTTP 401). TypeSafe did not accept the stored credential.",
                    ));
                }
                403 => {
                    return Err(error(
                        "Jev access denied (HTTP 403). Check TypeSafe account and API access permissions; this does not necessarily mean the key is wrong.",
                    ));
                }
                429 => {
                    return Err(error(
                        "Jev rate limit reached. Retry later or disable remote review.",
                    ));
                }
                300..=399 => {
                    return Err(error("Jev returned a redirect; redirects are not allowed."));
                }
                500..=599 => return Err(error("Jev is temporarily unavailable. Retry later.")),
                _ => return Err(error("Jev rejected the review request.")),
            }
            if response
                .content_length()
                .is_some_and(|size| size > MAX_RESPONSE)
            {
                return Err(error("Jev response exceeded the size limit."));
            }
            let mut bytes = Vec::new();
            response
                .by_ref()
                .take(MAX_RESPONSE + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| error("Could not read the Jev response within its timeout."))?;
            if bytes.len() as u64 > MAX_RESPONSE {
                return Err(error("Jev response exceeded the size limit."));
            }
            decode(&bytes)
        })();
        check_active(request.deadline, cancel)?;
        if !self
            .settings
            .read()
            .map_err(|_| error("Jev settings lock is unavailable."))?
            .jev_enabled
        {
            return Ok(ReviewDecision::Skipped);
        }
        result
    }
}

impl PromptReviewer for JevReviewer {
    fn enabled(&self) -> bool {
        // A poisoned lock must reach review's explicit error, not silently skip review.
        self.settings
            .read()
            .map(|settings| settings.jev_enabled)
            .unwrap_or(true)
    }

    fn review(
        &self,
        request: &ReviewRequest<'_>,
        cancel: &CancelToken,
    ) -> Result<ReviewDecision, BackendError> {
        self.run(request, cancel, ENDPOINT)
    }
}

fn error(message: impl Into<String>) -> BackendError {
    BackendError(message.into())
}

fn check_active(deadline: Instant, cancel: &CancelToken) -> Result<(), BackendError> {
    if cancel.is_cancelled() {
        return Err(error("Jev review was cancelled."));
    }
    if Instant::now() >= deadline {
        return Err(error("Jev review deadline expired."));
    }
    Ok(())
}

fn network_error(err: reqwest::Error) -> BackendError {
    error(if err.is_timeout() {
        "Jev review timed out."
    } else {
        "Could not connect securely to Jev. Check your connection."
    })
}

#[derive(Deserialize)]
struct Response {
    model: String,
    answers: Answers,
    usage: serde_json::Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Answers {
    fidelity: Answer,
    constraints: Answer,
    steps: Answer,
    completion: Answer,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Answer {
    #[serde(rename = "type")]
    kind: String,
    noul: f64,
}

fn decode(bytes: &[u8]) -> Result<ReviewDecision, BackendError> {
    let invalid = || error("Jev returned an invalid review response.");
    let response: Response = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    if response.model.is_empty() || !response.usage.is_object() {
        return Err(invalid());
    }
    let mut approved = true;
    let mut issues = Vec::new();
    let mut scores = Vec::new();
    for ((name, _, issue), answer) in QUESTIONS.into_iter().zip([
        &response.answers.fidelity,
        &response.answers.constraints,
        &response.answers.steps,
        &response.answers.completion,
    ]) {
        if answer.kind != "noul" || !answer.noul.is_finite() || !(0.0..=1.0).contains(&answer.noul)
        {
            return Err(invalid());
        }
        approved &= answer.noul >= 0.85;
        scores.push(format!("{name}: {}", answer.noul));
        if answer.noul <= 0.2 {
            issues.push(issue.to_string());
        }
    }
    if !issues.is_empty() {
        Ok(ReviewDecision::Revise { issues })
    } else if approved {
        Ok(ReviewDecision::Approved)
    } else {
        Ok(ReviewDecision::Uncertain { detail: format!(
            "Jev connected and completed the review, but approval was inconclusive. Scores (0-1): {}. \
             Approval requires every check to reach 0.85; a check at or below 0.20 requests a rewrite. \
             These experimental thresholds are not calibrated quality grades. Review and copy the draft if suitable; it was not pasted.",
            scores.join(", ")
        ) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;
    use std::sync::{Mutex, RwLock};

    #[derive(Default)]
    struct TestCredentials {
        key: Mutex<Option<String>>,
        delete_fails: bool,
        read_fails: bool,
    }

    impl Credentials for TestCredentials {
        fn get(&self) -> Result<Option<String>, String> {
            if self.read_fails {
                return Err("OS credential store locked.".into());
            }
            Ok(self.key.lock().unwrap().clone())
        }
        fn save(&self, key: &str) -> Result<(), String> {
            *self.key.lock().unwrap() = Some(key.into());
            Ok(())
        }
        fn delete(&self) -> Result<(), String> {
            if self.delete_fails {
                return Err("Mock credential removal failed.".into());
            }
            *self.key.lock().unwrap() = None;
            Ok(())
        }
    }

    fn settings(enabled: bool) -> SharedSettings {
        Arc::new(RwLock::new(settings::AppSettings {
            jev_enabled: enabled,
            ..Default::default()
        }))
    }

    fn reviewer(enabled: bool) -> JevReviewer {
        JevReviewer {
            settings: settings(enabled),
            credentials: Arc::new(TestCredentials {
                key: Mutex::new(Some("synthetic-secret".into())),
                delete_fails: false,
                read_fails: false,
            }),
        }
    }

    fn request() -> ReviewRequest<'static> {
        ReviewRequest {
            transcript: "Suggest more ways to improve prompt quality",
            draft: "Synthetic task graph",
            deadline: Instant::now() + Duration::from_secs(3),
        }
    }

    fn response(probability: f64) -> serde_json::Value {
        serde_json::json!({
            "model": "jev-latest",
            "usage": {},
            "answers": QUESTIONS.iter().map(|(key, _, _)| (*key, serde_json::json!({"type":"noul", "noul":probability}))).collect::<BTreeMap<_, _>>()
        })
    }

    fn decode_value(value: serde_json::Value) -> Result<ReviewDecision, BackendError> {
        decode(&serde_json::to_vec(&value).unwrap())
    }

    // This endpoint exists only inside unit tests and only binds loopback.
    fn mock(
        status: u16,
        body: String,
        after_request: impl FnOnce() + Send + 'static,
    ) -> (String, std::thread::JoinHandle<serde_json::Value>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/review", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut bytes = Vec::new();
            let mut buffer = [0; 2048];
            let (headers_end, length) = loop {
                let read = stream.read(&mut buffer).unwrap();
                assert_ne!(read, 0);
                bytes.extend_from_slice(&buffer[..read]);
                if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..end]).to_lowercase();
                    assert!(headers.starts_with("post /review "));
                    assert!(headers.contains("authorization: bearer synthetic-secret\r\n"));
                    assert!(!headers.contains("authorization: synthetic-secret\r\n"));
                    let length = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length: "))
                        .unwrap()
                        .parse::<usize>()
                        .unwrap();
                    break (end + 4, length);
                }
            };
            while bytes.len() < headers_end + length {
                let read = stream.read(&mut buffer).unwrap();
                assert_ne!(read, 0);
                bytes.extend_from_slice(&buffer[..read]);
            }
            let payload =
                serde_json::from_slice(&bytes[headers_end..headers_end + length]).unwrap();
            after_request();
            write!(
                stream,
                "HTTP/1.1 {status} Mock\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            payload
        });
        (endpoint, handle)
    }

    #[test]
    fn jev_thresholds_are_conservative_and_issues_are_owned() {
        assert!(matches!(
            decode_value(response(0.85)).unwrap(),
            ReviewDecision::Approved
        ));
        assert!(matches!(
            decode_value(response(0.849)).unwrap(),
            ReviewDecision::Uncertain { .. }
        ));
        assert!(matches!(
            decode_value(response(0.201)).unwrap(),
            ReviewDecision::Uncertain { .. }
        ));
        let mut value = response(1.0);
        value["answers"]["constraints"]["noul"] = serde_json::json!(0.2);
        let ReviewDecision::Revise { issues } = decode_value(value).unwrap() else {
            panic!("expected revise")
        };
        assert_eq!(issues, vec![QUESTIONS[1].2]);
        assert!(QUESTIONS[2].1.contains("never penalize"));
        assert!(
            QUESTIONS[0]
                .1
                .contains("Missing prior context is not proof")
        );
        assert!(QUESTIONS[1].1.contains("conditional instructions"));
    }

    #[test]
    fn jev_inconclusive_review_shows_scores_without_claiming_connection_failure() {
        let mut value = response(0.91);
        value["answers"]["completion"]["noul"] = serde_json::json!(0.8499);
        let ReviewDecision::Uncertain { detail } = decode_value(value).unwrap() else {
            panic!("expected inconclusive review");
        };
        assert!(detail.contains("connected and completed"));
        for check in ["fidelity: 0.91", "constraints: 0.91", "steps: 0.91", "completion: 0.8499"] {
            assert!(detail.contains(check));
        }
        assert!(detail.contains("every check to reach 0.85"));
        assert!(detail.contains("not calibrated quality grades"));
        assert!(detail.contains("it was not pasted"));
    }

    #[test]
    fn jev_rejects_malformed_and_incomplete_probabilities_without_echoing() {
        for invalid in [
            serde_json::json!(-0.1),
            serde_json::json!(1.1),
            serde_json::json!(null),
            serde_json::json!("synthetic-secret"),
        ] {
            let mut value = response(1.0);
            value["answers"]["fidelity"]["noul"] = invalid;
            assert_eq!(
                decode_value(value).err().unwrap().0,
                "Jev returned an invalid review response."
            );
        }
        for key in ["fidelity", "constraints", "steps", "completion"] {
            let mut value = response(1.0);
            value["answers"].as_object_mut().unwrap().remove(key);
            assert!(decode_value(value).is_err());
        }
        let mut value = response(1.0);
        value["answers"]["steps"]["type"] = serde_json::json!("text");
        assert!(decode_value(value).is_err());
        let mut value = response(1.0);
        value["answers"]["extra"] = serde_json::json!({"type":"noul","noul":1});
        assert!(decode_value(value).is_err());
        assert!(decode(b"{\"answers\":\"synthetic-secret\"}").is_err());
        assert!(decode(b"{\"answers\":{\"fidelity\":{\"type\":\"noul\",\"noul\":NaN}}}").is_err());
    }

    #[test]
    fn jev_posts_only_request_and_draft_using_bearer_auth() {
        let (endpoint, server) = mock(200, response(1.0).to_string(), || {});
        assert!(matches!(
            reviewer(true)
                .run(&request(), &CancelToken::default(), &endpoint)
                .unwrap(),
            ReviewDecision::Approved
        ));
        let payload = server.join().unwrap();
        assert_eq!(payload["model"], "jev-latest");
        assert_eq!(
            payload["state"],
            serde_json::json!({"request":request().transcript,"draft":request().draft})
        );
        assert_eq!(payload["questions"].as_object().unwrap().len(), 4);
        assert_eq!(payload.as_object().unwrap().len(), 3);
    }

    #[test]
    fn jev_classifies_http_errors_and_never_echoes_remote_body_or_key() {
        for (status, expected) in [
            (401, "authentication failed (HTTP 401)"),
            (403, "access denied (HTTP 403)"),
            (429, "rate limit"),
            (503, "temporarily"),
            (302, "redirect"),
            (400, "rejected"),
        ] {
            let (endpoint, server) =
                mock(status, "remote-private-body synthetic-secret".into(), || {});
            let message = reviewer(true)
                .run(&request(), &CancelToken::default(), &endpoint)
                .err()
                .unwrap()
                .0;
            assert!(message.contains(expected));
            assert!(!message.contains("remote-private-body"));
            assert!(!message.contains("synthetic-secret"));
            server.join().unwrap();
        }
    }

    #[test]
    fn jev_bounds_responses_and_honors_cancellation_after_network() {
        let (endpoint, server) = mock(200, "x".repeat(MAX_RESPONSE as usize + 1), || {});
        assert!(
            reviewer(true)
                .run(&request(), &CancelToken::default(), &endpoint)
                .err()
                .unwrap()
                .0
                .contains("size limit")
        );
        server.join().unwrap();
        let cancel = CancelToken::default();
        let other = cancel.clone();
        let (endpoint, server) = mock(200, response(1.0).to_string(), move || other.cancel());
        assert!(
            reviewer(true)
                .run(&request(), &cancel, &endpoint)
                .err()
                .unwrap()
                .0
                .contains("cancelled")
        );
        server.join().unwrap();
    }

    #[test]
    fn jev_disabled_cancelled_or_expired_never_contacts_network() {
        let endpoint = "http://127.0.0.1:1";
        assert!(matches!(
            reviewer(false)
                .run(&request(), &CancelToken::default(), endpoint)
                .unwrap(),
            ReviewDecision::Skipped
        ));
        let cancel = CancelToken::default();
        cancel.cancel();
        assert!(
            reviewer(true)
                .run(&request(), &cancel, endpoint)
                .err()
                .unwrap()
                .0
                .contains("cancelled")
        );
        let mut expired = request();
        expired.deadline = Instant::now();
        assert!(
            reviewer(true)
                .run(&expired, &CancelToken::default(), endpoint)
                .err()
                .unwrap()
                .0
                .contains("deadline")
        );
        let no_key = JevReviewer {
            settings: settings(true),
            credentials: Arc::new(TestCredentials::default()),
        };
        assert!(
            no_key
                .run(&request(), &CancelToken::default(), endpoint)
                .err()
                .unwrap()
                .0
                .contains("no API key")
        );
    }

    #[test]
    fn jev_disabled_during_inflight_review_discards_remote_decision() {
        for status in [200, 401] {
            let reviewer = reviewer(true);
            let settings = reviewer.settings.clone();
            let (endpoint, server) = mock(status, response(1.0).to_string(), move || {
                settings.write().unwrap().jev_enabled = false;
            });
            assert!(matches!(
                reviewer
                    .run(&request(), &CancelToken::default(), &endpoint)
                    .unwrap(),
                ReviewDecision::Skipped
            ));
            server.join().unwrap();
            assert!(!reviewer.enabled());
        }
    }

    #[test]
    fn jev_key_storage_and_opt_in_are_separate_and_transactional() {
        let dir = tempfile::tempdir().unwrap();
        let settings = settings(false);
        let credentials = TestCredentials::default();
        assert!(manage(&settings, dir.path(), &credentials, Action::Enable(true)).is_err());
        let status = manage(
            &settings,
            dir.path(),
            &credentials,
            Action::Save("synthetic-secret".into()),
        )
        .unwrap();
        assert_eq!(status.key_configured, Some(true));
        assert!(!status.enabled);
        assert!(!dir.path().join("settings.json").exists());
        assert!(
            manage(&settings, dir.path(), &credentials, Action::Enable(true))
                .unwrap()
                .enabled
        );
        let saved = std::fs::read_to_string(dir.path().join("settings.json")).unwrap();
        assert!(!saved.contains("synthetic-secret"));
        assert!(!saved.contains("api_key"));
        let blocked = dir.path().join("not-a-directory");
        std::fs::write(&blocked, "blocked").unwrap();
        assert!(manage(&settings, &blocked, &credentials, Action::Enable(false)).is_err());
        assert!(settings.read().unwrap().jev_enabled);
        assert!(manage(&settings, &blocked, &credentials, Action::Delete).is_err());
        assert!(credentials.get().unwrap().is_some());
        let status = manage(&settings, dir.path(), &credentials, Action::Delete).unwrap();
        assert!(!status.enabled);
        assert_eq!(status.key_configured, Some(false));
        assert!(
            !settings::load_checked(dir.path())
                .unwrap()
                .unwrap()
                .jev_enabled
        );
        assert!(!settings::AppSettings::default().jev_enabled);
    }

    #[test]
    fn jev_can_be_disabled_when_credential_store_is_unavailable() {
        let dir = tempfile::tempdir().unwrap();
        let settings = settings(true);
        let credentials = TestCredentials { read_fails: true, ..Default::default() };
        let status = manage(&settings, dir.path(), &credentials, Action::Status).unwrap();
        assert!(status.enabled);
        assert_eq!(status.key_configured, None);
        assert_eq!(status.credential_error.as_deref(), Some("OS credential store locked."));
        let status = manage(&settings, dir.path(), &credentials, Action::Enable(false)).unwrap();
        assert!(!status.enabled);
        assert!(status.credential_error.is_some());
        assert!(!settings::load_checked(dir.path()).unwrap().unwrap().jev_enabled);
        assert!(manage(&settings, dir.path(), &credentials, Action::Enable(true)).is_err());
    }

    #[test]
    fn jev_failed_key_deletion_still_persists_disable() {
        let dir = tempfile::tempdir().unwrap();
        let settings = settings(true);
        let credentials = TestCredentials {
            key: Mutex::new(Some("synthetic-secret".into())),
            delete_fails: true,
            read_fails: false,
        };
        assert!(manage(&settings, dir.path(), &credentials, Action::Delete).is_err());
        assert!(!settings.read().unwrap().jev_enabled);
        assert!(
            !settings::load_checked(dir.path())
                .unwrap()
                .unwrap()
                .jev_enabled
        );
        assert!(credentials.get().unwrap().is_some());
    }
}

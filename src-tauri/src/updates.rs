use std::cmp::Ordering;
use std::io::Read;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::update_install::{Installer, ReleaseAsset};
use reqwest::StatusCode;
use reqwest::blocking::Client;
use semver::Version;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_opener::OpenerExt;

const LATEST_RELEASE: &str = "https://api.github.com/repos/jrynks/promptify/releases/latest";
const RELEASE_PREFIX: &str = "https://github.com/jrynks/promptify/releases/tag/";
const SUCCESS_INTERVAL: Duration = Duration::from_secs(3600);
const ERROR_INTERVAL: Duration = Duration::from_secs(60);
const MAX_RESPONSE_BYTES: u64 = 256 * 1024;

#[derive(Clone, Debug, Serialize)]
pub struct ReleaseInfo {
    pub version: String,
    pub url: String,
    pub update_available: bool,
    pub install_error: Option<String>,
    #[serde(skip)]
    pub installer: Option<Installer>,
}

#[derive(Clone, Debug, Serialize)]
pub struct UpdateInfo {
    pub current_version: String,
    pub check_on_startup: bool,
    pub checking: bool,
    pub checked: bool,
    pub release: Option<ReleaseInfo>,
    pub error: Option<String>,
    pub installation: &'static str,
    pub installation_error: Option<String>,
}

#[derive(Default)]
struct Cache {
    checking: bool,
    checked: bool,
    release: Option<ReleaseInfo>,
    error: Option<String>,
    next_check: Option<Instant>,
    installing: bool,
    installation: Option<&'static str>,
    installation_error: Option<String>,
}

#[derive(Default)]
pub struct Updates(Mutex<Cache>);

#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    html_url: String,
    draft: bool,
    prerelease: bool,
    #[serde(default)]
    assets: Vec<ReleaseAsset>,
}

fn parse_release(body: &str, current: &str) -> Result<ReleaseInfo, String> {
    let release: GitHubRelease = serde_json::from_str(body)
        .map_err(|error| format!("GitHub returned invalid release metadata: {error}"))?;
    if release.draft || release.prerelease {
        return Err("GitHub's latest release is not a published stable release.".into());
    }
    let version = Version::parse(
        release
            .tag_name
            .strip_prefix('v')
            .unwrap_or(&release.tag_name),
    )
    .map_err(|error| format!("GitHub release tag is not a semantic version: {error}"))?;
    if !version.pre.is_empty() {
        return Err("GitHub's latest release tag identifies a prerelease.".into());
    }
    let installed = Version::parse(current)
        .map_err(|error| format!("Installed application version is invalid: {error}"))?;
    let expected_url = format!("{RELEASE_PREFIX}{}", release.tag_name);
    if release.html_url != expected_url {
        return Err("GitHub returned an unexpected release URL.".into());
    }
    let (installer, install_error) =
        match Installer::select(&release.assets, &version.to_string(), &release.tag_name) {
            Ok(installer) => (Some(installer), None),
            Err(error) => (None, Some(error)),
        };
    Ok(ReleaseInfo {
        version: version.to_string(),
        url: expected_url,
        update_available: version.cmp_precedence(&installed) == Ordering::Greater,
        install_error,
        installer,
    })
}

fn response_result(
    status: StatusCode,
    body: &str,
    current: &str,
) -> Result<Option<ReleaseInfo>, String> {
    match status {
        StatusCode::OK => parse_release(body, current).map(Some),
        StatusCode::NOT_FOUND => Ok(None),
        StatusCode::FORBIDDEN | StatusCode::TOO_MANY_REQUESTS => Err(
            "GitHub refused the update check (access denied or rate limited). Try again later."
                .into(),
        ),
        _ => Err(format!("GitHub update check failed (HTTP {status}).")),
    }
}

fn fetch_release(current: &str) -> Result<Option<ReleaseInfo>, String> {
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| format!("Could not initialize update checking: {error}"))?;
    let response = client
        .get(LATEST_RELEASE)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header("User-Agent", format!("Promptify/{current}"))
        .send()
        .map_err(|error| format!("Could not contact GitHub for updates: {error}"))?;
    let status = response.status();
    // Error pages and 404 responses need no body to classify.
    if status != StatusCode::OK {
        return response_result(status, "", current);
    }
    let mut body = String::new();
    response
        .take(MAX_RESPONSE_BYTES + 1)
        .read_to_string(&mut body)
        .map_err(|error| format!("Could not read GitHub release metadata: {error}"))?;
    if body.len() as u64 > MAX_RESPONSE_BYTES {
        return Err("GitHub release metadata exceeds the update check size limit.".into());
    }
    response_result(status, &body, current)
}

impl Updates {
    fn info(&self, check_on_startup: bool) -> UpdateInfo {
        let cache = self.0.lock().unwrap();
        UpdateInfo {
            current_version: env!("CARGO_PKG_VERSION").into(),
            check_on_startup,
            checking: cache.checking,
            checked: cache.checked,
            release: cache.release.clone(),
            error: cache.error.clone(),
            installation: cache.installation.unwrap_or("idle"),
            installation_error: cache.installation_error.clone(),
        }
    }

    fn begin(&self, now: Instant) -> bool {
        let mut cache = self.0.lock().unwrap();
        if cache.checking || cache.installing || cache.next_check.is_some_and(|next| now < next) {
            return false;
        }
        cache.checking = true;
        cache.error = None;
        true
    }

    fn finish(&self, now: Instant, result: Result<Option<ReleaseInfo>, String>) {
        let mut cache = self.0.lock().unwrap();
        cache.checking = false;
        match result {
            Ok(release) => {
                cache.checked = true;
                cache.release = release;
                cache.error = None;
                cache.next_check = Some(now + SUCCESS_INTERVAL);
            }
            Err(error) => {
                log::warn!("update check: {error}");
                cache.error = Some(error);
                cache.next_check = Some(now + ERROR_INTERVAL);
            }
        }
    }
}

pub(crate) fn publish(app: &AppHandle) {
    crate::tray::refresh_updates(app);
    if let Err(error) = app.emit("updates-changed", update_info(app.clone())) {
        log::warn!("Could not publish update status: {error}");
    }
}

#[tauri::command]
pub fn update_info(app: AppHandle) -> UpdateInfo {
    let enabled = app
        .state::<crate::AppState>()
        .settings
        .read()
        .unwrap()
        .check_updates_on_startup;
    app.state::<Updates>().info(enabled)
}

#[tauri::command]
pub async fn check_for_updates(app: AppHandle) -> Result<UpdateInfo, String> {
    if !app.state::<Updates>().begin(Instant::now()) {
        return Ok(update_info(app));
    }
    publish(&app);
    let result = tauri::async_runtime::spawn_blocking(|| fetch_release(env!("CARGO_PKG_VERSION")))
        .await
        .unwrap_or_else(|error| Err(format!("Update check worker failed: {error}")));
    app.state::<Updates>().finish(Instant::now(), result);
    publish(&app);
    Ok(update_info(app))
}

#[tauri::command]
pub fn set_update_checks(app: AppHandle, enabled: bool) -> Result<UpdateInfo, String> {
    let state = app.state::<crate::AppState>();
    crate::settings::update(&state.settings, &state.data_dir, |settings| {
        settings.check_updates_on_startup = enabled;
    })?;
    publish(&app);
    Ok(update_info(app))
}

#[tauri::command]
pub fn open_update_release(app: AppHandle) -> Result<(), String> {
    let info = update_info(app.clone());
    let release = info
        .release
        .filter(|release| release.update_available)
        .ok_or("No newer release is available to open.")?;
    app.opener()
        .open_url(release.url, None::<&str>)
        .map_err(|error| format!("Could not open the release in your browser: {error}"))
}

#[tauri::command]
pub async fn install_update(app: AppHandle) -> Result<UpdateInfo, String> {
    let installer = {
        let state = app.state::<Updates>();
        let mut cache = state.0.lock().unwrap();
        if cache.installing || cache.checking {
            return Err("An update operation is already running.".into());
        }
        let release = cache
            .release
            .as_ref()
            .filter(|release| release.update_available)
            .ok_or("No newer release is available to install.")?;
        let installer = release.installer.clone().ok_or_else(|| {
            release
                .install_error
                .clone()
                .unwrap_or("No compatible installer is available.".into())
        })?;
        cache.installing = true;
        cache.installation = Some("downloading");
        cache.installation_error = None;
        installer
    };
    publish(&app);
    let data_dir = app.state::<crate::AppState>().data_dir.clone();
    let handle = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        crate::update_install::install(&installer, &data_dir, || {
            handle.state::<Updates>().0.lock().unwrap().installation = Some("installing");
            publish(&handle);
        })
    })
    .await
    .unwrap_or_else(|error| Err(format!("Update installer worker failed: {error}")));
    let exit = {
        let state = app.state::<Updates>();
        let mut cache = state.0.lock().unwrap();
        cache.installing = false;
        match result {
            #[cfg(windows)]
            Ok(crate::update_install::InstallOutcome::Launched) => {
                cache.installation = Some("installer_started");
                true
            }
            #[cfg(target_os = "linux")]
            Ok(crate::update_install::InstallOutcome::Installed) => {
                cache.installation = Some("installed");
                false
            }
            Err(error) => {
                log::error!("Could not install update: {error}");
                cache.installation = Some("failed");
                cache.installation_error = Some(error);
                false
            }
        }
    };
    publish(&app);
    let info = update_info(app.clone());
    if exit {
        // Windows NSIS needs the running app/worker to release executable files.
        app.exit(0);
    }
    Ok(info)
}

#[tauri::command]
pub fn restart_after_update(app: AppHandle) -> Result<(), String> {
    if update_info(app.clone()).installation != "installed" {
        return Err("An update has not finished installing.".into());
    }
    app.restart();
}

pub fn start(app: AppHandle) {
    if update_info(app.clone()).check_on_startup {
        tauri::async_runtime::spawn(async move {
            if let Err(error) = check_for_updates(app).await {
                log::warn!("Startup update check failed: {error}");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str) -> String {
        serde_json::json!({
            "tag_name": tag, "html_url": format!("{RELEASE_PREFIX}{tag}"),
            "draft": false, "prerelease": false
        })
        .to_string()
    }

    #[test]
    fn newer_equal_older_and_numeric_versions() {
        for (tag, installed, available) in [
            ("v1.1.2", "1.1.1", true),
            ("v1.1.1", "1.1.1", false),
            ("v1.0.9", "1.1.1", false),
            ("v1.10.0", "1.9.0", true),
            ("2.0.0", "1.1.1", true),
            ("v1.1.1+release", "1.1.1+local", false),
            ("v1.1.1", "1.1.1-rc.1", true),
        ] {
            assert_eq!(
                parse_release(&release(tag), installed)
                    .unwrap()
                    .update_available,
                available
            );
        }
    }

    #[test]
    fn untrusted_invalid_or_unstable_metadata_is_rejected() {
        for body in [
            "not JSON".to_string(),
            release("not-a-version"),
            release("v2.0.0-beta.1"),
            release("v2.0.0").replace("\"draft\":false", "\"draft\":true"),
            release("v2.0.0").replace("\"prerelease\":false", "\"prerelease\":true"),
            release("v2.0.0").replace("github.com", "example.com"),
            release("v2.0.0").replace("/jrynks/", "/other/"),
        ] {
            assert!(parse_release(&body, "1.1.1").is_err(), "{body}");
        }
        assert!(parse_release(&release("v2.0.0"), "invalid").is_err());
    }

    #[test]
    fn no_release_and_api_failures_are_distinct() {
        assert!(
            response_result(StatusCode::NOT_FOUND, "", "1.1.1")
                .unwrap()
                .is_none()
        );
        for status in [
            StatusCode::FORBIDDEN,
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::INTERNAL_SERVER_ERROR,
        ] {
            assert!(response_result(status, "", "1.1.1").is_err());
        }
        assert!(response_result(StatusCode::OK, "", "1.1.1").is_err());
        assert!(
            response_result(StatusCode::OK, &release("v1.2.0"), "1.1.1")
                .unwrap()
                .unwrap()
                .update_available
        );
    }

    #[test]
    fn checks_are_coalesced_and_success_is_cached() {
        let updates = Updates::default();
        let now = Instant::now();
        assert!(updates.begin(now));
        assert!(!updates.begin(now));
        updates.finish(
            now,
            Ok(Some(parse_release(&release("v1.2.0"), "1.1.1").unwrap())),
        );
        assert!(updates.info(true).checked);
        assert!(!updates.begin(now + SUCCESS_INTERVAL - Duration::from_secs(1)));
        assert!(updates.begin(now + SUCCESS_INTERVAL));
    }

    #[test]
    fn failed_checks_are_not_success_and_can_retry() {
        let updates = Updates::default();
        let now = Instant::now();
        assert!(updates.begin(now));
        updates.finish(now, Err("offline".into()));
        let info = updates.info(false);
        assert!(!info.checked);
        assert!(!info.checking);
        assert_eq!(info.error.as_deref(), Some("offline"));
        assert!(!updates.begin(now + ERROR_INTERVAL - Duration::from_secs(1)));
        assert!(updates.begin(now + ERROR_INTERVAL));
        updates.finish(now, Ok(None));
        assert!(updates.info(false).checked);
        assert!(updates.info(false).error.is_none());
    }

    #[test]
    #[ignore = "requires public GitHub network access"]
    fn live_github_release_is_detected_from_an_older_version() {
        let release = fetch_release("0.0.0")
            .unwrap()
            .expect("repository has a published release");
        assert!(release.update_available);
        assert!(release.url.starts_with(RELEASE_PREFIX));
        if let Some(installer) = &release.installer {
            crate::update_install::verify_published_download(installer).unwrap();
            println!("Published installer download matches its release SHA-256 checksum.");
        }
        println!(
            "Detected published GitHub release {} at {}",
            release.version, release.url
        );
        let current = fetch_release(env!("CARGO_PKG_VERSION")).unwrap().unwrap();
        assert_eq!(
            current.update_available,
            Version::parse(&current.version)
                .unwrap()
                .cmp_precedence(&Version::parse(env!("CARGO_PKG_VERSION")).unwrap())
                == Ordering::Greater
        );
    }
}

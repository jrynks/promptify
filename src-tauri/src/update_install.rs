use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
#[cfg(target_os = "linux")]
use std::process::Command;
use std::time::Duration;

use reqwest::blocking::Client;
use serde::Deserialize;

const MAX_INSTALLER_SIZE: u64 = 1024 * 1024 * 1024;
const MAX_CHECKSUM_SIZE: u64 = 64 * 1024;

#[derive(Clone, Debug, Deserialize)]
pub struct ReleaseAsset {
    name: String,
    browser_download_url: String,
    size: u64,
}

#[derive(Clone, Debug)]
enum Package {
    #[cfg(not(target_os = "linux"))]
    Windows,
    #[cfg(target_os = "linux")]
    Deb,
    #[cfg(target_os = "linux")]
    Rpm,
    #[cfg(target_os = "linux")]
    AppImage(PathBuf),
}

#[derive(Clone, Debug)]
pub struct Installer {
    asset: ReleaseAsset,
    checksums: ReleaseAsset,
    package: Package,
}

pub enum InstallOutcome {
    #[cfg(windows)]
    Launched,
    #[cfg(target_os = "linux")]
    Installed,
}

fn select_asset(
    assets: &[ReleaseAsset],
    name: &str,
    tag: &str,
    max_size: u64,
) -> Result<ReleaseAsset, String> {
    let matches: Vec<_> = assets.iter().filter(|asset| asset.name == name).collect();
    if matches.len() != 1 {
        return Err(format!(
            "The release must include exactly one {name} asset."
        ));
    }
    let asset = matches[0];
    let expected = format!("https://github.com/jrynks/promptify/releases/download/{tag}/{name}");
    if asset.browser_download_url != expected || asset.size == 0 || asset.size > max_size {
        return Err(format!(
            "The release asset {name} has invalid download metadata."
        ));
    }
    Ok(asset.clone())
}

#[cfg(target_os = "linux")]
fn linux_package() -> Result<Package, String> {
    if let Some(path) = std::env::var_os("APPIMAGE") {
        let path = PathBuf::from(path);
        if !path.is_absolute() || !path.is_file() {
            return Err("The running AppImage path is not a valid installed file.".into());
        }
        return Ok(Package::AppImage(path));
    }
    let executable = std::env::current_exe()
        .map_err(|error| format!("Cannot locate this installation: {error}"))?;
    // Only update an installation actually owned by the relevant package manager.
    for (program, args, package) in [
        ("dpkg-query", vec!["-S"], Package::Deb),
        ("rpm", vec!["-qf"], Package::Rpm),
    ] {
        match Command::new(program).args(args).arg(&executable).output() {
            Ok(output) if output.status.success() => return Ok(package),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("Could not inspect package ownership: {error}")),
        }
    }
    Err("This Linux build is not a package-managed installation or AppImage. Install a published package first.".into())
}

impl Installer {
    pub fn select(assets: &[ReleaseAsset], version: &str, tag: &str) -> Result<Self, String> {
        if std::env::consts::ARCH != "x86_64" {
            return Err("No update installer is published for this CPU architecture.".into());
        }
        #[cfg(windows)]
        let (package, name) = (
            Package::Windows,
            format!("Promptify_{version}_x64-setup.exe"),
        );
        #[cfg(target_os = "linux")]
        let (package, name) = {
            let package = linux_package()?;
            let name = match &package {
                Package::Deb => format!("Promptify_{version}_amd64.deb"),
                Package::Rpm => format!("Promptify-{version}-1.x86_64.rpm"),
                Package::AppImage(_) => format!("Promptify_{version}_amd64.AppImage"),
            };
            (package, name)
        };
        #[cfg(not(any(windows, target_os = "linux")))]
        let (package, name) = (Package::Windows, String::new());
        if name.is_empty() {
            return Err(
                "No update installer is currently published for this operating system.".into(),
            );
        }
        Ok(Self {
            asset: select_asset(assets, &name, tag, MAX_INSTALLER_SIZE)?,
            checksums: select_asset(assets, "SHA256SUMS.txt", tag, MAX_CHECKSUM_SIZE)?,
            package,
        })
    }
}

fn allowed_download_url(url: &reqwest::Url) -> bool {
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
        && url.host_str().is_some_and(|host| {
            matches!(
                host,
                "github.com"
                    | "release-assets.githubusercontent.com"
                    | "objects.githubusercontent.com"
            )
        })
}

fn client() -> Result<Client, String> {
    Client::builder()
        .https_only(true)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(300))
        .user_agent(concat!("Promptify/", env!("CARGO_PKG_VERSION")))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() < 5 && allowed_download_url(attempt.url()) {
                attempt.follow()
            } else {
                attempt.error("Update download redirected outside the GitHub allowlist.")
            }
        }))
        .build()
        .map_err(|error| format!("Could not initialize update downloading: {error}"))
}

fn download(client: &Client, asset: &ReleaseAsset, path: &Path) -> Result<(), String> {
    let mut response = client
        .get(&asset.browser_download_url)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|error| format!("Could not download {}: {error}", asset.name))?;
    let mut file =
        File::create(path).map_err(|error| format!("Could not create update file: {error}"))?;
    let copied = std::io::copy(&mut response.by_ref().take(asset.size + 1), &mut file)
        .map_err(|error| format!("Update download interrupted: {error}"))?;
    if copied != asset.size {
        return Err(format!("Update download size mismatch for {}.", asset.name));
    }
    file.sync_all()
        .map_err(|error| format!("Could not flush update download: {error}"))
}

fn checksum_for(body: &str, name: &str) -> Result<String, String> {
    let matches: Vec<_> = body
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let hash = fields.next()?;
            let file = fields.next()?.trim_start_matches('*');
            (file == name && fields.next().is_none()).then_some(hash)
        })
        .collect();
    if matches.len() != 1
        || matches[0].len() != 64
        || !matches[0].bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(format!(
            "The published checksums do not contain one valid SHA-256 for {name}."
        ));
    }
    Ok(matches[0].to_ascii_lowercase())
}

fn verify_download(path: &Path, expected: &str) -> Result<(), String> {
    let actual = promptify_core::models::sha256_file(path)
        .map_err(|error| format!("Could not verify downloaded installer: {error}"))?;
    if actual != expected {
        return Err(
            "Downloaded installer failed its SHA-256 integrity check; installation was blocked."
                .into(),
        );
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn replace_appimage(download: &Path, installed: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let installed = fs::canonicalize(installed)
        .map_err(|error| format!("Could not locate installed AppImage: {error}"))?;
    let parent = installed
        .parent()
        .ok_or("Installed AppImage has no parent directory.")?;
    let mut replacement = tempfile::NamedTempFile::new_in(parent).map_err(|error| {
        format!("Cannot update this AppImage in place (check folder permissions): {error}")
    })?;
    let mut source =
        File::open(download).map_err(|error| format!("Cannot read AppImage update: {error}"))?;
    std::io::copy(&mut source, replacement.as_file_mut())
        .map_err(|error| format!("Cannot stage AppImage update: {error}"))?;
    replacement
        .as_file()
        .set_permissions(fs::Permissions::from_mode(0o755))
        .map_err(|error| format!("Cannot set AppImage permissions: {error}"))?;
    replacement
        .as_file()
        .sync_all()
        .map_err(|error| format!("Cannot flush AppImage update: {error}"))?;
    // Persist uses a same-directory atomic rename; the old inode stays available to the running app.
    replacement
        .persist(&installed)
        .map_err(|error| format!("Cannot replace AppImage: {error}"))?;
    File::open(parent)
        .and_then(|dir| dir.sync_all())
        .map_err(|error| format!("Cannot flush AppImage directory: {error}"))?;
    Ok(())
}

pub fn install(
    installer: &Installer,
    data_dir: &Path,
    on_installing: impl FnOnce(),
) -> Result<InstallOutcome, String> {
    let root = data_dir.join("updates");
    fs::create_dir_all(&root).map_err(|error| format!("Could not create update cache: {error}"))?;
    let scratch = tempfile::tempdir_in(root)
        .map_err(|error| format!("Could not prepare update download: {error}"))?;
    let path = prepare(installer, scratch.path())?;
    on_installing();
    match &installer.package {
        #[cfg(not(target_os = "linux"))]
        Package::Windows => {
            #[cfg(windows)]
            {
                tauri_plugin_opener::open_path(&path, None::<&str>)
                    .map_err(|error| format!("Could not start update installer: {error}"))?;
                // NSIS still needs its source executable after Promptify exits.
                let _retained = scratch.keep();
                Ok(InstallOutcome::Launched)
            }
            #[cfg(not(windows))]
            Err("Cannot execute a Windows installer on this operating system.".into())
        }
        #[cfg(target_os = "linux")]
        Package::Deb | Package::Rpm => {
            let (program, args) = match installer.package {
                Package::Deb => ("apt", vec!["install", "-y"]),
                _ => ("dnf", vec!["install", "-y"]),
            };
            let output = Command::new("pkexec").arg(program).args(args).arg(&path).output()
                .map_err(|error| format!("Could not start authorized package installation (pkexec and {program} are required): {error}"))?;
            if !output.status.success() {
                return Err(format!(
                    "Package installation failed or authorization was cancelled ({}): {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr)
                        .chars()
                        .take(2000)
                        .collect::<String>()
                ));
            }
            Ok(InstallOutcome::Installed)
        }
        #[cfg(target_os = "linux")]
        Package::AppImage(installed) => {
            replace_appimage(&path, installed)?;
            Ok(InstallOutcome::Installed)
        }
    }
}

fn prepare(installer: &Installer, directory: &Path) -> Result<PathBuf, String> {
    let client = client()?;
    let checksums = directory.join("SHA256SUMS.txt");
    download(&client, &installer.checksums, &checksums)?;
    let body = fs::read_to_string(checksums)
        .map_err(|error| format!("Could not read update checksums: {error}"))?;
    let checksum = checksum_for(&body, &installer.asset.name)?;
    let path = directory.join(&installer.asset.name);
    download(&client, &installer.asset, &path)?;
    verify_download(&path, &checksum)?;
    Ok(path)
}

#[cfg(test)]
pub fn verify_published_download(installer: &Installer) -> Result<(), String> {
    let dir = tempfile::tempdir().map_err(|error| error.to_string())?;
    prepare(installer, dir.path())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn asset_selection_requires_pinned_url_unique_name_and_valid_size() {
        let name = "Promptify_1.2.0_x64-setup.exe";
        let asset = ReleaseAsset {
            name: name.into(),
            browser_download_url: format!(
                "https://github.com/jrynks/promptify/releases/download/v1.2.0/{name}"
            ),
            size: 10,
        };
        assert!(select_asset(std::slice::from_ref(&asset), name, "v1.2.0", 100).is_ok());
        assert!(select_asset(&[asset.clone(), asset.clone()], name, "v1.2.0", 100).is_err());
        assert!(select_asset(std::slice::from_ref(&asset), name, "v1.1.0", 100).is_err());
        assert!(select_asset(std::slice::from_ref(&asset), name, "v1.2.0", 1).is_err());
        assert!(select_asset(&[], name, "v1.2.0", 100).is_err());
        let bad = ReleaseAsset {
            browser_download_url: "https://example.com/file".into(),
            ..asset
        };
        assert!(select_asset(&[bad], name, "v1.2.0", 100).is_err());
    }

    #[test]
    fn checksum_must_be_present_valid_and_unique() {
        let hash = "a".repeat(64);
        assert_eq!(
            checksum_for(&format!("{hash}  installer.exe\n"), "installer.exe").unwrap(),
            hash
        );
        assert_eq!(
            checksum_for(&format!("{hash} *installer.exe\n"), "installer.exe").unwrap(),
            hash
        );
        for body in [
            format!("{hash} other.exe"),
            "bad installer.exe".into(),
            format!("{hash} installer.exe\n{hash} installer.exe"),
            format!("{hash} installer.exe extra"),
        ] {
            assert!(checksum_for(&body, "installer.exe").is_err());
        }
    }

    #[test]
    fn corrupted_installer_is_rejected_before_execution() {
        let file = tempfile::NamedTempFile::new().unwrap();
        fs::write(file.path(), b"test installer").unwrap();
        let expected = promptify_core::models::sha256_file(file.path()).unwrap();
        assert!(verify_download(file.path(), &expected).is_ok());
        fs::write(file.path(), b"corrupted").unwrap();
        assert!(verify_download(file.path(), &expected).is_err());
    }

    #[test]
    fn download_redirects_are_https_github_only() {
        for url in [
            "https://github.com/path",
            "https://release-assets.githubusercontent.com/path",
            "https://objects.githubusercontent.com/path",
        ] {
            assert!(allowed_download_url(&reqwest::Url::parse(url).unwrap()));
        }
        for url in [
            "http://github.com/path",
            "https://github.com.evil.test/path",
            "https://example.com/path",
            "https://user:pass@github.com/path",
            "https://github.com:8443/path",
        ] {
            assert!(!allowed_download_url(&reqwest::Url::parse(url).unwrap()));
        }
    }

    #[test]
    fn download_requires_exact_published_size() {
        use std::net::TcpListener;
        for size in [2, 3, 4] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0; 4096];
                assert!(stream.read(&mut request).unwrap() > 0);
                stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nabc",
                    )
                    .unwrap();
            });
            let asset = ReleaseAsset {
                name: "installer".into(),
                browser_download_url: format!("http://{address}/installer"),
                size,
            };
            let dir = tempfile::tempdir().unwrap();
            let result = download(&Client::new(), &asset, &dir.path().join("installer"));
            assert_eq!(result.is_ok(), size == 3);
            server.join().unwrap();
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn appimage_replacement_is_atomic_and_executable() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let installed = dir.path().join("Promptify.AppImage");
        let download = dir.path().join("new.AppImage");
        fs::write(&installed, b"old").unwrap();
        fs::write(&download, b"new").unwrap();
        replace_appimage(&download, &installed).unwrap();
        assert_eq!(fs::read(&installed).unwrap(), b"new");
        assert_eq!(
            fs::metadata(&installed).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert!(replace_appimage(&dir.path().join("missing"), &installed).is_err());
        assert_eq!(fs::read(&installed).unwrap(), b"new");
    }
}

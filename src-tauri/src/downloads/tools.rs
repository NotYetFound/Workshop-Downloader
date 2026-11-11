//! Install official downloader releases into immutable, app-owned version directories.
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
    time::Duration,
};

use reqwest::{blocking::Client, redirect::Policy};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ts_rs::TS;
use url::Url;
use uuid::Uuid;

use super::{BackendChoice, files, providers};
use crate::{AppError, Result};

const ARCHIVE_LIMIT: u64 = 128 * 1024 * 1024;
const EXTRACTED_LIMIT: u64 = 256 * 1024 * 1024;
const ENTRY_LIMIT: usize = 1024;

#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BackendRelease {
    pub source: String,
    pub version: String,
    pub checksum_available: bool,
    pub update_available: bool,
}

struct Release {
    source: Url,
    version: String,
    size: Option<u64>,
    digest: Option<String>,
}

#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    assets: Vec<GitHubAsset>,
}

#[derive(Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
    size: u64,
    digest: Option<String>,
}

#[derive(Deserialize, Serialize)]
struct Selection {
    current: Uuid,
    version: String,
    previous: Option<(Uuid, String)>,
}

pub(super) fn detected(root: &Path, backend: BackendChoice) -> Option<PathBuf> {
    let root = root.join(backend.id());
    let selected = selection(&root).ok()??;
    selected_executable(&root, selected.current, backend)
}

fn selected_executable(root: &Path, id: Uuid, backend: BackendChoice) -> Option<PathBuf> {
    let directory = root.join(id.to_string());
    files::ensure_directory(&directory, false).ok()?;
    let path = directory.join(executable_name(backend));
    let metadata = fs::symlink_metadata(&path).ok()?;
    metadata.file_type().is_file().then_some(path)
}

pub(super) fn check(root: &Path, backend: BackendChoice) -> Result<BackendRelease> {
    let release = release(&client()?, backend)?;
    let current = selection(&root.join(backend.id()))?;
    let update_available = detected(root, backend).is_none()
        || current.is_none_or(|entry| entry.version != release.version);
    Ok(BackendRelease {
        source: release.source.into(),
        version: release.version,
        checksum_available: release.digest.is_some(),
        update_available,
    })
}

pub(super) fn install(root: &Path, backend: BackendChoice, confirmed: &str) -> Result<()> {
    let provider = providers::PROVIDERS
        .iter()
        .find(|provider| provider.id() == backend)
        .ok_or_else(|| invalid("Choose a download engine."))?;
    let client = client()?;
    let release = release(&client, backend)?;
    if release.source.as_str() != confirmed {
        return Err(invalid(
            "The official source changed. Check the release again.",
        ));
    }
    let root = root.join(backend.id());
    files::ensure_directory(&root, true)?;
    let previous = selection(&root)?;
    let scratch = tempfile::tempdir_in(&root)?;
    let mut archive = tempfile::tempfile_in(scratch.path())?;
    let mut response = client.get(release.source).send()?.error_for_status()?;
    if response
        .content_length()
        .is_some_and(|size| size > ARCHIVE_LIMIT)
    {
        return Err(invalid("The backend archive exceeds the size limit."));
    }
    let mut hasher = Sha256::new();
    let mut bytes = 0;
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = response.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        bytes += count as u64;
        if bytes > ARCHIVE_LIMIT {
            return Err(invalid("The backend archive exceeds the size limit."));
        }
        hasher.update(&buffer[..count]);
        archive.write_all(&buffer[..count])?;
    }
    if bytes == 0
        || release.size.is_some_and(|size| size != bytes)
        || release
            .digest
            .is_some_and(|digest| digest != format!("{:x}", hasher.finalize()))
    {
        return Err(invalid(
            "The archive did not match the official release metadata.",
        ));
    }
    use std::io::Seek;
    archive.rewind()?;
    let payload = scratch.path().join("distribution");
    files::create_private_directory(&payload)?;
    if backend == BackendChoice::SteamCmd && cfg!(target_os = "linux") {
        extract_tar(archive, &payload)?;
    } else {
        extract_zip(archive, &payload)?;
    }
    let executable = payload.join(executable_name(backend));
    if !fs::symlink_metadata(&executable).is_ok_and(|meta| meta.file_type().is_file()) {
        return Err(invalid(
            "The archive did not contain the expected executable.",
        ));
    }
    #[cfg(unix)]
    {
        executable_permissions(&executable)?;
        if backend == BackendChoice::SteamCmd {
            executable_permissions(&payload.join("linux32/steamcmd"))?;
            let reporter = payload.join("linux32/steamerrorreporter");
            if reporter.is_file() {
                executable_permissions(&reporter)?;
            }
        }
    }

    // An invalid, incompatible or failed bootstrap never replaces the selected tool.
    provider
        .probe(&executable, &scratch.path().join("probe"))
        .map_err(|error| invalid(&error.message))?;
    let current = Uuid::new_v4();
    let target = root.join(current.to_string());
    fs::rename(payload, &target)?;
    let obsolete = previous
        .as_ref()
        .and_then(|entry| entry.previous.as_ref())
        .map(|entry| entry.0);
    let selected = Selection {
        current,
        version: release.version,
        previous: previous.map(|entry| (entry.current, entry.version)),
    };
    if let Err(error) = save_selection(&root, &selected) {
        let _ = fs::remove_dir_all(target);
        return Err(error);
    }
    if let Some(id) = obsolete {
        let _ = fs::remove_dir_all(root.join(id.to_string()));
    }
    Ok(())
}

pub(super) fn restore(root: &Path, backend: BackendChoice) -> Result<()> {
    let root = root.join(backend.id());
    let selection =
        selection(&root)?.ok_or_else(|| invalid("No managed installation is selected."))?;
    let (current, version) = selection
        .previous
        .ok_or_else(|| invalid("No previous installation is available."))?;
    if selected_executable(&root, current, backend).is_none() {
        return Err(invalid("The previous installation is missing."));
    }
    save_selection(
        &root,
        &Selection {
            current,
            version,
            previous: Some((selection.current, selection.version)),
        },
    )
}

pub(super) fn can_restore(root: &Path, backend: BackendChoice) -> bool {
    selection(&root.join(backend.id()))
        .ok()
        .flatten()
        .is_some_and(|entry| entry.previous.is_some())
}

fn executable_name(backend: BackendChoice) -> &'static str {
    match (backend, cfg!(windows)) {
        (BackendChoice::SteamCmd, true) => "steamcmd.exe",
        (BackendChoice::SteamCmd, false) => "steamcmd.sh",
        (BackendChoice::DepotDownloader, true) => "DepotDownloader.exe",
        _ => "DepotDownloader",
    }
}

fn selection(root: &Path) -> Result<Option<Selection>> {
    match fs::read(root.join("selection.json")) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn save_selection(root: &Path, selected: &Selection) -> Result<()> {
    let mut file = tempfile::NamedTempFile::new_in(root)?;
    serde_json::to_writer(&mut file, selected)?;
    file.as_file().sync_all()?;
    file.persist(root.join("selection.json"))
        .map_err(|error| error.error)?;
    Ok(())
}

fn client() -> Result<Client> {
    Ok(Client::builder()
        .user_agent(concat!("WorkshopDownloader/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(120))
        .redirect(Policy::custom(|attempt| {
            if attempt.previous().len() < 5
                && official_host(attempt.url())
                && attempt.url().host_str() != Some("api.github.com")
            {
                attempt.follow()
            } else {
                attempt.error("The backend download redirected outside the official release hosts.")
            }
        }))
        .build()?)
}

fn official_host(url: &Url) -> bool {
    url.scheme() == "https"
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && matches!(
            url.host_str(),
            Some(
                "api.github.com"
                    | "github.com"
                    | "release-assets.githubusercontent.com"
                    | "objects.githubusercontent.com"
                    | "steamcdn-a.akamaihd.net"
            )
        )
}

fn release(client: &Client, backend: BackendChoice) -> Result<Release> {
    if backend == BackendChoice::SteamCmd {
        let source = match (std::env::consts::OS, std::env::consts::ARCH) {
            ("linux", "x86_64" | "x86") => {
                "https://steamcdn-a.akamaihd.net/client/installer/steamcmd_linux.tar.gz"
            }
            ("windows", "x86_64" | "x86" | "aarch64") => {
                "https://steamcdn-a.akamaihd.net/client/installer/steamcmd.zip"
            }
            _ => {
                return Err(invalid(
                    "Valve’s SteamCMD bootstrap is unavailable for this platform.",
                ));
            }
        };
        return Ok(Release {
            source: Url::parse(source)?,
            version: "Valve bootstrap".into(),
            size: None,
            digest: None,
        });
    }
    if backend != BackendChoice::DepotDownloader {
        return Err(invalid("Choose a download engine."));
    }
    let platform = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "linux-x64",
        ("linux", "aarch64") => "linux-arm64",
        ("linux", "arm") => "linux-arm",
        ("windows", "x86_64") => "windows-x64",
        ("windows", "aarch64") => "windows-arm64",
        _ => {
            return Err(invalid(
                "DepotDownloader does not publish a native archive for this platform.",
            ));
        }
    };
    let endpoint = "https://api.github.com/repos/SteamRE/DepotDownloader/releases/latest";
    let response = client.get(endpoint).send()?.error_for_status()?;
    let bytes = read_bounded(response, 1024 * 1024)?;
    let latest: GitHubRelease = serde_json::from_slice(&bytes)?;
    let version = latest
        .tag_name
        .strip_prefix("DepotDownloader_")
        .filter(|version| {
            !version.is_empty()
                && version.len() < 40
                && version
                    .split('.')
                    .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
        })
        .ok_or_else(|| invalid("DepotDownloader returned an unsupported release tag."))?;
    let name = format!("DepotDownloader-{platform}.zip");
    let asset = latest
        .assets
        .into_iter()
        .find(|asset| asset.name == name)
        .ok_or_else(|| invalid("The latest release has no matching DepotDownloader archive."))?;
    let expected = format!(
        "https://github.com/SteamRE/DepotDownloader/releases/download/{}/{name}",
        latest.tag_name
    );
    if asset.browser_download_url != expected || !(1..=ARCHIVE_LIMIT).contains(&asset.size) {
        return Err(invalid(
            "DepotDownloader returned unexpected release metadata.",
        ));
    }
    let digest = asset
        .digest
        .map(|digest| {
            digest
                .strip_prefix("sha256:")
                .filter(|value| value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit()))
                .map(str::to_ascii_lowercase)
                .ok_or_else(|| invalid("DepotDownloader returned an unsupported release checksum."))
        })
        .transpose()?;
    Ok(Release {
        source: Url::parse(&expected)?,
        version: version.into(),
        size: Some(asset.size),
        digest,
    })
}

fn read_bounded(response: impl Read, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    response.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(invalid("The release metadata exceeds the size limit."));
    }
    Ok(bytes)
}

fn archive_path(root: &Path, path: &Path) -> Result<PathBuf> {
    let mut destination = root.to_owned();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::Normal(name) if name.to_str().is_some_and(files::valid_component) => {
                destination.push(name)
            }
            _ => return Err(invalid("The backend archive contains an unsafe path.")),
        }
    }
    Ok(destination)
}

fn extract_entry(
    root: &Path,
    path: &Path,
    directory: bool,
    size: u64,
    bytes: &mut u64,
    reader: &mut impl Read,
) -> Result<()> {
    let target = archive_path(root, path)?;
    if directory {
        fs::create_dir_all(target)?;
        return Ok(());
    }
    if target == root || size > EXTRACTED_LIMIT.saturating_sub(*bytes) {
        return Err(invalid(
            "The extracted backend exceeds the size limit or has an invalid path.",
        ));
    }
    fs::create_dir_all(target.parent().expect("entry is below root"))?;
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(target)?;
    let copied = io::copy(&mut reader.take(size + 1), &mut output)?;
    if copied != size {
        return Err(invalid(
            "A backend archive entry did not match its declared size.",
        ));
    }
    *bytes += copied;
    Ok(())
}

fn extract_tar(archive: File, root: &Path) -> Result<()> {
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    let mut bytes = 0;
    for (index, entry) in archive.entries()?.enumerate() {
        let mut entry = entry?;
        let kind = entry.header().entry_type();
        if index >= ENTRY_LIMIT || !(kind.is_file() || kind.is_dir()) {
            return Err(invalid(
                "The backend archive contains too many entries or unsupported links.",
            ));
        }
        let path = entry.path()?.into_owned();
        extract_entry(
            root,
            &path,
            kind.is_dir(),
            entry.size(),
            &mut bytes,
            &mut entry,
        )?;
    }
    Ok(())
}

fn extract_zip(archive: File, root: &Path) -> Result<()> {
    let mut archive = zip::ZipArchive::new(archive).map_err(zip_error)?;
    if archive.len() > ENTRY_LIMIT {
        return Err(invalid("The backend archive contains too many entries."));
    }
    let mut bytes = 0;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(zip_error)?;
        if entry.is_symlink() {
            return Err(invalid("The backend archive contains a symbolic link."));
        }
        extract_entry(
            root,
            Path::new(&entry.name().to_owned()),
            entry.is_dir(),
            entry.size(),
            &mut bytes,
            &mut entry,
        )?;
    }
    Ok(())
}

#[cfg(unix)]
fn executable_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

fn zip_error(error: zip::result::ZipError) -> AppError {
    invalid(&format!("Could not read the backend ZIP archive. {error}"))
}

fn invalid(message: &str) -> AppError {
    AppError::Invalid(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archives_reject_traversal_and_windows_special_paths() {
        for entry in [
            "../escape",
            "safe/../file",
            "/etc/passwd",
            "C:/escape",
            "NUL.txt",
            "file:stream",
            "file.",
            "x\\..\\file",
        ] {
            assert!(
                archive_path(Path::new("root"), Path::new(entry)).is_err(),
                "{entry}"
            );
        }
        assert_eq!(
            archive_path(Path::new("root"), Path::new("./linux32/steamcmd")).unwrap(),
            Path::new("root").join("linux32").join("steamcmd"),
        );
    }

    #[test]
    fn archive_never_overwrites_an_existing_entry() {
        let root = tempfile::tempdir().unwrap();
        let mut size = 0;
        extract_entry(
            root.path(),
            Path::new("payload"),
            false,
            2,
            &mut size,
            &mut &b"ok"[..],
        )
        .unwrap();
        assert!(
            extract_entry(
                root.path(),
                Path::new("payload"),
                false,
                3,
                &mut size,
                &mut &b"bad"[..]
            )
            .is_err()
        );
        assert_eq!(fs::read(root.path().join("payload")).unwrap(), b"ok");
    }
}

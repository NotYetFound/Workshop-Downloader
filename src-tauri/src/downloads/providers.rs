use std::{
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
    time::Duration,
};

use portable_pty::CommandBuilder;
use serde::Serialize;
use ts_rs::TS;

use super::{
    BackendChoice, FailureKind, ProviderError, files,
    process::{self, Control, PromptKind},
};
use crate::{settings::Settings, steam::WorkshopItem};

#[derive(Clone, Copy, Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum BackendState {
    Missing,
    Detected,
    Ready,
    Unavailable,
}

#[derive(Clone, Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BackendInfo {
    pub backend: BackendChoice,
    pub path: Option<String>,
    pub version: Option<String>,
    pub managed: bool,
    pub can_restore: bool,
    pub state: BackendState,
    pub message: Option<String>,
}

pub fn official_source(choice: BackendChoice) -> Option<&'static str> {
    match choice {
        BackendChoice::SteamCmd => Some("https://developer.valvesoftware.com/wiki/SteamCMD"),
        BackendChoice::DepotDownloader => {
            Some("https://github.com/SteamRE/DepotDownloader/releases/latest")
        }
        BackendChoice::Automatic => None,
    }
}

pub(super) struct DownloadRequest<'a> {
    pub item: &'a WorkshopItem,
    pub stage: &'a Path,
    pub runtime: &'a Path,
    pub account: Option<&'a str>,
    pub qr: bool,
}

pub(super) struct Payload {
    pub path: PathBuf,
    pub exclude: Option<&'static str>,
    pub reported_bytes: Option<u64>,
}

pub(super) enum Event {
    Downloading,
    Percent(f32),
    Bytes(u64),
    Note(&'static str),
    Exit(u32),
}

pub(super) trait DownloadProvider: Sync {
    fn id(&self) -> BackendChoice;
    fn detect(&self, settings: &Settings) -> Option<PathBuf>;
    fn supports_account(&self) -> bool {
        true
    }
    fn probe(&self, executable: &Path, workspace: &Path) -> Result<Option<String>, ProviderError>;
    fn download(
        &self,
        executable: &Path,
        request: &DownloadRequest<'_>,
        control: &Control,
        report: &mut dyn FnMut(Event),
    ) -> Result<Payload, ProviderError>;
}

pub(super) static PROVIDERS: [&dyn DownloadProvider; 2] = [&SteamCmd, &DepotDownloader];

pub(super) struct Candidate {
    pub provider: &'static dyn DownloadProvider,
    pub executable: PathBuf,
}

pub(super) fn eligible(
    providers: &[&'static dyn DownloadProvider],
    settings: &Settings,
    choice: BackendChoice,
    signed_in: bool,
    tools: &Path,
) -> Vec<Candidate> {
    providers
        .iter()
        .filter(|provider| {
            (choice == BackendChoice::Automatic || choice == provider.id())
                && (!signed_in || provider.supports_account())
        })
        .filter_map(|&provider| {
            Some(Candidate {
                provider,
                executable: detection(provider, settings, tools)?.0,
            })
        })
        .collect()
}

fn detection(
    provider: &dyn DownloadProvider,
    settings: &Settings,
    tools: &Path,
) -> Option<(PathBuf, bool)> {
    let backend = provider.id();
    let manual = match backend {
        BackendChoice::SteamCmd => settings.steamcmd_path.is_some(),
        BackendChoice::DepotDownloader => settings.depot_downloader_path.is_some(),
        BackendChoice::Automatic => false,
    };
    if !manual && let Some(path) = super::tools::detected(tools, backend) {
        return Some((path, true));
    }
    provider.detect(settings).map(|path| (path, false))
}

pub(super) fn inspect(
    settings: &Settings,
    tools: &Path,
    workspace: Option<&Path>,
) -> Vec<BackendInfo> {
    PROVIDERS
        .iter()
        .map(|provider| {
            let backend = provider.id();
            let candidate = detection(*provider, settings, tools);
            let path = candidate.as_ref().map(|(path, _)| path);
            let (state, version, message) = match (&path, workspace) {
                (None, _) => (
                    BackendState::Missing,
                    None,
                    Some("Install the official tool or select its executable.".into()),
                ),
                (Some(_), None) => (BackendState::Detected, None, None),
                (Some(exe), Some(root)) => match provider.probe(exe, &root.join(backend.id())) {
                    Ok(version) => (BackendState::Ready, version, None),
                    Err(error) => (BackendState::Unavailable, None, Some(error.message)),
                },
            };
            BackendInfo {
                backend,
                path: path.map(|path| path.to_string_lossy().into_owned()),
                managed: candidate.is_some_and(|(_, managed)| managed),
                can_restore: super::tools::can_restore(tools, backend),
                version,
                state,
                message,
            }
        })
        .collect()
}

struct SteamCmd;
struct DepotDownloader;

impl DownloadProvider for SteamCmd {
    fn id(&self) -> BackendChoice {
        BackendChoice::SteamCmd
    }

    fn detect(&self, settings: &Settings) -> Option<PathBuf> {
        if let Some(path) = &settings.steamcmd_path {
            return executable(Path::new(path));
        }
        #[cfg(target_os = "linux")]
        if let Some(path) = std::env::var_os("HOME")
            .map(|home| PathBuf::from(home).join(".local/share/steamcmd/steamcmd.sh"))
            .and_then(|path| executable(&path))
        {
            return Some(path);
        }
        discover(if cfg!(windows) {
            &["steamcmd.exe"]
        } else {
            &["steamcmd.sh", "steamcmd"]
        })
    }

    fn probe(&self, executable: &Path, workspace: &Path) -> Result<Option<String>, ProviderError> {
        let mut command = steam_command(executable, workspace)?;
        command.arg("+quit");
        probe(
            self.id(),
            process::private_workspace(command, workspace, None, false)?,
        )
    }

    fn download(
        &self,
        executable: &Path,
        request: &DownloadRequest<'_>,
        control: &Control,
        report: &mut dyn FnMut(Event),
    ) -> Result<Payload, ProviderError> {
        let runtime = request.runtime.join("steamcmd");
        let mut command = steam_command(executable, &runtime)?;
        let install = request.stage.join("content");
        fs::create_dir_all(&install).map_err(file_error)?;
        let relative = PathBuf::from("steamapps/workshop/content")
            .join(request.item.app_id.to_string())
            .join(&request.item.id);
        let managed = runtime.join("tool").join(&relative);
        if fs::symlink_metadata(&managed).is_ok() {
            files::ensure_directory(&managed, false).map_err(unsafe_workspace)?;
            fs::remove_dir_all(&managed).map_err(file_error)?;
        }
        command.args(steam_args(request.item, &install, request.account));
        let command = process::private_workspace(command, request.stage, Some(&runtime), false)?;
        let output = download(self.id(), command, request, control, report)?;
        let staged = install.join(&relative);
        let path = if staged.is_dir() { staged } else { managed };
        Ok(Payload {
            path,
            exclude: None,
            reported_bytes: output.bytes,
        })
    }
}

impl DownloadProvider for DepotDownloader {
    fn id(&self) -> BackendChoice {
        BackendChoice::DepotDownloader
    }

    fn detect(&self, settings: &Settings) -> Option<PathBuf> {
        if let Some(path) = &settings.depot_downloader_path {
            return executable(Path::new(path));
        }
        discover(if cfg!(windows) {
            &["DepotDownloader.exe"]
        } else {
            &["DepotDownloader", "depotdownloader"]
        })
    }

    fn probe(&self, executable: &Path, workspace: &Path) -> Result<Option<String>, ProviderError> {
        fs::create_dir_all(workspace).map_err(file_error)?;
        let mut command = base_command(executable, workspace);
        isolate_home(&mut command, &workspace.join("home"))?;
        command.arg("--version");
        probe(
            self.id(),
            process::private_workspace(command, workspace, None, true)?,
        )
    }

    fn download(
        &self,
        executable: &Path,
        request: &DownloadRequest<'_>,
        control: &Control,
        report: &mut dyn FnMut(Event),
    ) -> Result<Payload, ProviderError> {
        let directory = request.stage.join("content");
        fs::create_dir_all(&directory).map_err(file_error)?;
        let runtime = request.runtime.join("depotDownloader");
        files::ensure_directory(&runtime, true).map_err(unsafe_workspace)?;
        let mut command = base_command(executable, &runtime);
        isolate_home(&mut command, &runtime.join("home"))?;
        command.args(depot_args(request.item, &directory, request.account));
        if request.qr {
            command.arg("-qr");
        }
        // .NET on Windows resolves IsolatedStorage through Windows known folders.
        // Only request a persistent token when the CLI's home can be isolated.
        if cfg!(target_os = "linux") && (request.account.is_some() || request.qr) {
            command.arg("-remember-password");
        }
        let command = process::private_workspace(command, request.stage, Some(&runtime), true)?;
        let output = download(self.id(), command, request, control, report)?;
        Ok(Payload {
            path: directory,
            exclude: Some(".DepotDownloader"),
            reported_bytes: output.bytes,
        })
    }
}

fn executable(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() || !path.is_file() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if path.metadata().ok()?.permissions().mode() & 0o111 == 0 {
            return None;
        }
    }
    #[cfg(windows)]
    if !path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
    {
        return None;
    }
    // Preserve symlink launcher names and paths. Some packages use the path to locate support files.
    Some(path.to_path_buf())
}

fn discover(names: &[&str]) -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut directories = std::env::split_paths(&path).collect::<Vec<_>>();
    #[cfg(target_os = "linux")]
    {
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            directories.push(home.join(".local/bin"));
            directories.push(home.join("bin"));
        }
        directories.extend(["/usr/local/bin", "/usr/bin", "/usr/games"].map(PathBuf::from));
    }
    #[cfg(windows)]
    {
        if let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
            directories.push(local.join("Microsoft/WinGet/Links"));
            directories.push(local.join("Programs/DepotDownloader"));
            directories.push(local.join("Programs/SteamCMD"));
        }
        if let Some(programs) = std::env::var_os("ProgramFiles").map(PathBuf::from) {
            directories.push(programs.join("SteamCMD"));
            directories.push(programs.join("DepotDownloader"));
        }
    }
    directories
        .into_iter()
        .filter(|dir| dir.is_absolute())
        .find_map(|dir| names.iter().find_map(|name| executable(&dir.join(name))))
}

fn base_command(executable: &Path, cwd: &Path) -> CommandBuilder {
    let mut command = CommandBuilder::new(executable);
    command.cwd(cwd);
    command.env("LC_ALL", "C");
    command.env("TERM", "dumb");
    for key in [
        "DEBUGGER",
        "DEBUGGER_ARGS",
        "LD_PRELOAD",
        "SteamAppId",
        "SteamGameId",
    ] {
        command.env_remove(key);
    }
    command
}

fn steam_command(source: &Path, workspace: &Path) -> Result<CommandBuilder, ProviderError> {
    let tool = workspace.join("tool");
    let home = workspace.join("home");
    files::ensure_directory(&tool, true).map_err(unsafe_workspace)?;
    let executable = prepare_steam_tool(source, &tool)?;
    let mut command = base_command(&executable, &tool);
    isolate_home(&mut command, &home)?;
    Ok(command)
}

fn isolate_home(command: &mut CommandBuilder, home: &Path) -> Result<(), ProviderError> {
    files::ensure_directory(home, true).map_err(unsafe_workspace)?;
    for key in ["HOME", "USERPROFILE", "LOCALAPPDATA", "APPDATA"] {
        command.env(key, home);
    }
    for (key, directory) in [
        ("XDG_DATA_HOME", "data"),
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_CACHE_HOME", "cache"),
    ] {
        command.env(key, home.join(directory));
    }
    Ok(())
}

fn prepare_steam_tool(source: &Path, tool: &Path) -> Result<PathBuf, ProviderError> {
    let source = fs::canonicalize(source).map_err(file_error)?;
    let location = tool.join(if cfg!(windows) {
        "steamcmd.exe"
    } else {
        "steamcmd.sh"
    });
    let stamp = tool.join("workshop-desk-source");
    if fs::symlink_metadata(&location).is_ok_and(|file| file.file_type().is_file())
        && fs::symlink_metadata(&stamp).is_ok_and(|file| file.file_type().is_file())
        && fs::read_to_string(&stamp).ok().as_deref() == source.to_str()
    {
        return Ok(location);
    }
    // Rebuild the app-owned installation rather than overwriting unknown files.
    fs::remove_dir_all(tool).map_err(file_error)?;
    files::create_private_directory(tool).map_err(file_error)?;
    #[cfg(target_os = "linux")]
    {
        let base = source.parent().ok_or_else(|| {
            ProviderError::new(
                FailureKind::BackendUnavailable,
                "Select steamcmd.sh from a SteamCMD installation.",
            )
        })?;
        let platform = if cfg!(target_arch = "aarch64") {
            "linuxarm64"
        } else {
            "linux32"
        };
        let (script, binaries) = if base.ends_with(platform) {
            (
                base.parent().map(|parent| parent.join("steamcmd.sh")),
                base.to_path_buf(),
            )
        } else {
            (Some(source.clone()), base.join(platform))
        };
        let script = script.filter(|script| {
            script.is_file() && script.file_name() == Some(std::ffi::OsStr::new("steamcmd.sh"))
        });
        let Some(script) = script.filter(|_| binaries.join("steamcmd").is_file()) else {
            return Err(ProviderError::new(
                FailureKind::Unsupported,
                "Select steamcmd.sh from an extracted Valve SteamCMD installation, alongside its Linux binaries. Package launchers are not portable into the managed runtime.",
            ));
        };
        let target = tool.join(platform);
        fs::create_dir_all(&target).map_err(file_error)?;
        for entry in fs::read_dir(binaries).map_err(file_error)? {
            let entry = entry.map_err(file_error)?;
            if entry.file_type().map_err(file_error)?.is_file() {
                fs::copy(entry.path(), target.join(entry.file_name())).map_err(file_error)?;
            }
        }
        fs::copy(script, &location).map_err(file_error)?;
    }
    #[cfg(windows)]
    fs::copy(&source, &location).map_err(file_error)?;
    fs::write(&stamp, source.to_string_lossy().as_bytes()).map_err(file_error)?;
    Ok(location)
}

fn steam_args(item: &WorkshopItem, directory: &Path, account: Option<&str>) -> Vec<OsString> {
    let mut args: Vec<OsString> = ["+@ShutdownOnFailedCommand", "1"]
        .into_iter()
        .map(Into::into)
        .collect();
    if account.is_none() {
        args.extend(["+@NoPromptForPassword".into(), "1".into()]);
    }
    args.extend([
        "+force_install_dir".into(),
        directory.as_os_str().into(),
        "+login".into(),
        account.unwrap_or("anonymous").into(),
        "+workshop_download_item".into(),
        item.app_id.to_string().into(),
        item.id.as_str().into(),
        "validate".into(),
        "+quit".into(),
    ]);
    args
}

fn depot_args(item: &WorkshopItem, directory: &Path, account: Option<&str>) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec![
        "-app".into(),
        item.app_id.to_string().into(),
        "-pubfile".into(),
        item.id.as_str().into(),
        "-dir".into(),
        directory.as_os_str().into(),
        "-validate".into(),
    ];
    if let Some(account) = account {
        args.extend(["-username".into(), account.into()]);
    }
    args
}

fn probe(id: BackendChoice, command: CommandBuilder) -> Result<Option<String>, ProviderError> {
    let mut version = None;
    let mut sandbox_error = false;
    let status = process::run(
        command,
        format!("{} version check", id.name()),
        &Control::default(),
        Duration::from_secs(45),
        |line, complete| {
            if complete {
                sandbox_error |= sandbox_failed(line);
                let candidate = if id == BackendChoice::SteamCmd {
                    line.strip_prefix("Steam Console Client")
                        .and_then(|line| line.split("version ").nth(1))
                } else {
                    line.strip_prefix("DepotDownloader v")
                };
                if let Some(value) = candidate.map(str::trim).filter(|value| {
                    !value.is_empty()
                        && value.len() <= 80
                        && value
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || ".+-".contains(c))
                }) {
                    version = Some(value.into());
                }
            }
        },
    )?;
    if sandbox_error {
        Err(sandbox_error_message())
    } else if status.success() && version.is_some() {
        Ok(version)
    } else {
        Err(ProviderError::new(
            FailureKind::BackendUnavailable,
            format!(
                "{} did not report its expected version (exit code {}).",
                id.name(),
                status.exit_code()
            ),
        ))
    }
}

#[derive(Default)]
struct Output {
    success: bool,
    bytes: Option<u64>,
    failure: Option<FailureKind>,
    session_expired: bool,
}

fn download(
    backend: BackendChoice,
    command: CommandBuilder,
    request: &DownloadRequest<'_>,
    control: &Control,
    report: &mut dyn FnMut(Event),
) -> Result<Output, ProviderError> {
    let mut output = Output::default();
    let mut sandbox_error = false;
    let mut last_prompt = None;
    let mut started = false;
    let signing_in = request.account.is_some() || request.qr;
    control.hide_output(signing_in);
    let status = process::run(
        command,
        format!("{} · {}", backend.name(), request.item.title),
        control,
        Duration::from_secs(600),
        |line, complete| {
            if request.qr && complete {
                if line.trim() == "Use the Steam Mobile App to sign in with this QR code:" {
                    control.set_prompt(Some(PromptKind::Qr));
                } else {
                    control.add_qr_line(line);
                }
            }
            let line = line.trim();
            sandbox_error |= sandbox_failed(line);
            let lower = line.to_ascii_lowercase();
            if signing_in {
                let prompt = authentication_prompt(&lower);
                if prompt.is_some()
                    && (last_prompt != prompt || complete && control.prompt().is_none())
                {
                    control.set_prompt(prompt);
                    report(Event::Note(
                        "The backend is waiting for Steam authentication.",
                    ));
                    last_prompt = prompt;
                }
                if complete {
                    last_prompt = None;
                }
            }
            if !complete {
                return;
            }

            let begin = if backend == BackendChoice::SteamCmd {
                lower.starts_with("downloading item ")
            } else {
                lower.starts_with("downloading depot ") && !lower.contains(" manifest")
            };
            if begin && !started {
                started = true;
                control.hide_output(false);
                control.set_prompt(None);
                report(Event::Downloading);
                report(Event::Note(
                    "The backend started downloading Workshop files.",
                ));
            }

            output.session_expired |= lower.contains("access token was rejected");
            if let Some(kind) = classify(&lower)
                && output
                    .failure
                    .is_none_or(|previous| failure_priority(kind) >= failure_priority(previous))
            {
                output.failure = Some(kind);
            }
            if backend == BackendChoice::SteamCmd {
                let marker = format!("Success. Downloaded item {} ", request.item.id);
                if line.starts_with(&marker) {
                    output.success = true;
                    output.bytes = line
                        .rsplit_once('(')
                        .and_then(|(_, tail)| tail.split_whitespace().next())
                        .and_then(|n| n.parse().ok());
                }
            } else if let Some(totals) = line.strip_prefix("Total downloaded: ") {
                output.success = true;
                output.bytes = totals
                    .split_once('(')
                    .and_then(|(_, tail)| tail.split_whitespace().next())
                    .and_then(|n| n.parse().ok());
            } else if started && let Some(percent) = progress(line) {
                report(Event::Percent(percent));
            }
        },
    )?;
    report(Event::Exit(status.exit_code()));
    if sandbox_error {
        return Err(sandbox_error_message());
    }
    if let Some(bytes) = output.bytes {
        report(Event::Bytes(bytes));
    }
    if status.success() && output.success {
        report(Event::Note("The backend confirmed completion."));
        return Ok(output);
    }
    let kind = output.failure.unwrap_or(FailureKind::Process);
    let explanation = match kind {
        FailureKind::Authentication if output.session_expired => {
            "The saved Steam session expired. Retry to sign in again."
        }
        FailureKind::Authentication => {
            "Steam sign-in is required or was rejected. Retry with an authorized Steam account."
        }
        FailureKind::Ownership => {
            "Steam reported a license or ownership restriction for this item or game."
        }
        FailureKind::InvalidItem => "Steam could not locate this Workshop item.",
        FailureKind::Network => {
            "Steam could not be reached or the download connection failed. You can retry."
        }
        FailureKind::Unsupported => "The backend does not support this Workshop operation.",
        FailureKind::Files => {
            "Check write permissions and free space for the download and application data folders."
        }
        _ => {
            "The backend did not confirm a successful Workshop download. If this item requires sign-in, retry with an authorized Steam account."
        }
    };
    Err(ProviderError::new(
        kind,
        format!(
            "{explanation} {} exited with code {}.",
            backend.name(),
            status.exit_code()
        ),
    ))
}

fn sandbox_failed(line: &str) -> bool {
    #[cfg(target_os = "linux")]
    return line.starts_with(super::sandbox::START_ERROR);
    #[cfg(not(target_os = "linux"))]
    {
        let _ = line;
        false
    }
}

fn sandbox_error_message() -> ProviderError {
    ProviderError::new(
        FailureKind::Files,
        "Linux backend write isolation could not start. Use a kernel with Landlock ABI 3 or later enabled.",
    )
}

fn authentication_prompt(line: &str) -> Option<PromptKind> {
    if line.contains("confirm your sign in")
        || line.contains("confirm the login")
        || line.contains("approve the login")
        || line.contains("waiting for mobile confirmation")
        || line.contains("waiting for confirmation")
        || line.contains("confirm the authentication")
        || line.contains("accept the login")
    {
        Some(PromptKind::Mobile)
    } else if line.ends_with(':')
        && (line.contains("auth code")
            || line.contains("authentication code")
            || line.contains("steam guard code")
            || line.contains("two-factor")
            || line.contains("two factor")
            || line.contains("2 factor"))
    {
        Some(PromptKind::GuardCode)
    } else if line.ends_with(':')
        && (line == "password:"
            || line.contains("enter password")
            || line.contains("enter your password")
            || line.contains("enter account password")
            || line.contains("password for "))
    {
        Some(PromptKind::Password)
    } else {
        None
    }
}

fn failure_priority(kind: FailureKind) -> u8 {
    match kind {
        FailureKind::Ownership | FailureKind::Authentication | FailureKind::InvalidItem => 3,
        FailureKind::Network => 2,
        _ => 1,
    }
}

fn classify(line: &str) -> Option<FailureKind> {
    if ["unknown command", "unsupported workshop"]
        .iter()
        .any(|text| line.contains(text))
    {
        Some(FailureKind::Unsupported)
    } else if [
        "no subscription",
        "no license",
        "does not own",
        "does not have access to app",
    ]
    .iter()
    .any(|text| line.contains(text))
    {
        Some(FailureKind::Ownership)
    } else if [
        "invalid password",
        "account logon denied",
        "not logged on",
        "login failure",
        "logon failed",
        "accessdenied",
        "not authenticated",
        "failed to authenticate",
        "requires authentication",
        "authentication required",
        "access token was rejected",
        "invalid login auth code",
    ]
    .iter()
    .any(|text| line.contains(text))
    {
        Some(FailureKind::Authentication)
    } else if [
        "published file not found",
        "file not found",
        "invalid publishedfile",
        "filenotfound",
    ]
    .iter()
    .any(|text| line.contains(text))
    {
        Some(FailureKind::InvalidItem)
    } else if [
        "no space left",
        "disk full",
        "not enough space",
        "read-only file system",
        "permission denied",
        "access is denied",
    ]
    .iter()
    .any(|text| line.contains(text))
    {
        Some(FailureKind::Files)
    } else if [
        "no connection",
        "timed out",
        "connection timeout",
        "could not connect",
        "failed to connect",
    ]
    .iter()
    .any(|text| line.contains(text))
    {
        Some(FailureKind::Network)
    } else {
        None
    }
}

fn progress(line: &str) -> Option<f32> {
    let (value, file) = line.split_once("% ")?;
    let number: f32 = value.trim().replace(',', ".").parse().ok()?;
    (!file.is_empty() && (0.0..=100.0).contains(&number)).then_some(number)
}

fn unsafe_workspace(error: crate::AppError) -> ProviderError {
    ProviderError::new(FailureKind::Files, error.to_string())
}

fn file_error(error: io::Error) -> ProviderError {
    ProviderError::new(
        FailureKind::Files,
        format!("Could not prepare the backend workspace. {error}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item() -> WorkshopItem {
        WorkshopItem {
            id: "378160973".into(),
            app_id: 322330,
            title: "Global Positions".into(),
        }
    }

    #[test]
    fn command_arguments_keep_paths_and_credentials_separate() {
        let path = Path::new("directory with spaces");
        let steam = steam_args(&item(), path, None);
        assert!(
            steam
                .windows(3)
                .any(|args| args == ["+workshop_download_item", "322330", "378160973"])
        );
        assert!(steam.windows(2).any(|args| args == ["+login", "anonymous"]));
        assert!(steam.iter().any(|arg| arg == path.as_os_str()));
        let signed = steam_args(&item(), path, Some("my_account"));
        assert!(
            signed
                .windows(2)
                .any(|args| args == ["+login", "my_account"])
        );
        assert!(!signed.iter().any(|arg| arg == "+@NoPromptForPassword"));
        let depot = depot_args(&item(), path, Some("my_account"));
        assert_eq!(&depot[..4], ["-app", "322330", "-pubfile", "378160973"]);
        assert!(
            depot
                .windows(2)
                .any(|args| args == ["-username", "my_account"])
        );
        assert!(!depot.iter().any(|arg| arg == "-password"));
    }

    #[test]
    fn selects_only_eligible_backends() {
        let mut settings = Settings::new(crate::settings::test_root()).unwrap();
        let temp = tempfile::tempdir().unwrap();
        #[cfg(windows)]
        let file = temp.path().join("DepotDownloader.exe");
        #[cfg(unix)]
        let file = temp.path().join("DepotDownloader");
        fs::write(&file, b"test").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&file, fs::Permissions::from_mode(0o700)).unwrap();
        }
        settings.steamcmd_path = Some(temp.path().join("missing").to_string_lossy().into_owned());
        settings.depot_downloader_path = Some(file.to_string_lossy().into_owned());
        assert_eq!(
            eligible(
                &PROVIDERS,
                &settings,
                BackendChoice::Automatic,
                true,
                temp.path()
            )[0]
            .provider
            .id(),
            BackendChoice::DepotDownloader
        );
        assert!(
            eligible(
                &PROVIDERS,
                &settings,
                BackendChoice::SteamCmd,
                false,
                temp.path()
            )
            .is_empty()
        );
        assert!(FailureKind::Unsupported.can_fallback());
        assert!(!FailureKind::Ownership.can_fallback());
    }

    #[test]
    fn progress_parser_ignores_invalid_or_unrelated_values() {
        assert_eq!(progress(" 27.15% /path/file.txt"), Some(27.15));
        assert_eq!(progress("[100%] updating client"), None);
        assert_eq!(progress("NaN% file"), None);
        assert_eq!(progress("120% file"), None);
    }
}

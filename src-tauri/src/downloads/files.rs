use std::{
    collections::HashSet,
    fs::{self, File, Metadata, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use super::{DownloadJob, InstalledItem, process::Control, providers::Payload};
use crate::{AppError, Result, destination, settings::Settings, steam::WorkshopItem};

const MARKER: &str = ".workshop-desk.json";
const MANIFEST_LIMIT: u64 = 32 * 1024 * 1024;

#[derive(Debug, Deserialize, Serialize)]
struct Manifest {
    job_id: String,
    app_id: u32,
    item_id: String,
    files: Vec<Entry>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub(crate) struct Entry {
    pub path: String,
    pub size: u64,
}

pub(super) fn choose_destination(
    settings: &Settings,
    item: &WorkshopItem,
    previous: Option<InstalledItem>,
    replace: bool,
) -> Result<(PathBuf, Option<InstalledItem>)> {
    let root = destination::game_root(settings, item.app_id);
    if replace
        && let Some(previous) = previous
        && Path::new(&previous.path).parent() == Some(root.as_path())
        && verified(&previous).is_some()
    {
        return Ok((PathBuf::from(&previous.path), Some(previous)));
    }
    Ok((destination::plan(settings, item)?, None))
}

pub(super) fn verify_payload(payload: &Payload) -> Result<u64> {
    let entries = scan(&payload.path, payload.exclude, false)?;
    let bytes = size(&entries)?;
    if bytes == 0
        || payload
            .reported_bytes
            .is_some_and(|reported| reported != bytes)
    {
        return Err(AppError::Invalid(
            "Downloaded files are empty or do not match the backend’s reported size.".into(),
        ));
    }
    Ok(bytes)
}

pub(super) fn publish(
    job: &DownloadJob,
    payload: &Payload,
    control: &Control,
) -> Result<(InstalledItem, u64)> {
    let install = installation(job);
    let path = Path::new(&install.path);
    let (parent, temporary, backup) = publication_paths(job)?;
    ensure_directory(parent, false)?;
    let entries = scan(&payload.path, payload.exclude, false)?;
    let bytes = size(&entries)?;
    if bytes == 0
        || payload
            .reported_bytes
            .is_some_and(|reported| reported != bytes)
    {
        return Err(AppError::Invalid(
            "Downloaded files are empty or incomplete.".into(),
        ));
    }
    create_private_directory(&temporary)?;
    for entry in &entries {
        if control.cancelled() {
            return Err(AppError::Invalid("Download cancelled.".into()));
        }
        let source = payload.path.join(&entry.path);
        let target = temporary.join(&entry.path);
        fs::create_dir_all(target.parent().expect("file has a staging parent"))?;
        let input = read_regular(&source)?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)?;
        copy_checked(input, &mut output, entry.size, control)?;
        output.sync_all()?;
    }
    let manifest = Manifest {
        job_id: install.job_id.clone(),
        app_id: install.app_id,
        item_id: install.item_id.clone(),
        files: entries,
    };
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(temporary.join(MARKER))?;
    serde_json::to_writer(&mut file, &manifest)?;
    if file.metadata()?.len() > MANIFEST_LIMIT {
        return Err(AppError::Invalid(
            "The Workshop file inventory is too large to store safely.".into(),
        ));
    }
    file.sync_all()?;
    drop(file);
    if control.cancelled() {
        return Err(AppError::Invalid("Download cancelled.".into()));
    }

    // From here either the verified tree is published or the prior tree is restored.
    ensure_directory(parent, false)?;
    if let Some(previous) = &job.previous_install {
        if previous.path != job.destination || verified(previous).is_none() {
            return Err(AppError::Invalid(
                "The previous download changed before it could be replaced.".into(),
            ));
        }
        rename_new(path, &backup)?;
    }
    if let Err(error) = ensure_unoccupied(path).and_then(|()| Ok(rename_new(&temporary, path)?)) {
        if job.previous_install.is_some() {
            // A concurrently created destination must never be overwritten. Leave the backup intact.
            let _ = rename_new(&backup, path);
        }
        return Err(error);
    }
    sync_directory(parent)?;
    if verified(&install) != Some(bytes) {
        return Err(AppError::Invalid(
            "The published Workshop files could not be verified.".into(),
        ));
    }
    Ok((install, bytes))
}

pub(super) fn recover_publication(job: &DownloadJob) -> Result<Option<(InstalledItem, u64)>> {
    let install = installation(job);
    let (parent, temporary, backup) = publication_paths(job)?;
    ensure_directory(parent, false)?;
    if let Some(bytes) = verified(&install) {
        remove_scratch(&temporary)?;
        return Ok(Some((install, bytes)));
    }
    let destination = Path::new(&job.destination);
    if let Some(previous) = &job.previous_install
        && verified_at(previous, &backup).is_some()
    {
        match ensure_unoccupied(destination) {
            Ok(()) => rename_new(&backup, destination)?,
            Err(_) if verified(previous).is_some() => {}
            Err(_) => {
                return Err(AppError::Invalid(format!(
                    "The destination is occupied. The previous download is preserved at {}.",
                    backup.display()
                )));
            }
        }
    }
    remove_scratch(&temporary)?;
    Ok(None)
}

pub(super) fn cleanup_backup(job: &DownloadJob) -> Result<()> {
    let (parent, _, backup) = publication_paths(job)?;
    ensure_directory(parent, false)?;
    if let Some(previous) = &job.previous_install
        && verified_at(previous, &backup).is_some()
    {
        fs::remove_dir_all(&backup)?;
    }
    Ok(())
}

pub(crate) fn verified(install: &InstalledItem) -> Option<u64> {
    verified_at(install, Path::new(&install.path))
}

pub(crate) fn external_snapshot(path: &Path) -> Result<Vec<Entry>> {
    let entries = scan(path, Some(".DepotDownloader"), path.join(MARKER).is_file())?;
    if size(&entries)? == 0 {
        return Err(AppError::Invalid(
            "The external item has no downloaded files.".into(),
        ));
    }
    Ok(entries)
}

fn verified_at(install: &InstalledItem, path: &Path) -> Option<u64> {
    ensure_directory(path, false).ok()?;
    let marker = read_regular(&path.join(MARKER)).ok()?;
    let manifest: Manifest = serde_json::from_reader(marker.take(MANIFEST_LIMIT)).ok()?;
    if manifest.job_id != install.job_id
        || manifest.app_id != install.app_id
        || manifest.item_id != install.item_id
    {
        return None;
    }
    let actual = scan(path, None, true).ok()?;
    if actual != manifest.files {
        return None;
    }
    size(&actual).ok().filter(|bytes| *bytes > 0)
}

fn installation(job: &DownloadJob) -> InstalledItem {
    InstalledItem {
        job_id: job.id.clone(),
        app_id: job.item.app_id,
        item_id: job.item.id.clone(),
        path: job.destination.clone(),
    }
}

fn publication_paths(job: &DownloadJob) -> Result<(&Path, PathBuf, PathBuf)> {
    let path = Path::new(&job.destination);
    if !path.is_absolute() || !super::valid_job_id(&job.id) {
        return Err(AppError::Invalid(
            "The download has an invalid publication path.".into(),
        ));
    }
    let parent = path
        .parent()
        .ok_or_else(|| AppError::Invalid("The download destination has no parent.".into()))?;
    Ok((
        parent,
        parent.join(format!(".workshop-desk-{}.new", job.id)),
        parent.join(format!(".workshop-desk-{}.old", job.id)),
    ))
}

fn scan(root: &Path, excluded: Option<&str>, has_marker: bool) -> Result<Vec<Entry>> {
    ensure_directory(root, false)?;
    let mut entries = Vec::new();
    scan_dir(root, "", excluded, has_marker, &mut entries, 0)?;
    entries.sort_unstable_by(|left, right| left.path.cmp(&right.path));
    Ok(entries)
}

fn scan_dir(
    root: &Path,
    prefix: &str,
    excluded: Option<&str>,
    has_marker: bool,
    entries: &mut Vec<Entry>,
    depth: usize,
) -> Result<()> {
    if depth > 64 {
        return Err(AppError::Invalid(
            "Workshop output contains too many nested directories.".into(),
        ));
    }
    let directory = root.join(prefix);
    let mut names = HashSet::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name().into_string().map_err(|_| {
            AppError::Invalid(
                "Workshop output contains a filename that is not valid Unicode.".into(),
            )
        })?;
        let key = destination::name_key(&name);
        if depth == 0 && (excluded == Some(&name) || (has_marker && name == MARKER)) {
            continue;
        }
        if !valid_component(&name) || key == destination::name_key(MARKER) || !names.insert(key) {
            return Err(AppError::Invalid(
                "Workshop output contains unsafe or conflicting filenames.".into(),
            ));
        }
        let metadata = fs::symlink_metadata(entry.path())?;
        if unsafe_link(&metadata) {
            return Err(AppError::Invalid(
                "Workshop output contains a symbolic link or reparse point.".into(),
            ));
        }
        let relative = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        if metadata.is_dir() {
            scan_dir(root, &relative, None, false, entries, depth + 1)?;
        } else if metadata.is_file() && !has_multiple_links(&metadata) {
            if entries.len() >= 100_000 {
                return Err(AppError::Invalid(
                    "The Workshop item contains too many files.".into(),
                ));
            }
            entries.push(Entry {
                path: relative,
                size: metadata.len(),
            });
        } else {
            return Err(AppError::Invalid(
                "Workshop output contains a special file or hard link.".into(),
            ));
        }
    }
    Ok(())
}

pub(super) fn valid_component(name: &str) -> bool {
    if name.is_empty()
        || name.ends_with([' ', '.'])
        || name
            .chars()
            .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
    {
        return false;
    }
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end()
        .to_uppercase();
    !matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) && !stem
        .strip_prefix("COM")
        .or_else(|| stem.strip_prefix("LPT"))
        .is_some_and(|digit| {
            matches!(
                digit,
                "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
}

fn ensure_unoccupied(path: &Path) -> Result<()> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| AppError::Invalid("Invalid destination name.".into()))?;
    let key = destination::name_key(name);
    let parent = path
        .parent()
        .ok_or_else(|| AppError::Invalid("Invalid destination directory.".into()))?;
    for entry in fs::read_dir(parent)? {
        if destination::name_key(&entry?.file_name().to_string_lossy()) == key {
            return Err(AppError::Invalid(
                "The download destination is already occupied. Retry to choose a unique folder."
                    .into(),
            ));
        }
    }
    Ok(())
}

pub(crate) fn ensure_directory(path: &Path, create: bool) -> Result<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| component == std::path::Component::ParentDir)
    {
        return Err(AppError::Invalid(
            "Expected an absolute directory without parent segments.".into(),
        ));
    }
    let ancestors: Vec<_> = path.ancestors().filter(|path| path.is_absolute()).collect();
    for ancestor in ancestors.into_iter().rev() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) if metadata.is_dir() && !unsafe_link(&metadata) => {}
            Ok(_) => {
                return Err(AppError::Invalid(format!(
                    "{} is not a safe directory.",
                    ancestor.display()
                )));
            }
            Err(error) if create && error.kind() == io::ErrorKind::NotFound => {
                fs::create_dir(ancestor)?
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

pub(super) fn create_private_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().mode(0o700).create(path)
    }
    #[cfg(not(unix))]
    fs::create_dir(path)
}

fn remove_scratch(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !unsafe_link(&metadata) => {
            Ok(fs::remove_dir_all(path)?)
        }
        Ok(_) => Err(AppError::Invalid(
            "The publication staging directory has an unexpected type.".into(),
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn read_regular(path: &Path) -> Result<File> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || unsafe_link(&metadata) || has_multiple_links(&metadata) {
        return Err(AppError::Invalid(
            "Expected a regular file without links.".into(),
        ));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || unsafe_link(&metadata) || has_multiple_links(&metadata) {
        return Err(AppError::Invalid(
            "Expected a regular file without links.".into(),
        ));
    }
    Ok(file)
}

fn copy_checked(input: File, output: &mut File, expected: u64, control: &Control) -> Result<()> {
    let mut input = io::BufReader::with_capacity(128 * 1024, input);
    let mut buffer = vec![0; 128 * 1024];
    let mut copied = 0;
    loop {
        if control.cancelled() {
            return Err(AppError::Invalid("Download cancelled.".into()));
        }
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        copied += count as u64;
        if copied > expected {
            return Err(AppError::Invalid(
                "A staged file changed during finalization.".into(),
            ));
        }
        output.write_all(&buffer[..count])?;
    }
    if copied != expected {
        return Err(AppError::Invalid(
            "A staged file changed during finalization.".into(),
        ));
    }
    Ok(())
}

fn size(entries: &[Entry]) -> Result<u64> {
    entries
        .iter()
        .try_fold(0u64, |total, entry| total.checked_add(entry.size))
        .ok_or_else(|| AppError::Invalid("Workshop output is too large to verify.".into()))
}

fn unsafe_link(metadata: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes()
            & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
            != 0
        {
            return true;
        }
    }
    metadata.file_type().is_symlink()
}

fn has_multiple_links(metadata: &Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        metadata.nlink() > 1
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        false
    }
}

#[cfg(target_os = "linux")]
fn rename_new(source: &Path, destination: &Path) -> io::Result<()> {
    use rustix::fs::{Mode, OFlags, RenameFlags, open, renameat_with};
    let parent = source
        .parent()
        .ok_or_else(|| io::Error::other("missing publication parent"))?;
    if destination.parent() != Some(parent) {
        return Err(io::Error::other(
            "publication must stay on the same filesystem",
        ));
    }
    let dir = open(
        parent,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW,
        Mode::empty(),
    )?;
    let source = source
        .file_name()
        .ok_or_else(|| io::Error::other("missing source name"))?;
    let destination = destination
        .file_name()
        .ok_or_else(|| io::Error::other("missing destination name"))?;
    renameat_with(&dir, source, &dir, destination, RenameFlags::NOREPLACE)?;
    Ok(())
}

#[cfg(windows)]
fn rename_new(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::MoveFileW;
    let source: Vec<_> = source.as_os_str().encode_wide().chain([0]).collect();
    let destination: Vec<_> = destination.as_os_str().encode_wide().chain([0]).collect();
    // MoveFileW does not overwrite an existing file or directory.
    if unsafe { MoveFileW(source.as_ptr(), destination.as_ptr()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::downloads::QueueOptions;

    fn prepared(root: &Path) -> (DownloadJob, Payload) {
        let item = WorkshopItem {
            id: "123".into(),
            app_id: 294100,
            title: "Coast".into(),
        };
        let mut job = DownloadJob::new(item, &QueueOptions::default(), None);
        job.destination = root.join("Coast").to_str().unwrap().into();
        let stage = root.join("stage");
        fs::create_dir(&stage).unwrap();
        fs::write(stage.join("file.txt"), b"abc").unwrap();
        (
            job,
            Payload {
                path: stage,
                exclude: None,
                reported_bytes: Some(3),
            },
        )
    }

    #[test]
    fn missing_files_clear_verified_status() {
        let root = tempfile::tempdir().unwrap();
        let (job, payload) = prepared(root.path());
        let (install, bytes) = publish(&job, &payload, &Control::default()).unwrap();
        assert_eq!(bytes, 3);
        assert_eq!(verified(&install), Some(3));
        fs::remove_file(Path::new(&install.path).join("file.txt")).unwrap();
        assert_eq!(verified(&install), None);
    }

    #[test]
    fn recovers_a_committed_tree_or_restores_the_old_one() {
        let root = tempfile::tempdir().unwrap();
        let (job, payload) = prepared(root.path());
        let (previous, _) = publish(&job, &payload, &Control::default()).unwrap();
        assert_eq!(
            recover_publication(&job).unwrap(),
            Some((previous.clone(), 3))
        );

        let mut replacement = DownloadJob::new(job.item.clone(), &QueueOptions::default(), None);
        replacement.destination = job.destination.clone();
        replacement.previous_install = Some(previous.clone());
        let (_, temporary, backup) = publication_paths(&replacement).unwrap();
        rename_new(Path::new(&previous.path), &backup).unwrap();
        fs::create_dir(&temporary).unwrap();
        assert!(recover_publication(&replacement).unwrap().is_none());
        assert_eq!(verified(&previous), Some(3));
        assert!(!temporary.exists());
        assert!(!backup.exists());
        let (latest, _) = publish(&replacement, &payload, &Control::default()).unwrap();
        assert_eq!(verified(&previous), None);
        assert_eq!(verified(&latest), Some(3));
        cleanup_backup(&replacement).unwrap();
        assert!(!backup.exists());
    }

    #[test]
    fn refuses_conflicts_and_wrong_reported_sizes() {
        let root = tempfile::tempdir().unwrap();
        let (job, mut payload) = prepared(root.path());
        fs::create_dir(root.path().join("COAST")).unwrap();
        assert!(publish(&job, &payload, &Control::default()).is_err());
        assert!(!Path::new(&job.destination).exists() || cfg!(windows));
        payload.reported_bytes = Some(4);
        assert!(verify_payload(&payload).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_downloaded_links_and_symlinked_destination_roots() {
        let root = tempfile::tempdir().unwrap();
        let (job, payload) = prepared(root.path());
        std::os::unix::fs::symlink(root.path(), payload.path.join("escape")).unwrap();
        assert!(publish(&job, &payload, &Control::default()).is_err());
        let alias = root.path().join("alias");
        std::os::unix::fs::symlink(&payload.path, &alias).unwrap();
        assert!(ensure_directory(&alias, false).is_err());
    }
}

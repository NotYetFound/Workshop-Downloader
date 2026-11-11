//! Read-only discovery. External paths are never passed to publication/replacement.
use crate::{
    AppError, Result,
    downloads::{files, timestamp},
    steam::{WorkshopItem, validate_id},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    io::Read,
    path::{Path, PathBuf},
};
use ts_rs::TS;

#[derive(Clone, Debug, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ExternalItem {
    pub item: WorkshopItem,
    pub path: String,
    pub source: String,
    pub detected_at: String,
    #[ts(skip)]
    pub(crate) files: Vec<files::Entry>,
}
#[derive(Clone, Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ExternalSummary {
    pub item: WorkshopItem,
    pub path: String,
    pub source: String,
    pub detected_at: String,
    pub verified: bool,
}
impl ExternalSummary {
    pub fn new(item: &ExternalItem, verified: bool) -> Self {
        Self {
            item: item.item.clone(),
            path: item.path.clone(),
            source: item.source.clone(),
            detected_at: item.detected_at.clone(),
            verified,
        }
    }
}
impl ExternalItem {
    pub fn verify(&self) -> bool {
        files::external_snapshot(Path::new(&self.path)).is_ok_and(|files| files == self.files)
    }
    pub fn capture(item: WorkshopItem, path: &Path, source: String) -> Result<Self> {
        Ok(Self {
            item,
            path: path.to_string_lossy().into_owned(),
            source,
            detected_at: timestamp(),
            files: files::external_snapshot(path)?,
        })
    }
}
#[derive(Debug)]
pub struct Candidate {
    pub app_id: u32,
    pub item_id: Option<String>,
    pub manifest: Option<String>,
    pub path: PathBuf,
    pub source: String,
    pub expected_bytes: Option<u64>,
}
#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ScanReport {
    pub imported: usize,
    pub recognized: usize,
    pub warnings: Vec<String>,
}

pub fn discover(root: &Path) -> Result<(Vec<Candidate>, Vec<String>)> {
    // This also rejects symlink/reparse ancestors before traversing a user root.
    files::ensure_directory(root, false)?;
    let mut pending = vec![(root.to_owned(), 0)];
    let mut candidates = Vec::new();
    let mut warnings = Vec::new();
    let mut visited = 0;
    while let Some((directory, depth)) = pending.pop() {
        if depth > 8 {
            continue;
        }
        visited += 1;
        if visited > 20_000 {
            warnings.push("Scan stopped at 20,000 directories; choose a narrower root.".into());
            break;
        }
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) => {
                warnings.push(format!("{}: {error}", directory.display()));
                continue;
            }
        };
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let kind = entry.file_type()?;
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    if fs::symlink_metadata(&path)?.file_attributes() & 0x400 != 0 {
                        continue;
                    }
                }
                if name == ".DepotDownloader" {
                    for metadata in fs::read_dir(&path)? {
                        let metadata = metadata?;
                        let filename = metadata.file_name().to_string_lossy().into_owned();
                        if let Some((app, manifest)) = depot_identity(&filename) {
                            candidates.push(Candidate {
                                app_id: app,
                                item_id: None,
                                manifest: Some(manifest),
                                path: directory.clone(),
                                source: "DepotDownloader".into(),
                                expected_bytes: None,
                            });
                        }
                    }
                } else {
                    pending.push((path, depth + 1));
                }
            } else if kind.is_file() && name.starts_with("appworkshop_") && name.ends_with(".acf") {
                match steam_manifest(&path) {
                    Ok(mut found) => candidates.append(&mut found),
                    Err(error) => warnings.push(format!("{name}: {error}")),
                }
            } else if kind.is_file() && name == ".workshop-desk.json" {
                #[derive(Deserialize)]
                struct Marker {
                    job_id: String,
                    app_id: u32,
                    item_id: String,
                }
                if let Ok(marker) =
                    read_text(&path).and_then(|text| Ok(serde_json::from_str::<Marker>(&text)?))
                    && marker.app_id > 0
                    && validate_id(&marker.item_id).is_ok()
                {
                    let installed = crate::downloads::InstalledItem {
                        job_id: marker.job_id,
                        app_id: marker.app_id,
                        item_id: marker.item_id.clone(),
                        path: directory.to_string_lossy().into_owned(),
                    };
                    if files::verified(&installed).is_some() {
                        candidates.push(Candidate {
                            app_id: marker.app_id,
                            item_id: Some(marker.item_id),
                            manifest: None,
                            path: directory.clone(),
                            source: "Workshop Downloader export".into(),
                            expected_bytes: None,
                        });
                    } else {
                        warnings.push(format!(
                            "{}: exported Workshop Downloader inventory is incomplete or changed.",
                            directory.display()
                        ));
                    }
                }
            }
        }
    }
    let mut seen = HashSet::new();
    candidates.retain(|item| {
        seen.insert((
            item.path.clone(),
            item.item_id.clone(),
            item.manifest.clone(),
        ))
    });
    if candidates.is_empty() {
        warnings.push("No trustworthy Workshop manifests found. Choose the steamapps/workshop directory containing appworkshop_*.acf, or a DepotDownloader output directory containing .DepotDownloader.".into());
    }
    Ok((candidates, warnings))
}
fn read_text(path: &Path) -> Result<String> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 4 * 1024 * 1024
    {
        return Err(AppError::Invalid(
            "Invalid or oversized external manifest.".into(),
        ));
    }
    let mut text = String::new();
    fs::File::open(path)?
        .take(4 * 1024 * 1024 + 1)
        .read_to_string(&mut text)?;
    Ok(text)
}
fn depot_identity(name: &str) -> Option<(u32, String)> {
    let (app, id) = name.strip_suffix(".manifest")?.split_once('_')?;
    let app = app.parse::<u32>().ok().filter(|app| *app > 0)?;
    Some((app, validate_id(id).ok()?))
}
fn steam_manifest(path: &Path) -> Result<Vec<Candidate>> {
    let root = keyvalues(&read_text(path)?)?;
    let root = root.object("AppWorkshop")?;
    let app = root
        .text("appid")?
        .parse::<u32>()
        .ok()
        .filter(|id| *id > 0)
        .ok_or_else(|| AppError::Invalid("Invalid manifest App ID.".into()))?;
    if path.file_name().and_then(|n| n.to_str()) != Some(format!("appworkshop_{app}.acf").as_str())
    {
        return Err(AppError::Invalid(
            "Manifest filename and App ID do not match.".into(),
        ));
    }
    let mut result = vec![];
    for (id, value) in root.object("WorkshopItemsInstalled")? {
        let Ok(id) = validate_id(id) else {
            continue;
        };
        if value
            .object_value()?
            .text("manifest")
            .ok()
            .and_then(|id| validate_id(id).ok())
            .is_none()
        {
            continue;
        }
        let directory = path
            .parent()
            .unwrap()
            .join("content")
            .join(app.to_string())
            .join(&id);
        result.push(Candidate {
            app_id: app,
            item_id: Some(id),
            manifest: None,
            path: directory,
            source: "Steam Workshop / SteamCMD".into(),
            expected_bytes: value
                .object_value()?
                .text("size")
                .ok()
                .and_then(|size| size.parse().ok()),
        });
    }
    Ok(result)
}
#[derive(Debug)]
enum Value {
    Text(String),
    Object(BTreeMap<String, Value>),
}
impl Value {
    fn object_value(&self) -> Result<&BTreeMap<String, Value>> {
        match self {
            Self::Object(value) => Ok(value),
            _ => Err(invalid_kv()),
        }
    }
}
trait Fields {
    fn object(&self, key: &str) -> Result<&BTreeMap<String, Value>>;
    fn text(&self, key: &str) -> Result<&str>;
}
impl Fields for BTreeMap<String, Value> {
    fn object(&self, key: &str) -> Result<&BTreeMap<String, Value>> {
        self.get(key).ok_or_else(invalid_kv)?.object_value()
    }
    fn text(&self, key: &str) -> Result<&str> {
        match self.get(key) {
            Some(Value::Text(value)) => Ok(value),
            _ => Err(invalid_kv()),
        }
    }
}
fn invalid_kv() -> AppError {
    AppError::Invalid("Unsupported or invalid Workshop ACF manifest.".into())
}
fn keyvalues(text: &str) -> Result<BTreeMap<String, Value>> {
    let mut tokens = vec![];
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {}
            '/' if chars.peek() == Some(&'/') => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            }
            '{' | '}' => tokens.push(c.to_string()),
            '"' => {
                let mut value = String::new();
                let mut closed = false;
                while let Some(c) = chars.next() {
                    if c == '"' {
                        closed = true;
                        break;
                    }
                    if c == '\\' {
                        value.push(chars.next().ok_or_else(invalid_kv)?);
                    } else {
                        value.push(c);
                    }
                }
                if !closed {
                    return Err(invalid_kv());
                }
                tokens.push(format!("q{value}"));
            }
            _ => return Err(invalid_kv()),
        }
        if tokens.len() > 100_000 {
            return Err(invalid_kv());
        }
    }
    fn object(
        tokens: &[String],
        index: &mut usize,
        depth: usize,
    ) -> Result<BTreeMap<String, Value>> {
        if depth > 16 {
            return Err(invalid_kv());
        }
        let mut map = BTreeMap::new();
        while *index < tokens.len() {
            if tokens[*index] == "}" {
                if depth == 0 {
                    return Err(invalid_kv());
                }
                *index += 1;
                return Ok(map);
            }
            let key = tokens[*index]
                .strip_prefix('q')
                .ok_or_else(invalid_kv)?
                .to_owned();
            *index += 1;
            let value = match tokens.get(*index).map(String::as_str) {
                Some("{") => {
                    *index += 1;
                    Value::Object(object(tokens, index, depth + 1)?)
                }
                Some(value) if value.starts_with('q') => {
                    *index += 1;
                    Value::Text(value[1..].into())
                }
                _ => return Err(invalid_kv()),
            };
            if map.insert(key, value).is_some() {
                return Err(invalid_kv());
            }
        }
        if depth != 0 {
            return Err(invalid_kv());
        }
        Ok(map)
    }
    object(&tokens, &mut 0, 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reads_steam_identity_from_acf_and_never_numeric_folder_names_alone() {
        let root = tempfile::tempdir().unwrap();
        let workshop = root.path().join("steamapps/workshop");
        let content = workshop.join("content/322330/378160973");
        fs::create_dir_all(&content).unwrap();
        fs::write(content.join("mod.lua"), "payload").unwrap();
        assert!(discover(root.path()).unwrap().0.is_empty());
        fs::write(workshop.join("appworkshop_322330.acf"),r#""AppWorkshop" { "appid" "322330" "WorkshopItemsInstalled" { "378160973" { "manifest" "4506020981624356616" } } }"#).unwrap();
        let candidates = discover(root.path()).unwrap().0;
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].item_id.as_deref(), Some("378160973"));
        assert_eq!(candidates[0].path, content);
        let record = ExternalItem::capture(
            WorkshopItem {
                id: "378160973".into(),
                app_id: 322330,
                title: "Example".into(),
            },
            &content,
            candidates[0].source.clone(),
        )
        .unwrap();
        assert!(record.verify());
        fs::write(content.join("mod.lua"), "changed payload").unwrap();
        assert!(!record.verify());
        fs::remove_file(content.join("mod.lua")).unwrap();
        assert!(!record.verify());
        assert!(workshop.join("appworkshop_322330.acf").is_file());
    }
    #[test]
    fn copied_workshop_desk_exports_require_the_complete_original_inventory() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("mod.lua"), "payload").unwrap();
        fs::write(root.path().join(".workshop-desk.json"),r#"{"job_id":"export","app_id":322330,"item_id":"123","files":[{"path":"mod.lua","size":7}]}"#).unwrap();
        assert_eq!(discover(root.path()).unwrap().0.len(), 1);
        fs::remove_file(root.path().join("mod.lua")).unwrap();
        let (found, warnings) = discover(root.path()).unwrap();
        assert!(found.is_empty());
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("incomplete"))
        );
    }
    #[test]
    fn depot_manifest_requires_independent_published_id_and_ignores_cache_changes() {
        let root = tempfile::tempdir().unwrap();
        let meta = root.path().join(".DepotDownloader");
        fs::create_dir(&meta).unwrap();
        fs::write(meta.join("322330_4506020981624356616.manifest"), "fixture").unwrap();
        fs::write(root.path().join("mod.lua"), "payload").unwrap();
        let candidates = discover(root.path()).unwrap().0;
        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].item_id.is_none());
        assert_eq!(
            candidates[0].manifest.as_deref(),
            Some("4506020981624356616")
        );
        let record = ExternalItem::capture(
            WorkshopItem {
                id: "378160973".into(),
                app_id: 322330,
                title: "Example".into(),
            },
            root.path(),
            "DepotDownloader".into(),
        )
        .unwrap();
        fs::write(meta.join("depot.config"), "new cache").unwrap();
        assert!(record.verify());
        assert!(depot_identity("0_42.manifest").is_none());
        assert!(depot_identity("1_../../2.manifest").is_none());
    }
    #[test]
    fn rejects_malformed_duplicate_and_nested_acf_data() {
        for text in [
            r#""a" { "b" "c""#,
            r#""a" "b" "a" "c""#,
            "{ unquoted }",
            "\"a\" { }",
        ] {
            if text == "\"a\" { }" {
                assert!(keyvalues(text).is_ok());
            } else {
                assert!(keyvalues(text).is_err());
            }
        }
        assert!(keyvalues(&format!("{}{}", "\"a\" {".repeat(18), "}".repeat(18))).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn external_verification_never_follows_symlink_payloads() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("real.lua"), "payload").unwrap();
        std::os::unix::fs::symlink(root.path().join("real.lua"), root.path().join("link.lua"))
            .unwrap();
        assert!(
            ExternalItem::capture(
                WorkshopItem {
                    id: "1".into(),
                    app_id: 1,
                    title: "Example".into()
                },
                root.path(),
                "External".into()
            )
            .is_err()
        );
    }
}

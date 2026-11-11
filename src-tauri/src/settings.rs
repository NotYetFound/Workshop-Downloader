use std::{
    collections::HashSet,
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{AppError, Result, downloads::BackendChoice};

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum FolderNaming {
    Title,
    TitleAndId,
    Id,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct GamePreference {
    pub app_id: u32,
    pub name: String,
    pub download_dir: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub download_dir: String,
    pub organize_by_game: bool,
    pub folder_naming: FolderNaming,
    #[serde(default)]
    pub games: Vec<GamePreference>,
    #[serde(default)]
    pub backend: BackendChoice,
    #[serde(default)]
    pub sign_in: bool,
    #[serde(default)]
    pub steam_account: String,
    #[serde(default)]
    pub steamcmd_path: Option<String>,
    #[serde(default)]
    pub depot_downloader_path: Option<String>,
}

impl Settings {
    pub fn new(downloads: PathBuf) -> Result<Self> {
        let download_dir = downloads
            .join("SteamWorkshop")
            .into_os_string()
            .into_string()
            .map_err(|_| {
                AppError::Invalid(
                    "The system Downloads path could not be represented as text.".into(),
                )
            })?;
        Ok(Self {
            download_dir,
            organize_by_game: true,
            folder_naming: FolderNaming::Title,
            backend: BackendChoice::Automatic,
            sign_in: false,
            steam_account: String::new(),
            steamcmd_path: None,
            depot_downloader_path: None,
            games: [
                (294100, "RimWorld"),
                (4000, "Garry's Mod"),
                (108600, "Project Zomboid"),
                (431960, "Wallpaper Engine"),
                (255710, "Cities: Skylines"),
                (346110, "ARK: Survival Evolved"),
            ]
            .into_iter()
            .map(|(app_id, name)| GamePreference {
                app_id,
                name: name.into(),
                download_dir: None,
            })
            .collect(),
        })
    }

    pub fn validate(&self) -> Result<()> {
        validate_directory(&self.download_dir)?;
        if !self.steam_account.is_empty()
            && (self.steam_account.len() > 64
                || !self
                    .steam_account
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_'))
        {
            return Err(AppError::Invalid(
                "Enter a valid Steam account name.".into(),
            ));
        }
        if self.sign_in && self.steam_account.is_empty() {
            return Err(AppError::Invalid(
                "Enter a Steam account name to enable sign-in.".into(),
            ));
        }
        for path in [&self.steamcmd_path, &self.depot_downloader_path]
            .into_iter()
            .flatten()
        {
            validate_directory(path).map_err(|_| {
                AppError::Invalid("Choose an absolute path to the backend executable.".into())
            })?;
        }
        let mut ids = HashSet::new();
        for game in &self.games {
            if game.app_id == 0 || !ids.insert(game.app_id) {
                return Err(AppError::Invalid(
                    "Every saved game needs a unique positive Steam App ID.".into(),
                ));
            }
            if game.name.trim().is_empty()
                || game.name.chars().count() > 120
                || game.name.chars().any(char::is_control)
            {
                return Err(AppError::Invalid(
                    "Enter a game name between 1 and 120 characters.".into(),
                ));
            }
            if let Some(path) = &game.download_dir {
                validate_directory(path)?;
            }
        }
        Ok(())
    }

    pub fn game(&self, app_id: u32) -> Option<&GamePreference> {
        self.games.iter().find(|game| game.app_id == app_id)
    }
}

fn validate_directory(value: &str) -> Result<()> {
    let path = Path::new(value);
    if value.contains('\0')
        || !path.is_absolute()
        || path.components().any(|part| part == Component::ParentDir)
    {
        return Err(AppError::Invalid(
            "Choose an absolute download directory without parent-directory segments.".into(),
        ));
    }

    #[cfg(windows)]
    if !matches!(
        path.components().next(),
        Some(Component::Prefix(prefix))
            if matches!(prefix.kind(), std::path::Prefix::Disk(_) | std::path::Prefix::UNC(_, _))
    ) {
        return Err(AppError::Invalid(
            "Choose a drive or network share as the download directory.".into(),
        ));
    }

    Ok(())
}

#[cfg(test)]
pub(crate) fn test_root() -> PathBuf {
    #[cfg(windows)]
    return PathBuf::from(r"C:\WorkshopTests");
    #[cfg(not(windows))]
    return PathBuf::from("/workshop-tests");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_use_downloads_and_title_folders() {
        let root = test_root();
        let settings = Settings::new(root.clone()).unwrap();
        assert_eq!(
            Path::new(&settings.download_dir),
            root.join("SteamWorkshop")
        );
        assert_eq!(settings.folder_naming, FolderNaming::Title);
        assert!(settings.organize_by_game);
        assert!(settings.validate().is_ok());
    }

    #[test]
    fn validates_global_paths_and_per_game_overrides() {
        let mut settings = Settings::new(test_root()).unwrap();
        settings.download_dir = "relative".into();
        assert!(settings.validate().is_err());
        settings.download_dir = test_root().to_string_lossy().into_owned();
        settings.games[0].download_dir =
            Some(test_root().join("..").to_string_lossy().into_owned());
        assert!(settings.validate().is_err());
        settings.games[0].download_dir = None;
        settings.games.push(settings.games[0].clone());
        assert!(settings.validate().is_err());
    }

    #[test]
    fn migrates_and_validates_download_login_preferences() {
        let settings = Settings::new(test_root()).unwrap();
        let mut legacy = serde_json::to_value(&settings).unwrap();
        legacy.as_object_mut().unwrap().remove("signIn");
        legacy.as_object_mut().unwrap().remove("steamAccount");
        let mut migrated: Settings = serde_json::from_value(legacy).unwrap();
        assert!(!migrated.sign_in);
        assert!(migrated.steam_account.is_empty());
        migrated.sign_in = true;
        assert!(migrated.validate().is_err());
        migrated.steam_account = "workshop_user1".into();
        migrated.backend = BackendChoice::DepotDownloader;
        assert!(migrated.validate().is_ok());
        let saved = serde_json::to_string(&migrated).unwrap();
        assert_eq!(serde_json::from_str::<Settings>(&saved).unwrap(), migrated);
        migrated.steam_account = "name\npassword".into();
        assert!(migrated.validate().is_err());
    }
}

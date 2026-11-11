use std::{collections::HashSet, fs, io, path::PathBuf};

use unicode_normalization::UnicodeNormalization;

use crate::{
    Result,
    settings::{FolderNaming, Settings},
    steam::WorkshopItem,
};

const NAME_LIMIT: usize = 150;

pub fn safe_name(title: &str) -> String {
    let cleaned: String = title
        .nfkc()
        .filter(|character| !matches!(character, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
        .map(|character| {
            if character.is_control()
                || matches!(
                    character,
                    '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
                )
            {
                ' '
            } else {
                character
            }
        })
        .collect();
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let cleaned = cleaned.trim_matches([' ', '.']);
    let cleaned = truncate_name(cleaned, NAME_LIMIT);
    if cleaned.is_empty() {
        return "Workshop item".into();
    }
    let stem = cleaned
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end()
        .to_ascii_uppercase();
    if matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || stem
        .strip_prefix("COM")
        .or_else(|| stem.strip_prefix("LPT"))
        .is_some_and(|digit| digit.len() == 1 && digit.as_bytes()[0].is_ascii_digit())
    {
        return format!("_{}", truncate_name(cleaned, NAME_LIMIT - 1));
    }
    cleaned.into()
}

fn truncate_name(name: &str, max: usize) -> &str {
    let mut end = name.len().min(max);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    name[..end].trim_end_matches([' ', '.'])
}

fn suffixed(name: &str, suffix: &str) -> String {
    format!("{}{suffix}", truncate_name(name, NAME_LIMIT - suffix.len()))
}

pub fn game_root(settings: &Settings, app_id: u32) -> PathBuf {
    let game = settings.game(app_id);
    if let Some(directory) = game.and_then(|game| game.download_dir.as_deref()) {
        return PathBuf::from(directory);
    }
    let root = PathBuf::from(&settings.download_dir);
    if !settings.organize_by_game {
        return root;
    }
    let name = game.map_or_else(
        || format!("App {app_id}"),
        |game| {
            let name = safe_name(&game.name);
            if settings.games.iter().any(|other| {
                other.app_id != app_id && name_key(&safe_name(&other.name)) == name_key(&name)
            }) {
                suffixed(&name, &format!(" ({app_id})"))
            } else {
                name
            }
        },
    );
    root.join(name)
}

pub fn plan(settings: &Settings, item: &WorkshopItem) -> Result<PathBuf> {
    let root = game_root(settings, item.app_id);
    let occupied = match fs::read_dir(&root) {
        Ok(entries) => entries
            .map(|entry| entry.map(|entry| name_key(&entry.file_name().to_string_lossy())))
            .collect::<io::Result<HashSet<_>>>()?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => HashSet::new(),
        Err(error) => return Err(error.into()),
    };
    Ok(root.join(available_name(settings.folder_naming, item, &occupied)))
}

pub(crate) fn name_key(name: &str) -> String {
    name.nfkc()
        .collect::<String>()
        .trim_end_matches([' ', '.'])
        .to_uppercase()
}

fn available_name(naming: FolderNaming, item: &WorkshopItem, occupied: &HashSet<String>) -> String {
    let title = safe_name(&item.title);
    let naming = if title == item.id {
        FolderNaming::Id
    } else {
        naming
    };
    let id_suffix = format!(" ({})", item.id);
    let primary = match naming {
        FolderNaming::Title => title.clone(),
        FolderNaming::TitleAndId => suffixed(&title, &id_suffix),
        FolderNaming::Id => item.id.clone(),
    };
    if !occupied.contains(&name_key(&primary)) {
        return primary;
    }
    let identified = match naming {
        FolderNaming::Title => suffixed(&title, &id_suffix),
        _ => primary,
    };
    if !occupied.contains(&name_key(&identified)) {
        return identified;
    }
    for ordinal in 2u64.. {
        let candidate = match naming {
            FolderNaming::Id => format!("{} ({ordinal})", item.id),
            _ => suffixed(&title, &format!("{} ({ordinal})", id_suffix)),
        };
        if !occupied.contains(&name_key(&candidate)) {
            return candidate;
        }
    }
    unreachable!("the directory cannot contain all u64 suffixes")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::test_root;
    use std::path::Path;

    fn item() -> WorkshopItem {
        WorkshopItem {
            id: "123".into(),
            app_id: 294100,
            title: "Coast".into(),
        }
    }

    #[test]
    fn normalizes_reserved_and_invalid_names_on_both_platforms() {
        assert_eq!(safe_name(" ../ Night:Rain | blue? . "), "Night Rain blue");
        assert_eq!(safe_name("ＣＯＭ¹.txt"), "_COM1.txt");
        assert_eq!(safe_name("NUL .txt"), "_NUL .txt");
        assert_eq!(safe_name("..."), "Workshop item");
        assert_eq!(safe_name("A\u{202e}B"), "AB");
        assert_eq!(safe_name(&"雨".repeat(100)).len(), NAME_LIMIT);
    }

    #[test]
    fn handles_duplicate_titles_and_existing_id_suffixes() {
        let item = item();
        let occupied = ["coast", "COAST (123)", "123"]
            .into_iter()
            .map(name_key)
            .collect();
        assert_eq!(
            available_name(FolderNaming::Title, &item, &occupied),
            "Coast (123) (2)"
        );
        assert_eq!(
            available_name(FolderNaming::TitleAndId, &item, &occupied),
            "Coast (123) (2)"
        );
        assert_eq!(
            available_name(FolderNaming::Id, &item, &occupied),
            "123 (2)"
        );
        let occupied = ["COAST."].into_iter().map(name_key).collect();
        assert_eq!(
            available_name(FolderNaming::Title, &item, &occupied),
            "Coast (123)"
        );
    }

    #[test]
    fn override_is_the_game_root_in_both_organization_modes() {
        let mut settings = Settings::new(test_root()).unwrap();
        assert!(game_root(&settings, 294100).ends_with("RimWorld"));
        assert!(game_root(&settings, 99).ends_with("App 99"));
        let override_path = test_root().join("override");
        settings.games[0].download_dir = Some(override_path.to_string_lossy().into_owned());
        assert_eq!(game_root(&settings, 294100), override_path);
        settings.organize_by_game = false;
        assert_eq!(game_root(&settings, 294100), override_path);
        assert_eq!(game_root(&settings, 99), Path::new(&settings.download_dir));
    }
}

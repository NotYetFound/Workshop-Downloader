use crate::workshop::{PreviewEntry, WorkshopPreview};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use ts_rs::TS;
use url::Url;

use crate::{AppError, Result};

pub const WORKSHOP_HOME: &str = "https://steamcommunity.com/workshop/";
const ITEM_DETAILS: &str =
    "https://api.steampowered.com/ISteamRemoteStorage/GetPublishedFileDetails/v1/";

pub fn is_community_url(url: &Url) -> bool {
    is_secure_origin(url) && url.host_str() == Some("steamcommunity.com")
}

pub fn is_browser_url(url: &Url) -> bool {
    is_secure_origin(url)
        && match url.host_str() {
            Some("steamcommunity.com" | "login.steampowered.com") => true,
            Some("store.steampowered.com") => {
                url.path() == "/login" || url.path().starts_with("/login/")
            }
            _ => false,
        }
}

fn is_secure_origin(url: &Url) -> bool {
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
}

pub fn parse_address(address: &str) -> Result<Url> {
    let address = address.trim();
    let with_scheme;
    let address = if address.starts_with("steamcommunity.com/") {
        with_scheme = format!("https://{address}");
        &with_scheme
    } else {
        address
    };
    Url::parse(address)
        .ok()
        .filter(is_community_url)
        .ok_or_else(|| AppError::Invalid("Enter an HTTPS URL on steamcommunity.com.".into()))
}

pub fn game_url(app_id: u32) -> Result<Url> {
    if app_id == 0 {
        return Err(AppError::Invalid("Enter a positive Steam App ID.".into()));
    }
    Ok(Url::parse(&format!(
        "https://steamcommunity.com/workshop/browse/?appid={app_id}"
    ))?)
}

pub fn item_id(url: &Url) -> Option<String> {
    if !is_community_url(url)
        || !matches!(
            url.path().trim_end_matches('/'),
            "/sharedfiles/filedetails" | "/workshop/filedetails"
        )
    {
        return None;
    }
    let mut ids = url
        .query_pairs()
        .filter_map(|(key, value)| (key == "id").then_some(value));
    let id = ids.next()?;
    if ids.next().is_some() || !id.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    id.parse::<u64>()
        .ok()
        .filter(|id| *id > 0)
        .map(|id| id.to_string())
}

pub fn item_url(id: &str) -> String {
    format!("https://steamcommunity.com/sharedfiles/filedetails/?id={id}")
}

pub fn app_id(url: &Url) -> Option<u32> {
    if !is_community_url(url) {
        return None;
    }
    let path_id = url
        .path()
        .strip_prefix("/app/")
        .and_then(|path| path.split('/').next());
    let query_id = url.query_pairs().find(|(key, _)| key == "appid");
    path_id
        .or(query_id.as_ref().map(|(_, id)| id.as_ref()))?
        .parse()
        .ok()
        .filter(|id| *id > 0)
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct WorkshopItem {
    pub id: String,
    pub app_id: u32,
    pub title: String,
}

#[derive(Deserialize)]
struct DetailsEnvelope {
    response: DetailsResponse,
}

#[derive(Deserialize)]
struct DetailsResponse {
    publishedfiledetails: Vec<PublishedFile>,
}

#[derive(Clone, Deserialize)]
pub(crate) struct PublishedFile {
    publishedfileid: String,
    result: u32,
    consumer_app_id: Option<u32>,
    title: Option<String>,
    time_updated: Option<u32>,
    file_type: Option<u32>,
    hcontent_file: Option<String>,
    file_size: Option<serde_json::Value>,
    #[serde(skip)]
    children: Option<Vec<CollectionChild>>,
}

pub struct ResolvedItem {
    pub item: WorkshopItem,
    pub updated_at: Option<u32>,
    pub size_bytes: Option<u64>,
}

impl PublishedFile {
    fn size_bytes(&self) -> Option<u64> {
        let value = self.file_size.as_ref()?;
        value
            .as_u64()
            .or_else(|| value.as_str()?.parse().ok())
            .filter(|size| *size > 0)
    }
}

pub struct SteamClient(reqwest::Client, tokio::sync::Mutex<()>);

impl SteamClient {
    pub fn new() -> Result<Self> {
        Ok(Self(
            reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            tokio::sync::Mutex::new(()),
        ))
    }

    pub async fn resolve_item(&self, id: &str) -> Result<WorkshopItem> {
        self.resolve_metadata(id, false)
            .await
            .map(|metadata| metadata.item)
    }

    pub async fn resolve_metadata(&self, id: &str, downloadable: bool) -> Result<ResolvedItem> {
        validate_id(id)?;
        let files = self.metadata(&[id.to_owned()]).await?;
        let response = DetailsEnvelope {
            response: DetailsResponse {
                publishedfiledetails: files,
            },
        };
        extract_item(response, id, downloadable)
    }
    async fn post<T: serde::de::DeserializeOwned>(
        &self,
        endpoint: &str,
        form: &[(String, String)],
    ) -> Result<T> {
        let _permit = self.1.lock().await;
        // One bounded request at a time, including across previews and item downloads.
        tokio::time::sleep(Duration::from_millis(160)).await;
        let response = self.0.post(endpoint).form(form).send().await?;
        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(AppError::Invalid(
                "Steam is rate limiting metadata requests. Wait a moment and retry.".into(),
            ));
        }
        let mut response = response.error_for_status()?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if bytes.len() + chunk.len() > 16 * 1024 * 1024 {
                return Err(AppError::Invalid(
                    "Steam returned an oversized metadata response.".into(),
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(serde_json::from_slice(&bytes)?)
    }

    async fn metadata(&self, ids: &[String]) -> Result<Vec<PublishedFile>> {
        let mut result = Vec::new();
        for chunk in ids.chunks(50) {
            let mut form = vec![("itemcount".into(), chunk.len().to_string())];
            for (index, id) in chunk.iter().enumerate() {
                validate_id(id)?;
                form.push((format!("publishedfileids[{index}]"), id.clone()));
            }
            let mut response: DetailsEnvelope = self.post(ITEM_DETAILS, &form).await?;
            // GetPublishedFileDetails currently omits file_type, even for collections.
            // The official collection endpoint supplies the authoritative distinction.
            form[0] = ("collectioncount".into(), chunk.len().to_string());
            let collections: CollectionEnvelope = self
                .post(
                    "https://api.steampowered.com/ISteamRemoteStorage/GetCollectionDetails/v1/",
                    &form,
                )
                .await?;
            apply_collection_details(&mut response, collections);
            result.extend(response.response.publishedfiledetails);
        }
        Ok(result)
    }

    pub async fn external_identity(&self, id: &str) -> Result<(WorkshopItem, String, Option<u64>)> {
        let files = self.metadata(&[validate_id(id)?]).await?;
        let file = files
            .iter()
            .find(|file| {
                file.publishedfileid == id && file.result == 1 && file.file_type != Some(2)
            })
            .ok_or_else(|| AppError::Invalid("Workshop item metadata is unavailable.".into()))?;
        let entry = preview_entry(id, Some(file), None);
        let item = entry
            .item
            .ok_or_else(|| AppError::Invalid("Workshop game metadata is unavailable.".into()))?;
        let manifest = file
            .hcontent_file
            .as_ref()
            .and_then(|id| validate_id(id).ok())
            .ok_or_else(|| {
                AppError::Invalid("Steam did not provide a content manifest for this item.".into())
            })?;
        Ok((item, manifest, file.size_bytes()))
    }

    pub async fn is_collection(&self, id: &str) -> Result<bool> {
        let files = self.metadata(&[validate_id(id)?]).await?;
        let file = files
            .iter()
            .find(|file| file.publishedfileid == id && file.result == 1)
            .ok_or_else(|| {
                AppError::Invalid("Steam did not return public metadata for this item.".into())
            })?;
        Ok(file.file_type == Some(2))
    }

    pub async fn preview_items(&self, ids: Vec<String>) -> Result<WorkshopPreview> {
        let ids = unique_ids(ids)?;
        let files = self.metadata(&ids).await?;
        let map: HashMap<_, _> = files
            .into_iter()
            .map(|file| (file.publishedfileid.clone(), file))
            .collect();
        Ok(WorkshopPreview {
            id: uuid::Uuid::new_v4().to_string(),
            collection_id: None,
            name: "Workshop selection".into(),
            app_id: None,
            warnings: vec![],
            items: ids
                .iter()
                .map(|id| preview_entry(id, map.get(id), None))
                .collect(),
        })
    }

    pub async fn preview_collection(&self, id: &str) -> Result<WorkshopPreview> {
        let id = validate_id(id)?;
        let root_files = self.metadata(std::slice::from_ref(&id)).await?;
        let root = root_files
            .iter()
            .find(|file| {
                file.publishedfileid == id && file.result == 1 && file.file_type == Some(2)
            })
            .ok_or_else(|| {
                AppError::Invalid("This is not an accessible public Workshop collection.".into())
            })?;
        let app_id = root.consumer_app_id.filter(|id| *id > 0).ok_or_else(|| {
            AppError::Invalid("Steam did not identify the collection's game.".into())
        })?;
        let mut preview = WorkshopPreview {
            id: uuid::Uuid::new_v4().to_string(),
            collection_id: Some(id.clone()),
            name: root.title.clone().unwrap_or_else(|| id.clone()),
            app_id: Some(app_id),
            items: vec![],
            warnings: vec![],
        };
        let mut collections = HashMap::from([(id.clone(), root.clone())]);
        let mut metadata_cache = collections.clone();
        let mut graph = CollectionWalk::new(id);
        while let Some((collection, depth)) = graph.next() {
            let Some(children) = collections
                .remove(&collection)
                .and_then(|file| file.children)
            else {
                preview
                    .warnings
                    .push(format!("Collection #{collection} is unavailable."));
                continue;
            };
            let ids = unique_ids(
                children
                    .into_iter()
                    .map(|child| child.publishedfileid)
                    .collect(),
            )?;
            let missing = ids
                .iter()
                .filter(|id| !metadata_cache.contains_key(*id))
                .cloned()
                .collect::<Vec<_>>();
            if metadata_cache.len() + missing.len() > 2128 {
                return Err(AppError::Invalid(
                    "Collection exceeds the safe metadata limit.".into(),
                ));
            }
            for file in self.metadata(&missing).await? {
                metadata_cache.insert(file.publishedfileid.clone(), file);
            }
            for child in ids {
                let file = metadata_cache.get(&child);
                if file.is_some_and(|file| file.result == 1 && file.file_type == Some(2)) {
                    if file.and_then(|file| file.consumer_app_id) != Some(app_id) {
                        preview.warnings.push(format!(
                            "Nested collection #{child} belongs to another game."
                        ));
                        continue;
                    }
                    collections.insert(child.clone(), file.unwrap().clone());
                    if !graph.collection(child.clone(), depth + 1)? {
                        preview.warnings.push(format!(
                            "Repeated or cyclic collection #{child} was not traversed again."
                        ));
                    }
                } else if graph.item(&child)? {
                    preview
                        .items
                        .push(preview_entry(&child, file, Some(app_id)));
                }
            }
        }
        Ok(preview)
    }
}

fn apply_collection_details(response: &mut DetailsEnvelope, collections: CollectionEnvelope) {
    for collection in collections
        .response
        .collectiondetails
        .into_iter()
        .filter(|c| c.result == 1)
    {
        if let Some(file) = response
            .response
            .publishedfiledetails
            .iter_mut()
            .find(|file| file.publishedfileid == collection.publishedfileid)
        {
            file.file_type = Some(2);
            file.children = Some(collection.children);
        }
    }
}

fn extract_item(envelope: DetailsEnvelope, id: &str, downloadable: bool) -> Result<ResolvedItem> {
    let item = envelope
        .response
        .publishedfiledetails
        .into_iter()
        .find(|item| item.publishedfileid == id && item.result == 1)
        .ok_or_else(|| {
            AppError::Invalid("Steam did not return public metadata for this Workshop item.".into())
        })?;
    if downloadable && item.file_type == Some(2) {
        return Err(AppError::Invalid(
            "This item is a collection. Preview its contents before downloading.".into(),
        ));
    }
    let app_id = item.consumer_app_id.filter(|id| *id > 0).ok_or_else(|| {
        AppError::Invalid("Steam did not identify the Workshop item's game.".into())
    })?;
    let size_bytes = item.size_bytes();
    let title = item
        .title
        .filter(|title| !title.trim().is_empty())
        .unwrap_or_else(|| id.into());
    Ok(ResolvedItem {
        item: WorkshopItem {
            id: id.into(),
            app_id,
            title,
        },
        updated_at: item.time_updated,
        size_bytes,
    })
}

pub fn validate_id(id: &str) -> Result<String> {
    if id.len() > 20
        || id.is_empty()
        || !id.bytes().all(|b| b.is_ascii_digit())
        || id.parse::<u64>().ok().filter(|id| *id > 0).is_none()
    {
        return Err(AppError::Invalid(
            "Enter a valid positive Workshop ID.".into(),
        ));
    }
    Ok(id.parse::<u64>().unwrap().to_string())
}
fn unique_ids(ids: Vec<String>) -> Result<Vec<String>> {
    if ids.len() > 2000 {
        return Err(AppError::Invalid(
            "At most 2,000 Workshop items can be previewed at once.".into(),
        ));
    }
    let mut seen = HashSet::new();
    ids.into_iter()
        .map(|id| validate_id(&id))
        .filter_map(|value| match value {
            Ok(id) if seen.insert(id.clone()) => Some(Ok(id)),
            Err(error) => Some(Err(error)),
            _ => None,
        })
        .collect()
}
fn preview_entry(id: &str, file: Option<&PublishedFile>, app: Option<u32>) -> PreviewEntry {
    let mut entry = PreviewEntry {
        id: id.into(),
        item: None,
        updated_at: None,
        size_bytes: None,
        unavailable: None,
        status: Default::default(),
    };
    let Some(file) = file.filter(|f| f.result == 1) else {
        entry.unavailable = Some("Private, deleted or unavailable".into());
        return entry;
    };
    if file.file_type == Some(2) {
        entry.unavailable = Some("Collection: open its page to preview contents".into());
        return entry;
    }
    let Some(app_id) = file.consumer_app_id.filter(|id| *id > 0) else {
        entry.unavailable = Some("Game metadata unavailable".into());
        return entry;
    };
    if app.is_some_and(|expected| expected != app_id) {
        entry.unavailable = Some("Item belongs to another game".into());
        return entry;
    }
    entry.item = Some(WorkshopItem {
        id: id.into(),
        app_id,
        title: file
            .title
            .clone()
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| id.into()),
    });
    entry.updated_at = file.time_updated;
    entry.size_bytes = file.size_bytes();
    entry
}
#[derive(Deserialize)]
struct CollectionEnvelope {
    response: CollectionResponse,
}
#[derive(Deserialize)]
struct CollectionResponse {
    collectiondetails: Vec<CollectionDetail>,
}
#[derive(Deserialize)]
struct CollectionDetail {
    publishedfileid: String,
    result: u32,
    #[serde(default)]
    children: Vec<CollectionChild>,
}
#[derive(Clone, Deserialize)]
struct CollectionChild {
    publishedfileid: String,
}
struct CollectionWalk {
    pending: VecDeque<(String, usize)>,
    collections: HashSet<String>,
    items: HashSet<String>,
}
impl CollectionWalk {
    fn new(id: String) -> Self {
        Self {
            pending: VecDeque::from([(id.clone(), 0)]),
            collections: HashSet::from([id]),
            items: HashSet::new(),
        }
    }
    fn next(&mut self) -> Option<(String, usize)> {
        self.pending.pop_front()
    }
    fn collection(&mut self, id: String, depth: usize) -> Result<bool> {
        if depth > 16 || self.collections.len() >= 128 {
            return Err(AppError::Invalid(
                "Collection nesting exceeds the safe limit (16 levels, 128 collections).".into(),
            ));
        }
        if !self.collections.insert(id.clone()) {
            return Ok(false);
        }
        self.pending.push_back((id, depth));
        Ok(true)
    }
    fn item(&mut self, id: &str) -> Result<bool> {
        if self.items.len() >= 2000 {
            return Err(AppError::Invalid(
                "Collection exceeds the safe limit of 2,000 unique items.".into(),
            ));
        }
        Ok(self.items.insert(id.into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_full_width_item_id_and_ignores_unrelated_query_parameters() {
        let url = parse_address(
            "steamcommunity.com/sharedfiles/filedetails/?searchtext=coast&id=18446744073709551615#comments",
        )
        .unwrap();
        assert_eq!(item_id(&url).as_deref(), Some("18446744073709551615"));
        assert_eq!(
            item_id(
                &parse_address("https://steamcommunity.com/workshop/filedetails?id=00042").unwrap()
            )
            .as_deref(),
            Some("42")
        );
    }

    #[test]
    fn rejects_ambiguous_or_unrelated_ids() {
        for suffix in [
            "/sharedfiles/filedetails?id=1&id=2",
            "/sharedfiles/filedetails?id=0",
            "/sharedfiles/filedetails?id=%2B4",
            "/sharedfiles/filedetails?id=18446744073709551616",
            "/workshop/browse?id=123",
            "/sharedfiles/filedetails-extra?id=123",
        ] {
            assert_eq!(
                item_id(&parse_address(&format!("https://steamcommunity.com{suffix}")).unwrap()),
                None
            );
        }
    }

    #[test]
    fn rejects_spoofed_domains_and_nonstandard_origins() {
        for address in [
            "https://steamcommunity.com.example/workshop/",
            "https://user@steamcommunity.com/workshop/",
            "https://steamcommunity.com:5000/workshop/",
            "http://steamcommunity.com/workshop/",
            "file:///tmp/workshop",
        ] {
            assert!(parse_address(address).is_err(), "{address}");
        }
    }

    #[test]
    fn game_shortcut_opens_the_workshop_item_list() {
        let url = game_url(294100).unwrap();
        assert_eq!(
            url.as_str(),
            "https://steamcommunity.com/workshop/browse/?appid=294100"
        );
        assert_eq!(app_id(&url), Some(294100));
        assert!(game_url(0).is_err());
    }

    #[test]
    fn reads_app_id_from_workshop_hub_url() {
        let url = parse_address("https://steamcommunity.com/app/4000/workshop/").unwrap();
        assert_eq!(app_id(&url), Some(4000));
    }

    #[test]
    fn permits_steam_sign_in_redirects_without_widening_workshop_address_input() {
        for address in [
            "https://steamcommunity.com/login/home/",
            "https://login.steampowered.com/jwt/finalizelogin",
            "https://store.steampowered.com/login/",
        ] {
            assert!(is_browser_url(&Url::parse(address).unwrap()));
        }
        for address in [
            "https://store.steampowered.com/app/294100/",
            "https://store.steampowered.com/login-lookalike",
            "https://login.steampowered.com.example/login/",
            "https://login.steampowered.com:444/login/",
            "http://login.steampowered.com/login/",
        ] {
            assert!(!is_browser_url(&Url::parse(address).unwrap()));
        }
        assert!(parse_address("https://store.steampowered.com/login/").is_err());
    }

    #[test]
    fn resolves_consumer_app_and_requires_success() {
        let json = r#"{"response":{"publishedfiledetails":[
            {"publishedfileid":"123","result":1,"creator_app_id":766,"consumer_app_id":4000,"title":"Coast"},
            {"publishedfileid":"124","result":9}
        ]}}"#;
        assert_eq!(
            extract_item(serde_json::from_str(json).unwrap(), "123", false)
                .unwrap()
                .item
                .app_id,
            4000
        );
        assert!(extract_item(serde_json::from_str(json).unwrap(), "124", false).is_err());
    }
}

#[cfg(test)]
mod collection_tests {
    use super::*;
    #[test]
    fn collection_details_parse_and_ids_are_strict_and_deduplicated() {
        let response:CollectionEnvelope=serde_json::from_str(r#"{"response":{"collectiondetails":[{"publishedfileid":"99","result":1,"children":[{"publishedfileid":"1","filetype":0},{"publishedfileid":"1","filetype":0},{"publishedfileid":"2","filetype":2}]}]}}"#).unwrap();
        let ids = unique_ids(
            response.response.collectiondetails[0]
                .children
                .iter()
                .map(|c| c.publishedfileid.clone())
                .collect(),
        )
        .unwrap();
        assert_eq!(ids, ["1", "2"]);
        for id in ["0", "-1", "+1", "1/2", "18446744073709551616", ""] {
            assert!(validate_id(id).is_err());
        }
        assert!(unique_ids(vec!["1".into(); 2001]).is_err());
    }
    #[test]
    fn official_collection_details_classify_metadata_without_a_file_type() {
        let mut metadata:DetailsEnvelope=serde_json::from_str(r#"{"response":{"publishedfiledetails":[{"publishedfileid":"99","result":1,"consumer_app_id":322330,"title":"A collection"}]}}"#).unwrap();
        let collections:CollectionEnvelope=serde_json::from_str(r#"{"response":{"collectiondetails":[{"publishedfileid":"99","result":1,"children":[{"publishedfileid":"1","filetype":0}]}]}}"#).unwrap();
        apply_collection_details(&mut metadata, collections);
        assert_eq!(metadata.response.publishedfiledetails[0].file_type, Some(2));
        assert_eq!(
            metadata.response.publishedfiledetails[0]
                .children
                .as_ref()
                .unwrap()
                .len(),
            1
        );
        assert!(extract_item(metadata, "99", true).is_err());
    }
    #[test]
    fn nested_cycles_and_diamonds_visit_each_collection_and_item_once() {
        let mut graph = CollectionWalk::new("1".into());
        assert_eq!(graph.next(), Some(("1".into(), 0)));
        assert!(graph.collection("2".into(), 1).unwrap());
        assert!(!graph.collection("1".into(), 2).unwrap());
        assert!(!graph.collection("2".into(), 2).unwrap());
        assert!(graph.item("3").unwrap());
        assert!(!graph.item("3").unwrap());
        assert!(graph.collection("4".into(), 17).is_err());
        assert_eq!(graph.next(), Some(("2".into(), 1)));
        assert_eq!(graph.next(), None);
    }
    #[test]
    fn unavailable_collections_and_cross_game_items_cannot_be_mod_jobs() {
        let files:DetailsEnvelope=serde_json::from_str(r#"{"response":{"publishedfiledetails":[{"publishedfileid":"1","result":9},{"publishedfileid":"2","result":1,"consumer_app_id":4000,"title":"Other game"},{"publishedfileid":"3","result":1,"consumer_app_id":294100,"file_type":2}]}}"#).unwrap();
        assert!(
            preview_entry(
                "1",
                files.response.publishedfiledetails.first(),
                Some(294100)
            )
            .unavailable
            .is_some()
        );
        assert!(
            preview_entry(
                "2",
                files.response.publishedfiledetails.get(1),
                Some(294100)
            )
            .item
            .is_none()
        );
        assert!(
            preview_entry(
                "3",
                files.response.publishedfiledetails.get(2),
                Some(294100)
            )
            .item
            .is_none()
        );
        assert!(extract_item(files, "3", true).is_err());
        assert!(preview_entry("4", None, None).unavailable.is_some());
    }
}

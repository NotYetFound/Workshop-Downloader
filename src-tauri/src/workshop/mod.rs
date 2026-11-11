pub mod commands;
pub mod external;

use crate::{
    downloads::{DownloadJob, DownloadStatus},
    steam::WorkshopItem,
};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ContentStatus {
    #[default]
    NotDownloaded,
    Queued,
    Downloading,
    Downloaded,
    DownloadedExternally,
    MissingFiles,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PreviewEntry {
    pub id: String,
    pub item: Option<WorkshopItem>,
    pub updated_at: Option<u32>,
    #[serde(default)]
    #[ts(type = "number | null")]
    pub size_bytes: Option<u64>,
    pub unavailable: Option<String>,
    #[serde(default)]
    pub status: ContentStatus,
}

#[derive(Clone, Debug, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct WorkshopPreview {
    pub id: String,
    pub collection_id: Option<String>,
    pub name: String,
    pub app_id: Option<u32>,
    pub items: Vec<PreviewEntry>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BatchEntry {
    pub item_id: String,
    pub title: String,
    pub job_id: Option<String>,
    pub skipped: bool,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DownloadBatch {
    pub id: String,
    pub collection_id: Option<String>,
    pub name: String,
    pub created_at: String,
    pub items: Vec<BatchEntry>,
}

#[derive(Clone, Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BatchProgress {
    pub batch: DownloadBatch,
    pub total: usize,
    pub completed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub cancelled: usize,
}
impl BatchProgress {
    pub fn new(batch: DownloadBatch, jobs: &[DownloadJob]) -> Self {
        let mut result = Self {
            total: batch.items.len(),
            completed: 0,
            failed: 0,
            skipped: 0,
            cancelled: 0,
            batch,
        };
        for entry in &result.batch.items {
            if entry.skipped {
                result.skipped += 1;
                continue;
            }
            match entry
                .job_id
                .as_ref()
                .and_then(|id| jobs.iter().find(|job| &job.id == id))
                .map(|job| job.status)
            {
                Some(DownloadStatus::Completed) => result.completed += 1,
                Some(DownloadStatus::Failed) => result.failed += 1,
                Some(DownloadStatus::Cancelled) => result.cancelled += 1,
                None => result.failed += 1,
                _ => {}
            }
        }
        result
    }
}

pub(crate) mod files;
mod manager;
mod process;
mod providers;
#[cfg(target_os = "linux")]
pub mod sandbox;

use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use ts_rs::TS;

use crate::{AppError, Result, steam::WorkshopItem};

pub use manager::{DownloadManager, ItemDownloadState};
pub use process::{AuthPrompt, PromptKind};
pub use providers::{BackendInfo, BackendState, official_source};

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum BackendChoice {
    #[default]
    Automatic,
    #[serde(rename = "steamcmd")]
    SteamCmd,
    DepotDownloader,
}

impl BackendChoice {
    pub fn id(self) -> &'static str {
        match self {
            Self::Automatic => "automatic",
            Self::SteamCmd => "steamcmd",
            Self::DepotDownloader => "depotDownloader",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Automatic => "Automatic",
            Self::SteamCmd => "SteamCMD",
            Self::DepotDownloader => "DepotDownloader",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum DownloadStatus {
    Queued,
    Preparing,
    Authenticating,
    Downloading,
    Finalizing,
    Completed,
    Failed,
    Cancelled,
}

impl DownloadStatus {
    pub fn is_active(self) -> bool {
        matches!(
            self,
            Self::Preparing | Self::Authenticating | Self::Downloading | Self::Finalizing
        )
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum FailureKind {
    BackendUnavailable,
    Unsupported,
    Authentication,
    Ownership,
    InvalidItem,
    Network,
    Files,
    Interrupted,
    Process,
}

impl FailureKind {
    fn can_fallback(self) -> bool {
        matches!(self, Self::BackendUnavailable | Self::Unsupported)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DownloadJob {
    pub id: String,
    pub item: WorkshopItem,
    pub provider: String,
    pub destination: String,
    pub status: DownloadStatus,
    pub created_at: String,
    pub updated_at: String,
    pub error: Option<String>,
    #[serde(default)]
    pub requested_backend: BackendChoice,
    #[serde(default)]
    pub source_url: String,
    #[serde(default)]
    pub downloaded_at: Option<String>,
    #[serde(default)]
    pub remote_updated_at: Option<u32>,
    #[serde(default)]
    pub failure_kind: Option<FailureKind>,
    #[serde(default)]
    pub percent: Option<f32>,
    #[serde(default)]
    #[ts(type = "number | null")]
    pub downloaded_bytes: Option<u64>,
    #[serde(default)]
    #[ts(type = "number | null")]
    pub expected_bytes: Option<u64>,
    #[serde(default)]
    #[ts(type = "number | null")]
    pub bytes_per_second: Option<u64>,
    #[serde(default)]
    pub replace: bool,
    #[serde(default)]
    pub account_required: bool,
    #[serde(default)]
    #[ts(skip)]
    pub previous_install: Option<InstalledItem>,
}

impl DownloadJob {
    pub fn new(item: WorkshopItem, options: &QueueOptions, remote_updated_at: Option<u32>) -> Self {
        let now = timestamp();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            source_url: crate::steam::item_url(&item.id),
            item,
            provider: options.backend.id().into(),
            requested_backend: options.backend,
            destination: String::new(),
            status: DownloadStatus::Queued,
            created_at: now.clone(),
            updated_at: now,
            downloaded_at: None,
            remote_updated_at,
            error: None,
            failure_kind: None,
            percent: None,
            downloaded_bytes: None,
            expected_bytes: None,
            bytes_per_second: None,
            replace: options.replace,
            account_required: options.account.is_some(),
            previous_install: None,
        }
    }

    fn transition(&mut self, status: DownloadStatus) -> Result<()> {
        use DownloadStatus::*;
        if !matches!(
            (self.status, status),
            (Queued, Preparing | Cancelled)
                | (Preparing, Authenticating | Downloading | Failed | Cancelled)
                | (Authenticating, Preparing | Downloading | Failed | Cancelled)
                | (
                    Downloading,
                    Preparing | Authenticating | Finalizing | Failed | Cancelled
                )
                | (Finalizing, Completed | Failed | Cancelled)
                | (Failed | Cancelled, Queued)
        ) {
            return Err(AppError::Invalid(
                "Invalid download state transition.".into(),
            ));
        }
        self.status = status;
        self.updated_at = timestamp();
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
pub struct QueueOptions {
    pub backend: BackendChoice,
    pub replace: bool,
    pub account: Option<String>,
}

impl QueueOptions {
    fn validate(&self) -> Result<()> {
        if let Some(account) = &self.account
            && (account.is_empty()
                || account.len() > 64
                || account.starts_with(['-', '+'])
                || !account
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_'))
        {
            return Err(AppError::Invalid(
                "Enter a valid Steam account name.".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct InstalledItem {
    pub job_id: String,
    pub app_id: u32,
    pub item_id: String,
    pub path: String,
}

#[derive(Clone, Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DownloadLog {
    pub time: String,
    pub message: String,
}

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
struct ProviderError {
    kind: FailureKind,
    message: String,
}

impl ProviderError {
    fn new(kind: FailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

fn valid_job_id(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| id.to_string() == value)
}

pub(crate) fn timestamp() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .expect("current UTC time is representable")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transitions_reject_skipping_verification() {
        let item = WorkshopItem {
            id: "9".into(),
            app_id: 1,
            title: "Test".into(),
        };
        let mut job = DownloadJob::new(item, &QueueOptions::default(), None);
        assert!(job.transition(DownloadStatus::Completed).is_err());
        for status in [
            DownloadStatus::Preparing,
            DownloadStatus::Authenticating,
            DownloadStatus::Downloading,
            DownloadStatus::Finalizing,
            DownloadStatus::Completed,
        ] {
            job.transition(status).unwrap();
        }
        assert!(job.transition(DownloadStatus::Cancelled).is_err());
    }
}

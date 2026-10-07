use super::*;

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct MediaPolicyDto {
    pub auto_archive: bool,
}

impl From<MediaPolicy> for MediaPolicyDto {
    fn from(value: MediaPolicy) -> Self {
        Self {
            auto_archive: value.auto_archive,
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct MediaDownloadDto {
    pub id: i64,
    pub ordinal: i64,
    pub variant: String,
    pub state: String,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub byte_size: Option<i64>,
    pub next_attempt_at: Option<i64>,
    pub content_url: Option<String>,
    pub last_error: Option<String>,
}

impl From<MediaDownload> for MediaDownloadDto {
    fn from(value: MediaDownload) -> Self {
        let content_url =
            (value.state == "succeeded").then(|| format!("/api/v1/media/{}/content", value.id));
        Self {
            id: value.id,
            ordinal: value.ordinal,
            variant: value.variant,
            state: value.state,
            width: value.width,
            height: value.height,
            byte_size: value.byte_size,
            next_attempt_at: value.next_attempt_at,
            content_url,
            last_error: value.last_error,
        }
    }
}

/// Cumulative current image downloads, grouped by chat and variant. Retrying is a subset of failed.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct MediaProgressDto {
    pub chat_id: i64,
    pub title: Option<String>,
    pub variant: String,
    pub total: i64,
    pub queued: i64,
    pub running: i64,
    pub succeeded: i64,
    pub failed: i64,
    pub interrupted: i64,
    pub unavailable: i64,
    pub superseded: i64,
    pub retrying: i64,
}

impl From<crate::infrastructure::persistence::sqlite::media::MediaProgress> for MediaProgressDto {
    fn from(value: crate::infrastructure::persistence::sqlite::media::MediaProgress) -> Self {
        Self {
            chat_id: value.chat_id,
            title: value.title,
            variant: value.variant,
            total: value.total,
            queued: value.queued,
            running: value.running,
            succeeded: value.succeeded,
            failed: value.failed,
            interrupted: value.interrupted,
            unavailable: value.unavailable,
            superseded: value.superseded,
            retrying: value.retrying,
        }
    }
}

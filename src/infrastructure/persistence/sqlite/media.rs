use sqlx::{Row, Sqlite, Transaction};

use super::{
    storage_error,
    store::{SqliteStore, invalid_data},
};
use crate::application::RepositoryError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaDownload {
    pub id: i64,
    pub chat_id: i64,
    pub message_id: i64,
    pub ordinal: i64,
    pub telegram_media_id: String,
    pub media_kind: String,
    pub variant: String,
    pub trigger: String,
    pub state: String,
    pub relative_path: Option<String>,
    pub content_type: Option<String>,
    pub byte_size: Option<i64>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub attempts: i64,
    pub next_attempt_at: Option<i64>,
    pub last_error: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MediaPolicy {
    pub auto_archive: bool,
}

fn from_row(row: &sqlx::sqlite::SqliteRow) -> Result<MediaDownload, RepositoryError> {
    Ok(MediaDownload {
        id: row.try_get("id").map_err(storage_error)?,
        chat_id: row.try_get("chat_id").map_err(storage_error)?,
        message_id: row.try_get("message_id").map_err(storage_error)?,
        ordinal: row.try_get("ordinal").map_err(storage_error)?,
        telegram_media_id: row.try_get("telegram_media_id").map_err(storage_error)?,
        media_kind: row.try_get("media_kind").map_err(storage_error)?,
        variant: row.try_get("variant").map_err(storage_error)?,
        trigger: row.try_get("trigger").map_err(storage_error)?,
        state: row.try_get("state").map_err(storage_error)?,
        relative_path: row.try_get("relative_path").map_err(storage_error)?,
        content_type: row.try_get("content_type").map_err(storage_error)?,
        byte_size: row.try_get("byte_size").map_err(storage_error)?,
        width: row.try_get("width").map_err(storage_error)?,
        height: row.try_get("height").map_err(storage_error)?,
        attempts: row.try_get("attempts").map_err(storage_error)?,
        next_attempt_at: row.try_get("next_attempt_at").map_err(storage_error)?,
        last_error: row.try_get("last_error").map_err(storage_error)?,
    })
}

const COLUMNS: &str = "id, chat_id, message_id, ordinal, telegram_media_id, media_kind, variant, trigger, state, relative_path, content_type, byte_size, width, height, attempts, next_attempt_at, last_error";

impl SqliteStore {
    pub async fn get_media_policy(&self, chat_id: i64) -> Result<MediaPolicy, RepositoryError> {
        if self.pre_0009_media {
            return Ok(MediaPolicy::default());
        }
        let value: Option<i64> =
            sqlx::query_scalar("SELECT auto_archive FROM media_policies WHERE chat_id=?")
                .bind(chat_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(MediaPolicy {
            auto_archive: value.unwrap_or(0) != 0,
        })
    }

    pub async fn set_media_policy(
        &self,
        chat_id: i64,
        policy: MediaPolicy,
    ) -> Result<(), RepositoryError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("INSERT INTO media_policies(chat_id, auto_archive) VALUES (?, ?) ON CONFLICT(chat_id) DO UPDATE SET auto_archive=excluded.auto_archive")
            .bind(chat_id).bind(i64::from(policy.auto_archive)).execute(&mut *tx).await?;
        if !policy.auto_archive {
            sqlx::query("UPDATE media_downloads SET state='interrupted', last_error='automatic archive disabled' WHERE chat_id=? AND variant='archive' AND trigger='auto' AND state IN ('queued','running')")
                .bind(chat_id).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn message_media(
        &self,
        chat_id: i64,
        message_id: i64,
    ) -> Result<Vec<MediaDownload>, RepositoryError> {
        if self.pre_0009_media {
            return Ok(Vec::new());
        }
        let rows = sqlx::query(&format!("SELECT d.{0} FROM media_downloads d JOIN messages m ON m.chat_id=d.chat_id AND m.message_id=d.message_id JOIN attachments a ON a.message_row_id=m.row_id AND a.ordinal=d.ordinal WHERE d.chat_id=? AND d.message_id=? AND m.is_deleted=0 AND a.telegram_file_id=d.telegram_media_id AND a.kind=d.media_kind ORDER BY d.ordinal, d.variant", COLUMNS.replace(", ", ", d.")))
            .bind(chat_id).bind(message_id).fetch_all(&self.pool).await?;
        rows.iter().map(from_row).collect()
    }

    pub async fn get_media(&self, id: i64) -> Result<Option<MediaDownload>, RepositoryError> {
        if self.pre_0009_media {
            return Ok(None);
        }
        let rows = sqlx::query(&format!("SELECT d.{0} FROM media_downloads d JOIN messages m ON m.chat_id=d.chat_id AND m.message_id=d.message_id JOIN attachments a ON a.message_row_id=m.row_id AND a.ordinal=d.ordinal WHERE d.id=? AND m.is_deleted=0 AND a.telegram_file_id=d.telegram_media_id AND a.kind=d.media_kind", COLUMNS.replace(", ", ", d.")))
            .bind(id).fetch_optional(&self.pool).await?;
        rows.as_ref().map(from_row).transpose()
    }

    pub async fn request_archive(
        &self,
        preview_id: i64,
    ) -> Result<Option<MediaDownload>, RepositoryError> {
        let mut tx = self.pool.begin().await?;
        let identity: Option<(i64, i64, i64, String, String)> = sqlx::query_as("SELECT d.chat_id,d.message_id,d.ordinal,d.media_kind,d.telegram_media_id FROM media_downloads d JOIN messages m ON m.chat_id=d.chat_id AND m.message_id=d.message_id JOIN attachments a ON a.message_row_id=m.row_id AND a.ordinal=d.ordinal WHERE d.id=? AND d.variant='preview' AND m.is_deleted=0 AND a.kind=d.media_kind AND a.telegram_file_id=d.telegram_media_id")
            .bind(preview_id).fetch_optional(&mut *tx).await?;
        let Some((chat, message, ordinal, kind, media_id)) = identity else {
            return Ok(None);
        };
        sqlx::query("INSERT INTO media_downloads(chat_id,message_id,ordinal,telegram_media_id,media_kind,variant,trigger,state) SELECT d.chat_id,d.message_id,d.ordinal,d.telegram_media_id,d.media_kind,'archive','manual','queued' FROM media_downloads d JOIN messages m ON m.chat_id=d.chat_id AND m.message_id=d.message_id JOIN attachments a ON a.message_row_id=m.row_id AND a.ordinal=d.ordinal WHERE d.id=? AND m.is_deleted=0 AND a.telegram_file_id=d.telegram_media_id AND a.kind=d.media_kind ON CONFLICT(chat_id,message_id,ordinal,media_kind,telegram_media_id,variant) DO UPDATE SET trigger='manual', state=CASE WHEN media_downloads.state IN ('failed','interrupted','unavailable') THEN 'queued' ELSE media_downloads.state END, next_attempt_at=NULL")
            .bind(preview_id).execute(&mut *tx).await?;
        let row = sqlx::query(&format!("SELECT {COLUMNS} FROM media_downloads WHERE chat_id=? AND message_id=? AND ordinal=? AND media_kind=? AND telegram_media_id=? AND variant='archive'"))
            .bind(chat).bind(message).bind(ordinal).bind(kind).bind(media_id).fetch_optional(&mut *tx).await?;
        tx.commit().await?;
        row.as_ref().map(from_row).transpose()
    }

    pub async fn enqueue_media(
        &self,
        chat_id: i64,
        variant: &str,
    ) -> Result<Vec<MediaDownload>, RepositoryError> {
        if !matches!(variant, "preview" | "archive") {
            return Err(invalid_data("media variant must be preview or archive"));
        }
        let mut tx = self.pool.begin().await?;
        let trigger = "manual";
        sqlx::query("INSERT INTO media_downloads(chat_id,message_id,ordinal,telegram_media_id,media_kind,variant,trigger) SELECT m.chat_id,m.message_id,a.ordinal,a.telegram_file_id,a.kind,?1,?2 FROM messages m JOIN attachments a ON a.message_row_id=m.row_id JOIN chats c ON c.id=m.chat_id WHERE m.chat_id=?3 AND m.is_deleted=0 AND c.tracked=1 AND a.telegram_file_id IS NOT NULL AND (a.kind='photo' OR (a.kind='document' AND a.mime_type IN ('image/jpeg','image/png','image/webp'))) ON CONFLICT(chat_id,message_id,ordinal,media_kind,telegram_media_id,variant) DO UPDATE SET trigger='manual', state=CASE WHEN media_downloads.state IN ('failed','interrupted','unavailable') THEN 'queued' ELSE media_downloads.state END, next_attempt_at=NULL")
            .bind(variant).bind(trigger).bind(chat_id).execute(&mut *tx).await?;
        tx.commit().await?;
        self.all_media_for_chat(chat_id).await
    }

    async fn all_media_for_chat(
        &self,
        chat_id: i64,
    ) -> Result<Vec<MediaDownload>, RepositoryError> {
        let rows = sqlx::query(&format!("SELECT d.{0} FROM media_downloads d JOIN messages m ON m.chat_id=d.chat_id AND m.message_id=d.message_id JOIN attachments a ON a.message_row_id=m.row_id AND a.ordinal=d.ordinal WHERE d.chat_id=? AND m.is_deleted=0 AND a.kind=d.media_kind AND a.telegram_file_id=d.telegram_media_id ORDER BY d.message_id,d.ordinal,d.variant", COLUMNS.replace(", ", ", d.")))
            .bind(chat_id).fetch_all(&self.pool).await?;
        rows.iter().map(from_row).collect()
    }

    pub async fn claim_media(&self) -> Result<Option<MediaDownload>, RepositoryError> {
        let row = sqlx::query(&format!("UPDATE media_downloads SET state='running',attempts=attempts+1,last_error=NULL WHERE id=(SELECT id FROM media_downloads WHERE state='queued' OR (state='failed' AND next_attempt_at IS NOT NULL AND next_attempt_at<=unixepoch()) ORDER BY id LIMIT 1) AND (state='queued' OR (state='failed' AND next_attempt_at IS NOT NULL AND next_attempt_at<=unixepoch())) RETURNING {COLUMNS}"))
            .fetch_optional(&self.pool).await?;
        row.as_ref().map(from_row).transpose()
    }

    pub async fn media_is_current(&self, job: &MediaDownload) -> Result<bool, RepositoryError> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM media_downloads d JOIN messages m ON m.chat_id=d.chat_id AND m.message_id=d.message_id JOIN attachments a ON a.message_row_id=m.row_id AND a.ordinal=d.ordinal WHERE d.id=? AND d.chat_id=? AND d.message_id=? AND d.ordinal=? AND d.telegram_media_id=? AND d.media_kind=? AND m.is_deleted=0 AND a.telegram_file_id=d.telegram_media_id AND a.kind=d.media_kind")
            .bind(job.id).bind(job.chat_id).bind(job.message_id).bind(job.ordinal).bind(&job.telegram_media_id).bind(&job.media_kind).fetch_one(&self.pool).await?;
        Ok(count != 0)
    }

    pub async fn media_allowed(&self, job: &MediaDownload) -> Result<bool, RepositoryError> {
        let allowed: Option<i64> = sqlx::query_scalar("SELECT (c.tracked AND (d.variant='preview' OR d.trigger='manual' OR COALESCE(p.auto_archive,0)=1)) OR (d.variant='archive' AND d.trigger='manual') FROM media_downloads d JOIN chats c ON c.id=d.chat_id LEFT JOIN media_policies p ON p.chat_id=d.chat_id WHERE d.id=?")
            .bind(job.id).fetch_optional(&self.pool).await?;
        Ok(allowed.unwrap_or(0) != 0)
    }

    pub async fn finish_media(
        &self,
        id: i64,
        path: &str,
        mime: &str,
        bytes: i64,
        width: Option<i32>,
        height: Option<i32>,
    ) -> Result<(), RepositoryError> {
        let result = sqlx::query("UPDATE media_downloads SET state='succeeded',relative_path=?,content_type=?,byte_size=?,width=?,height=?,next_attempt_at=NULL,last_error=NULL WHERE id=? AND state='running' AND EXISTS (SELECT 1 FROM messages m JOIN attachments a ON a.message_row_id=m.row_id WHERE m.chat_id=media_downloads.chat_id AND m.message_id=media_downloads.message_id AND m.is_deleted=0 AND a.ordinal=media_downloads.ordinal AND a.kind=media_downloads.media_kind AND a.telegram_file_id=media_downloads.telegram_media_id) AND EXISTS (SELECT 1 FROM chats c LEFT JOIN media_policies p ON p.chat_id=c.id WHERE c.id=media_downloads.chat_id AND ((c.tracked AND (media_downloads.variant='preview' OR media_downloads.trigger='manual' OR COALESCE(p.auto_archive,0)=1)) OR (media_downloads.variant='archive' AND media_downloads.trigger='manual'))) ")
            .bind(path).bind(mime).bind(bytes).bind(width).bind(height).bind(id).execute(&self.pool).await?;
        if result.rows_affected() == 0 {
            return Err(invalid_data(
                "media download is no longer current or allowed",
            ));
        }
        Ok(())
    }

    pub async fn fail_media(
        &self,
        id: i64,
        state: &str,
        error: &str,
        retry_at: Option<i64>,
    ) -> Result<(), RepositoryError> {
        if !matches!(
            state,
            "failed" | "interrupted" | "unavailable" | "superseded"
        ) {
            return Err(invalid_data("invalid media failure state"));
        }
        sqlx::query("UPDATE media_downloads SET state=?,last_error=?,next_attempt_at=? WHERE id=? AND state!='succeeded'")
            .bind(state).bind(error).bind(retry_at).bind(id).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn retry_media(&self, id: i64) -> Result<Option<MediaDownload>, RepositoryError> {
        let Some(job) = self.get_media(id).await? else {
            return Ok(None);
        };
        if !self.media_allowed(&job).await? {
            return Ok(None);
        }
        sqlx::query("UPDATE media_downloads SET state='queued',attempts=0,next_attempt_at=NULL,last_error=NULL WHERE id=? AND state IN ('failed','interrupted','unavailable','superseded')")
            .bind(id).execute(&self.pool).await?;
        self.get_media(id).await
    }

    pub async fn recover_media(&self) -> Result<(), RepositoryError> {
        sqlx::query(
            "UPDATE media_downloads SET state='queued',next_attempt_at=NULL WHERE state='running' OR (state='interrupted' AND last_error='worker stopped')",
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

pub(super) async fn enqueue_attachment_downloads(
    tx: &mut Transaction<'_, Sqlite>,
    chat_id: i64,
    message_id: i64,
    row_id: i64,
) -> Result<(), RepositoryError> {
    let tracked: i64 = sqlx::query_scalar("SELECT tracked FROM chats WHERE id=?")
        .bind(chat_id)
        .fetch_one(&mut **tx)
        .await?;
    if tracked == 0 {
        return Ok(());
    }
    let auto_archive: i64 = sqlx::query_scalar(
        "SELECT COALESCE((SELECT auto_archive FROM media_policies WHERE chat_id=?),0)",
    )
    .bind(chat_id)
    .fetch_one(&mut **tx)
    .await?;
    let attachments = sqlx::query("SELECT ordinal,kind,telegram_file_id,mime_type FROM attachments WHERE message_row_id=? ORDER BY ordinal")
        .bind(row_id).fetch_all(&mut **tx).await?;
    for a in attachments {
        let Some(file_id) = a
            .try_get::<Option<String>, _>("telegram_file_id")
            .map_err(storage_error)?
        else {
            continue;
        };
        let db_kind: String = a.try_get("kind").map_err(storage_error)?;
        let mime: Option<String> = a.try_get("mime_type").map_err(storage_error)?;
        let kind = match db_kind.as_str() {
            "photo" => "photo",
            "document"
                if matches!(
                    mime.as_deref(),
                    Some("image/jpeg" | "image/png" | "image/webp")
                ) =>
            {
                "document"
            }
            _ => continue,
        };
        let ordinal: i64 = a.try_get("ordinal").map_err(storage_error)?;
        for variant in ["preview"]
            .into_iter()
            .chain((auto_archive != 0).then_some("archive"))
        {
            sqlx::query("INSERT INTO media_downloads(chat_id,message_id,ordinal,telegram_media_id,media_kind,variant,trigger,state) VALUES (?,?,?,?,?,?, 'auto','queued') ON CONFLICT(chat_id,message_id,ordinal,media_kind,telegram_media_id,variant) DO NOTHING")
                .bind(chat_id).bind(message_id).bind(ordinal).bind(&file_id).bind(kind).bind(variant).execute(&mut **tx).await?;
        }
    }
    Ok(())
}

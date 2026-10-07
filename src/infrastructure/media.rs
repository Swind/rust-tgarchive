//! Durable single-worker media downloader.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use tokio::{io::AsyncWriteExt, task::JoinHandle};
use tokio_util::sync::CancellationToken;

use crate::{
    application::pacer::RatePacer,
    infrastructure::{
        persistence::sqlite::{SqliteStore, media::MediaDownload},
        telegram::{TelegramAdapter, media::MediaFetchError},
    },
};

const MAX_BYTES: usize = 20 * 1024 * 1024;
const MAX_ATTEMPTS: i64 = 5;

pub fn checked_media_size(current: usize, chunk: usize) -> Option<usize> {
    current.checked_add(chunk).filter(|size| *size <= MAX_BYTES)
}

pub fn detect_image_type(bytes: &[u8]) -> Option<(&'static str, &'static str)> {
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some(("image/jpeg", "jpg"))
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(("image/png", "png"))
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(("image/webp", "webp"))
    } else {
        None
    }
}

#[cfg(test)]
#[path = "media/tests.rs"]
mod tests;

pub struct MediaWorker;

impl MediaWorker {
    pub fn spawn(
        adapter: Arc<TelegramAdapter>,
        store: SqliteStore,
        directory: PathBuf,
        pacer: Arc<RatePacer>,
        cancel: CancellationToken,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            if let Err(error) = tokio::fs::create_dir_all(&directory).await {
                tracing::error!(%error, "cannot create media directory");
                return;
            }
            if let Err(error) = cleanup_partials(&directory).await {
                tracing::error!(%error, "cannot clean partial media files");
                return;
            }
            if let Err(error) = store.recover_media().await {
                tracing::error!(%error, "cannot recover media jobs");
                return;
            }
            if let Err(error) = reconcile_completed(&store, &directory).await {
                tracing::error!(%error, "cannot reconcile completed media files");
                return;
            }
            loop {
                if cancel.is_cancelled() {
                    break;
                }
                let job = match store.claim_media().await {
                    Ok(Some(job)) => job,
                    Ok(None) => {
                        tokio::select! { _ = tokio::time::sleep(Duration::from_secs(2)) => {}, _ = cancel.cancelled() => break }
                        continue;
                    }
                    Err(error) => {
                        tracing::error!(%error, "cannot claim media job");
                        tokio::select! { _ = tokio::time::sleep(Duration::from_secs(2)) => {}, _ = cancel.cancelled() => break }
                        continue;
                    }
                };
                if cancel.is_cancelled() {
                    let _ = store
                        .fail_media(job.id, "interrupted", "worker stopped", None)
                        .await;
                    break;
                }
                if let Err(error) =
                    process(&adapter, &store, &directory, &pacer, &cancel, &job).await
                {
                    report_failure(&store, &pacer, &cancel, &job, error).await;
                }
            }
        })
    }
}

async fn cleanup_partials(directory: &Path) -> std::io::Result<()> {
    let mut entries = tokio::fs::read_dir(directory).await?;
    while let Some(entry) = entries.next_entry().await? {
        if entry.file_type().await?.is_file()
            && entry.file_name().to_string_lossy().ends_with(".part")
        {
            tokio::fs::remove_file(entry.path()).await?;
        }
    }
    Ok(())
}

async fn process(
    adapter: &TelegramAdapter,
    store: &SqliteStore,
    directory: &Path,
    pacer: &RatePacer,
    cancel: &CancellationToken,
    job: &MediaDownload,
) -> Result<(), WorkerError> {
    if !store.media_is_current(job).await.map_err(db_error)? {
        return Err(WorkerError::Terminal("media changed"));
    }
    if !store.media_allowed(job).await.map_err(db_error)? {
        return Err(WorkerError::Terminal(
            "media policy no longer allows download",
        ));
    }
    // Reconcile the rename-before-database crash window before asking Telegram again.
    for extension in ["jpg", "png", "webp"] {
        let recovered = directory.join(format!("{}.{}.{extension}", job.id, job.variant));
        if let Ok((mime, _)) = image_type(&recovered).await {
            let size = tokio::fs::metadata(&recovered)
                .await
                .map_err(|_| WorkerError::Io)?
                .len();
            if size <= MAX_BYTES as u64
                && store.media_is_current(job).await.map_err(db_error)?
                && store.media_allowed(job).await.map_err(db_error)?
            {
                store
                    .finish_media(
                        job.id,
                        recovered.file_name().unwrap().to_str().unwrap(),
                        mime,
                        size as i64,
                        None,
                        None,
                    )
                    .await
                    .map_err(db_error)?;
                return Ok(());
            }
            let _ = tokio::fs::remove_file(&recovered).await;
        }
    }
    if pacer.acquire(cancel).await {
        return Err(WorkerError::Stopped);
    }
    let is_photo = job.media_kind == "photo";
    let downloadable = tokio::select! {
        result = adapter.media_for_download(job.chat_id, job.message_id, &job.telegram_media_id, is_photo, job.variant == "archive") => result.map_err(WorkerError::Fetch)?,
        _ = cancel.cancelled() => return Err(WorkerError::Stopped),
    };

    let part_path = directory.join(format!("{}.{}.part", job.id, job.variant));
    let _ = tokio::fs::remove_file(&part_path).await;
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(&part_path)
        .await
        .map_err(|_| WorkerError::Io)?;
    let mut stream = adapter.download_iter(&downloadable);
    let mut bytes = 0usize;
    let transfer = async {
        while let Some(chunk) = stream.next().await.map_err(WorkerError::Telegram)? {
            bytes = checked_media_size(bytes, chunk.len()).ok_or(WorkerError::TooLarge)?;
            file.write_all(&chunk).await.map_err(|_| WorkerError::Io)?;
        }
        file.flush().await.map_err(|_| WorkerError::Io)?;
        file.sync_all().await.map_err(|_| WorkerError::Io)?;
        Ok::<(), WorkerError>(())
    };
    let transfer = tokio::select! { result = transfer => result, _ = cancel.cancelled() => Err(WorkerError::Stopped) };
    if let Err(error) = transfer {
        drop(file);
        let _ = tokio::fs::remove_file(&part_path).await;
        return Err(error);
    }
    drop(file);
    let (mime, ext) = match image_type(&part_path).await {
        Ok(image) => image,
        Err(error) => {
            let _ = tokio::fs::remove_file(&part_path).await;
            return Err(error);
        }
    };
    if cancel.is_cancelled() {
        let _ = tokio::fs::remove_file(&part_path).await;
        return Err(WorkerError::Stopped);
    }
    if !store.media_is_current(job).await.map_err(db_error)?
        || !store.media_allowed(job).await.map_err(db_error)?
    {
        let _ = tokio::fs::remove_file(&part_path).await;
        return Err(WorkerError::Terminal("media policy or identity changed"));
    }
    let final_path = directory.join(format!("{}.{}.{ext}", job.id, job.variant));
    tokio::fs::rename(&part_path, &final_path)
        .await
        .map_err(|_| WorkerError::Io)?;
    // The file is durable before the database points at it.
    tokio::fs::File::open(directory)
        .await
        .map_err(|_| WorkerError::Io)?
        .sync_all()
        .await
        .map_err(|_| WorkerError::Io)?;
    let relative = final_path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let (width, height) = downloadable.dimensions();
    store
        .finish_media(job.id, &relative, mime, bytes as i64, width, height)
        .await
        .map_err(db_error)?;
    pacer.on_success();
    Ok(())
}

async fn reconcile_completed(store: &SqliteStore, directory: &Path) -> Result<(), String> {
    for job in store
        .list_completed_media()
        .await
        .map_err(|e| e.to_string())?
    {
        let Some(name) = job.relative_path.as_deref() else {
            store
                .invalidate_missing_media(job.id)
                .await
                .map_err(|e| e.to_string())?;
            continue;
        };
        let relative = Path::new(name);
        let generated = ["jpg", "png", "webp"]
            .iter()
            .any(|ext| name == format!("{}.{}.{ext}", job.id, job.variant));
        if relative.components().count() != 1 || !generated {
            store
                .invalidate_missing_media(job.id)
                .await
                .map_err(|e| e.to_string())?;
            continue;
        }
        let path = directory.join(relative);
        let valid = match (image_type(&path).await, tokio::fs::metadata(&path).await) {
            (Ok((mime, _)), Ok(meta)) => {
                meta.len() > 0
                    && meta.len() as i64 == job.byte_size.unwrap_or(-1)
                    && Some(mime) == job.content_type.as_deref()
            }
            _ => false,
        };
        if !valid {
            store
                .invalidate_missing_media(job.id)
                .await
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

async fn image_type(path: &Path) -> Result<(&'static str, &'static str), WorkerError> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|_| WorkerError::Io)?;
    let mut head = [0u8; 12];
    let mut n = 0;
    while n < head.len() {
        let read = tokio::io::AsyncReadExt::read(&mut file, &mut head[n..])
            .await
            .map_err(|_| WorkerError::Io)?;
        if read == 0 {
            break;
        }
        n += read;
    }
    detect_image_type(&head[..n]).ok_or(WorkerError::InvalidImage)
}

#[derive(Debug)]
enum WorkerError {
    Io,
    Telegram(grammers_client::InvocationError),
    Fetch(MediaFetchError),
    TooLarge,
    InvalidImage,
    Terminal(&'static str),
    Stopped,
    Database,
}

fn db_error(_: impl std::fmt::Display) -> WorkerError {
    WorkerError::Database
}

async fn report_failure(
    store: &SqliteStore,
    pacer: &RatePacer,
    cancel: &CancellationToken,
    job: &MediaDownload,
    error: WorkerError,
) {
    let now = chrono::Utc::now().timestamp();
    let (state, message, retry_at) = match error {
        WorkerError::Stopped => ("interrupted", "worker stopped".to_owned(), None),
        WorkerError::Terminal(message) => ("interrupted", message.to_owned(), None),
        WorkerError::Fetch(MediaFetchError::Missing) => {
            ("unavailable", "source message unavailable".into(), None)
        }
        WorkerError::Fetch(MediaFetchError::Changed) => {
            ("superseded", "media changed".into(), None)
        }
        WorkerError::Fetch(MediaFetchError::Unavailable) => {
            ("unavailable", "image version unavailable".into(), None)
        }
        WorkerError::Fetch(MediaFetchError::Telegram(grammers_client::InvocationError::Rpc(
            ref rpc,
        )))
        | WorkerError::Telegram(grammers_client::InvocationError::Rpc(ref rpc))
            if rpc.name == "FLOOD_WAIT" =>
        {
            let wait = u64::from(rpc.value.unwrap_or(60));
            pacer.on_flood(wait, true);
            if job.attempts >= MAX_ATTEMPTS {
                (
                    "interrupted",
                    "Telegram rate limit; retry limit reached".into(),
                    None,
                )
            } else {
                (
                    "failed",
                    "Telegram rate limit".into(),
                    Some(now + wait as i64),
                )
            }
        }
        other @ (WorkerError::TooLarge | WorkerError::InvalidImage) => {
            let message = if matches!(other, WorkerError::TooLarge) {
                "file exceeds 20 MiB"
            } else {
                "download is not a supported image"
            };
            ("unavailable", message.into(), None)
        }
        other => {
            let message = match other {
                WorkerError::TooLarge => "file exceeds 20 MiB",
                WorkerError::InvalidImage => "download is not a supported image",
                WorkerError::Io => "media file operation failed",
                WorkerError::Telegram(_) => "Telegram download failed",
                WorkerError::Fetch(_) => "Telegram source unavailable",
                WorkerError::Database => "media database operation failed",
                WorkerError::Terminal(_) | WorkerError::Stopped => unreachable!(),
            };
            if job.attempts >= MAX_ATTEMPTS {
                ("interrupted", message.into(), None)
            } else {
                (
                    "failed",
                    message.into(),
                    Some(now + 2_i64.pow(job.attempts.min(10) as u32)),
                )
            }
        }
    };
    if let Err(db) = store.fail_media(job.id, state, &message, retry_at).await {
        tracing::error!(%db, media_id = job.id, "cannot update failed media job");
    }
    if cancel.is_cancelled() {
        tracing::debug!(media_id = job.id, "media worker cancelled");
    }
}

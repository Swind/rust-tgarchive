//! Bounded media selection from a freshly fetched Telegram message.

use grammers_client::media::{Document, Downloadable, Media, PhotoSize};

use super::session::TelegramAdapter;

pub enum DownloadableMedia {
    Photo {
        size: PhotoSize,
        width: i32,
        height: i32,
    },
    Document(Document),
}

impl DownloadableMedia {
    pub fn dimensions(&self) -> (Option<i32>, Option<i32>) {
        match self {
            Self::Photo { width, height, .. } => (Some(*width), Some(*height)),
            Self::Document(_) => (None, None),
        }
    }
}

impl Downloadable for DownloadableMedia {
    fn to_raw_input_location(&self) -> Option<grammers_client::tl::enums::InputFileLocation> {
        match self {
            Self::Photo { size, .. } => size.to_raw_input_location(),
            Self::Document(document) => document.to_raw_input_location(),
        }
    }
    fn size(&self) -> Option<usize> {
        match self {
            Self::Photo { size, .. } => Some(size.size()),
            Self::Document(document) => document.size(),
        }
    }
    fn to_data(&self) -> Option<Vec<u8>> {
        match self {
            Self::Photo { size, .. } => size.to_data(),
            Self::Document(document) => document.to_data(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum MediaFetchError {
    #[error("source message is missing")]
    Missing,
    #[error("source media no longer matches the queued attachment")]
    Changed,
    #[error("no downloadable image version is available")]
    Unavailable,
    #[error("Telegram request failed: {0}")]
    Telegram(#[from] grammers_client::InvocationError),
    #[error("could not resolve Telegram chat: {0}")]
    Peer(String),
}

impl TelegramAdapter {
    /// Refetches the message to obtain a live file reference, then checks its media identity.
    pub async fn media_for_download(
        &self,
        chat_id: i64,
        message_id: i64,
        expected_media_id: &str,
        is_photo: bool,
        archive: bool,
    ) -> Result<DownloadableMedia, MediaFetchError> {
        let (_, peer_ref) = self
            .resolve_chat(chat_id)
            .await
            .map_err(|e| MediaFetchError::Peer(e.to_string()))?;
        let id = i32::try_from(message_id).map_err(|_| MediaFetchError::Missing)?;
        let client = self.client();
        let message = client
            .get_messages_by_id(peer_ref, &[id])
            .await?
            .into_iter()
            .next()
            .flatten()
            .ok_or(MediaFetchError::Missing)?;
        let Some(media) = message.media() else {
            return Err(MediaFetchError::Missing);
        };
        match media {
            Media::Photo(photo) if is_photo && photo.id().to_string() == expected_media_id => {
                choose_photo_size(photo.thumbs(), archive).ok_or(MediaFetchError::Unavailable)
            }
            Media::Document(document)
                if !is_photo
                    && document.id().to_string() == expected_media_id
                    && matches!(
                        document.mime_type(),
                        Some("image/jpeg" | "image/png" | "image/webp")
                    ) =>
            {
                if archive {
                    Ok(DownloadableMedia::Document(document))
                } else {
                    choose_photo_size(document.thumbs(), false).ok_or(MediaFetchError::Unavailable)
                }
            }
            _ => Err(MediaFetchError::Changed),
        }
    }

    pub(crate) fn download_iter(
        &self,
        media: &DownloadableMedia,
    ) -> grammers_client::client::DownloadIter {
        self.client().iter_download(media)
    }
}

fn choose_photo_size(sizes: Vec<PhotoSize>, archive: bool) -> Option<DownloadableMedia> {
    let candidates = sizes
        .into_iter()
        .filter_map(|size| {
            let (width, height) = match &size {
                PhotoSize::Size(s) => (s.width, s.height),
                PhotoSize::Progressive(s) => (s.width, s.height),
                PhotoSize::Cached(s) => (s.width, s.height),
                _ => return None, // excludes stripped, path and empty special thumbnails
            };
            Some((size, width, height))
        })
        .collect::<Vec<_>>();
    let dimensions = candidates
        .iter()
        .map(|(size, width, height)| (*width, *height, size.photo_type()))
        .collect::<Vec<_>>();
    let selected = select_size_index(&dimensions, archive)?;
    let (size, width, height) = candidates.into_iter().nth(selected)?;
    Some(DownloadableMedia::Photo {
        size,
        width,
        height,
    })
}

fn select_size_index(sizes: &[(i32, i32, String)], archive: bool) -> Option<usize> {
    sizes
        .iter()
        .enumerate()
        .filter(|(_, (width, height, _))| {
            *width > 0 && *height > 0 && (archive || (*width).max(*height) <= 800)
        })
        .max_by_key(|(_, (width, height, kind))| {
            (
                i64::from(*width) * i64::from(*height),
                i32::from(kind == "x"),
            )
        })
        .map(|(index, _)| index)
}

#[cfg(test)]
mod tests {
    use super::select_size_index;

    #[test]
    fn preview_picks_largest_raster_within_800px_and_archive_picks_largest() {
        let sizes = vec![
            (320, 240, "s".into()),
            (800, 600, "m".into()),
            (1600, 1200, "x".into()),
        ];
        assert_eq!(select_size_index(&sizes, false), Some(1));
        assert_eq!(select_size_index(&sizes, true), Some(2));
        assert_eq!(select_size_index(&[(1600, 1200, "x".into())], false), None);
        assert_eq!(select_size_index(&[], true), None);
    }
}

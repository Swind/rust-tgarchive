use super::*;

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct MessageDto {
    pub id: i64,
    pub chat_id: i64,
    pub sender_id: Option<i64>,
    /// Resolved sender; null for messages without a sender.
    pub sender: Option<SenderDto>,
    pub timestamp: DateTime<Utc>,
    pub edited_at: Option<DateTime<Utc>>,
    pub collected_at: DateTime<Utc>,
    pub text: Option<String>,
    pub reply_to: Option<i64>,
    pub attachments: Vec<AttachmentDto>,
    /// Channel post signature (`post_author`); not the sender.
    pub post_author: Option<String>,
    /// Origin of a forwarded message; null when the message was not forwarded.
    pub forward: Option<ForwardDto>,
    pub is_deleted: bool,
    /// Set when the message was deleted on Telegram (only returned with `include_deleted=true`).
    pub deleted_at: Option<DateTime<Utc>>,
    /// Search results only: excerpt (about 120 characters) around the first match, `null` when
    /// no match position could be determined. `text` always carries the full message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AttachmentDto {
    pub kind: AttachmentKindDto,
    pub telegram_file_id: Option<String>,
    pub mime_type: Option<String>,
    pub file_name: Option<String>,
    pub size: Option<i64>,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentKindDto {
    Photo,
    Video,
    Audio,
    Voice,
    Document,
    Sticker,
    Animation,
    Other,
}

impl From<Attachment> for AttachmentDto {
    fn from(value: Attachment) -> Self {
        Self {
            kind: match value.kind {
                AttachmentKind::Photo => AttachmentKindDto::Photo,
                AttachmentKind::Video => AttachmentKindDto::Video,
                AttachmentKind::Audio => AttachmentKindDto::Audio,
                AttachmentKind::Voice => AttachmentKindDto::Voice,
                AttachmentKind::Document => AttachmentKindDto::Document,
                AttachmentKind::Sticker => AttachmentKindDto::Sticker,
                AttachmentKind::Animation => AttachmentKindDto::Animation,
                AttachmentKind::Other => AttachmentKindDto::Other,
            },
            telegram_file_id: value.telegram_file_id,
            mime_type: value.mime_type,
            file_name: value.file_name,
            size: value.size,
        }
    }
}

impl From<MessageView> for MessageDto {
    fn from(view: MessageView) -> Self {
        let MessageView {
            message,
            sender,
            deleted_at,
            snippet,
        } = view;
        Self {
            snippet,
            sender: sender.map(Into::into),
            is_deleted: deleted_at.is_some(),
            deleted_at,
            id: message.id.get(),
            chat_id: message.chat_id.get(),
            sender_id: message.sender_id.map(SenderId::get),
            timestamp: message.timestamp,
            edited_at: message.edited_at,
            collected_at: message.collected_at,
            text: message.text,
            reply_to: message.reply_to.map(|id| id.get()),
            attachments: message.attachments.into_iter().map(Into::into).collect(),
            post_author: message.post_author,
            forward: message.forward.map(Into::into),
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct MessagePageDto {
    pub items: Vec<MessageDto>,
    pub has_more: bool,
    /// Opaque cursor; pass it back as `before` to get the next page.
    pub next_cursor: Option<String>,
}

/// Result order of `GET /messages/search`.
#[derive(Debug, Clone, Copy, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SearchSortDto {
    /// BM25 relevance, best first (default). The cursor is a position, so pages are stable
    /// unless messages are written between requests.
    Relevance,
    /// Newest first with keyset cursors. Also what relevance falls back to for single-character
    /// queries and while the search index is not ready.
    Time,
}

impl From<SearchSortDto> for SearchSort {
    fn from(sort: SearchSortDto) -> Self {
        match sort {
            SearchSortDto::Relevance => Self::Relevance,
            SearchSortDto::Time => Self::Time,
        }
    }
}

impl TryFrom<MessagePage> for MessagePageDto {
    type Error = crate::interface::cursor::CursorError;

    fn try_from(page: MessagePage) -> Result<Self, Self::Error> {
        Ok(Self {
            items: page.items.into_iter().map(Into::into).collect(),
            has_more: page.has_more,
            next_cursor: match (page.next_cursor.as_ref(), page.next_offset) {
                (Some(cursor), _) => Some(crate::interface::cursor::encode(cursor)?),
                (None, Some(offset)) => Some(crate::interface::cursor::encode_offset(offset)?),
                (None, None) => None,
            },
        })
    }
}

/// Messages around an anchor, both lists oldest first.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct MessageContextDto {
    pub anchor: MessageDto,
    pub before: Vec<MessageDto>,
    pub after: Vec<MessageDto>,
    pub has_more_before: bool,
    pub has_more_after: bool,
    /// Pass as `before` to `GET .../messages` to keep scrolling to older messages.
    pub before_cursor: Option<String>,
    /// Pass as `after` to `GET .../messages` to keep scrolling to newer messages.
    pub after_cursor: Option<String>,
}

impl TryFrom<MessageContext> for MessageContextDto {
    type Error = crate::interface::cursor::CursorError;

    fn try_from(context: MessageContext) -> Result<Self, Self::Error> {
        let encode = |cursor: Option<crate::application::MessageCursor>| {
            cursor
                .as_ref()
                .map(crate::interface::cursor::encode)
                .transpose()
        };
        Ok(Self {
            anchor: context.anchor.into(),
            before: context.before.into_iter().map(Into::into).collect(),
            after: context.after.into_iter().map(Into::into).collect(),
            has_more_before: context.has_more_before,
            has_more_after: context.has_more_after,
            before_cursor: encode(context.before_cursor)?,
            after_cursor: encode(context.after_cursor)?,
        })
    }
}

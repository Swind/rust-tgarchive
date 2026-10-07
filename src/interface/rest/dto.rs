use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    application::{
        ChatSort, ChatStats, ChatSummary, MessageContext, MessagePage, MessageView,
        SearchIndexStatus, SearchSort, SenderChatStat, SenderDetail, SenderInfo,
        SenderNameHistoryEntry, SenderNameVersion, SenderPage, SenderProfile, SenderSort,
        SenderSummary, SyncChatProgress, SyncJob, SyncJobState, services::ApplicationStatus,
    },
    domain::{Attachment, AttachmentKind, Chat, ChatKind, Forward, SenderId, SenderKind},
    infrastructure::persistence::sqlite::media::{MediaDownload, MediaPolicy},
};

mod chat;
mod media;
mod message;
mod sender;
mod status;

pub use chat::*;
pub use media::*;
pub use message::*;
pub use sender::*;
pub use status::*;

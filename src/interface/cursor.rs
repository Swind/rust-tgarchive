use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    application::MessageCursor,
    domain::{ChatId, MessageId},
};

const VERSION: u8 = 1;
const MAX_LENGTH: usize = 512;

pub fn encode(cursor: &MessageCursor) -> Result<String, CursorError> {
    let payload = CursorPayload {
        version: VERSION,
        timestamp: cursor.timestamp,
        chat_id: cursor.chat_id.get(),
        message_id: cursor.message_id.get(),
    };
    let json = serde_json::to_vec(&payload).map_err(|_| CursorError::Malformed)?;
    Ok(URL_SAFE_NO_PAD.encode(json))
}

pub fn decode(value: &str) -> Result<MessageCursor, CursorError> {
    if value.is_empty() || value.len() > MAX_LENGTH {
        return Err(CursorError::Malformed);
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| CursorError::Malformed)?;
    let payload: CursorPayload =
        serde_json::from_slice(&bytes).map_err(|_| CursorError::Malformed)?;
    if payload.version != VERSION {
        return Err(CursorError::UnsupportedVersion(payload.version));
    }
    Ok(MessageCursor {
        timestamp: payload.timestamp,
        chat_id: ChatId::from_marked(payload.chat_id).map_err(|_| CursorError::Malformed)?,
        message_id: MessageId::new(payload.message_id).map_err(|_| CursorError::Malformed)?,
    })
}

#[derive(Debug, Serialize, Deserialize)]
struct CursorPayload {
    version: u8,
    timestamp: DateTime<Utc>,
    chat_id: i64,
    message_id: i64,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CursorError {
    #[error("cursor is malformed")]
    Malformed,
    #[error("cursor version {0} is not supported")]
    UnsupportedVersion(u8),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ChatKind;

    #[test]
    fn codec_round_trips_compound_key_and_rejects_bad_version_or_length() {
        let cursor = MessageCursor {
            timestamp: DateTime::from_timestamp(10, 123).unwrap(),
            chat_id: ChatId::from_telegram(ChatKind::Channel, 9).unwrap(),
            message_id: MessageId::new(5).unwrap(),
        };
        assert_eq!(decode(&encode(&cursor).unwrap()).unwrap(), cursor);
        let payload = CursorPayload {
            version: 2,
            timestamp: cursor.timestamp,
            chat_id: cursor.chat_id.get(),
            message_id: 5,
        };
        let unsupported = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap());
        assert_eq!(
            decode(&unsupported),
            Err(CursorError::UnsupportedVersion(2))
        );
        assert!(decode(&"a".repeat(MAX_LENGTH + 1)).is_err());
    }
}

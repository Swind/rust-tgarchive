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
const OFFSET_KIND: &str = "relevance";
/// Deep relevance paging re-ranks every match; refuse absurd positions.
const MAX_OFFSET: u64 = 100_000;

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

/// Position cursor of a relevance-sorted search. Same envelope as the keyset cursor (versioned
/// JSON, base64url) but with a `kind` tag, so the two cannot be confused.
pub fn encode_offset(offset: u64) -> Result<String, CursorError> {
    let payload = OffsetPayload {
        version: VERSION,
        kind: OFFSET_KIND.to_owned(),
        offset,
    };
    let json = serde_json::to_vec(&payload).map_err(|_| CursorError::Malformed)?;
    Ok(URL_SAFE_NO_PAD.encode(json))
}

pub fn decode_offset(value: &str) -> Result<u64, CursorError> {
    if value.is_empty() || value.len() > MAX_LENGTH {
        return Err(CursorError::Malformed);
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| CursorError::Malformed)?;
    let payload: OffsetPayload =
        serde_json::from_slice(&bytes).map_err(|_| CursorError::Malformed)?;
    if payload.version != VERSION {
        return Err(CursorError::UnsupportedVersion(payload.version));
    }
    if payload.kind != OFFSET_KIND || payload.offset > MAX_OFFSET {
        return Err(CursorError::Malformed);
    }
    Ok(payload.offset)
}

#[derive(Debug, Serialize, Deserialize)]
struct OffsetPayload {
    version: u8,
    kind: String,
    offset: u64,
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

    #[test]
    fn offset_cursor_round_trips_and_is_not_a_keyset_cursor() {
        let encoded = encode_offset(60).unwrap();
        assert_eq!(decode_offset(&encoded), Ok(60));
        assert_eq!(decode(&encoded), Err(CursorError::Malformed));
        let keyset = encode(&MessageCursor {
            timestamp: DateTime::from_timestamp(10, 0).unwrap(),
            chat_id: ChatId::from_telegram(ChatKind::Channel, 9).unwrap(),
            message_id: MessageId::new(5).unwrap(),
        })
        .unwrap();
        assert_eq!(decode_offset(&keyset), Err(CursorError::Malformed));
        assert_eq!(
            decode_offset(&encode_offset(MAX_OFFSET + 1).unwrap()),
            Err(CursorError::Malformed)
        );
        let future = URL_SAFE_NO_PAD.encode(br#"{"version":2,"kind":"relevance","offset":1}"#);
        assert_eq!(
            decode_offset(&future),
            Err(CursorError::UnsupportedVersion(2))
        );
    }
}

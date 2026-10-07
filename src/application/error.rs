use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ValidationError {
    #[error("page size must be between 1 and 1000")]
    InvalidPageSize,
    #[error("before and after cursors cannot be used together")]
    ConflictingCursors,
    #[error("time range must have from < to")]
    InvalidTimeRange,
    #[error("search query cannot be empty")]
    EmptySearch,
    #[error("search query exceeds 4096 bytes")]
    SearchTooLong,
    #[error("context size must be between 0 and 100")]
    InvalidContextSize,
}

#[derive(Debug, Error)]
pub enum ApplicationError {
    #[error("validation failed: {0}")]
    Validation(#[from] ValidationError),
    #[error("resource not found")]
    NotFound,
    #[error("chat is not tracked; track it first (`chats track <CHAT_ID>`)")]
    NotTracked,
    #[error("operation conflicts with current state")]
    Conflict,
    #[error("service is busy")]
    Busy,
    #[error("Telegram is unavailable: {0}")]
    TelegramUnavailable(String),
    #[error("Telegram rate limited for {retry_after_seconds} seconds")]
    TelegramFloodWait { retry_after_seconds: u64 },
    #[error("repository is unavailable: {0}")]
    RepositoryUnavailable(String),
    #[error("internal application error: {0}")]
    Internal(String),
}

impl From<RepositoryError> for ApplicationError {
    fn from(error: RepositoryError) -> Self {
        match error {
            RepositoryError::Unavailable(message) => Self::RepositoryUnavailable(message),
            RepositoryError::InvalidData(message) => Self::Internal(message),
        }
    }
}

impl From<TelegramError> for ApplicationError {
    fn from(error: TelegramError) -> Self {
        match error {
            TelegramError::Unavailable(message) => Self::TelegramUnavailable(message),
            TelegramError::Unauthorized => {
                Self::TelegramUnavailable("Telegram account is not authorized".into())
            }
            TelegramError::FloodWait {
                retry_after_seconds,
            } => Self::TelegramFloodWait {
                retry_after_seconds,
            },
        }
    }
}

#[derive(Debug, Error)]
pub enum RepositoryError {
    #[error("storage operation failed: {0}")]
    Unavailable(String),
    #[error("stored data is invalid: {0}")]
    InvalidData(String),
}

#[derive(Debug, Error)]
pub enum TelegramError {
    #[error("Telegram request failed: {0}")]
    Unavailable(String),
    #[error("Telegram rate limited for {retry_after_seconds} seconds")]
    FloodWait { retry_after_seconds: u64 },
    #[error("Telegram access is not authorized")]
    Unauthorized,
}

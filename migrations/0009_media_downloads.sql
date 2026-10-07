CREATE TABLE media_policies (
    chat_id INTEGER PRIMARY KEY REFERENCES chats(id) ON DELETE CASCADE,
    auto_archive INTEGER NOT NULL DEFAULT 0 CHECK (auto_archive IN (0, 1))
);

CREATE TABLE media_downloads (
    id INTEGER PRIMARY KEY,
    chat_id INTEGER NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
    message_id INTEGER NOT NULL CHECK (message_id > 0),
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    telegram_media_id TEXT NOT NULL,
    media_kind TEXT NOT NULL CHECK (media_kind IN ('photo', 'document')),
    variant TEXT NOT NULL CHECK (variant IN ('preview', 'archive')),
    trigger TEXT NOT NULL CHECK (trigger IN ('auto', 'manual')),
    state TEXT NOT NULL DEFAULT 'queued' CHECK (state IN ('queued', 'running', 'succeeded', 'failed', 'interrupted', 'unavailable', 'superseded')),
    relative_path TEXT,
    content_type TEXT,
    byte_size INTEGER CHECK (byte_size IS NULL OR byte_size >= 0),
    width INTEGER,
    height INTEGER,
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    next_attempt_at INTEGER,
    last_error TEXT,
    UNIQUE (chat_id, message_id, ordinal, media_kind, telegram_media_id, variant),
    FOREIGN KEY (chat_id, message_id) REFERENCES messages(chat_id, message_id) ON DELETE CASCADE
);

CREATE INDEX media_downloads_queue ON media_downloads(state, next_attempt_at, id);
CREATE INDEX media_downloads_message ON media_downloads(chat_id, message_id, ordinal);

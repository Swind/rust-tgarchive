CREATE TABLE chats (
    id            INTEGER PRIMARY KEY,
    kind          TEXT NOT NULL CHECK (kind IN ('private', 'group', 'supergroup', 'channel')),
    title         TEXT,
    username      TEXT,
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL
);

CREATE TABLE senders (
    id            INTEGER PRIMARY KEY,
    kind          TEXT NOT NULL CHECK (kind IN ('user', 'chat', 'channel', 'unknown')),
    display_name  TEXT,
    username      TEXT,
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL
);

CREATE TABLE messages (
    row_id         INTEGER PRIMARY KEY,
    chat_id        INTEGER NOT NULL REFERENCES chats(id),
    message_id     INTEGER NOT NULL CHECK (message_id > 0),
    sender_id      INTEGER,
    timestamp      INTEGER NOT NULL,
    edited_at      INTEGER,
    collected_at   INTEGER NOT NULL,
    created_at     INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL,
    text           TEXT,
    reply_to       INTEGER,
    version_at     INTEGER NOT NULL,
    source_priority INTEGER NOT NULL CHECK (source_priority IN (0, 1)),
    is_deleted     INTEGER NOT NULL DEFAULT 0 CHECK (is_deleted IN (0, 1)),
    deleted_at     INTEGER,
    UNIQUE (chat_id, message_id)
);

CREATE TABLE attachments (
    id                   INTEGER PRIMARY KEY,
    message_row_id       INTEGER NOT NULL REFERENCES messages(row_id) ON DELETE CASCADE,
    ordinal              INTEGER NOT NULL CHECK (ordinal >= 0),
    kind                 TEXT NOT NULL CHECK (kind IN ('photo', 'video', 'audio', 'voice', 'document', 'sticker', 'animation', 'other')),
    telegram_file_id     TEXT,
    mime_type            TEXT,
    file_name            TEXT,
    size                 INTEGER CHECK (size IS NULL OR size >= 0),
    UNIQUE (message_row_id, ordinal)
);

CREATE TABLE message_tombstones (
    chat_id      INTEGER NOT NULL REFERENCES chats(id),
    message_id   INTEGER NOT NULL CHECK (message_id > 0),
    deleted_at   INTEGER NOT NULL,
    PRIMARY KEY (chat_id, message_id)
);

CREATE TABLE chat_sync_state (
    chat_id              INTEGER PRIMARY KEY REFERENCES chats(id) ON DELETE CASCADE,
    history_before_id    INTEGER,
    history_complete     INTEGER NOT NULL DEFAULT 0 CHECK (history_complete IN (0, 1)),
    catchup_after_id     INTEGER,
    oldest_message_id    INTEGER,
    newest_message_id    INTEGER,
    last_sync_started_at INTEGER,
    last_sync_completed_at INTEGER,
    last_error           TEXT,
    updated_at           INTEGER NOT NULL
);

CREATE TABLE sync_jobs (
    id             TEXT PRIMARY KEY,
    scope          TEXT NOT NULL CHECK (scope IN ('chat', 'all')),
    chat_id        INTEGER REFERENCES chats(id),
    state          TEXT NOT NULL CHECK (state IN ('queued', 'running', 'succeeded', 'failed', 'interrupted')),
    created_at     INTEGER NOT NULL,
    started_at     INTEGER,
    finished_at    INTEGER,
    error_summary  TEXT,
    CHECK ((scope = 'chat' AND chat_id IS NOT NULL) OR (scope = 'all' AND chat_id IS NULL))
);

CREATE TABLE sync_job_chats (
    job_id          TEXT NOT NULL REFERENCES sync_jobs(id) ON DELETE CASCADE,
    chat_id         INTEGER NOT NULL REFERENCES chats(id),
    state           TEXT NOT NULL CHECK (state IN ('queued', 'running', 'succeeded', 'failed', 'interrupted')),
    committed_count INTEGER NOT NULL DEFAULT 0 CHECK (committed_count >= 0),
    error_summary   TEXT,
    PRIMARY KEY (job_id, chat_id)
);

CREATE INDEX messages_global_order ON messages(timestamp DESC, chat_id DESC, message_id DESC) WHERE is_deleted = 0;
CREATE INDEX messages_chat_order ON messages(chat_id, timestamp DESC, message_id DESC) WHERE is_deleted = 0;
CREATE INDEX messages_sender_order ON messages(sender_id, timestamp DESC, chat_id DESC, message_id DESC) WHERE is_deleted = 0;
CREATE INDEX sync_jobs_active_chat ON sync_jobs(chat_id, state) WHERE state IN ('queued', 'running');

CREATE VIRTUAL TABLE messages_fts USING fts5(
    text,
    content='messages',
    content_rowid='row_id',
    tokenize='unicode61'
);

CREATE TRIGGER messages_fts_insert AFTER INSERT ON messages BEGIN
    INSERT INTO messages_fts(rowid, text) VALUES (new.row_id, new.text);
END;

CREATE TRIGGER messages_fts_delete AFTER DELETE ON messages BEGIN
    INSERT INTO messages_fts(messages_fts, rowid, text) VALUES ('delete', old.row_id, old.text);
END;

CREATE TRIGGER messages_fts_update AFTER UPDATE OF text ON messages BEGIN
    INSERT INTO messages_fts(messages_fts, rowid, text) VALUES ('delete', old.row_id, old.text);
    INSERT INTO messages_fts(rowid, text) VALUES (new.row_id, new.text);
END;

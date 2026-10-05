-- Sender attribution metadata. Only adds nullable columns and indexes; existing rows keep NULLs
-- until `tgarchive repair senders` or `sync chat <ID> --refetch` fills them.
ALTER TABLE messages ADD COLUMN post_author TEXT;
ALTER TABLE messages ADD COLUMN fwd_from_id INTEGER;
ALTER TABLE messages ADD COLUMN fwd_from_name TEXT;
ALTER TABLE messages ADD COLUMN fwd_date INTEGER;

-- Resumable `--refetch` walk (independent of the normal history/catch-up checkpoints).
ALTER TABLE chat_sync_state ADD COLUMN refetch_active INTEGER NOT NULL DEFAULT 0 CHECK (refetch_active IN (0, 1));
ALTER TABLE chat_sync_state ADD COLUMN refetch_before_id INTEGER;

-- Per-sender aggregates and listings, including include_deleted (0001 indexes are partial on is_deleted=0).
CREATE INDEX messages_sender_stats ON messages(sender_id, is_deleted, chat_id, timestamp) WHERE sender_id IS NOT NULL;
CREATE INDEX messages_sender_all_order ON messages(sender_id, timestamp DESC, chat_id DESC, message_id DESC) WHERE sender_id IS NOT NULL;
CREATE INDEX messages_post_author ON messages(post_author, timestamp DESC, chat_id DESC, message_id DESC) WHERE post_author IS NOT NULL;
-- Finds legacy rows without a sender (repair).
CREATE INDEX messages_null_sender ON messages(chat_id, row_id) WHERE sender_id IS NULL;

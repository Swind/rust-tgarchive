-- Read-side indexes for chat stats (covering aggregate per chat), include_deleted listings
-- (the 0001 order indexes are partial on is_deleted = 0) and the per-chat sender breakdown.
-- No data is changed.
CREATE INDEX messages_chat_stats ON messages(chat_id, is_deleted, timestamp);
CREATE INDEX messages_chat_all_order ON messages(chat_id, timestamp DESC, message_id DESC);
CREATE INDEX messages_chat_sender ON messages(chat_id, sender_id) WHERE is_deleted = 0;

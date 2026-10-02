-- Opt-in collection: no chat is collected unless explicitly tracked. Existing rows stay untracked;
-- no data is deleted.
ALTER TABLE chats ADD COLUMN tracked INTEGER NOT NULL DEFAULT 0 CHECK (tracked IN (0, 1));
ALTER TABLE chats ADD COLUMN tracked_at INTEGER;
CREATE INDEX chats_tracked ON chats(id) WHERE tracked = 1;

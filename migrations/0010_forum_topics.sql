-- Existing messages stay unclassified until Telegram supplies an authoritative topic ID.
ALTER TABLE messages ADD COLUMN topic_id INTEGER CHECK (topic_id > 0);

-- Only pre-upgrade groups with archived rows need a one-time classification walk.
ALTER TABLE chats ADD COLUMN topic_repair_needed INTEGER NOT NULL DEFAULT 0 CHECK (topic_repair_needed IN (0, 1));
UPDATE chats SET topic_repair_needed = 1
WHERE kind = 'supergroup' AND EXISTS (SELECT 1 FROM messages WHERE chat_id = chats.id);

CREATE TABLE forum_topics (
    chat_id          INTEGER NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
    topic_id         INTEGER NOT NULL CHECK (topic_id > 0),
    title            TEXT NOT NULL,
    active           INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1)),
    collected        INTEGER NOT NULL DEFAULT 1 CHECK (collected IN (0, 1)),
    generation       INTEGER NOT NULL DEFAULT 0 CHECK (generation >= 0),
    backfill_pending INTEGER NOT NULL DEFAULT 0 CHECK (backfill_pending IN (0, 1)),
    before_id        INTEGER CHECK (before_id > 0),
    PRIMARY KEY (chat_id, topic_id),
    CHECK (backfill_pending = 0 OR collected = 1),
    CHECK (before_id IS NULL OR backfill_pending = 1)
) WITHOUT ROWID;

CREATE INDEX forum_topics_pending ON forum_topics(chat_id, topic_id) WHERE backfill_pending = 1;

-- Bot flag (NULL = unknown / not a user) and observed name history. Additive only.
ALTER TABLE senders ADD COLUMN is_bot INTEGER CHECK (is_bot IN (0, 1));

-- One row per distinct (display_name, username) combination tgarchive observed for a sender.
-- Reflects when tgarchive saw the name, not when the user changed it.
CREATE TABLE sender_name_history (
    id            INTEGER PRIMARY KEY,
    sender_id     INTEGER NOT NULL REFERENCES senders(id),
    display_name  TEXT,
    username      TEXT,
    first_seen_at INTEGER NOT NULL,
    last_seen_at  INTEGER NOT NULL
);
CREATE INDEX sender_name_history_sender ON sender_name_history(sender_id, first_seen_at);

-- Current values of every sender that has a name or username (all-NULL senders get no row).
INSERT INTO sender_name_history(sender_id, display_name, username, first_seen_at, last_seen_at)
SELECT id, display_name, username, created_at, updated_at
FROM senders
WHERE display_name IS NOT NULL OR username IS NOT NULL
ORDER BY id;

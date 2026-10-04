-- Chinese-friendly search: replaces the external-content FTS table (and its triggers, which
-- cannot call the Rust tokenizer) with a contentless-delete FTS5 table holding jieba words and
-- CJK bigrams. rowid == messages.row_id. The application maintains it in the same transaction
-- as every message write; soft deletes keep their index entries.
--
-- This migration only changes structure. Tokenizing needs Rust, so existing messages are
-- (re)indexed by `tgarchive db init` / `tgarchive search rebuild-index`. Until then
-- search_index_state is 'stale' and search falls back to a LIKE scan.
DROP TRIGGER messages_fts_insert;
DROP TRIGGER messages_fts_delete;
DROP TRIGGER messages_fts_update;
DROP TABLE messages_fts;

CREATE VIRTUAL TABLE messages_fts USING fts5(
    words,
    bigrams,
    content='',
    contentless_delete=1,
    tokenize='unicode61'
);

CREATE TABLE app_metadata (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- An archive without indexable text is trivially up to date; otherwise it needs a rebuild.
INSERT INTO app_metadata(key, value)
SELECT 'search_index_state',
       CASE WHEN EXISTS(SELECT 1 FROM messages WHERE text IS NOT NULL AND text <> '')
            THEN 'stale' ELSE 'ready' END;
INSERT INTO app_metadata(key, value)
SELECT 'search_index_version',
       CASE WHEN EXISTS(SELECT 1 FROM messages WHERE text IS NOT NULL AND text <> '')
            THEN '0' ELSE '1' END;

-- Adds the resumable 'rate_limited' job state. SQLite cannot alter a CHECK constraint, so the
-- two job tables are rebuilt (child first, so no cascade deletes rows). No data is lost.
CREATE TABLE sync_jobs_new (
    id             TEXT PRIMARY KEY,
    scope          TEXT NOT NULL CHECK (scope IN ('chat', 'all')),
    chat_id        INTEGER REFERENCES chats(id),
    state          TEXT NOT NULL CHECK (state IN ('queued', 'running', 'succeeded', 'failed', 'interrupted', 'rate_limited')),
    created_at     INTEGER NOT NULL,
    started_at     INTEGER,
    finished_at    INTEGER,
    error_summary  TEXT,
    CHECK ((scope = 'chat' AND chat_id IS NOT NULL) OR (scope = 'all' AND chat_id IS NULL))
);
CREATE TABLE sync_job_chats_new (
    job_id          TEXT NOT NULL REFERENCES sync_jobs_new(id) ON DELETE CASCADE,
    chat_id         INTEGER NOT NULL REFERENCES chats(id),
    state           TEXT NOT NULL CHECK (state IN ('queued', 'running', 'succeeded', 'failed', 'interrupted', 'rate_limited')),
    committed_count INTEGER NOT NULL DEFAULT 0 CHECK (committed_count >= 0),
    error_summary   TEXT,
    PRIMARY KEY (job_id, chat_id)
);
INSERT INTO sync_jobs_new SELECT id, scope, chat_id, state, created_at, started_at, finished_at, error_summary FROM sync_jobs;
INSERT INTO sync_job_chats_new SELECT job_id, chat_id, state, committed_count, error_summary FROM sync_job_chats;
DROP TABLE sync_job_chats;
DROP TABLE sync_jobs;
ALTER TABLE sync_jobs_new RENAME TO sync_jobs;
ALTER TABLE sync_job_chats_new RENAME TO sync_job_chats;
CREATE INDEX sync_jobs_active_chat ON sync_jobs(chat_id, state) WHERE state IN ('queued', 'running');

ALTER TABLE media_downloads
    ADD COLUMN prioritized INTEGER NOT NULL DEFAULT 0 CHECK (prioritized IN (0, 1));

CREATE INDEX media_downloads_priority ON media_downloads (
    CASE WHEN state='queued' THEN 0 ELSE next_attempt_at END, id
)
WHERE prioritized=1 AND (state='queued' OR (state='failed' AND next_attempt_at IS NOT NULL));

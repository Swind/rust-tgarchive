-- V1 archives are bound to one Telegram user account. Common-message update
-- IDs have no peer, so retain tombstones independently of any chat row.
CREATE TABLE telegram_account_identity (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    user_id   INTEGER NOT NULL CHECK (user_id > 0),
    bound_at  INTEGER NOT NULL
);

CREATE TABLE common_message_tombstones (
    message_id INTEGER PRIMARY KEY CHECK (message_id > 0),
    deleted_at INTEGER NOT NULL
);

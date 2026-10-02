# Architecture decisions (Phase 0)

These decisions freeze the core contracts from [the execution plan](../telegram_message_archive_execution_plan.md) for P1. Product scope remains defined by [the original plan](../telegram_message_archive_implementation_plan.md). They do not add a new framework or dependency.

## AD-1: One signed 64-bit chat/sender ID namespace

Use Telegram's Bot API dialog ID encoding in `ChatId(i64)` and `SenderId(i64)`, with `ChatKind` retained as validated metadata:

- user: `id` in `1..=0xffffffffff`;
- basic group: `-id`, where MTProto `id` is in `1..=999999999999`;
- channel/supergroup: `-(1_000_000_000_000 + id)`, using the Telegram MTProto channel range;
- reject zero, out-of-range values, and special identities without an ordinary peer ID. Do not encode access hashes.

The namespace conversion is injective across user/basic-group/channel kinds in the documented ranges; `ChatKind` still distinguishes basic group from channel and supports validation. A peer ID by itself does not authorize API requests: Grammers needs cached peer info/access hash for user/channel peers. That state stays in infrastructure/session and is resolved from dialogs/session cache. `SenderId` uses the same conversion; unknown sender remains `None`.

`MessageId` is a positive Telegram `i32` widened to `i64` only after checked conversion. The primary message identity is always `(ChatId, MessageId)`: basic-group/private-chat message IDs share one account sequence, but every channel has its own message sequence.

## AD-2: Cursors and archive ordering

History progress and API pagination are separate concepts.

- Historical backfill is per chat and moves toward older messages. Persist `history_before_id` as the last successfully committed exclusive message-ID boundary, plus `history_complete`; never derive it from a newest/realtime ID.
- Realtime update recovery is per Telegram account update scope: common `pts`, secondary `qts`, global `date`/`seq`, and per-channel `pts`. These are not message IDs and do not replace history cursors.
- Catch-up toward newer message IDs uses a separately persisted upper-bound/cursor; pin the round's upper boundary and advance only with committed pages.
- Public archive list/search use `(timestamp DESC, chat_id DESC, message_id DESC)` keyset order. The opaque API cursor is versioned JSON encoded as URL-safe Base64 and carries all three order keys. Cursor and filter/sort/direction validation belong in application/interface boundary code; neither SQLx nor Grammers types escape through ports.

## AD-3: Ingest and writer transaction

Use one `ArchiveWriter::write_batch(IngestBatch)` port as the persistence boundary. A batch carries normalized events, chat/sender metadata, source/version metadata, and optional history or update checkpoint. It carries no SQL transaction.

For each batch, one SQLite transaction applies metadata, event version checks, message and attachment replacement, tombstones, FTS triggers, and the relevant checkpoint. A checkpoint advances only in the same commit as the messages/deletions it covers. The caller receives success only after commit. Older message versions cannot overwrite newer edits; tombstoned IDs cannot be recreated. Application ports expose domain data and typed errors only.

## AD-4: Deletion identity

Channel/supergroup delete updates include `channel_id`, so map each message ID to that channel's `ChatId` and write a scoped tombstone.

Common delete updates contain message IDs but no peer. Telegram documents that private/basic-group message IDs share a sequence for a given account, so when exactly one archived row matches `(account, message_id)` the adapter can resolve its chat and persist the ordinary `(chat_id, message_id)` tombstone. When there is no archived row, preserve a durable **account-scoped common tombstone** keyed by `(account_id, message_id)`; ingestion checks it before inserting so a delayed/history message cannot resurrect it. Never apply a common deletion to a channel row. If multiple/no matching common candidates make scope uncertain, keep the account-scoped tombstone and count/report it as unresolved instead of inventing a chat ID.

The V1 single Telegram owner is also one account; still persist account scope in the data contract so future multi-account support cannot merge common message sequences accidentally.

## AD-5: History boundary and retry

Use Grammers' newest-to-oldest exclusive history boundary (`offset_id`/equivalent server `max_id` semantics) after verifying exact adapter behavior in P0's replayable test plan. A successful page is committed before the next exclusive boundary is stored. Completion comes from remote iterator exhaustion, never from an ID gap or a short page alone. Re-fetching a committed page is safe through `(chat_id, message_id)` idempotency.

## AD-6: One process owns Telegram

`serve`, auth, refresh, and one-shot sync all acquire one OS advisory exclusive lock before opening the Telegram session. A second owner exits with a clear busy error. Query-only CLI processes may read SQLite concurrently. Do not guess stale PID state or delete sessions. The HTTP server defaults to loopback; do not expose archive data on non-loopback without authentication.

## AD-7: Opt-in collection (tracked chats)

No chat — channel, supergroup, basic group or private — is collected by default. `chats.tracked` (migration `0003`, default 0, existing rows stay untracked, nothing deleted) is the single source of truth, changed only by `chats track|untrack` (CLI) or `PUT|DELETE /api/v1/chats/{id}/tracking` through `ChatService`. `chats refresh` upserts metadata for all dialogs and never touches the flag.

- Untrack stops collection; stored messages stay and remain queryable.
- `sync chat <id>` / `POST /api/v1/chats/{id}/sync` on an untracked chat fails: CLI exits non-zero (checked locally before Telegram is contacted), REST answers `409` with code `chat_not_tracked` (unknown chat stays `404`). `sync all` / `POST /api/v1/sync` refresh metadata and then sync tracked chats only; with none tracked the job (or CLI run) succeeds as a no-op (CLI prints "No tracked chats").
- Realtime: every stream batch is reduced by `restrict_to_tracked` before any write, reading the tracked set from SQLite each batch, so track/untrack apply without a restart. Records, chat and sender upserts, and channel deletions of untracked chats are dropped; the Telegram update-state checkpoint still advances (ignored, not lost). Common-namespace deletions carry no chat ID, so they are written only when the ID matches an already archived message of a tracked non-channel chat; otherwise no tombstone is created (a deletion that arrives before its message is archived is therefore not remembered).
- Catch-up rounds iterate tracked chats only. While a live session runs, the supervisor also polls (default 15 s) and gives tracked chats without a catch-up baseline one, so a chat tracked mid-session is protected against a later reconnect gap. Tracking does not itself fetch history; use `sync chat`.
- Not enforced mid-job: untracking while a history job for that chat runs lets that job finish its current run.

## Evidence and pending checks

Telegram's official ID guide documents the non-overlapping Bot API dialog-ID ranges and conversion formulas; the peer guide explains overlapping MTProto ID sequences and that user/channel access hashes are required to form input peers. Telegram's update guide documents the common private/basic-group message-ID sequence versus independent channel sequences. See the linked sources in [the adapter decisions](telegram-adapter-decisions.md).

Pure encoding/collision tests, keyset pagination tests, and SQLite atomicity tests are acceptance items in P1/P2, not tests run in Phase 0. Exact Grammers history boundary behavior and update-state mapping remain Phase 0/Phase 8 verification items; real-account confirmation is pending credentials.

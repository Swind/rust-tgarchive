# tgarchive

[![Image build](https://github.com/Swind/rust-tgarchive/actions/workflows/release.yml/badge.svg?branch=main)](https://github.com/Swind/rust-tgarchive/actions/workflows/release.yml)
[![CI](https://github.com/Swind/rust-tgarchive/actions/workflows/ci.yml/badge.svg)](https://github.com/Swind/rust-tgarchive/actions/workflows/ci.yml)

> Unofficial archive tool using the Telegram API; not affiliated with Telegram.

> The binary was previously named `telegram-archive` and is now `tgarchive`. The old environment variable `TELEGRAM_ARCHIVE_NO_DOTENV` is still accepted (deprecated); use `TGARCHIVE_NO_DOTENV` instead.

A Telegram message archiver written in Rust. It signs in with your own Telegram **user account** (via grammers, not a bot), stores messages from the chats you **explicitly choose** in a local SQLite database, and offers querying and full-text search through a CLI and a local-only REST API.

By default **no chat is collected** (including private conversations). You must opt in with `chats track`.

The web UI is in Traditional Chinese; UI labels mentioned below are given in English with the original label in parentheses.

## Table of contents

- [Prerequisites](#prerequisites)
- [Configuration](#configuration)
- [Quick start](#quick-start)
- [Querying](#querying)
  - [Full-text search (CJK-friendly)](#full-text-search-cjk-friendly)
- [Senders, repair and refetch](#senders-repair-and-refetch)
- [Rate limiting and pacing](#rate-limiting-and-pacing)
  - [Track and backfill: `--backfill`](#track-and-backfill---backfill)
  - [Terms of service reminder](#terms-of-service-reminder)
- [serve and status](#serve-and-status)
- [Web UI](#web-ui)
- [Image previews and archives](#image-previews-and-archives)
- [REST and OpenAPI](#rest-and-openapi)
- [One process per session](#one-process-per-session)
- [Live-account acceptance test](#live-account-acceptance-test)
- [Docker](#docker)
- [Backup and security](#backup-and-security)
- [Recovery behavior and limitations](#recovery-behavior-and-limitations)
- [Development](#development)

## Prerequisites

- Rust toolchain (version in `rust-toolchain.toml`; rustup installs it automatically)
- Optional: the `sqlite3` CLI (for backups and inspecting the database)
- Telegram API credentials: sign in at <https://my.telegram.org> with your account → "API development tools", create an application, and obtain `api_id` and `api_hash`.

## Configuration

Create `.env` in the project root (it is git-ignored; never commit it):

```sh
TELEGRAM_API_ID=123456
TELEGRAM_API_HASH=your_api_hash
TELEGRAM_PHONE=+886912345678      # used only by scripts/login.sh; may be omitted and entered interactively
TELEGRAM_SESSION_FILE=./telegram.session
DATABASE_URL=sqlite://telegram.db
SERVER_BIND=127.0.0.1:8080
RUST_LOG=info                      # defaults to warn when unset
SYNC_PAGE_DELAY_MS=1000            # optional; minimum interval between history requests, see "Rate limiting and pacing"
SYNC_MAX_FLOOD_WAIT_SECS=300       # optional; longest FLOOD_WAIT we are willing to wait
TGARCHIVE_DEV_CORS_ORIGIN=http://127.0.0.1:5173  # optional; local frontend development only, see "REST and OpenAPI"
MEDIA_DIR=./media                 # optional; defaults to media beside the SQLite database
```

**The binary automatically loads `.env` from the current working directory on startup** (before configuration is parsed):

- Existing process environment variables **take precedence**; `.env` never overrides them.
- The default `.env` is silently skipped if it does not exist. With `--env-file <PATH>` the file must exist, otherwise an error is reported. `--env-file` is a global flag and replaces `./.env`.
- Set `TGARCHIVE_NO_DOTENV=1` to disable automatic loading of `./.env` (an explicit `--env-file` is unaffected).
- The format is plain `KEY=VALUE` (comments and quotes are supported; no shell expansion). Error messages never print values.

In the examples below, `tgarchive` stands for `cargo run -q --` or `target/debug/tgarchive`.

## Quick start

```sh
# 1. Log in (runs db init and auth login; prompts interactively for the login code / 2FA password)
scripts/login.sh

# 2. Initialize the database (login.sh already does this; safe to repeat)
tgarchive db init

# 3. Fetch the chat list (writes metadata only; collects no messages, does not change tracking)
tgarchive chats refresh
tgarchive chats list

# 4. Choose the chats to collect (chat ids come from chats list)
tgarchive chats track -1001234567890
tgarchive chats list --tracked

# 5. Backfill history
tgarchive sync chat -1001234567890
tgarchive sync all            # tracked chats only; a successful no-op when there are none

# 6. Start the server (live collection + REST)
tgarchive serve
```

Notes:

- `chats track` does **not** fetch history by default; it only decides which chats are collected from now on. Fetch history manually with `sync chat` / `sync all`, or add `--backfill` (see "Rate limiting and pacing").
- `chats untrack` stops collection but keeps the messages already stored.
- Running `sync chat` on a chat whose history is already complete performs a **forward catch-up** instead: it fetches messages newer than `catchup_after_id` (or the largest archived message ID). The upper bound is fixed when the run starts and the run is subject to the same pacing. This fills gaps without running `serve`. The first page of a history sync also records `catchup_after_id`, so the later `serve` baseline does not skip messages that appeared after the history sync. For chats that already have archived messages, the baseline is the largest archived ID; only a brand-new chat with no data uses Telegram's current latest ID as its baseline.
- `sync chat` on an untracked chat is rejected (REST returns 409 `chat_not_tracked`).
- Tracking and untracking while `serve` is running does not require a restart.

## Querying

```sh
tgarchive messages list --chat-id -1001234567890 --limit 20
tgarchive messages get -1001234567890 42
tgarchive messages search "keyword" --chat-id -1001234567890 --from 2024-01-01T00:00:00Z --to 2024-02-01T00:00:00Z
tgarchive messages search "台北咖啡" --sort time
tgarchive search status            # search index status
tgarchive search rebuild-index     # re-tokenize and rebuild the index
tgarchive --output json messages list --limit 5
```

- `--include-deleted` (on `messages list` / `get` / `search`) also shows soft-deleted messages (human output marks them `[deleted]`; JSON includes `deleted_at`). They are hidden by default. Human output adds a sender column after the timestamp (display name, otherwise username / ID).
- `--from` / `--to` are RFC 3339 timestamps. `--before` / `--after` take the `next_cursor` returned by the previous page. `--limit` is 1–1000, default 100.
- Read-only commands only read the database and do not need Telegram. For an older database, run `db init` first (see [docs/migrations.md](docs/migrations.md)).

### Full-text search (CJK-friendly)

Search terms are always **plain text** (no FTS5 syntax; `"`, `*` and `OR` are ordinary characters). Queries and the index share the same processing: Unicode NFKC normalization + lowercasing (so full-width "ＳＱＬｉｔｅ" is found with `sqlite`), [jieba-rs](https://crates.io/crates/jieba-rs) search-mode word segmentation, and bigrams of adjacent Han characters. The index is the SQLite FTS5 contentless-delete table `messages_fts(words, bigrams)`, which does not store the original text a second time.

- **Words**: "台北", "咖啡", "GitLab Runner" and "chromium" are matched as words. Multiple words must all appear, in any order. English is case-insensitive; alphanumeric words of 3+ characters match by prefix (`benchmark` finds `benchmarks`), while words of 2 characters or fewer must match exactly.
- **Chinese substrings**: consecutive Chinese text is matched as a bigram phrase (adjacent and in order). So "北咖啡" finds "新北咖啡店" and does not match text where "台北…咖啡" merely appear separately.
- **Single Han character** (e.g. "北"): the index does not store single characters, so it falls back to a `LIKE '%北%'` scan over message text (slower; results sorted by time).
- **Sorting**: with query text the default is `--sort relevance` (BM25, words 5 : bigrams 1); `--sort time` is newest first. The relevance `next_cursor` is an opaque, versioned position cursor (also passed back via `--before`; `--after` is not supported); time sorting keeps using keyset cursors. REST: `GET /api/v1/messages/search?q=...&sort=relevance|time`, next page via `before=<next_cursor>`.
- **Snippet**: the `snippet` in a search result is a fragment of about 120 characters around the first match (counted in characters, never splitting a character; `null` if no match can be located). `text` is still the full message.
- **Soft-deleted messages**: soft deletion keeps the index entry, so they can still be found with `include_deleted=true`. Filters (`chat_id` / `sender_id` / time / deleted) apply to all paths and sort orders.
- **Index maintenance**: updated by Rust code in the same transaction as the message write (only when the text actually changes; a history sync does not overwrite newer live edits). There are no SQL triggers.
- **Index version and rebuild**: `app_metadata` records `search_index_version` (currently 1; it must be bumped whenever the tokenization / normalization / bigram rules change) and a state of `ready` / `rebuilding` / `stale`. After upgrading from an older version (migration 0006 clears the old index) the state is `stale`:
  - `tgarchive db init` rebuilds it automatically (in batched transactions; progress is printed to stderr);
  - you can also run `tgarchive search rebuild-index` at any time (it can run alongside `serve`; each batch is a short transaction), and `tgarchive search status` shows the state and the indexed count;
  - while the index is not ready (`stale` / `rebuilding`), search automatically falls back to a `LIKE` substring scan (slower, sorted by time, no NFKC full-width matching). `search_index: {version, state, indexed, total}` in `GET /api/v1/status` and a UI banner report this. `serve` does not block startup because of it and does not rebuild by itself; it only logs a warning.
- Limitations: bigrams are built for Han characters only (kana / Hangul go through jieba / unicode61 word matching only); no Traditional/Simplified conversion ("硬碟" does not find "硬盘"); custom jieba dictionaries are not supported; `indexed` / `total` in `/status` are count queries (roughly one full table scan on a large database).

## Senders, repair and refetch

- `tgarchive senders search <text> [--sort messages|last-message|name]`: matches a name / @username substring (NFKC + case-insensitive; partial Chinese names work), an exact `@username`, or a numeric ID. It shows message count, chat count, last activity, and whether the sender is yourself. `tgarchive senders get <id|@username|name|me>` additionally lists the per-chat breakdown.
- `messages list|search --sender <id|@username|name|me>`: resolves to a single user; if a name has several candidates they are listed and the command exits non-zero. `me` is the Telegram account bound to the archive. There is also `--post-author` (channel post signature, exact match).
- REST: `GET /api/v1/senders?q=&sort=messages|last_message|name&limit=&cursor=&include_deleted=` and `GET /api/v1/senders/{id}` (includes the `chats` breakdown); `/messages` and `/messages/search` support `sender_id` and `post_author`. Messages include `post_author` and `forward{from_id,from_name,date}` (the forward source does not affect sender attribution). UI: the Users entry in the top bar (使用者) and `/senders/:id`; clicking a sender name navigates there.
- Bots and previous names (migration 0008): messages / senders carry `is_bot` (Telegram's bot flag; `null` for groups / channels and unknown senders; it is only updated when a non-NULL value is later observed, and NULL never clears it). `GET /api/v1/senders?is_bot=true|false`; `exclude_bots=true` applies to `GET /messages`, `/chats/{id}/messages` and `/messages/search` (CLI `--exclude-bots`; messages with an unknown sender are kept). `GET /api/v1/senders/{id}` also includes `name_history: [{display_name, username, first_seen_at, last_seen_at}]` (newest first), and `/senders?q=` also matches previous names / previous @usernames (results carry `matched_history: true` and `matched_name`). **Previous names only reflect names and times that tgarchive observed, not when the person actually renamed**; a history sync only sees the user's *current* data. CLI: `senders get` lists previous names, and `senders search` marks them with `[matched old name: …]` and accepts `--is-bot true|false`. UI: the 🤖 marker, the user list filter All / People / Bots (全部／人／Bot), the Hide bots toggle (隱藏 bot) in search and chat views, the Previous names section (曾用名稱) on the sender page, and "Previous names: …" (曾用名：…) in the list.
- Messages you sent yourself (`out` with no `from_id`) are attributed to the bound account, and the account's own data (get_me) is written to `senders`.
- Rows in older data where `sender_id` is NULL:
  - `tgarchive repair senders [--dry-run]` (local only, no Telegram needed): for channels (kind=channel), NULLs get the channel itself as sender; it runs in batched transactions, is repeatable, and reports counts per chat. **NULLs in private chats / groups have no determinable direction and are never guessed**; they are reported as "still without sender" and need the refetch below to be filled. If the database is locked by another process, it asks you to retry later.
  - `tgarchive sync chat <ID> --refetch` (or `POST /api/v1/chats/{id}/sync?refetch=true`): re-downloads the full history under the usual pacing and fills `sender_id` / `post_author` / forward source / attachment fields **only where the existing value is NULL**. It does not change text or edit versions, does not resurrect deleted messages, and does not overwrite non-NULL values. Progress is stored in a separate checkpoint (`chat_sync_state.refetch_*`); after an interruption or rate limiting, running the same command again resumes, and the checkpoint is cleared on completion so the next run starts again from the latest message.

## Rate limiting and pacing

Telegram has **no published fixed rate limit**; limits are dynamic (depending on account, method and behavior), and the server asks you to wait X seconds with `FLOOD_WAIT_X` (420). This tool therefore takes a conservative approach:

- **Request interval (pacing)**: all `getHistory` requests (history backfill, reconnect catch-up, and the baseline probe of newly tracked chats) go through **one in-process shared pacer**, which enforces at least `SYNC_PAGE_DELAY_MS` between any two requests (default `1000`, integer `0..=60000`; `0` disables pacing and adaptive slowdown; an invalid value makes `serve` / `sync` / `chats track --backfill` fail at startup with an explanation). Pages stay at 100 messages (the API maximum) to avoid issuing more requests.
- **Serialization**: history jobs are executed one at a time by a single worker, so only one history job fetches at any moment and the rest are queued (a full queue returns 503, a duplicate for the same chat returns 409; semantics unchanged). Live catch-up does not go through this queue and may interleave between history pages, but because it shares the pacer the overall request rate still respects the minimum interval.
- **Adaptive slowdown**: after a FLOOD_WAIT the pacer interval doubles (capped at 10 s) and all history requests (including catch-up) wait out the requested number of seconds. After every 50 consecutive successful requests the interval is halved, until it returns to `SYNC_PAGE_DELAY_MS`. The log records the wait seconds and the new interval at WARN level (no message content).
- **FLOOD_WAIT cap**: `SYNC_MAX_FLOOD_WAIT_SECS` (default `300`, `0..=86400`). If Telegram asks for a wait longer than the cap, or the same page is still limited after 5 consecutive FLOOD_WAITs, the tool **stops sleeping and waiting**:
  - The history job ends in the resumable `rate_limited` state with the error summary `Telegram rate limit: retry after N s`; committed progress and the checkpoint are kept, and running `sync chat` (or `POST /api/v1/chats/{id}/sync`) again resumes from the checkpoint. `sync all` stops when this happens and does not try the remaining chats. The CLI exits non-zero and prints the summary.
  - Live catch-up skips that chat for the current round (WARN + writes `last_error`) and tries again in the next round, without blocking live updates.
- **Status**: `rate_limit` in `GET /api/v1/status` contains `interval_ms` (current interval), `base_interval_ms`, `last_flood_wait_secs` and `last_flood_at` (present only for `serve` in Telegram mode; the CLI `status` is a separate process and cannot see the pacer, but it does list job states including `RateLimited`).

### Track and backfill: `--backfill`

```sh
tgarchive chats track -1001234567890 --backfill
curl -X PUT "http://127.0.0.1:8080/api/v1/chats/-1001234567890/tracking?backfill=true"
```

- Without the flag, behavior is unchanged (track only).
- REST (requires `serve` in Telegram mode; otherwise it returns 503 `busy` and leaves tracking unchanged): after tracking, a history job is queued on the coordinator, and the response adds `backfill_job_id` and `backfill: "queued"` to the ChatDto. If the chat (or `sync all`) already has a queued / running job, no duplicate is created and the response is `backfill: "already_running"` with the existing job id. Follow progress with `GET /api/v1/sync/jobs/{id}`.
- CLI (no server; needs Telegram credentials and no other process holding the session): it tracks and commits first, then runs the same history backfill as `sync chat` in this process. If credentials are missing, tracking is still saved, but the command exits non-zero and explains how to run `sync chat` later.

### Terms of service reminder

Data obtained through the Telegram API **must not be used for AI / machine-learning training** (see [Telegram API Terms of Service 1.5](https://core.telegram.org/api/terms)). Please also comply with the other terms, and accept the risk that heavy fetching may get your account restricted; the defaults above are conservative but do not guarantee you will not be rate limited.

## serve and status

```sh
tgarchive serve                  # Telegram mode: live collection + REST
tgarchive serve --query-only     # serve queries over stored data only, no Telegram connection
tgarchive serve --bind 127.0.0.1:9000
```

- If `TELEGRAM_API_ID` / `TELEGRAM_API_HASH` are not set (for example because `.env` is not in the current working directory), `serve` falls back to query-only mode: it prints a warning on stderr at startup, **live collection does not run**, and `collector.detail` in `/api/v1/status` says Telegram is not configured. With an explicit `--query-only` no warning is printed and the detail says `--query-only`.
- By default only loopback addresses can be bound, and the **API has no authentication**. For remote access, use an SSH tunnel or an authenticated reverse proxy. Containers must bind `0.0.0.0`, so there is an explicit opt-in: `serve --allow-non-loopback` or the environment variable `TGARCHIVE_ALLOW_NON_LOOPBACK=1` (`0` / `false` / empty count as unset); when enabled and bound to a non-loopback address, startup prints a WARN. Default behavior is unchanged.
- `tgarchive healthcheck [--url http://127.0.0.1:8080/health/ready]`: probes a running server with plain std HTTP/1.0 (exit 0 on 2xx); the default port comes from `SERVER_BIND`. It is meant for the Docker HEALTHCHECK (the image has no curl).
- Check the collector state via `GET /api/v1/status`: `starting → catching_up → running → reconnecting → stopped / failed` (`disabled` with `--query-only`). The response also contains `unresolved_deletions` (the number of deletions that could not be mapped to a chat). `/health/live` is a liveness check; `/health/ready` returns 503 when the database is unavailable or the collector is `failed`.
- `tgarchive status` is a separate process that only reads the database; it **cannot see** the collector of a running server and shows `disabled`. Use REST instead.

```sh
curl -s http://127.0.0.1:8080/api/v1/status
curl -s "http://127.0.0.1:8080/api/v1/messages/search?q=keyword"
```

## Web UI

`tgarchive serve` (including `--query-only`) serves an embedded web UI (React + Vite + TypeScript) at `/`; the API remains at `/api/v1`, `/health/*` and `/openapi.*`. After starting, open <http://127.0.0.1:8080/>: conversations (timeline, infinite scroll, context view, start / stop collecting), search, sync and status. The UI is same-origin with the API and uses no external CDN or fonts; UI responses carry `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer` and a CSP that allows only `self`. It is still bound to loopback only and has **no authentication**.

- The build output `web/dist` is **committed** and embedded into the binary by `build.rs` via `include_bytes!`, so `cargo build` does not need Node.
- Frontend development: run `tgarchive serve` first, then `npm --prefix web ci && npm --prefix web run dev` and open <http://127.0.0.1:5173/>. Vite proxies `/api`, `/health` and `/openapi.*` to `127.0.0.1:8080`, so CORS is not needed (alternatively use `TGARCHIVE_DEV_CORS_ORIGIN`).
- Rebuild and commit: `npm --prefix web run build` (includes typecheck). After changing `openapi.yml`, run `npm --prefix web run gen:api` to regenerate `web/src/api/schema.d.ts`.
- Checks: `npm --prefix web run typecheck`, `lint`, `test`; when `npm` is available, `scripts/check.sh` runs them and verifies that `web/dist` matches the sources.
- The search page supports Chinese substrings (jieba words + character bigrams), a relevance / time sort switch, snippets and the index-status banner; see "Full-text search (CJK-friendly)".

## REST and OpenAPI

Routes (all under `/api/v1`): `chats`, `chats/refresh`, `chats/{id}`, `chats/{id}/tracking` (PUT, optionally with `?backfill=true` / DELETE), `chats/{id}/messages`, `chats/{id}/messages/{message_id}`, `chats/{id}/messages/{message_id}/context`, `chats/{id}/senders`, `chats/{id}/sync`, `messages`, `messages/search`, `sync`, `sync/jobs/{id}`, `sync/status`, `status`; plus `/health/live`, `/health/ready`, `/openapi.json` and `/openapi.yml`. Requests time out after 30 seconds, queries and bodies have size limits, and every response carries `x-request-id`.

Read capabilities used by the web UI:

- **Senders**: `MessageDto.sender` is `{ id, display_name, username, is_bot }` (`null` when there is no sender; for channel posts made in the channel's own name, `display_name` is the chat title). It comes from a JOIN on `senders` in the query, so there are no extra N+1 queries. `GET /chats/{id}/senders?limit=` (default 100, max 1000) returns `[{ id, display_name, username, is_bot, message_count }]`, ordered by `message_count` descending and excluding deleted messages.
- **Deleted messages**: `messages`, `chats/{id}/messages`, `messages/search`, `chats/{id}/messages/{message_id}` and `.../context` all support `include_deleted=true` (default `false`, behavior unchanged). `MessageDto` always includes `is_deleted` and `deleted_at`. Deletions that only left a tombstone (no stored message body) do not appear.
- **Chat statistics**: `ChatDto.stats` (`GET /chats`, `GET /chats/{id}`, tracking responses; omitted by `POST /chats/refresh`) contains `message_count` (non-deleted), `deleted_count`, `first_message_at`, `last_message_at`, `history_complete`, `last_sync_completed_at` and `last_error` (sanitized, may be `null`). `GET /chats?sort=title|last_message|message_count` (default: by chat ID).
- **Message context**: `GET /chats/{id}/messages/{message_id}/context?before=20&after=20` (each 0–100; larger values return 400 `invalid_context_size`) returns `{ anchor, before, after, has_more_before, has_more_after, before_cursor, after_cursor }`, with `before` / `after` both ordered oldest to newest; `before_cursor` can be used directly as `before` on the list endpoints and `after_cursor` as `after` to keep scrolling. A missing anchor (or a deleted one without `include_deleted`) returns 404.
- **CORS (local frontend development only)**: off by default. After setting `TGARCHIVE_DEV_CORS_ORIGIN=http://127.0.0.1:5173`, `serve` enables CORS for exactly that origin (GET/POST/PUT/DELETE, headers `content-type`, `x-request-id`). The value must be a loopback `http://` origin (`127.0.0.1` / `localhost` / `[::1]`, optional port, no path); otherwise `serve` fails to start. The API itself still has no authentication and binds loopback only.

The spec is [`openapi.yml`](openapi.yml) in the repository root. It is generated by the program; do not edit it by hand:

```sh
cargo run -q -- openapi --format yaml > openapi.yml
```

A test compares the committed file with the generated output and fails if they differ.

## One process per session

The session file has an owner lock (`<session>.lock`). If `serve`, `auth login`, `chats refresh`, `sync ...` or any other process that needs Telegram runs concurrently, the later one fails with a conflict. Stop `serve` first, or trigger the action via REST (`POST /api/v1/chats/refresh`, `/api/v1/sync`).

## Live-account acceptance test

`tests/live_telegram.rs` uses a real account to automatically verify live collection, offline gap recovery, idempotency across restarts and isolation of untracked chats (replacing manual testing on a phone). It is `#[ignore]`d by default and is skipped when the environment variables are missing.

It uses **two sessions of the same account**: the archive session (`TELEGRAM_SESSION_FILE`, used by the `serve` that the test starts) and a separate driver session (`TELEGRAM_DRIVER_SESSION_FILE`, which only sends / edits / deletes test messages and never uses the archive session, so that it does not advance the archive's update state and distort the gap test).

One-time driver login (enter the verification code interactively):

```sh
scripts/login-driver.sh   # writes TELEGRAM_DRIVER_SESSION_FILE (default ./telegram-driver.session)
```

Required environment variables: `TELEGRAM_API_ID`, `TELEGRAM_API_HASH`, `TELEGRAM_SESSION_FILE`, `TELEGRAM_DRIVER_SESSION_FILE`, `LIVE_TEST_CHAT_ID` (bot-API-style id, e.g. `-4893203104`), and set `LIVE_TELEGRAM=1`. If the test chat is a supergroup / channel, `LIVE_TEST_ALLOW_CHANNEL=1` is also required.

```sh
set -a; . ./.env; set +a; LIVE_TELEGRAM=1 LIVE_TEST_CHAT_ID=-4893203104 cargo test --test live_telegram -- --ignored --nocapture --test-threads=1
```

Notes:

- It **really sends, edits and deletes messages in the `LIVE_TEST_CHAT_ID` chat** (all texts start with `[archive-live <run_id>]`, only messages created by the current run are touched, and leftovers are deleted on a best-effort basis at the end). Use a dedicated test chat.
- The test runs `sync chat` on that chat's existing history into a temporary database, which takes longer if the history is long.
- The test uses a fresh temporary database and does not touch your `telegram.db`.
- No archive `serve` (or any process using the same archive session) may be running beforehand (owner lock).
- On failure it prints the tail of the serve log (with api_hash / phone number masked).

## Image previews and archives

When `serve` is connected to Telegram, new images in tracked chats automatically queue a preview download. The Web UI shows the local preview and its download state. Photo previews use an available image version with a longest edge of at most 800 pixels; JPEG/PNG/WebP documents use Telegram's thumbnail. A document without a suitable thumbnail shows an unavailable preview instead of downloading its full file automatically.

Each chat has an **Automatically download archive version** (自動下載封存版本) checkbox, off by default. Enabling it adds the largest available Photo version or the full image document alongside the preview. Previews remain enabled. Changing this setting does not scan existing history or delete completed files. You can also request an individual archive with **Download archive version** (下載封存版本).

The chat view and Sync page show cumulative image-download counts for each chat and preview/archive variant: queued, downloading, completed, failed (including scheduled retries), interrupted and unavailable. Counts refresh every two seconds; requesting preview backfill refreshes both the progress panel and visible message previews immediately. These counts cover current attachments from automatic and manual downloads, rather than a separate batch history for each click. `GET /api/v1/media/downloads/status` provides the same read-only summary, including in query-only mode.

For images already in the database, use **Backfill previews** (補下載預覽) in the chat. Downloads are durable and deduplicated, use the current Telegram session, and are limited to 20 MiB per file. Missing/deleted sources or lost chat access can prevent a later download. Telegram's largest Photo version can still be compressed and is not necessarily the original uploaded file.

Files live in `MEDIA_DIR`, defaulting to a `media` directory beside the database (`/data/media` with the supplied Compose configuration). SQLite stores source identity, preview/archive state and file paths; image bytes remain on disk. Query-only servers can display completed local images, but cannot enqueue Telegram downloads or save settings to their read-only database. Back up both the database and media directory. Deleted/replaced images remain on disk but their old content URLs are no longer available through the API.

## Docker

Image `ghcr.io/swind/rust-tgarchive` (linux/amd64 and linux/arm64, selected automatically under the same tag; `gcr.io/distroless/cc-debian12:nonroot` base, default uid 65532). The runtime contains only the stripped `tgarchive` binary, glibc/libgcc and CA certificates: **no shell, package manager, curl or sqlite3**, so `docker exec ... sh` does not work. All state lives in `/data`: `telegram.db` (with `-wal` / `-shm`) and `telegram.session` (equivalent to account access, mode 0600). Default environment variables inside the container: `DATABASE_URL=sqlite:///data/telegram.db`, `TELEGRAM_SESSION_FILE=/data/telegram.session`, `SERVER_BIND=0.0.0.0:8080`, `TGARCHIVE_ALLOW_NON_LOOPBACK=1`.

Setup with [docker-compose.yml](docker-compose.yml) (state in the host directory `./data`):

```sh
mkdir -p data                      # create it yourself so it is owned by you, not root
cat > .env <<'EOF'
TELEGRAM_API_ID=...
TELEGRAM_API_HASH=...
EOF
docker compose run --rm tgarchive auth login   # first time only (interactive, needs a TTY)
docker compose up -d
```

- The compose service runs as `${TGARCHIVE_UID:-1000}:${TGARCHIVE_GID:-1000}`, so `./data` must be writable by that user. If your host user is not 1000:1000, run `export TGARCHIVE_UID=$(id -u) TGARCHIVE_GID=$(id -g)` before running compose, or put the numeric IDs in `.env`.
- **Network exposure**: the REST API has no authentication, so the port is published on `127.0.0.1:8080` by default. To expose it on the LAN, explicitly opt in with `TGARCHIVE_BIND_IP=<host LAN IP>` (or `0.0.0.0`), and only on a network you trust.
- Management commands (e.g. `docker compose run --rm tgarchive chats list`) use the same `./data`.
- **One process per session**: before logging in or running management commands, run `docker compose stop` first (or use a separate container running `serve --query-only`, which does not connect to Telegram).
- With plain `docker run`, bind-mount a directory writable by the container user (`-v "$PWD/data:/data" --user "$(id -u):$(id -g)"`).
- The working directory is `/data`, so `/data/.env` is loaded automatically (process environment variables still take precedence); ignore it if you do not need it.
- **Backup**: stop the stack (`docker compose stop`, which merges the WAL) and copy `./data` on the host, including `media`; a SQLite-only backup does not include images. If `MEDIA_DIR` points outside `./data`, back up that directory too. Store the session file encrypted.
- **Upgrade**: Compose defaults to the latest successful `main` image. Run `docker compose pull && docker compose up -d` to update. To pin a build, set `TGARCHIVE_IMAGE_TAG=0.1.5-build.123` in `.env`; to use a manual release, set it to `0.1.5` or `latest`. Database migrations are applied automatically at startup; back up `./data` first.
- **Healthcheck**: the image has a built-in `HEALTHCHECK` (runs `tgarchive healthcheck` every 30 seconds, i.e. `GET /health/ready`); `docker ps` shows `healthy` / `unhealthy`. `/health/ready` returns 503 when the database is unavailable or the collector is `failed`.
- Local build: `docker build -t tgarchive .` (no Node needed; `web/dist` is committed).

## Backup and security

The database uses WAL, so **do not just `cp telegram.db`** (it would miss the contents of `-wal`). Use a consistent SQLite backup:

```sh
sqlite3 telegram.db ".backup 'backup.db'"
# or
sqlite3 telegram.db "VACUUM INTO 'backup.db'"
```

- The session file (mode 0600) is **equivalent to account access**: whoever obtains it can sign in to your account. Back it up carefully, store it encrypted, and never commit it to Git.
- `api_hash`, phone numbers, login codes and passwords are never written to logs; logs (INFO/WARN) also contain no message content.
- Configuration errors (database / session paths, a non-loopback bind without opt-in) fail at startup with an explanation of how to fix them.
- `TGARCHIVE_ALLOW_NON_LOOPBACK=1` / `--allow-non-loopback` exposes the **unauthenticated** REST API (including write endpoints such as sync and tracking, and all message content) to anyone who can reach the port. Use it only inside containers and publish with `-p 127.0.0.1:8080:8080`; for remote access, put it behind an authenticated reverse proxy or an SSH tunnel.

## Recovery behavior and limitations

- After a restart or disconnection, a catch-up runs (in rounds and pages, committed in the same transaction as the checkpoint), and the process reconnects automatically (exponential backoff, capped at 60 seconds).
- Reprocessing is **at-least-once** and kept idempotent with upserts. **Exactly-once or zero-loss delivery is not claimed.**
- Ordinary (non-channel) deletion events carry no chat information and can be ambiguous: a message is marked deleted only when it maps to an archived, tracked message; otherwise it is counted in `unresolved_deletions`.
- After being offline too long (Telegram reports an update gap that is too large), the update state is reset automatically and tracked chats are caught up: the status briefly shows `degraded`, then returns to `running` with an explanation and a count. **Edits and deletions of older messages during the gap may be missed**; repeated occurrences within a short time back off.
- Updates from untracked chats are always ignored.
- A deletion event that arrives before the message is archived is not remembered.
- Live-account real-time updates, offline gap recovery, real network-loss reconnection and basic groups still await manual verification; see [IMPLEMENTATION_STATUS.md](IMPLEMENTATION_STATUS.md).

## Development

`src/application.rs` re-exports the application contracts from message, sender, chat, ingestion, error and port modules; existing imports remain stable. `src/bootstrap.rs` dispatches commands, with authentication, database setup, server lifecycle, background runtime and CLI sync wiring under `src/bootstrap/`. Sync coordination is separate from the engine. SQLite repository implementations live under `sqlite/store/`; CLI parsing, execution, query conversion and rendering, and REST route/DTO groups each have their own modules. Large inline unit tests live beside their production module in a `tests.rs` child module.

```sh
scripts/check.sh   # fmt, test, clippy (with npm, also web typecheck / lint / test / build and dist consistency)
```

### CI and releases

- The base version (currently `0.1.5`) is changed manually. CI never bumps it or writes version changes back to the repository.
- Every push to `main` runs `release.yml`: full Rust, web and Playwright CI, then a multi-arch (amd64 + arm64 via QEMU) GHCR image build with provenance and SBOM. Each build publishes `<version>-build.<number>` (for example `0.1.5-build.123`) and `sha-<short>`. The build number is GitHub's `github.run_number` for this workflow; release-tag runs also consume numbers, failed runs can leave gaps, and reruns keep the same number and may replace that build tag. The image labels include its build version, build number and source revision. The binary/API version stays at the manual base version.
- Successful builds update the rolling `main` tag only if their commit is still the head of `main`. Main builds leave `latest` and manual version tags unchanged. Separate main runs are not cancelled or replaced by CI concurrency grouping.
- `.github/workflows/ci.yml` is reused by image builds, runs directly on PRs, and supports manual dispatch (`run_e2e`). It checks Rust fmt/clippy/tests with `--locked`, web typecheck/lint/tests/build and committed `web/dist`; image builds always include E2E. Failure artifacts include `web/e2e/test-results`. `scripts/check.sh` matches the local CI checks.
- Manual release: update `version` in `Cargo.toml`, update `Cargo.lock`, regenerate `openapi.yml` with `cargo run -q -- openapi --format yaml > openapi.yml`, then commit, `git tag vX.Y.Z`, and `git push origin main vX.Y.Z`. The workflow validates the tag against Cargo, runs full CI, and additionally publishes `<version>`, `<major.minor>` and `latest`. Pre-release tags such as `v0.2.0-rc.1` publish their exact version but leave `latest` and `<major.minor>` unchanged.
- **After the first release**: on GitHub go to the repo's Packages → `rust-tgarchive` → Package settings, change visibility to Public (GHCR defaults to private), and confirm the package is linked to this repo.
- Recommended repo settings: enable branch protection / a ruleset on `main` requiring a PR and passing status checks `rust` and `web` (`e2e` does not run on PRs, so do not make it required); set the default Actions token permissions to read-only. Dependabot (`.github/dependabot.yml`) updates weekly: Actions, cargo (ignoring the vendored grammers-*), npm (`web`, `web/e2e`; `@playwright/test` is not upgraded automatically because screenshot baselines are tied to the image version) and the Dockerfile base image.
- The repo currently has no LICENSE file, so the image carries no license label.

### E2E tests

Browser E2E tests (Playwright, `web/e2e/`) do not need Telegram: `scripts/e2e.sh` runs `cargo build`, creates a fixture database with fixed timestamps via `cargo run --example seed_fixture -- <db>` (7 chats, 250+ messages, deleted / edited / reply / attachment / system messages, sync jobs in every state, and unattributed deletions), starts `serve --query-only` on a random loopback port, then runs the tests with `--network host` inside the official image `mcr.microsoft.com/playwright:v<version>-noble` (the version matches `@playwright/test` in `web/e2e/package.json`); the server is always shut down at the end. Docker is required; without docker it uses a locally installed browser (skipping screenshot comparison), and if neither is available it prints a message and skips.

```sh
scripts/e2e.sh                       # run everything
scripts/e2e.sh -g search             # extra arguments are passed to playwright test
scripts/e2e.sh --update-snapshots    # regenerate screenshot baselines (always inside the container so fonts match), then commit web/e2e/__screenshots__
E2E=1 scripts/check.sh               # also run E2E at the end of check.sh (needs docker)
```

In the tests the clock is pinned to 2026-03-12 with timezone Asia/Taipei, and requests to external hosts are aborted and make the test fail.

### Search quality and performance

- **Quality evaluation** (part of the regular `cargo test`, about 8 seconds): `tests/search_quality.rs` generates 8,000 realistic Traditional Chinese chat messages from a fixed seed (`tests/support/corpus.rs` + `vocab.rs`: Zipf word frequencies, about 6,400 words / phrases, place / person names, mixed Chinese-English technical terms, slang, emoji, URLs, numbers, full-width characters and a little Simplified Chinese, about 5% edited, about 2% soft-deleted, multiple chats and senders, spanning a year). It builds the index through the real write path and then runs 60+ queries (common / rare words, 2-character words, substrings crossing segmentation boundaries, single characters, consecutive phrases, scattered words, English exact / prefix / plural, mixed Chinese-English, full-width, no results, and chat / sender / time / include_deleted filters) against a **ground truth computed directly on the text, independent of the index**. Thresholds: recall 100% for all queries, precision 100% for exact-type queries, 0 results for no-result queries, identical result sets under both sort orders, and time sorting strictly newest first; for phrase-type queries (where the word path allows extra "scattered" results) the extra results must be "explainable", the first relevance result must be a contiguous match, and the share of contiguous matches among the top 10 must be ≥ 0.8. The definitions are in the comment at the top of the file. `cargo test --test search_quality -- --nocapture` prints a per-category summary table.
- **Speed benchmark** (manual, always use `--release`):

  ```sh
  cargo run --release --example search_bench -- --messages 100000 --messages 1000000 --out target/search-bench.md
  cargo run --release --example search_bench -- --db /path/to/telegram.db   # or SEARCH_BENCH_DB=...
  ```

  It builds the same generated corpus in a temporary directory (`--db` / `SEARCH_BENCH_DB`: the database is first **copied** to a temporary directory, migrations / index rebuilds touch only the copy, and the original file is never opened) and reports write time, `rebuild-index` time, DB and FTS sizes, and min / p50 / p95 / max for about 25 query types (at 1M, a relevance offset of 90000 takes about 1.1 s and emits a WARN) (common / rare words, full-table LIKE for rare single characters, no results, mixed, filters, deep relevance offset, deep time keyset, include_deleted, English prefix) × both sort orders, marking PASS / WARN against loose budgets (for ≤200k messages p95 < 100 ms, for larger sizes < 500 ms; full-table LIKE for single characters gets 10× slack). By default it only warns; `--strict` makes it exit non-zero. `cargo test --release --test search_bench_smoke -- --ignored` runs the example once with 20,000 messages so that it does not rot. The latest results and their interpretation are in [IMPLEMENTATION_STATUS.md](IMPLEMENTATION_STATUS.md).

Other documentation: [docs/](docs/), [migrations notes](docs/migrations.md).

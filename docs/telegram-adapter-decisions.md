# Grammers adapter decisions (Phase 0)

Checked against the published Grammers 0.10.0 docs/source and Telegram's official MTProto documentation on 2026-10-02. The current minimal Cargo project does not include Grammers and no adapter spike has been compiled, so API compile behavior is **not** claimed as locally tested. There are no Telegram API credentials, authenticated user session, or test chat available here; all real-account items remain pending.

## Selected library surface

Use registry `grammers-client = "0.10"`; use its `grammers_client::session` and `grammers_client::sender` re-exports to keep Grammers versions aligned. `grammers_client::sender::SenderPool::new(Arc<Session>, api_id)` provides a runner, handle, and the single updates receiver. Keep and supervise the runner task for the full application lifetime. One session instance must not be shared between multiple pools.

After login, persist a Grammers session implementation (the published `SqliteSession` is available) because it owns authorization keys, datacenter data, peer cache, and update state. Do not confuse this session DB with the archive DB. Use `Client::iter_dialogs()` once after login to cache dialogs/peer data; Grammers says this enables difference recovery for peers. Resolve archived `ChatId` through the session's cached `PeerId`/peer reference and report missing/expired peer data as a typed adapter error. Never serialize `access_hash` as a domain ID or expose it from the adapter.

Session update state has additional mutation sites to account for: `auth.rs` fetches `updates.getState` after user sign-in and stores `UpdateState::All`; `dialogs.rs` stores each channel dialog's `pts` with `UpdateState::Channel`. Any app-owned acknowledged-state mirror must mediate these writes too. In particular, a dialog refresh must not overwrite a previously committed channel cursor with a newer dialog snapshot while archive events remain uncommitted. Wrapper initialization and merge rules are part of the Phase 8 spike.

The library has a friendly high-level surface (`iter_dialogs`, `iter_messages`, `stream_updates`) plus raw TL access through `grammers_client::tl`. Avoid raw `invoke` for ordinary operations; it is explicitly outside the client's semver stability guarantees. Raw TL is only a documented adapter escape hatch for a missing field/event.

## History behavior

`Client::iter_messages(peer)` iterates most-recent to oldest by default. `MessageIter::offset_id(i32)` is documented as an **exclusive** starting message ID; `.reverse(true)` requests oldest-to-newest and uses `min_id` logic. The underlying implementation requests `messages.getHistory` with `offset_id`, `max_id`, and `min_id` and explicitly warns not to stop just because a returned slice is shorter than the request limit. Telegram says message IDs can have legitimate gaps from deletions and recommends using history APIs and the corresponding update state rather than filling gaps by arithmetic.

Decision: use descending history with an exclusive `history_before_id`, convert domain ID to the API's checked `i32` type, persist only after the page's archive write commits, and use iterator exhaustion as completion. Re-fetching boundary pages is safe. The source code's full pagination behavior still needs a deterministic adapter test with sparse IDs and a history larger than one network page (P0 repeatable check below).

## Real-time updates, persistence ordering, and failure boundary

The official 0.10.0 surface returns one non-cloneable, sequential `UpdateStream`; `next_raw()` returns `(tl::Update, grammers_session::updates::State, PeerMap)`. `State` is public and documented as state **up to and including the update it belongs to**, with `date`, `seq`, and an optional per-update `MessageBox`; `MessageBox::pts()` exposes the corresponding common, secondary, or channel sequence value. `UpdateStream::sync_update_state()` stores the stream's entire current `MessageBoxes` state, and docs say it is not automatically called on drop. `client.rs` contains a doc comment saying `Client` drop syncs state, but inspection of published 0.10.0 source found no `impl Drop for Client`; treat this as stale/unverified documentation, not observed behavior. Auth and dialogs do explicitly call `Session::set_update_state` as described above.

Do **not** persist the stream's aggregate state after committing one event: Grammers first processes an incoming Telegram update batch into its `MessageBoxes` and only then puts returned events into its internal buffer. Therefore aggregate state can be ahead of the event currently being handled. `UpdatesConfiguration::default()` sets a queue limit of 100; source explicitly truncates/drops excess queued updates while the message-box state has already been processed. Setting an arbitrary finite limit retains a silent-loss path (with a warning); `None` disables the limit but the docs warn of unbounded memory exhaustion.

### Checkpoint decision: safe ordering is not established yet

The following are **observed from public docs/source**:

- `UpdateStream::next_raw()` returns one `(tl::Update, State, PeerMap)` tuple. `State` is documented as state up to and including the update it belongs to; its public fields are `date`, `seq`, and `Option<MessageBox>`. `MessageBox::pts()` exposes the value, and the public enum distinguishes Common, Secondary, and Channel scopes.
- In `updates.rs`, Grammers stores a `VecDeque<(tl::Update, State, PeerMap)>`; socket processing calls `message_box.process_updates(updates)` before extending that queue. The source shows queue truncation when its configured finite limit is exceeded.
- `sync_update_state()` saves `message_box.session_state()` (the aggregate state), not the `State` paired with the last value returned by `next_raw()`. The `Client` source doc comment mentions drop synchronization, but no `impl Drop for Client` exists in the downloaded 0.10.0 source. `Session::set_update_state(UpdateState)` has no connection to an archive transaction unless we provide one.
- Telegram's global and channel differences use their independent update-state sequences, and recovery stops being sufficient when the server reports an over-retained/too-old difference.

The following remains **inference pending a Phase 8 spike**: the attached per-item `State` may be usable to advance one precise PTS/QTS scope, but the published surface does not yet prove to this investigation that `date`/`seq` are safe to commit per returned tuple when Grammers has already processed a whole `UpdatesLike` payload. It also does not prove that applying the attached `State` into `UpdatesState` preserves unrelated scopes. Therefore do not implement or document the proposed per-event `Session` mirror as a guaranteed recovery protocol until a compile/crash test proves both properties. In particular, never copy aggregate `sync_update_state()` after an individual archive commit.

**Decision for Phase 0:** preserve the release gate; make no exactly-once or zero-loss claim. Phase 8 must first implement the smallest isolated spike, then select one of these outcomes:

1. If the spike proves that each returned `State` can be merged without advancing unrelated scopes, atomically persist normalized event + only that event's scope checkpoint in the archive DB. Have an app-owned `Session` mirror expose the committed state for `stream_updates(catch_up: true)` and prevent Grammers aggregate drop/sync from writing ahead of it. On a writer failure, terminate/restart the stream from the last committed state.
2. Otherwise, test whether the runner's original `UpdatesLike` boundary can be durably captured before Grammers processes it, or implement a durable inbox containing a whole payload plus recoverable update-state checkpoint. The public high-level `UpdateStream` hides the original batch boundary after it queues individual updates, so this may require using the lower `SenderPool` update receiver and is **not yet known to be a small change**. If it is not small, record the exact loss window and defer a stronger guarantee; do not build a custom MTProto update engine.

Even outcome 1 is conditional on Telegram difference availability: a crash before local commit is recoverable only while Telegram can still serve the needed difference. `differenceTooLong` / `channelDifferenceTooLong` are handled by the automatic reconciliation decision below (superseding the earlier "unrecoverable" requirement). Session wrapper async access must not block a Tokio executor: keep an acknowledged in-memory mirror updated only after the archive transaction succeeds, and use async DB reads/writes outside blocking locks; this too needs implementation-level concurrency testing.

Set `update_queue_limit: None` only if the adapter owner also monitors queue/memory pressure and treats memory exhaustion as a known ceiling; otherwise the library's finite queue can drop events before the app sees them. This exact tradeoff needs a real load test. Do not claim zero loss based on a bounded Tokio queue or graceful shutdown.

## Deletion mapping

- `updateDeleteChannelMessages` contains `channel_id` plus message IDs. Map to the encoded channel `ChatId` and emit scoped Deleted events.
- `updateDeleteMessages` contains message IDs and no peer. Telegram documents that private/basic-group IDs share a common account message-ID sequence, so resolve exactly one matching archived common message by `(account, message_id)`. If not present, store an account-scoped common tombstone so later history ingestion cannot resurrect it. Do not match against channel rows. Keep unresolved-count telemetry for common tombstones without an archived chat row.
- Service/non-message updates may have no archive event, but their update checkpoint still has to be advanced in order. Test this explicitly.

## Flood waits and dialogs

Grammers documents flood-wait errors and `AutoSleep` retry for small waits; preserve the typed error/wait duration at the adapter boundary. For coordinated sync, prefer an explicit retry policy so the application can cancel the wait and expose progress. Never hold a DB transaction while sleeping. `iter_dialogs()` is a user-account API; bot accounts do not have dialogs. Product scope is a user account, so bot mode is unsupported.

## Repeatable verification plan (no credentials required for the first checks)

After Grammers is added in its implementation phase, run from repository root:

```sh
cargo tree -i grammers-client
cargo tree -i grammers-session
cargo doc --no-deps
cargo test telegram_adapter::mapper
```

Add a small deterministic fake/fixture test before live integration:

1. Convert a private/basic-group/channel peer with the selected Bot API ID conversion and assert no collision; missing peer/access hash returns a typed error.
2. Iterate fixture history with sparse message IDs across more than one returned page; prove exclusive boundary yields no duplicate, no skipped fixture record, and completion comes from iterator exhaustion rather than a short slice.
3. Feed fixture updates for common new/edit/delete and channel delete. Assert channel identity comes from `channel_id`; common delete never touches channel rows; unknown common deletion creates a durable account-scoped tombstone.
4. Assert cursor remains old if writer transaction fails; after successful transaction it equals only that event's common/secondary/channel scope state. Force a two-event buffered Telegram update payload and kill/restart between commits; verify restart baseline is last committed state and replay converges.
5. Force queue overflow and verify configured policy cannot silently claim healthy. Confirm no `sync_update_state()` or client-drop path writes a state newer than archive cursor.
6. In a fixture Updates payload with multiple per-scope events, compare every `next_raw()` attached `State` to source payload; prove or disprove that `date`/`seq`, common PTS, QTS, and each channel PTS can be merged independently at each tuple boundary. If not, verify the whole-payload inbox alternative or document the unavoidable crash window.

**Phase 8 implementation decision (automated-test evidence only):** the deferred-ack option is used: `process_stream` persists the aggregate update state only after the writer acknowledges the whole buffered batch. Tests with a real `FileSession`/SQLite store show that a simulated crash before the archive commit, and after the commit but before the checkpoint, leave the checkpoint un-advanced; a restart replays the payload and the archive keeps a single row. This shows at-least-once reprocessing for these two boundaries only; it is not an exactly-once or zero-loss claim, and kill-process tests on a real account remain pending. Message-level catch-up (`catchup_after_id`, fixed per-round upper bound, per-page commit) runs before each live session and is separate from Grammers' own difference recovery. `InvocationError::Dropped` is treated as reconnectable: the source tears down the old sender pool (disconnect, join runner, release the owner lock) and `TelegramAdapter::reconnect` builds a new `SenderPool`/client/update receiver over the same `FileSession`, so `stream_updates(catch_up: true)` resumes from the archive-acknowledged checkpoint. The adapter exposes its current client via an accessor, so history sync and the coordinator follow the rebuilt client and only one client owns the session at a time. Evidence is fake-connector and local-session tests only; behaviour after a real network disconnect (including getDifference recovery) is not yet verified, and calls in flight during a rebuild fail transiently.

Live manual checks (pending API ID/hash, user account, login code/2FA, and accessible test chats):

1. `auth login`; restart; verify session remains authorized without another code.
2. Refresh dialogs; open at least one private chat, basic group, and channel/supergroup, each with a peer visible in the session cache.
3. Sync history spanning >1 API page; kill process after a committed page and resume; compare message IDs against Telegram UI/export for the chosen test chat.
4. Send/edit/delete test messages in a private/basic group and channel; verify resulting rows and tombstones and observe startup difference recovery after forced termination at each side of the archive transaction.
5. Disconnect the collector, create messages/edits/deletions, reconnect and verify Grammers differences repair gaps; separately confirm/report the behavior if Telegram returns a too-long difference.
6. Generate sufficient update load to exercise the queue setting; measure memory and confirm a warning/drop cannot pass health checks unnoticed.

Until those credentials and checks exist, login/session survival, actual dialog/history access, observed FloodWait, event deletion behavior, and crash recovery are **pending manual verification**. Documentation/source inspection only is not a real-account test.

## Primary evidence

- [Grammers client 0.10.0](https://docs.rs/grammers-client/0.10.0/grammers_client/), [SenderPool](https://docs.rs/grammers-client/0.10.0/grammers_client/struct.SenderPool.html), [Client API](https://docs.rs/grammers-client/0.10.0/grammers_client/client/struct.Client.html), [MessageIter source](https://docs.rs/grammers-client/0.10.0/src/grammers_client/client/messages.rs.html)
- [UpdateStream 0.10.0](https://docs.rs/grammers-client/0.10.0/grammers_client/client/struct.UpdateStream.html), [updates.rs source](https://docs.rs/grammers-client/0.10.0/src/grammers_client/client/updates.rs.html), [client.rs source incl. UpdatesConfiguration defaults](https://docs.rs/grammers-client/0.10.0/src/grammers_client/client/client.rs.html)
- [Grammers Session trait](https://docs.rs/grammers-session/0.10.0/grammers_session/trait.Session.html), [SqliteSession](https://docs.rs/grammers-session/0.10.0/grammers_session/storages/struct.SqliteSession.html), [PeerId](https://docs.rs/grammers-session/0.10.0/grammers_session/types/struct.PeerId.html), [per-update State](https://docs.rs/grammers-session/0.10.0/grammers_session/updates/struct.State.html), [MessageBox public API/source](https://docs.rs/grammers-session/0.10.0/src/grammers_session/message_box/defs.rs.html), [MessageBoxes state source](https://docs.rs/grammers-session/0.10.0/src/grammers_session/message_box/mod.rs.html)
- Telegram [Bot API dialog ID conversion](https://core.telegram.org/api/bots/ids), [peer database and access hashes](https://core.telegram.org/api/peers), [update sequence/recovery rules](https://core.telegram.org/api/updates), [common delete constructor](https://core.telegram.org/constructor/updateDeleteMessages), [channel delete constructor](https://core.telegram.org/constructor/updateDeleteChannelMessages), [history method](https://core.telegram.org/method/messages.getHistory)
- Published Grammers [auth.rs](https://docs.rs/grammers-client/0.10.0/src/grammers_client/client/auth.rs.html) and [dialogs.rs](https://docs.rs/grammers-client/0.10.0/src/grammers_client/client/dialogs.rs.html); local source inspected at `/home/swind/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/grammers-client-0.10.0/src/` (no `impl Drop for Client` found).

## Update-gap reconciliation (supersedes "unrecoverable" for differenceTooLong)

Live finding: after ~5 h offline the account's difference was too old, the collector exited, and every restart failed identically. Treating it as fatal made the collector permanently dead, so it is now recovered automatically:

- The vendored Grammers raises a synthetic error before advancing its state (account-wide `ARCHIVE_DIFFERENCE_TOO_LONG`; channel `ARCHIVE_CHANNEL_DIFFERENCE_TOO_LONG_<bare id>`). The adapter resets the persisted `FileSession` update state: account-wide = `updates.getState` (pts/qts/date/seq) with all channel states dropped; channel-scoped = only that channel's entry dropped. Grammers re-initializes a channel without state from its next update (as upstream does for new channels, and as upstream adopts the new pts on TooLong). The sender pool is rebuilt and the stream restarted from the reset state.
- The supervisor then runs message-level catch-up (`catchup_after_id`, fixed upper bound, per-page commit): all tracked chats for account-wide gaps, only that chat for a tracked channel, nothing for an untracked channel. Collector status is `degraded` while re-syncing, then `running` with a detail note and a reconciliation count; a WARN with the scope (no message text) is logged.
- Loop guard: more than 3 resets in 10 minutes back off using the reconnect backoff and keep status `degraded`.

Limitations: only messages newer than the per-chat catch-up boundary are recovered. Edits and deletions of older messages, and anything in untracked chats, during the gap are lost; chats without a catch-up baseline get one now rather than a backfill. Dropping all channel states means live updates of channels resume from the next received update. Not yet verified against a real too-long response.

### Live-update readiness after a gap or long downtime

Grammers reads socket updates only when no account/channel difference is pending (`UpdateStream::next_raw` fetches differences first). After hours offline, tens of busy channels are drained at 100 messages per request before any live update is read, so "running" used to be reported minutes too early (observed: live messages seemed lost for 60 s after a channel gap reset; they were queued, not lost). The vendored stream now exposes `live_flag()`; the collector shows `catching_up` ("working through the Telegram update backlog") until it flips, then `running`. Also, a torn-down sender pool is rebuilt immediately so catch-up between sessions never uses a stopped client. Persisted state is only advanced by `sync_update_state` after a batch is archived, so updates queued in a discarded pool are re-fetched by getDifference from the persisted pts.

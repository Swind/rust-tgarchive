# Migrations

位於 `migrations/`，由 `tgarchive db init`（及任何寫入型程序）依序套用。只會新增、不刪除資料。

| 檔案 | 內容 |
|---|---|
| `0001_archive.sql` | 基礎 schema：`chats`、`senders`、`messages`（含版本／刪除標記）、FTS5 全文索引（external-content unicode61，已被 0006 取代）、同步 job 與 checkpoint 相關資料表。 |
| `0002_telegram_account_deletions.sql` | `telegram_account_identity`（封存綁定單一 Telegram 帳號）與 `common_message_tombstones`（無 chat 資訊的刪除墓碑）。 |
| `0003_chat_tracking.sql` | `chats.tracked`（預設 0）、`chats.tracked_at`、partial index `chats_tracked`。既有聊天室一律維持 untracked，不刪資料。 |
| `0004_sync_rate_limited_state.sql` | 同步 job 新增可續跑狀態 `rate_limited`。SQLite 無法修改 CHECK，故先子表後父表重建 `sync_jobs`／`sync_job_chats`（資料原樣複製）並重建 `sync_jobs_active_chat` index。 |
| `0005_read_indexes.sql` | 讀取用 index（不改資料）：`messages_chat_stats(chat_id, is_deleted, timestamp)` 供聊天室統計聚合、`messages_chat_all_order(chat_id, timestamp DESC, message_id DESC)` 供 `include_deleted` 的單一聊天室排序、`messages_chat_sender(chat_id, sender_id) WHERE is_deleted=0` 供寄件人統計。跨聊天室且 `include_deleted=true` 的全域列表沒有專屬 index（使用較少）。 |
| `0006_search_index.sql` | 中文友善全文搜尋：刪除舊的 `messages_fts` 與三個 trigger（`messages_fts_insert/delete/update`），建立 contentless-delete FTS5 表 `messages_fts(words, bigrams)`（`content=''`、`contentless_delete=1`、`tokenize='unicode61'`，rowid = `messages.row_id`）與 `app_metadata(key, value)`（`search_index_state`、`search_index_version`）。沒有可索引文字的資料庫直接標為 `ready`／版本 1；否則標為 `stale`／版本 0。**只動結構**：斷詞需要 Rust，回填由 `db init` 或 `search rebuild-index` 完成。 |
| `0007_sender_metadata.sql` | 寄件人歸屬：`messages` 新增可為 NULL 的 `post_author`、`fwd_from_id`、`fwd_from_name`、`fwd_date`；`chat_sync_state` 新增 `refetch_active`、`refetch_before_id`（`--refetch` 續跑 checkpoint）；index：`messages_sender_stats(sender_id,is_deleted,chat_id,timestamp)`、`messages_sender_all_order`（含已刪除的寄件人列表）、`messages_post_author`、`messages_null_sender`（repair）。不改資料、FTS 不變。 |
| `0008_sender_bot_and_name_history.sql` | `senders.is_bot INTEGER CHECK (is_bot IN (0,1))`（NULL = 未知／非使用者）；新增 `sender_name_history(id, sender_id → senders(id), display_name, username, first_seen_at, last_seen_at)` 與 index `sender_name_history_sender(sender_id, first_seen_at)`。回填：每個有名稱或 username 的既有 sender 一列目前值（`first_seen_at = senders.created_at`、`last_seen_at = updated_at`；名稱與 username 皆 NULL 者不建立列）。`is_bot` 既有列為 NULL，待之後 sync／即時事件觀察到才填入。 |

## 升級指引

- 升級程式後，**先執行 `tgarchive db init`**，再使用讀取型命令（`chats list`、`messages ...`、`status`）。讀取型命令不會自動套用 migration；舊資料庫（0003 之前）缺少 `tracked` 欄位會出錯或無法查詢。
- **0006（全文索引改版）**：套用後既有訊息尚未索引，狀態為 `stale`。`tgarchive db init` 會在 migration 之後自動重建索引（分批交易，進度顯示於 stderr，約每秒數萬則；100k 則訊息在 release 版約 3 秒）。也可手動 `tgarchive search rebuild-index`。重建期間及之前搜尋退回 `LIKE` 掃描（結果正確但較慢、依時間排序），`/api/v1/status` 的 `search_index.state` 會顯示 `stale`／`rebuilding`。`serve`／`serve --query-only` 不阻塞啟動、不自動重建；唯讀開啟尚未 migrate 的舊資料庫同樣回報 `stale` 並用 `LIKE` 搜尋。索引是可拋棄的衍生資料；更換 jieba 版本或規則時遞增 `SEARCH_INDEX_VERSION`，狀態自動變為 `stale`，重新執行重建即可。
- 升級前請以 `sqlite3 telegram.db ".backup 'backup.db'"` 備份。
- 套用 0005 只建立 index（大型資料庫可能需要數秒）；不需手動動作，缺少 0005 的資料庫仍可查詢（較慢）。
- 套用 0004 不需任何手動動作，既有 job 紀錄保留。
- 套用 0003 後所有既有聊天室為 untracked：需要繼續收集的請重新 `chats track <id>`。
- Migration 不可就地修改已發佈的檔案；變更請新增檔案。
- 套用 0007 後既有列的新欄位皆為 NULL；`sender_id` 為 NULL 的舊列請執行 `tgarchive repair senders`（頻道），私訊請 `sync chat <ID> --refetch`。尚未 migrate 的舊資料庫以唯讀開啟時仍可查詢（新欄位視為 NULL）。
- 0008：Bot 旗標與曾用名稱。`is_bot` 由 Telegram `User.bot` 設定（只有使用者；群組／頻道維持 NULL），upsert 時非 NULL 會覆蓋、NULL 不會清除。曾用名稱語意：合併（COALESCE）後的 `(display_name, username)` 與已存值任一欄位不同時，新增一列新值（`first_seen_at = last_seen_at = 觀察時間`）；相同則只把最新一列的 `last_seen_at` 往前推（`MAX`，不倒退）；NULL 欄位不算變更、兩欄皆 NULL 不建立列；與 sender upsert 同一交易。觀察時間 = 該批次訊息 `collected_at` 的最大值（沒有訊息時為現在），所以時間代表「tgarchive 觀察到的時間」，不是對方實際改名時間；歷史同步取得的是「現在」的使用者資料，不是訊息當時的名稱。已寫入的列不重排、不改寫（亂序觀察不會產生來回翻轉的列）。唯讀開啟尚未 migrate 的舊庫時 `is_bot` 視為 NULL、曾用名稱為空。需要 `tgarchive db init`（或任何寫入型程序）套用；既有 sender 的 `is_bot` 為 NULL，待之後同步／即時事件觀察到才填入。

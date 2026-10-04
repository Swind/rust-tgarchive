# Migrations

位於 `migrations/`，由 `tgarchive db init`（及任何寫入型程序）依序套用。只會新增、不刪除資料。

| 檔案 | 內容 |
|---|---|
| `0001_archive.sql` | 基礎 schema：`chats`、`senders`、`messages`（含版本／刪除標記）、FTS5 全文索引（unicode61）、同步 job 與 checkpoint 相關資料表。 |
| `0002_telegram_account_deletions.sql` | `telegram_account_identity`（封存綁定單一 Telegram 帳號）與 `common_message_tombstones`（無 chat 資訊的刪除墓碑）。 |
| `0003_chat_tracking.sql` | `chats.tracked`（預設 0）、`chats.tracked_at`、partial index `chats_tracked`。既有聊天室一律維持 untracked，不刪資料。 |
| `0004_sync_rate_limited_state.sql` | 同步 job 新增可續跑狀態 `rate_limited`。SQLite 無法修改 CHECK，故先子表後父表重建 `sync_jobs`／`sync_job_chats`（資料原樣複製）並重建 `sync_jobs_active_chat` index。 |

## 升級指引

- 升級程式後，**先執行 `tgarchive db init`**，再使用讀取型命令（`chats list`、`messages ...`、`status`）。讀取型命令不會自動套用 migration；舊資料庫（0003 之前）缺少 `tracked` 欄位會出錯或無法查詢。
- 升級前請以 `sqlite3 telegram.db ".backup 'backup.db'"` 備份。
- 套用 0004 不需任何手動動作，既有 job 紀錄保留。
- 套用 0003 後所有既有聊天室為 untracked：需要繼續收集的請重新 `chats track <id>`。
- Migration 不可就地修改已發佈的檔案；變更請新增檔案。

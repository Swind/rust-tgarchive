# Telegram 圖片自動下載設計

本文件為待實作設計；目前版本尚未提供圖片檔案下載。

## 目前能力與限制

- `Attachment`／`attachments` 表保存 kind、Telegram media ID、MIME、名稱、大小；沒有本地檔案與下載狀態。
- mapper 保存的 `telegram_file_id` 是 photo/document 的數字 ID，不是可直接交給 Bot API 下載的 file ID。
- 現有 Grammers 提供 `get_messages_by_id`、`iter_download`。下載沿用 `TelegramAdapter` 當前 client 與帳號 owner，不另開 Telegram session。
- 附件更新使用 delete + insert；下載工作不能以現有 `attachments.id` 作為長期唯一識別。
- Telegram file reference 會過期；依 `(chat_id, message_id)` 重取來源訊息可取得新 reference。來源不存在／失去存取權限時，未下载的檔案可能無法取得。

來源：本專案 domain、mapper、SQLite writer、vendored Grammers files/messages 實作；[Telegram file references](https://core.telegram.org/api/file-references)。

## V1 產品行為

- 全域設定 `MEDIA_DOWNLOAD_ENABLED=true` 才啟用；只處理 tracked chats。
- 下載 Telegram Photo 的最大可用尺寸，以及以 document 傳送的 JPEG／PNG／WebP 圖片。Photo 的最大尺寸不等同於上傳前的原始檔。
- 啟用後，新訊息、編輯、catch-up 與歷史同步寫入的圖片自動排入工作。
- 已存檔圖片透過「補下載圖片」明確排入工作，不在升級啟動時直接掃描整個歷史資料庫。
- 預設 1 個下載 worker，每檔上限 20 MiB；上限以實際串流位元組數強制執行，未知大小亦適用。
- 檔案保存於 `/data/media`，沿用現有 compose 的 `/data` bind mount。
- Web UI 顯示等待、下載中、可預覽、失敗／不可取得；本地成功檔案在 query-only 模式仍可瀏覽。
- untrack 停止新的自動下載，未執行工作停止；重新 track 可透過補下載重新排程。已下載檔案保留；已刪除訊息的圖片不透過一般圖片路由公開。自動刪檔政策另行決定。

## 工作流程

```mermaid
flowchart LR
    A[Realtime / History / Catch-up] --> B[Archive writer transaction]
    B --> C[保存訊息與附件]
    B --> D[建立持久化圖片工作]
    D --> E[下載 worker]
    E --> F[重取來源訊息與最新 media reference]
    F --> G[分塊寫入 .part]
    G --> H[驗證大小與檔案格式]
    H --> I[sync / atomic rename]
    I --> J[更新成功狀態]
    J --> K[REST 圖片串流 / Web UI]
```

writer 在訊息與附件交易中建立工作，commit 後即完成訊息 ACK；不等待檔案下載。下載 worker 從 DB 拉取少量工作，記憶體通知僅用來喚醒，不能作為唯一工作來源。

僅針對 writer 實際接受的最新附件建立工作；被版本規則拒絕的舊 history event 不建立舊圖片工作。Refetch 補足附件 metadata 時也走同一排程規則。

## 持久化資料

新增 `media_downloads`，保留現有 attachments 表的 metadata 語意：

| 欄位 | 用途 |
|---|---|
| id | 穩定工作 ID，亦用於本地檔名與 API |
| chat_id、message_id、ordinal | 原始訊息與附件位置 |
| media_kind、telegram_media_id、variant | media 身分及尺寸策略 |
| state | queued / running / succeeded / failed / unavailable / superseded |
| relative_path、content_type、byte_size | 已驗證的本地檔案資料 |
| attempts、next_attempt_at、last_error | 有限重試、限流等待與已清理的錯誤 |
| created_at、updated_at | 狀態時間 |

唯一鍵：`(chat_id, message_id, ordinal, media_kind, telegram_media_id, variant)`。重複 ingest 不重複建立工作；圖片替換建立新工作，舊工作標為 superseded。同一圖片出現在不同訊息時，V1 允許分別存檔。

成功記錄不依賴會被重新建立的 attachment row ID。對外查詢必須比對當前附件的 media 身分，避免舊圖片被當作新附件顯示。

## 下載、失敗與恢復

1. worker claim queued 工作後，再確認 tracked、訊息未刪除、附件身分仍相符。
2. 以原 chat/message 查詢 Telegram 訊息，核對 media ID；若已換圖，舊工作標為 superseded，不能下載替換後的圖冒充舊圖。
3. 使用 `iter_download` 分塊寫入由程式產生名稱的私人 `.part` 檔。磁碟不足、超過大小限制立即停止並清理 partial file。
4. 核對 JPEG／PNG／WebP signature 與檔案大小後同步檔案、atomic rename、同步父目錄，再將 DB 標記 succeeded。
5. 網路錯誤有限退避重試；FloodWait 保存下一次可執行時間，等待可取消，且不持有 DB transaction。file reference 過期時重新取得來源訊息後有限重試。
6. 訊息／圖片不存在、無存取權限或格式不支援，保存可辨識狀態；不阻塞訊息同步。
7. 啟動時回收 running 工作。若 rename 後、DB 成功更新前 crash，驗證最終檔案並補記成功；partial file 在下次嘗試重新下載。已標記成功但檔案遺失，轉為可重試狀態。
8. 完成前再核對附件身分與收集政策，防止下载期間的 edit/delete/untrack 造成過期圖片公開。

DB 與檔案系統沒有共同 transaction，因此恢復流程需覆蓋上述兩邊的 crash window。圖片下載失敗只影響圖片狀態；共享 Telegram 連線或帳號的致命錯誤交由既有 supervisor 處理。

下載流量使用帳號級限流等待與既有 pacing 協調，優先讓訊息收集持續運作。下載 worker 納入 runtime ownership、取消與 join；取消後 partial 檔可清理，工作在下次啟動恢復。

## CLI、REST 與 Web UI

建議介面：

- `tgarchive media enqueue --chat <ID>`：為該 tracked chat 已存檔、未刪除的圖片補建立工作；冪等，不直接在 CLI 開新 Telegram owner。允許離線排程，由下次啟用下載功能的 `serve` 執行。
- `tgarchive media status`：查詢工作計數與失敗原因。
- `POST /api/v1/chats/{chat_id}/media/downloads`：補建立圖片工作，回傳新增／既有工作數；目前 `serve` 已啟用下載 worker 才接受，否則明示下載功能未啟用。
- `GET /api/v1/media/status`：queued/running/succeeded/failed 等計數。
- `POST /api/v1/media/{id}/retry`：重新排入可重試工作，仍核對 tracked 與來源附件。
- `GET /api/v1/media/{id}/content`：只有已完成且目前可見的圖片才串流回應；回傳正確 MIME 與 `nosniff`。不接受使用者提供磁碟 path，也不把 `/data/media` 整個目錄直接掛成靜態網站。
- Message DTO 的附件新增可選 media ID、下載狀態、成功圖片 URL；query-only 不呼叫 Telegram。
- Web UI 使用本地 URL、lazy loading，提供圖片預覽、查看原尺寸、失敗重試與每 chat 補下載入口。

## Phases 與驗收 tasks

| Phase | Tasks | 驗收 |
|---|---|---|
| D1 資料與排程 | migration／設定；accepted attachment transaction 建立工作；補下載與 claim/recovery | 重複 ingest 不重複排程；rollback 不留下工作；edit/refetch 正確；untrack/delete 政策 |
| D2 下載 adapter | 重取 media reference；分塊下載與上限；型別驗證／原子檔案與重試 | fake chunks、超限／磁碟失敗；來源換圖／刪除；過期 reference／FloodWait |
| D3 Runtime | supervised worker；帳號 pacing／取消；檔案與 DB crash reconciliation | enqueue 前後、rename 前後、DB 成功前的重啟；worker join；query-only 不連 Telegram |
| D4 使用介面 | CLI／REST／OpenAPI；附件下載狀態；Web UI 預覽與補下載 | 路徑與可見性檢查；正在下載不顯示 partial；圖片替換不顯示舊檔；瀏覽器操作 |
| D5 交付 | 設定範例／資料備份文件；自動測試；真實 Photo 與 image document 驗收 | 新訊息／歷史同步／補下載／重啟；compose volume 內檔案存在；發布新 image |

每個 phase 或已完成 task 群組驗收後 commit。真實 Telegram 下載仍需實際帳號測試；fake tests 不代表已驗證所有 Telegram 圖片類型與權限情況。

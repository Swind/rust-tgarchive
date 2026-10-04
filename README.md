# tgarchive

> Unofficial archive tool using the Telegram API; not affiliated with Telegram.

> 先前的執行檔名稱為 `telegram-archive`，現已更名為 `tgarchive`。舊環境變數 `TELEGRAM_ARCHIVE_NO_DOTENV` 仍被接受（已棄用），請改用 `TGARCHIVE_NO_DOTENV`。

以 Rust 撰寫的 Telegram 訊息封存工具。使用你自己的 Telegram **使用者帳號**（透過 grammers，不是 bot）登入，把你**明確選擇**的聊天室訊息存入本機 SQLite，並提供 CLI 與僅限本機的 REST API 查詢、全文搜尋。

預設**不收集任何聊天室**（包含私人對話）。必須先用 `chats track` 明確選擇。

## 前置需求

- Rust toolchain（版本見 `rust-toolchain.toml`，rustup 會自動安裝）
- 選用：`sqlite3` CLI（用於備份與檢視資料庫）
- Telegram API credentials：到 <https://my.telegram.org> 以你的帳號登入 →「API development tools」建立應用程式，取得 `api_id` 與 `api_hash`。

## 設定

在專案根目錄建立 `.env`（已被 git 忽略，切勿提交）：

```sh
TELEGRAM_API_ID=123456
TELEGRAM_API_HASH=your_api_hash
TELEGRAM_PHONE=+886912345678      # 僅 scripts/login.sh 使用；可省略，改為互動輸入
TELEGRAM_SESSION_FILE=./telegram.session
DATABASE_URL=sqlite://telegram.db
SERVER_BIND=127.0.0.1:8080
RUST_LOG=info                      # 未設定時預設為 warn
SYNC_PAGE_DELAY_MS=1000            # 選用；history 請求最小間隔，見「抓取頻率與限流」
SYNC_MAX_FLOOD_WAIT_SECS=300       # 選用；願意等待的 FLOOD_WAIT 上限
TGARCHIVE_DEV_CORS_ORIGIN=http://127.0.0.1:5173  # 選用；僅供本機前端開發，見「REST 與 OpenAPI」
```

**執行檔啟動時會自動載入目前工作目錄的 `.env`**（在解析設定之前）：

- 已存在的行程環境變數**優先**，`.env` 不會覆蓋它們。
- 預設的 `.env` 不存在時靜默略過；以 `--env-file <PATH>` 指定檔案時，檔案必須存在，否則報錯。`--env-file` 為全域旗標，並取代 `./.env`。
- 設定 `TGARCHIVE_NO_DOTENV=1` 可停用自動載入 `./.env`（不影響明確指定的 `--env-file`）。
- 格式為單純的 `KEY=VALUE`（支援註解與引號，不做 shell 展開）；錯誤訊息不會印出值。

下列範例以 `tgarchive` 代表 `cargo run -q --` 或 `target/debug/tgarchive`。

## 快速開始

```sh
# 1. 登入（會執行 db init 與 auth login；會互動詢問登入碼／2FA 密碼）
scripts/login.sh

# 2. 初始化資料庫（login.sh 已做；可重複執行）
tgarchive db init

# 3. 抓取聊天室清單（只寫 metadata，不收集訊息、不改 tracking）
tgarchive chats refresh
tgarchive chats list

# 4. 選擇要收集的聊天室（chat id 取自 chats list）
tgarchive chats track -1001234567890
tgarchive chats list --tracked

# 5. 回補歷史訊息
tgarchive sync chat -1001234567890
tgarchive sync all            # 僅同步已 track 的聊天室；沒有時為成功的 no-op

# 6. 啟動伺服器（即時收集 + REST）
tgarchive serve
```

重點：

- `chats track` 預設**不會**抓取歷史，只決定之後收集哪些聊天室；歷史需手動 `sync chat`／`sync all`，或加 `--backfill`（見「抓取頻率與限流」）。
- `chats untrack` 停止收集，但保留已存訊息。
- 歷史已完成的聊天室再執行 `sync chat` 會改為**向前 catch-up**：抓取比 `catchup_after_id`（或已封存的最大訊息 ID）更新的訊息（上界於開始時固定、同樣受 pacing 限制），可在不啟動 `serve` 的情況下補上缺口。歷史回補第一頁會同時記錄 `catchup_after_id`，因此之後 `serve` 的 baseline 不會跳過歷史同步後才出現的訊息；已有封存訊息的聊天室，baseline 取已封存的最大 ID，只有完全沒有資料的新聊天室才以 Telegram 目前最新 ID 作 baseline。
- 對未 track 的聊天室執行 `sync chat` 會被拒絕（REST 回 409 `chat_not_tracked`）。
- `serve` 執行期間 track／untrack 不需重啟。

## 查詢

```sh
tgarchive messages list --chat-id -1001234567890 --limit 20
tgarchive messages get -1001234567890 42
tgarchive messages search "keyword" --chat-id -1001234567890 --from 2024-01-01T00:00:00Z --to 2024-02-01T00:00:00Z
tgarchive --output json messages list --limit 5
```

- `--include-deleted`（`messages list`／`get`／`search`）連同已標記刪除的訊息一起顯示（人類輸出以 `[deleted]` 標記；JSON 含 `deleted_at`）；預設隱藏。人類輸出在時間後多一欄寄件人（顯示名稱，否則 username／ID）。
- `--from`／`--to` 為 RFC 3339 時間。`--before`／`--after` 為上一頁回傳的 `next_cursor`。`--limit` 1–1000，預設 100。
- 讀取型命令只讀資料庫，不需要 Telegram。若是舊資料庫，請先執行 `db init`（見 [docs/migrations.md](docs/migrations.md)）。

### 全文搜尋的中文限制

FTS5 使用 `unicode61` tokenizer：只比對**完整 token**，**不支援**子字串搜尋，也沒有中文斷詞。例如搜尋「資料」不保證找得到「資料庫」。這是已知限制。

## 抓取頻率與限流

Telegram **沒有公開的固定頻率上限**，限制是動態的（依帳號、方法、行為而變）；伺服器會以 `FLOOD_WAIT_X`（420）要求等待 X 秒。本工具因此採保守策略：

- **請求間隔（pacing）**：所有 `getHistory` 請求（歷史回補、重連 catch-up、新 track 聊天室的 baseline probe）都經過**同一個行程內共用的 pacer**，任兩次請求間至少間隔 `SYNC_PAGE_DELAY_MS`（預設 `1000`，整數 `0..=60000`；`0` 表示關閉 pacing 與自適應降速；無效值會讓 `serve`／`sync`／`chats track --backfill` 於啟動時直接失敗並說明）。每頁維持 100 則（API 上限），以免請求數更多。
- **序列化**：歷史 job 由單一 worker 依序執行，同一時間只會有一個歷史 job 在抓；其餘排隊（佇列滿回 503、同聊天室重複回 409，語意不變）。即時 catch-up 不經過此佇列，可能在歷史頁與頁之間穿插，但因共用 pacer，整體請求仍保證符合最小間隔。
- **自適應降速**：收到 FLOOD_WAIT 後，pacer 間隔變為 2 倍（上限 10 秒），並讓所有 history 請求（含 catch-up）等滿該秒數；之後每連續 50 次成功請求，間隔減半，直到回到 `SYNC_PAGE_DELAY_MS`。日誌以 WARN 記錄等待秒數與新間隔（不含訊息內容）。
- **FLOOD_WAIT 上限**：`SYNC_MAX_FLOOD_WAIT_SECS`（預設 `300`，`0..=86400`）。Telegram 要求的等待超過上限、或同一頁連續 5 次 FLOOD_WAIT 後仍被限制時，**不再睡等**：
  - 歷史 job 以可續跑的 `rate_limited` 狀態結束，錯誤摘要為 `Telegram rate limit: retry after N s`；已提交的進度與 checkpoint 保留，之後再執行 `sync chat`（或 `POST /api/v1/chats/{id}/sync`）會從 checkpoint 接續。`sync all` 遇到時會停止，不再嘗試其餘聊天室。CLI 以非零結束並印出該摘要。
  - 即時 catch-up 該輪略過該聊天室（WARN + 寫入 `last_error`），下一輪再試，不阻擋即時更新。
- **狀態**：`GET /api/v1/status` 的 `rate_limit` 含 `interval_ms`（目前間隔）、`base_interval_ms`、`last_flood_wait_secs`、`last_flood_at`（僅 Telegram 模式的 `serve` 有；CLI `status` 為獨立程序，看不到 pacer，但會列出 job 狀態含 `RateLimited`）。

### 追蹤並回補：`--backfill`

```sh
tgarchive chats track -1001234567890 --backfill
curl -X PUT "http://127.0.0.1:8080/api/v1/chats/-1001234567890/tracking?backfill=true"
```

- 不加旗標時行為不變（只 track）。
- REST（需 Telegram 模式的 `serve`；否則回 503 `busy` 且不更動 tracking）：track 後將歷史 job 排入 coordinator，回應除 ChatDto 外多 `backfill_job_id` 與 `backfill: "queued"`；若該聊天室（或 `sync all`）已有排隊／執行中的 job，則不重複建立，回 `backfill: "already_running"` 與既有 job id。用 `GET /api/v1/sync/jobs/{id}` 追蹤。
- CLI（無伺服器；需 Telegram 憑證、且不可有另一程序持有 session）：先 track 並提交，再於本程序執行與 `sync chat` 相同的歷史回補。若缺憑證，tracking 仍已儲存，但命令以非零結束並說明之後如何執行 `sync chat`。

### 使用條款提醒

透過 Telegram API 取得的資料**不得用於 AI／機器學習訓練**（見 [Telegram API Terms of Service 1.5](https://core.telegram.org/api/terms)）。請同時遵守其他條款，並自行承擔大量抓取可能導致帳號被限制的風險；上述預設值是保守設定，不保證不會被限流。

## serve 與狀態

```sh
tgarchive serve                  # Telegram 模式：即時收集 + REST
tgarchive serve --query-only     # 只提供已存資料的查詢，不連 Telegram
tgarchive serve --bind 127.0.0.1:9000
```

- 若未設定 `TELEGRAM_API_ID`／`TELEGRAM_API_HASH`（例如 `.env` 不在目前工作目錄），`serve` 會退回只查詢模式：啟動時於 stderr 印出警告，**即時收集不會執行**，`/api/v1/status` 的 `collector.detail` 會註明 Telegram 未設定。明確指定 `--query-only` 則不印警告，detail 註明為 `--query-only`。
- 只能綁定 loopback 位址；**API 沒有身分驗證**。遠端存取請自行用 SSH tunnel 或有驗證的反向代理。
- Collector 狀態請查 `GET /api/v1/status`：`starting → catching_up → running → reconnecting → stopped／failed`（`--query-only` 為 `disabled`）。該回應也含 `unresolved_deletions`（無法對應聊天室的刪除數）。`/health/live` 為存活檢查；`/health/ready` 在資料庫不可用或 collector `failed` 時回 503。
- `tgarchive status` 是獨立程序，只讀資料庫，**看不到**執行中伺服器的 collector，會顯示 `disabled`；請用 REST。

```sh
curl -s http://127.0.0.1:8080/api/v1/status
curl -s "http://127.0.0.1:8080/api/v1/messages/search?q=keyword"
```

## Web UI

`tgarchive serve`（含 `--query-only`）會在 `/` 提供內嵌的 Web UI（React + Vite + TypeScript），API 仍在 `/api/v1`、`/health/*`、`/openapi.*`。啟動後開啟 <http://127.0.0.1:8080/>：對話（時間軸、無限捲動、脈絡檢視、開始／停止收集）、搜尋、同步、狀態。UI 與 API 同源，不使用任何外部 CDN／字型；UI 回應帶 `X-Content-Type-Options: nosniff`、`Referrer-Policy: no-referrer` 與僅允許 self 的 CSP。仍只綁定 loopback 且**沒有身分驗證**。

- 建置產物 `web/dist` **已提交**並由 `build.rs` 以 `include_bytes!` 嵌入執行檔，所以 `cargo build` 不需要 Node。
- 前端開發：先 `tgarchive serve`，再 `npm --prefix web ci && npm --prefix web run dev`，開 <http://127.0.0.1:5173/>；Vite 會把 `/api`、`/health`、`/openapi.*` proxy 到 `127.0.0.1:8080`，不需要 CORS（也可改用 `TGARCHIVE_DEV_CORS_ORIGIN`）。
- 重新建置並提交：`npm --prefix web run build`（含 typecheck）。`openapi.yml` 變更後執行 `npm --prefix web run gen:api` 重新產生 `web/src/api/schema.d.ts`。
- 檢查：`npm --prefix web run typecheck`、`lint`、`test`；`scripts/check.sh` 在有 `npm` 時會執行並確認 `web/dist` 與原始碼一致。
- 中文全文檢索限制：FTS5 `unicode61` 只能比對完整詞元，連續中文不會斷詞。

## REST 與 OpenAPI

路由（皆在 `/api/v1`）：`chats`、`chats/refresh`、`chats/{id}`、`chats/{id}/tracking`（PUT 可加 `?backfill=true`／DELETE）、`chats/{id}/messages`、`chats/{id}/messages/{message_id}`、`chats/{id}/messages/{message_id}/context`、`chats/{id}/senders`、`chats/{id}/sync`、`messages`、`messages/search`、`sync`、`sync/jobs/{id}`、`sync/status`、`status`；另有 `/health/live`、`/health/ready`、`/openapi.json`、`/openapi.yml`。請求逾時 30 秒，query 與 body 有大小上限，每個回應帶 `x-request-id`。

供 Web UI 使用的讀取能力：

- **寄件人**：`MessageDto.sender` 為 `{ id, display_name, username }`（無寄件人時為 `null`；頻道以自身名義發文時 `display_name` 取聊天室標題），由查詢 JOIN `senders` 取得，無額外 N+1 查詢。`GET /chats/{id}/senders?limit=`（預設 100、最大 1000）回傳 `[{ id, display_name, username, message_count }]`，依 `message_count` 由大到小，不含已刪除訊息。
- **已刪除訊息**：`messages`、`chats/{id}/messages`、`messages/search`、`chats/{id}/messages/{message_id}`、`.../context` 皆支援 `include_deleted=true`（預設 `false`，行為不變）。`MessageDto` 一律含 `is_deleted` 與 `deleted_at`。只有墓碑（沒有保存訊息本文）的刪除不會出現。
- **聊天室統計**：`ChatDto.stats`（`GET /chats`、`GET /chats/{id}`、tracking 回應；`POST /chats/refresh` 省略）含 `message_count`（未刪除）、`deleted_count`、`first_message_at`、`last_message_at`、`history_complete`、`last_sync_completed_at`、`last_error`（已清理，可為 `null`）。`GET /chats?sort=title|last_message|message_count`（預設依 chat ID）。
- **訊息脈絡**：`GET /chats/{id}/messages/{message_id}/context?before=20&after=20`（各 0–100，超過回 400 `invalid_context_size`）回傳 `{ anchor, before, after, has_more_before, has_more_after, before_cursor, after_cursor }`，`before`／`after` 皆由舊到新排序；`before_cursor` 可直接作為 list 端點的 `before`、`after_cursor` 作為 `after` 繼續捲動。錨點不存在（或已刪除且未帶 `include_deleted`）回 404。
- **CORS（僅本機前端開發）**：預設關閉。設定 `TGARCHIVE_DEV_CORS_ORIGIN=http://127.0.0.1:5173` 後 `serve` 只對該確切 origin 開啟 CORS（GET/POST/PUT/DELETE，標頭 `content-type`、`x-request-id`）。值必須是 loopback 的 `http://` origin（`127.0.0.1`／`localhost`／`[::1]`，可含 port，不可有路徑），否則 `serve` 啟動失敗。API 本身仍無驗證，僅綁定 loopback。

規格檔為根目錄的 [`openapi.yml`](openapi.yml)，由程式產生，請勿手改：

```sh
cargo run -q -- openapi --format yaml > openapi.yml
```

測試會比對提交的檔案與產生結果，不一致即失敗。

## 同一 session 只能有一個程序

session 檔有 owner lock（`<session>.lock`）。同時執行 `serve`、`auth login`、`chats refresh`、`sync ...` 等需要 Telegram 的程序，後者會因衝突而失敗。請先停止 `serve`，或改用 REST 觸發（`POST /api/v1/chats/refresh`、`/api/v1/sync`）。

## 真實帳號驗收測試

`tests/live_telegram.rs` 以真實帳號自動驗收即時收集、離線缺口補回、重啟冪等與未 track 隔離（取代手機人工操作）。預設 `#[ignore]` 且缺環境變數時直接跳過。

使用**同一帳號的兩個 session**：archive session（`TELEGRAM_SESSION_FILE`，由測試啟動的 `serve` 使用）與獨立的 driver session（`TELEGRAM_DRIVER_SESSION_FILE`，只負責送出／編輯／刪除測試訊息，絕不使用 archive session，以免推進其 update 狀態而使缺口測試失真）。

一次性登入 driver（互動輸入驗證碼）：

```sh
scripts/login-driver.sh   # 寫入 TELEGRAM_DRIVER_SESSION_FILE（預設 ./telegram-driver.session）
```

必要環境變數：`TELEGRAM_API_ID`、`TELEGRAM_API_HASH`、`TELEGRAM_SESSION_FILE`、`TELEGRAM_DRIVER_SESSION_FILE`、`LIVE_TEST_CHAT_ID`（bot-API 風格 id，如 `-4893203104`），並設 `LIVE_TELEGRAM=1`。若測試聊天室是 supergroup／channel 另需 `LIVE_TEST_ALLOW_CHANNEL=1`。

```sh
set -a; . ./.env; set +a; LIVE_TELEGRAM=1 LIVE_TEST_CHAT_ID=-4893203104 cargo test --test live_telegram -- --ignored --nocapture --test-threads=1
```

注意：

- **會在 `LIVE_TEST_CHAT_ID` 這個聊天室真的送出、編輯、刪除訊息**（文字皆以 `[archive-live <run_id>]` 開頭，只動本次建立的訊息；結束時盡力刪除剩餘者）。請用專用測試聊天室。
- 測試會 `sync chat` 該聊天室既有歷史到暫存資料庫，歷史很長時會較久。
- 測試使用全新暫存資料庫，不碰你的 `telegram.db`。
- 執行前**不可**有 archive `serve`（或任何使用同一 archive session 的程序）在跑（owner lock）。
- 失敗時會印出 serve 日誌尾段（已遮蔽 api_hash／手機號碼）。

## 備份與安全

資料庫使用 WAL，**不要只用 `cp telegram.db`**（會漏掉 `-wal` 內容）。使用 SQLite 一致性備份：

```sh
sqlite3 telegram.db ".backup 'backup.db'"
# 或
sqlite3 telegram.db "VACUUM INTO 'backup.db'"
```

- session 檔（權限 0600）**等同帳號存取權**：外洩者可登入你的帳號。請妥善備份、加密保存，不要提交 Git。
- `api_hash`、手機號碼、登入碼、密碼不會寫入日誌；日誌（INFO/WARN）也不含訊息內容。
- 設定錯誤（資料庫／session 路徑、非 loopback 綁定）會在啟動時失敗並說明修正方式。

## 復原行為與限制

- 重啟或斷線後會進行 catch-up（分輪、分頁、與 checkpoint 同交易提交），並在行程內自動重連（指數 backoff，上限 60 秒）。
- 採 **at-least-once** 重新處理，以 upsert 保持冪等。**不宣稱 exactly-once 或零遺失。**
- 一般（非 channel）刪除事件沒有聊天室資訊，可能有歧義：僅在能對應到已存檔且已 track 的訊息時才標記，否則計入 `unresolved_deletions`。
- 離線過久（Telegram 回報 update gap 過長）時會自動重設 update 狀態並對 tracked 聊天室 catch-up：狀態短暫顯示 `degraded`，完成後回到 `running` 並附說明與次數。gap 期間較舊訊息的**編輯與刪除可能遺漏**；短時間內重複發生會 backoff。
- 未 track 的聊天室的更新一律忽略。
- 刪除事件若早於訊息存檔到達，不會被記住。
- 真實帳號的即時更新、離線缺口補回、真實斷網重連與基本群組尚待人工驗收，見 [IMPLEMENTATION_STATUS.md](IMPLEMENTATION_STATUS.md)。

## 開發

```sh
scripts/check.sh   # fmt、test、clippy（有 npm 時另含 web typecheck／lint／test／build 與 dist 一致性）
```

### E2E 測試

瀏覽器 E2E（Playwright，`web/e2e/`）不需要 Telegram：`scripts/e2e.sh` 會 `cargo build`、用 `cargo run --example seed_fixture -- <db>` 建立固定時間戳的 fixture 資料庫（7 個聊天室、250+ 則訊息、已刪除／編輯／回覆／附件／系統訊息、各狀態的同步工作與未歸屬刪除），以 `serve --query-only` 在隨機 loopback 埠啟動，再於官方映像 `mcr.microsoft.com/playwright:v<版本>-noble`（版本與 `web/e2e/package.json` 的 `@playwright/test` 相同）內以 `--network host` 執行測試；結束時一律關閉伺服器。需要 docker；沒有 docker 時改用本機已安裝瀏覽器（略過截圖比對），都沒有則印出訊息並略過。

```sh
scripts/e2e.sh                       # 執行全部
scripts/e2e.sh -g search             # 額外參數傳給 playwright test
scripts/e2e.sh --update-snapshots    # 重新產生截圖基準（務必在容器內，字型才一致）後提交 web/e2e/__screenshots__
E2E=1 scripts/check.sh               # 在 check.sh 最後加跑 E2E（需 docker）
```

測試中時間被固定在 2026-03-12、時區 Asia/Taipei，且對外部主機的請求會被中止並使測試失敗。

其他文件：[docs/](docs/)、[migrations 說明](docs/migrations.md)。

# Telegram Message Archive

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
```

**執行檔不會自動載入 `.env`。** 手動載入到目前 shell：

```sh
set -a; . ./.env; set +a
```

下列範例以 `telegram-archive` 代表 `cargo run -q --` 或 `target/debug/telegram-archive`。

## 快速開始

```sh
# 1. 登入（會執行 db init 與 auth login；會互動詢問登入碼／2FA 密碼）
scripts/login.sh

# 2. 初始化資料庫（login.sh 已做；可重複執行）
telegram-archive db init

# 3. 抓取聊天室清單（只寫 metadata，不收集訊息、不改 tracking）
telegram-archive chats refresh
telegram-archive chats list

# 4. 選擇要收集的聊天室（chat id 取自 chats list）
telegram-archive chats track -1001234567890
telegram-archive chats list --tracked

# 5. 回補歷史訊息
telegram-archive sync chat -1001234567890
telegram-archive sync all            # 僅同步已 track 的聊天室；沒有時為成功的 no-op

# 6. 啟動伺服器（即時收集 + REST）
telegram-archive serve
```

重點：

- `chats track` **不會**抓取歷史，只決定之後收集哪些聊天室；歷史需手動 `sync chat`／`sync all`。
- `chats untrack` 停止收集，但保留已存訊息。
- 對未 track 的聊天室執行 `sync chat` 會被拒絕（REST 回 409 `chat_not_tracked`）。
- `serve` 執行期間 track／untrack 不需重啟。

## 查詢

```sh
telegram-archive messages list --chat-id -1001234567890 --limit 20
telegram-archive messages get -1001234567890 42
telegram-archive messages search "keyword" --chat-id -1001234567890 --from 2024-01-01T00:00:00Z --to 2024-02-01T00:00:00Z
telegram-archive --output json messages list --limit 5
```

- `--from`／`--to` 為 RFC 3339 時間。`--before`／`--after` 為上一頁回傳的 `next_cursor`。`--limit` 1–1000，預設 100。
- 讀取型命令只讀資料庫，不需要 Telegram。若是舊資料庫，請先執行 `db init`（見 [docs/migrations.md](docs/migrations.md)）。

### 全文搜尋的中文限制

FTS5 使用 `unicode61` tokenizer：只比對**完整 token**，**不支援**子字串搜尋，也沒有中文斷詞。例如搜尋「資料」不保證找得到「資料庫」。這是已知限制。

## serve 與狀態

```sh
telegram-archive serve                  # Telegram 模式：即時收集 + REST
telegram-archive serve --query-only     # 只提供已存資料的查詢，不連 Telegram
telegram-archive serve --bind 127.0.0.1:9000
```

- 若未設定 `TELEGRAM_API_ID`／`TELEGRAM_API_HASH`（例如忘了載入 `.env`），`serve` 會退回只查詢模式：啟動時於 stderr 印出警告，**即時收集不會執行**，`/api/v1/status` 的 `collector.detail` 會註明 Telegram 未設定。明確指定 `--query-only` 則不印警告，detail 註明為 `--query-only`。
- 只能綁定 loopback 位址；**API 沒有身分驗證**。遠端存取請自行用 SSH tunnel 或有驗證的反向代理。
- Collector 狀態請查 `GET /api/v1/status`：`starting → catching_up → running → reconnecting → stopped／failed`（`--query-only` 為 `disabled`）。該回應也含 `unresolved_deletions`（無法對應聊天室的刪除數）。`/health/live` 為存活檢查；`/health/ready` 在資料庫不可用或 collector `failed` 時回 503。
- `telegram-archive status` 是獨立程序，只讀資料庫，**看不到**執行中伺服器的 collector，會顯示 `disabled`；請用 REST。

```sh
curl -s http://127.0.0.1:8080/api/v1/status
curl -s "http://127.0.0.1:8080/api/v1/messages/search?q=keyword"
```

## REST 與 OpenAPI

路由（皆在 `/api/v1`）：`chats`、`chats/refresh`、`chats/{id}`、`chats/{id}/tracking`（PUT/DELETE）、`chats/{id}/messages`、`chats/{id}/sync`、`messages`、`messages/search`、`sync`、`sync/jobs/{id}`、`sync/status`、`status`；另有 `/health/live`、`/health/ready`、`/openapi.json`、`/openapi.yml`。請求逾時 30 秒，query 與 body 有大小上限，每個回應帶 `x-request-id`。

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
scripts/check.sh   # fmt、test、clippy
```

其他文件：[docs/](docs/)、[migrations 說明](docs/migrations.md)。

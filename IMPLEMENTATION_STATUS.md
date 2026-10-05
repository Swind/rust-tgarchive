# Implementation status

執行規劃：[telegram_message_archive_execution_plan.md](telegram_message_archive_execution_plan.md)。

實作者：GPT-6 Luna subagents。驗收及 commit：主 agent。

## Progress

| Phase | Status | Acceptance |
|---|---|---|
| 0 技術決策 | source review accepted; spike pending | P0-T01/T03 完成；P0-T02 編譯 spike／人工驗證待 Phase 6/8 |
| 1 核心 | accepted | 6 tests 通過、fmt/clippy 通過；核心無 adapter 依賴 |
| 2 SQLite | accepted | migrations、FTS、transactions、pagination、WAL/readers、jobs tests 通過 |
| 3 Application | accepted | 3 fake-port tests（多項成功／失敗案例）通過 |
| 4 CLI | accepted | 3 子程序 tests；隔離快照全 21 tests、fmt/clippy 通過 |
| 5 REST/OpenAPI | accepted | 11 CLI/router/runtime tests 通過；query serve、JSON/YAML OpenAPI、limits/request IDs/loopback policy |
| 6 Telegram | adapter/auth/refresh interfaces accepted | hidden interactive auth、CLI/REST refresh、帳號綁定已串接；完整 media fixtures 持續補充，真實帳號驗收延後 |
| 7 History | engine/coordinator/interfaces accepted | REST 即時 202、重複 409、未知 chat 404、queue full 503 與共用 coordinator tests 通過；真實帳號验收延後 |
| 8 Realtime | code complete; automated tests pass, real-account acceptance pending | supervisor、重連 backoff、catch-up、crash replay fake/SQLite tests 通過（全 79 tests、fmt、all-target clippy）；P8-T06 與真實斷線／crash 驗收待做 |
| 9 Hardening | automated parts done; manual items pending | 設定驗證、readiness/timeouts、敏感日誌 test、openapi.yml 比對 test、README、docs/migrations.md；真實帳號矩陣與 fresh-install 走讀待主 agent／人工，見下方 V1 DoD 表 |

## Commits

- `a0466c9`：原始規劃與執行規劃基準。
- `f60b0ee`：Phase 1 核心，6 tests、fmt、clippy 通過。
- `c70f7c3`：Phase 0 來源審查與架構決策。
- `2fe5276`：Phase 2 SQLite，含 8 integration tests。
- `80655ef`：Phase 3 共用 services，3 fake-port tests。
- `8584f07`：CLI/REST/Telegram 相容依賴。
- `8198b84`：Phase 4 CLI，隔離快照 21 tests、fmt/clippy 通過。
- `76ff399`：Phase 6 adapter foundation，14 tests、clippy 通過；不宣稱即時更新 crash recovery 已完成。
- `93c552a`：Phase 5 REST/OpenAPI，11 integration tests 通過，含真正子程序 HTTP query server。
- `3807609`：Phase 7 history engine/coordinator，bounded queue、commit ack、可取消重試、scope reservations、fatal failure propagation、joined shutdown 通過自動驗收。

主 agent 從 `3807609` 匯出隔離快照，`cargo test --offline --locked --all-targets` 全部 51 tests 通過。all-target clippy 發現 Telegram session test module 後的 helper 排序 lint，已交由實作者修正；不影響程式行為。

Phase 8 deletion persistence：未知／有歧義的 common deletion 保存 tombstone 並計數；只更新唯一 common chat match，不碰 channel namespace；綁定後拒絕不同 Telegram 帳號。5 tests 通過，並修正上述 helper 排序 lint。

Auth/refresh/sync interfaces 與 vendored update-buffer boundary：隔離快照全 60 tests、fmt、all-target clippy 通過。SIGTERM 子程序測試確認 query server 正常 exit 0。即時 listener 與完整 runtime supervisor 尚在下一批工作中。

Phase 8 realtime stream foundation：`realtime.rs` 於 archive 確認整批後才寫入 aggregate update checkpoint；全 66 tests、fmt、all-target clippy 通過。`process_stream` 於 stream 結束時回傳 `Telegram(Dropped)`，接入 supervisor 時須視為重連而非致命錯誤；listener supervisor、catch-up、crash recovery 仍待驗收。

Phase 8 supervisor／catch-up：`serve`（已設定 Telegram）啟動 realtime task，與 history 共用 engine 與 writer；shutdown 先取消 realtime 並 join，再停 coordinator、drain writer。`supervise_realtime` 每輪先做 catch-up，再跑 live session；`Dropped`、I/O、transport、FLOOD_WAIT、5xx 視為可重連（上限 60s 的可取消指數 backoff，不限次數），auth／帳號不符／storage／checkpoint 失敗為致命並使 `serve` 非零結束。Catch-up 每輪對每個已知 chat 先固定上界（當下最新 ID），自 `catchup_after_id` 起升序分頁，每頁與 checkpoint 同一交易提交後才前進，不跳到最新 ID；尚無 baseline 的 chat 以該輪上界為起點。自動 tests（fake gateway/store、paused time、SQLite）涵蓋：Dropped 重連、backoff 期間取消、致命錯誤傳遞、分頁與邊界前進、固定上界、commit 失敗不前進邊界、crash（commit 前／commit 後 checkpoint 前）後重啟重處理且無重複。

已知限制（未以真實帳號驗證）：grammers 的 update receiver 為一次性，`Dropped` 表示 sender pool 已停止。`ReconnectingSource` 在此情況先 teardown 舊連線（`disconnect` 並 join runner，釋放 owner lock），再由 `TelegramAdapter::reconnect` 以同一個 `FileSession`（含 archive 確認後的 update state）重建 `SenderPool`／client／receiver，並以 `catch_up: true` 重新開 stream；gateway／auth 每次呼叫皆讀取當前 client，故 history／coordinator 自動使用重建後的 client，且同一時間只有一個 live client。重建失敗：lock／session 錯誤視為致命，其餘由 stream 錯誤分類（401 等致命）。自動 tests 以 fake connector 涵蓋：Dropped 後重建且先 teardown、暫時性重建失敗重試、致命重建失敗傳遞、重建中取消；adapter test 驗證 reconnect 後 session 狀態保留且 lock 單一持有。**真實網路斷線後的重連與 getDifference 補回尚未以真實帳號驗證（pending）。** 暫時性 I/O／RPC 錯誤則保留同一 `UpdateStream` 重試，同樣尚無真實網路驗證。重建期間其他持有舊 client 的進行中呼叫會得到暫時性錯誤。Catch-up 只涵蓋已存在 chats 表的 chat，且與 grammers 內建 getDifference 為獨立機制，兩者重疊以 upsert 冪等處理；不宣稱 exactly-once 或零遺失。

真實帳號驗收後修正（自動測試證據；**真實驗證待主 agent**）：
1. Collector 狀態：新增共用 `CollectorStatusHandle`，supervisor 寫入 `starting → catching_up → running → reconnecting（含 attempt／下次重試毫秒與已清理的最後錯誤）→ stopped／failed（已清理錯誤）`；`serve` 的 REST `GET /api/v1/status`（`collector.state` 與可選 `detail`，OpenAPI 由 DTO 產生）讀同一 handle，`/health/ready` 在 `failed` 時回 503。`--query-only`／未設定 Telegram 仍為 `disabled`（附說明 detail）。CLI `status` 是獨立程序、只讀 DB，未持久化 heartbeat（計畫未要求，且 stale 狀態比明示更糟），故維持 `disabled` 並以 detail 指向執行中伺服器的 `/api/v1/status`。
2. 靜默 catch-up：最可能根因是**空 chat（probe 無訊息）時 `catch_up_chat` 直接回傳而不寫 baseline，`catchup_after_id` 永遠為 NULL 且無任何 log**。現在空 chat 寫入 baseline `0`（`MessageId::BEFORE_FIRST`，僅用於 catch-up 邊界，之後的新訊息會被補回）；每個被略過的 chat 皆 `warn!(chat_id, reason)`（reason 已清理為單行、≤200 字），失敗寫入 `chat_sync_state.last_error`、下次 checkpoint 提交時清除；非預期錯誤中止本輪前也會 warn。另預設 tracing filter 原為僅 ERROR（未設 `RUST_LOG` 時），改為預設 `warn`。
3. `auth login` 先檢查是否已授權，僅在需要輸入憑證時才要求互動終端機（`login_gate` 單元測試）。

自動測試：supervisor 狀態轉換、REST 狀態路由、空 chat baseline 與後續補回、失敗清理與 `last_error` 持久化／清除、baseline 0 的 SQLite 往返、預設 log filter、login gate。Owner-lock 偶發失敗已修正：`SenderPool::new` 預設 `ConnectionParams` 會 fork `getconf`，子程序短暫繼承 `flock` fd，使釋放後立即重新取得鎖失敗（正式 reconnect 亦可能觸發）。改以明確 `ConnectionParams` 建立 pool，不再產生子程序；session tests 連續 50 次通過。

## Opt-in collection（tracked chats）

預設**不收集任何** chat（channel、supergroup、basic group、private 皆然），只收集明確 track 的 chat。設計見 [AD-7](docs/architecture-decisions.md)。

- Schema：migration `0003_chat_tracking.sql` 新增 `chats.tracked`（預設 0）與 `tracked_at`；既有資料列維持 untracked，不刪除任何資料。
- CLI：`chats track|untrack <CHAT_ID>`（冪等；未知 chat 回錯並提示 `chats refresh`）、`chats list [--tracked]`；`chats list/get` 的 human/json 輸出皆含 tracked 狀態。`chats refresh` 只更新所有 dialog 的 metadata，不改 tracked、不抓訊息。
- REST：`PUT|DELETE /api/v1/chats/{chat_id}/tracking`（200 + ChatDto；未知 chat 404）、`GET /api/v1/chats?tracked=true`、ChatDto 新增 `tracked`；OpenAPI 由 annotations 產生。
- Sync：對 untracked chat 的 `sync chat` / `POST /chats/{id}/sync` 被拒（CLI 非零退出；REST 409 `chat_not_tracked`）。`sync all` 僅同步 tracked chats；無 tracked 時為成功的 no-op。
- Realtime：每個 batch 寫入前依 DB 的 tracked 集合過濾（訊息、編輯、chat/sender upsert、channel 刪除）；update-state checkpoint 仍前進。Common 刪除（無 chat ID）僅在符合已存檔的 tracked 非 channel 訊息時才寫入，否則不產生 tombstone。Catch-up 只走 tracked chats；執行中新 track 的 chat 由 supervisor 每 15 秒補 baseline。Track/untrack 不需重啟 serve。
- Untrack 保留已存訊息。
- 已知限制：common 刪除先於訊息存檔到達時不再被記住；讀取端 CLI 需 DB 已套用 0003（執行 `db init` 或任一寫入指令）；track 後的歷史回補須手動 `sync chat`；進行中的歷史 job 不因 untrack 中斷。
- 因前提「收集所有 chat」而調整的既有測試：`tests/sync.rs`、`tests/realtime_supervisor.rs`、`tests/rest_routes.rs` 的 fake chat 改為 `tracked: true`；`src/infrastructure/telegram/realtime.rs` 既有 stream/crash 測試改帶固定 scope（原更新視為在範圍內）。新測試見 `tests/tracking.rs` 與 realtime 單元測試 `untracked_updates_write_nothing_but_still_advance_the_update_checkpoint`。真實帳號驗證 pending。

## 抓取頻率與限流

- `application/pacer.rs` 的 `RatePacer`（每行程一個，掛在 `SyncEngine`，歷史 job 與 realtime catch-up/baseline 共用）以「預約時槽」方式強制 getHistory 最小間隔（`SYNC_PAGE_DELAY_MS`，預設 1000，0 關閉）；FLOOD_WAIT 時間隔 ×2（上限 10 s）、每 50 次成功減半回基準，並讓所有 history 請求等滿該秒數。`SYNC_MAX_FLOOD_WAIT_SECS`（預設 300）以上、或同頁連續 5 次 FLOOD_WAIT，不再睡等：歷史 job 轉為新狀態 `rate_limited`（可續跑，摘要 `Telegram rate limit: retry after N s`，進度保留），catch-up 略過該 chat（WARN + `last_error`）。
- Coordinator 本來就是單一 worker，歷史 job 已序列化；新增測試鎖定此行為。
- Migration `0004_sync_rate_limited_state.sql`：重建 `sync_jobs`／`sync_job_chats` 以放寬 state CHECK（保留資料）。
- `/api/v1/status` 新增 `rate_limit`；`PUT /chats/{id}/tracking?backfill=true` 與 CLI `chats track --backfill`。
- 測試：`tests/rate_pacing.rs`（paused time；間隔、共用 pacer、0 關閉、序列化、降速、上限暫停與續跑、catch-up 略過）、`src/application/pacer.rs` 與 `src/config.rs` 單元測試（衰減、取消、env 驗證）、`tests/rest_routes.rs`（status、backfill 不重複）、`tests/tracking.rs`（CLI backfill、migration 0004、真 SQLite backfill）。既有測試未修改。限制：CLI 的程序內 backfill 與真實 FLOOD_WAIT 行為未以真實帳號驗證。

## Manual acceptance

使用者指定本輪先完成程式與自動測試，真實帳號驗收稍後進行。真實 Telegram credentials 尚未提供。本專案不將 secrets 放入文件或 Git。登入、dialog、真實同步與更新、重啟恢復等人工驗收在取得環境前皆為 pending；自動 tests 不取代這些驗收。

## Storage semantics

- 部分 chat/sender metadata 的 null 值不會清除已知欄位；目前 refresh 也使用這個合併語意。
- 中文全文檢索：jieba 詞 + 漢字 bigram（見「中文友善全文搜尋」一節）；不做繁簡轉換。

## Phase 9 與 V1 DoD 證據

| DoD 項目 | 證據 | 狀態 |
|---|---|---|
| 1–3 auth/session/chats | `auth login` gate／session／owner lock 單元測試；先前真實帳號 login/dialog 紀錄（主 agent） | 自動測試完成；重啟後 dialog 以最新真實驗證為準 |
| 4–5 history/resume | `tests/sync.rs`、worker／checkpoint 測試 | 自動測試完成；真實帳號大量歷史 pending |
| 6–8 new/edit/delete | fixture 與 crash replay tests（`realtime*`） | **pending 人工**：真實即時新訊息／編輯／刪除、離線缺口補回、真實斷網重連、update 負載測試、基本群組（測試帳號無） |
| 9–12 SQLite/pagination/FTS/chats | `tests/persistence.rs`（真 SQLite、同秒跨 chat、FTS）、`tests/tracking.rs` | 完成 |
| 13–15 REST/CLI/shared logic | `tests/cli_smoke.rs`、`tests/rest_routes.rs`、`tests/application_services.rs` | 完成 |
| 16–18 OpenAPI | `/openapi.json`／`.yml` 路由測試；`committed_openapi_yml_matches_generated_output` 比對根目錄 `openapi.yml`（重新產生：`cargo run -q -- openapi --format yaml > openapi.yml`） | 完成 |
| 19 migrations | 空資料庫與 reopen tests；`docs/migrations.md` | 完成 |
| 20 shutdown | `tests/runtime_serve.rs`（SIGTERM）、supervisor drain tests | 自動測試完成 |
| 21 automated tests | 不需 Telegram credentials 即可執行；測試 session 檔改用 tempdir，不再遺留 /tmp 檔案 | 完成 |
| 22–24 fmt/clippy/test | `cargo fmt --check`、`cargo test --offline --all-targets`、`cargo clippy --offline --all-targets -- -D warnings` | 見最新一次執行結果（提交時由主 agent 重跑） |
| 25 README | `README.md`，命令已對照 `--help` | **fresh-install 走讀 pending（主 agent）** |

P9-T01/T02 自動證據：readiness 在 DB 不可用或 collector `failed` 回 503；status 含 `unresolved_deletions`；30 秒請求逾時、body／query 上限、request id；INFO 日誌不含查詢文字或 api_hash 的 test；`TelegramConfig` Debug 遮蔽 api_hash；錯誤的 DB／session 路徑與非 loopback bind 啟動失敗並附修正提示。

仍 pending（不以 fake tests 取代）：真實即時 new/edit/delete、離線缺口補回、真實斷網重連、update 負載測試、基本群組、fresh-install 走讀。

Update-gap reconciliation：`differenceTooLong`／`channelDifferenceTooLong`（離線過久）不再致命。adapter 重設 `FileSession` update state（account：`updates.getState`＋清除 channel states；channel：移除該 channel），重建 stream，supervisor 對 tracked chats 做 message-level catch-up（untracked channel 不 catch-up），狀態 `degraded`→`running`（附說明與次數）；3 次／10 分鐘後以 backoff 避免空轉。fake 測試涵蓋 account／channel（tracked／untracked）／重複／FileSession 持久化／狀態轉換；真實帳號尚未驗證。限制：gap 期間較舊訊息的編輯與刪除可能遺漏。

## Catch-up gap fix 與 .env 自動載入

- 根因：`sync_chat` 完成歷史後從不寫 `catchup_after_id`，之後 catch-up／`baseline_new_tracked` 以 Telegram 當下最新 ID 作 baseline，歷史同步到 serve 啟動之間的訊息永遠不會被封存；歷史完成後 `sync chat` 立即返回。
- 修正：歷史第一頁在 `catchup_after_id` 為空時於同一交易寫入該頁最大 ID（空聊天室為 0）；baseline 退路使用新增的 `SyncRepository::newest_archived_id`（`MAX(message_id)`），僅完全無資料且無歷史的聊天室才用 Telegram 最新 ID；歷史完成的 `sync chat` 改跑向前 catch-up。store 的 MAX-merge 為單調，不需修改。
- `.env`：`dotenvy` 於設定解析前載入 `./.env`，行程環境優先；`TGARCHIVE_NO_DOTENV=1` 停用；全域 `--env-file <PATH>` 必須存在。所有 CLI 子行程測試設定 `TGARCHIVE_NO_DOTENV=1`。
- `sync all`（無 tracked chats）現在先驗證 `SYNC_PAGE_DELAY_MS`／`SYNC_MAX_FLOOD_WAIT_SECS`。

## Web UI 讀取 API（寄件人、已刪除、統計、脈絡、dev CORS）

- 新增（皆經 application services，CLI 共用）：`MessageDto.sender`／`is_deleted`／`deleted_at`；`include_deleted`（list／chat messages／search／get／context，預設 false，CLI `--include-deleted`）；`GET /chats/{id}/senders`；`ChatDto.stats` 與 `GET /chats?sort=`；`GET /chats/{id}/messages/{mid}/context`；`TGARCHIVE_DEV_CORS_ORIGIN`（loopback http origin，預設關閉，無效值使 `serve` 啟動失敗）。`openapi.yml` 已重新產生。
- Migration `0005_read_indexes.sql`（僅 index，見 docs/migrations.md）。
- 設計：`MessageView`／`ChatSummary` 為 application 層讀取模型；寄件人以 LEFT JOIN 解析、聊天室統計為單一 GROUP BY 子查詢（無逐列查詢）；context 以既有 list 查詢（cursor 相容）組成，預設 trait 實作。
- Tests：`tests/read_api.rs`（sender join／senders、include_deleted、stats 與 sort、context 與 cursor 銜接、CORS 開／關／無效值、0004→0005 升級）、`tests/cli_smoke.rs` 新增 CLI 測試。

## Web UI（內嵌前端）

- `web/`：React 19 + Vite + TypeScript strict、react-router、TanStack Query；型別由 `openapi.yml` 經 `openapi-typescript` 產生（`npm run gen:api`），搭配小型 fetch 封裝。頁面：對話（篩選／排序／kind／收集中、無限捲動時間軸、context 檢視、已刪除、寄件人、日期跳轉、開始／停止收集、同步）、搜尋、同步（輪詢 2 秒）、狀態；頂欄 collector 徽章每 5 秒輪詢；深／淺主題可手動切換並存 localStorage。
- 嵌入：`web/dist` 提交進 repo，`build.rs` 產生 `include_bytes!` 表（無新增 Rust 依賴），`src/interface/rest/web.rs` 作為 router fallback：`/assets/*` 長快取 immutable、`index.html` no-cache、非 `/api`／`/health`／`/openapi` 的無副檔名路徑回 index（SPA）、API 404 仍為 JSON envelope；附 nosniff／no-referrer／CSP 標頭。
- 選擇提交 dist 而非 feature flag：Rust-only 流程零 Node 依賴；`tests/rest_routes.rs::web_ui` 在 dist 缺失時失敗，`scripts/check.sh` 驗證 dist 與原始碼一致。
- 限制：無 Telegram 即時驗證；`--query-only` 為唯讀，追蹤／同步／重試在該模式只會顯示友善錯誤。

## Web UI 修正與 E2E

- 寄件人：Telegram 對私訊（對方發出）與頻道貼文省略 `from_id`，mapper 現在以聊天室本身為寄件人（自己發出的私訊仍為 null）；API 對無 `senders` 列且 sender==chat 者以聊天室標題為顯示名；UI 順序為 display_name → @username → 聊天室標題 → `未知 (id)`。舊資料中 sender 為 NULL 者仍顯示「未知」（無法得知方向）。
- 無文字且無附件的訊息（系統訊息）顯示斜體提示；封存目前不儲存 service action，故僅有提示文字。
- `last_sync_completed_at`／`last_sync_started_at` 先前從未被寫入；現在在 `write_batch` 的 job progress（與最終進度同一交易）寫入：第一次進度寫 started，`Succeeded` 寫 completed。
- `SyncJobDto` 新增 `error_summary`（≤300 字、單行）、`retry_after_secs`（由 rate_limited 摘要解析）；`GET /sync/jobs/{id}` 另含 `chats[{chat_id,title,state,committed_count,error_summary}]`，且改由 application 讀取，query-only 也可用。`failure_summary` 現在經 `sanitize_reason`。UI：行內摘要、可展開每聊天室進度、failed／rate_limited／interrupted 的「重試」。
- 搜尋頁篩選列改為 grid，日期區間為一個不拆行單元。
- E2E：`examples/seed_fixture.rs`（走 store API，固定時間戳；選 example 而非 CLI 子命令，避免正式 binary 帶測試資料碼）、`web/e2e/*.spec.ts`（34 個測試含 3 張截圖）、`scripts/e2e.sh`；見 README「E2E 測試」。

## 中文友善全文搜尋（jieba + bigram + contentless FTS5）

依 `rust-jieba-bigram-sqlite-fts5.md` §65（權威）實作，全數完成：

- Migration `0006_search_index.sql`：移除舊 external-content FTS 與 3 個 trigger，建立 `messages_fts(words, bigrams)`（contentless-delete）與 `app_metadata`；`db init` 自動分批重建、`tgarchive search rebuild-index`／`search status`；`serve` 遇到 stale 索引不阻塞，回報 `stale`／`rebuilding` 並以 `LIKE` 退路搜尋。
- 寫入路徑（`write_batch` 同一交易）：僅在 guarded upsert 實際更新且文字改變時重新索引；NULL／空字串不索引；軟刪除保留索引（`include_deleted` 可搜）。
- 查詢：NFKC＋小寫、jieba 詞 AND、bigram 片語（相鄰）、停用單字功能詞只從詞路徑移除、單一漢字走 `LIKE`、`bm25(messages_fts, 5.0, 1.0)`；使用者文字不直接進入 MATCH。
- API：`sort=relevance|time`（有 `q` 預設 relevance，版本化不透明 offset 游標）、`snippet`、`/status` 的 `search_index`；`openapi.yml`／`web/src/api/schema.d.ts` 已重新產生。UI：排序切換、摘要、索引／單字橫幅、搜尋中文說明、狀態頁索引列、Intl.Segmenter 詞級高亮。
- Tokenizer 在 `src/infrastructure/search/`（`SearchTokenizer` trait、`HybridChineseTokenizer`、全程序共用且延遲建立的單一 `Jieba`）。
- Tests：`src/infrastructure/search/*`（單元）、`tests/search.rs`（評測集、片語相鄰、bigram 備援、單字 LIKE、軟刪除、陳舊 upsert／新即時編輯、交易回滾、contentless／無 trigger、關聯式分頁無重複／缺漏、篩選、snippet、LIKE 退路、0005 升級、唯讀舊庫）、`tests/search_api.rs`（REST 排序／游標／狀態、CLI `db init` 升級與 `search` 子命令）、`web/e2e/search.spec.ts`（中文子字串、連續性、混合語言／全形、單字、排序切換、snippet）。
- 品質評測：`tests/search_quality.rs`（8,000 則產生語料、60+ 查詢 × 兩種排序、獨立 ground truth；recall 100%、精確類 precision 100%、無結果 0、排序 sanity；約 8 s，`--nocapture` 印摘要表）。語料產生器 `tests/support/{corpus,vocab}.rs`（固定種子、Zipf、約 6,400 詞、植入罕見詞／單字／連續 vs 分散／英文複數等 needle，數量精確）與速度基準 example 共用。
- 速度基準：`cargo run --release --example search_bench -- --messages 100000 --messages 1000000 [--db PATH] [--iterations N] [--out FILE] [--strict]`（見 README「搜尋品質與效能」）。舊的 `#[ignore] measure_100k_message_corpus` 已移除（只印數字、沒有門檻、語料不寫實），改由 example 取代，並以 `#[ignore]` 的 `tests/search_bench_smoke.rs`（2 萬則）防止失修。
- 量測（release，32 CPU Ryzen 9 7945HX；寫實語料，30 筆一頁）：100k：寫入 18 s、`rebuild-index` 2.9 s、DB 43 MiB（FTS 10 MiB）；查詢 p95 多數 < 30 ms（今天 ~30、台北 ~12、gitlab ~14），罕見單字全表 LIKE 25–140 ms，深 offset 90000 約 57 ms，全部 PASS。1M：寫入 186 s、rebuild 31 s、DB 434 MiB（FTS 99 MiB）；p95：常見詞「今天」246 ms、台北 96 ms、gitlab 117 ms、罕見詞 < 1 ms、無結果 < 1 ms、罕見單字全表 LIKE 1.3–1.9 s、篩選 25–48 ms、深 offset 20000 325 ms、**offset 90000 約 1.1 s（WARN）**。
- 發現與修正：chat／sender 篩選 + time 排序（或 relevance）時，沒有統計資訊的 SQLite 會以 chat／sender 索引為外層、對每筆訊息各探一次 FTS（100k 下 146–400 ms，與詞的稀有度無關，罕見詞反而最慢）。`list_messages` 在有 MATCH 時改用 `+m.chat_id`／`+m.sender_id` 讓 FTS 命中驅動查詢（結果集不變）：100k 下降為 3 ms 級。未修的限制：常見詞（命中數 ∝ 語料）在 time 排序要排序全部命中（1M「今天」~240 ms）；罕見／無結果單字為全表 LIKE（線性，1M 約 1.3 s，有 chat 篩選約 0.5 s）；relevance 極深 offset（接近上限 100000）在 1M 約 1.1 s；單字 LIKE 時間有雙峰（25 vs 130 ms @100k，疑為各連線 page cache）。
- 已知取捨：詞路徑為「詞皆出現」（順序／距離不拘），因此「台北咖啡」會找到「台北車站附近的咖啡」，連續性只由 bigram 片語保證；片語可能跨越相鄰 CJK run 邊界（設計文件已接受的誤判）；relevance 分頁為 offset 型，兩次請求之間若有寫入可能位移；`/status` 的計數為 O(n)。

## 寄件人歸屬與使用者功能

- mapper：自己發出且無 `from_id` 的訊息歸給綁定帳號（`TelegramAdapter::authenticated_account` 快取 get_me，並寫入 `senders`）；`post_author` 與 `fwd_from`（from_id／from_name／date）存入 migration 0007 欄位，轉發不影響寄件人。upsert 對 sender／metadata 使用 COALESCE，不再以 NULL 覆蓋。
- `MessageSource::Refetch`＋`SyncEngine::refetch_chat`＋`SyncCoordinator::submit_with(scope, refetch)`：僅補 NULL 欄位與附件資訊，獨立可續跑 checkpoint；CLI `sync chat --refetch`、REST `?refetch=true`。
- `tgarchive repair senders [--dry-run]`（`SqliteStore::repair_senders`）：僅頻道規則，不猜私訊。
- API／CLI：`/senders`、`/senders/{id}`、`senders search|get`、`--sender`（含 `me`、歧義報錯）、`post_author` 篩選；UI：使用者清單／詳細頁、點名跳轉、搜尋頁寄件人自動完成、署名與轉發顯示。
- 測試：`tests/senders.rs`（端點、CJK、migration 0007、repair、refetch、CLI）、mapper 單元測試、`web/e2e/senders.spec.ts`。
- Bot 旗標與曾用名稱（migration 0008）：`senders.is_bot`（mapper 取 grammers `User.bot`；僅使用者，其餘維持 NULL；upsert 非 NULL 覆蓋、NULL 不清除）；`sender_name_history`（合併後名稱／username 變動才新增列，相同則推進最新列 `last_seen_at`；同交易；時間為 tgarchive 觀察時間）。API：`SenderDto`／`SenderProfileDto`／`SenderDetailDto` 含 `is_bot`；`GET /senders?is_bot=`、`/messages`、`/chats/{id}/messages`、`/messages/search` 的 `exclude_bots=true`（排除 `is_bot=1` 的寄件人；未知與無寄件人保留）；`GET /senders/{id}` 含 `name_history`（新到舊）；`/senders?q=` 也比對曾用名稱與 username（`matched_history`、`matched_name`）。CLI：`--exclude-bots`、`senders search --is-bot`、`senders get` 列曾用名稱、搜尋標示 `[matched old name: …]`。UI：🤖 標記（時間軸／搜尋／清單／詳細頁）、清單「全部／人／Bot」、搜尋與聊天室時間軸「隱藏 bot」（URL `bot=`／`hide_bots=1`）、寄件人頁「曾用名稱」、清單「曾用名：…」提示。測試：`tests/sender_bots_history.rs`、mapper 單元測試、`web/e2e/senders.spec.ts`；fixture 新增 bot 與三個名稱的改名者（`examples/seed_fixture.rs`）。
- 限制：曾用名稱只記錄 tgarchive 觀察到的名稱（未同步期間的改名看不到）；脈絡檢視（`/messages/{id}/context`）不套用 `exclude_bots`；senders 清單於 Rust 端過濾／排序（senders 表小；候選 ≤2000 以 IN 查詢聚合，否則全域聚合）；私訊舊資料需 refetch；轉發來源僅儲存名稱／ID，不解析名稱。

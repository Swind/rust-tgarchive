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
| 9 Hardening | pending | full checks、README、fresh setup |

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

已知限制（未以真實帳號驗證）：grammers 的 update receiver 為一次性；`Dropped` 表示 sender pool 已停止，同一 adapter 無法重建，實際上第二次嘗試會回報 `ReceiverUnavailable` 並致命退出（需由外部重啟程序）。暫時性 I/O／RPC 錯誤則保留同一 `UpdateStream` 重試，此行為尚無真實網路驗證。Catch-up 只涵蓋已存在 chats 表的 chat，且與 grammers 內建 getDifference 為獨立機制，兩者重疊以 upsert 冪等處理；不宣稱 exactly-once 或零遺失。

## Manual acceptance

使用者指定本輪先完成程式與自動測試，真實帳號驗收稍後進行。真實 Telegram credentials 尚未提供。本專案不將 secrets 放入文件或 Git。登入、dialog、真實同步與更新、重啟恢復等人工驗收在取得環境前皆為 pending；自動 tests 不取代這些驗收。

## Storage semantics

- 部分 chat/sender metadata 的 null 值不會清除已知欄位；目前 refresh 也使用這個合併語意。
- 中文 FTS5 以 unicode61 fixture 驗證精確詞，未承諾中文子字串／自然分詞。

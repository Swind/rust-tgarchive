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
| 8 Realtime | in progress | 帳號綁定／common deletion persistence 5 tests 通過；listener、catch-up、crash recovery 待驗收 |
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

## Manual acceptance

使用者指定本輪先完成程式與自動測試，真實帳號驗收稍後進行。真實 Telegram credentials 尚未提供。本專案不將 secrets 放入文件或 Git。登入、dialog、真實同步與更新、重啟恢復等人工驗收在取得環境前皆為 pending；自動 tests 不取代這些驗收。

## Storage semantics

- 部分 chat/sender metadata 的 null 值不會清除已知欄位；目前 refresh 也使用這個合併語意。
- 中文 FTS5 以 unicode61 fixture 驗證精確詞，未承諾中文子字串／自然分詞。

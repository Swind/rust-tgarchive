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
| 4 CLI | pending | temporary archive CLI smoke tests |
| 5 REST/OpenAPI | pending | router、schema、export tests |
| 6 Telegram | pending | adapter tests + manual login/restart/dialog verification |
| 7 History | pending | checkpoint、ack、restart、job tests |
| 8 Realtime | pending | update、shutdown、crash recovery tests + manual verification |
| 9 Hardening | pending | full checks、README、fresh setup |

## Commits

- `a0466c9`：原始規劃與執行規劃基準。
- `f60b0ee`：Phase 1 核心，6 tests、fmt、clippy 通過。

## Manual acceptance

真實 Telegram credentials 尚未提供。本專案不將 secrets 放入文件或 Git。登入、dialog、真實同步與更新、重啟恢復等人工驗收在取得環境前皆為 pending；自動 tests 不取代這些驗收。

## Storage semantics

- 部分 chat/sender metadata 的 null 值不會清除已知欄位；目前 refresh 也使用這個合併語意。
- 中文 FTS5 以 unicode61 fixture 驗證精確詞，未承諾中文子字串／自然分詞。

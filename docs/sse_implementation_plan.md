# SSE archive updates implementation plan

## Scope and contract

- SSE is an invalidation hint; existing REST APIs and SQLite remain authoritative.
- One native EventSource per browser tab, at `GET /api/v1/events`.
- Events: `archive_changed` with JSON `{ "chat_ids": [123], "global": false }`; `resync` with JSON `{}` on every connection and broadcast lag.
- Publish only after a successful ingestion batch commit. Deduplicate affected chat IDs; account-wide deletions trigger global invalidation.
- In-memory notifications cover ingestion in the same serve process only. Query-only mode does not observe writes from another process; return an explicit unavailable response rather than a misleading silent stream.
- No durable replay, Redis, WebSocket, or Last-Event-ID support in this version.
- Keep existing sync/media status polling; those have independent write paths.

## Phase 1 — Application event channel

- [x] Add a bounded Tokio broadcast event channel and serializable change event.
- [x] Wire it through runtime and the shared ingestion worker without breaking existing callers.
- [x] Collect affected chats before consuming a batch; publish after successful commit only.
- [x] Cover account-wide deletions, history/refetch, edits and deletes.
- [x] Test commit-before-notification, failed writes, and deduplication.

## Phase 2 — SSE REST endpoint

- [x] Add `/api/v1/events` using Axum SSE with 15-second heartbeat.
- [x] Subscribe before sending initial resync; send resync on receiver lag.
- [x] Disable response/proxy buffering and caching; reuse existing request/security layers.
- [x] Ensure shutdown closes streams so graceful shutdown completes.
- [x] Document API and query-only/cross-process limitations; regenerate API artifacts as appropriate.
- [x] Test headers, initial event, change delivery, lag, unavailable mode, and shutdown.

## Phase 3 — Frontend and timeline

- [x] Mount a single EventSource lifecycle at App level; close on unmount.
- [x] Coalesce archive changes for ~300ms; invalidate only related active queries and mark related inactive caches stale.
- [x] Resync on reconnect and when a tab becomes visible.
- [x] Preserve existing filters and refresh loaded pages for edit/delete correctness.
- [x] Keep reader position when not at bottom; follow latest when at bottom.
- [x] Show an accessible new-message action while reading older messages.
- [x] Handle transient refetch failures without unmounting the existing timeline.
- [x] Test event coalescing, invalidation targeting, lifecycle, and scrolling where existing tooling permits.

## Phase 4 — Integration and verification

- [x] Review frontend/backend contract and query keys across all pages.
- [x] Run Rust checks/tests and web typecheck, lint, tests, build.
- [x] Exercise the SSE endpoint with a real HTTP connection, including shutdown.
- [x] Verify no polling was added for archive updates and no unrelated user work was overwritten.
- [x] Record completed tasks, validation results, and remaining limitations.

## Delegation

- Backend phases 1–2: Claude Sonnet 5.5 subagent; owns `src/`, Rust tests, Cargo dependency changes, API and README changes.
- Frontend phase 3: Claude Sonnet 5.5 subagent; owns `web/src/` and frontend tests only (not generated API schema).
- Integration phase 4 and this plan: parent agent.

## Deliberate first-version limits

- Infinite-query invalidation re-fetches all loaded pages for consistency. Optimize only if many loaded history pages make this measurably expensive.
- Notifications may be redundant; correctness must not rely on notification counts.
- No automatic detection of external SQLite writes in query-only mode.

## Integration review notes

- Query-key audit covers all archive-derived queries in the current frontend; sync/media polling is unchanged.
- Browser tests added in `web/e2e/archive-events.spec.ts`, using mock SSE signals with actual REST query and DOM behaviour.
- Follow-up review fixed unchanged context data with edited extra pages (React Query structural sharing), native button accessibility, sticky notification placement, fetch-next-page/refetch races, date-mode following, and initial-fetch/resync races.
- Browser acceptance caught unchanged `dataUpdatedAt` under a frozen clock; context extra pages now refresh on completion of a refetch even when both timestamp and data identity are unchanged.

## Final validation

- `cargo fmt --all -- --check`: passed.
- `cargo test --all-targets --locked`: passed; live Telegram acceptance and release benchmark retain their existing ignored status.
- `cargo clippy --all-targets --locked -- -D warnings`: passed.
- `npm --prefix web run typecheck`, `lint`, `test`, `build`: passed; 22 unit tests.
- E2E `npx tsc --noEmit` in `web/e2e`: passed.
- `scripts/e2e.sh`: 70 passed, including 6 new SSE browser acceptance tests and existing screenshot checks.
- `git diff --check`: passed.
- Real TCP SSE tests verify initial resync, committed-change delivery, headers, and shutdown with an open stream. Browser tests mock EventSource notifications while using real fixture REST responses; no live Telegram account was exercised or modified.
- Rebuilt the committed embedded `web/dist` assets and regenerated OpenAPI/TypeScript API artifacts.

## Remaining limits

- An anchor message that is itself deleted falls back to browser scroll clamping; retained anchor messages preserve their viewport offset.
- Context extra-page refetch failures keep the previous data visible with an error until a later refresh succeeds.
- SSE availability in real Telegram mode has not been tested against a live account; application/HTTP/browser layers are covered independently without touching the user's Telegram session.

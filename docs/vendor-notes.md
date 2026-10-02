# Vendored dependencies

## grammers-client

`vendor/grammers-client` contains the published `grammers-client` 0.10.0 source from the Cargo registry. Its upstream manifest, README, source, examples, tests, and MIT/Apache license texts are retained. Cargo registry markers and the package-local lockfile are excluded.

The local patch is intentionally small:

- `UpdateStream::has_pending_updates` reports whether Grammers has already processed raw updates waiting in its local output buffer. It does not inspect the sender-pool input queue or process additional updates. Realtime ingestion drains this buffer into one normalized archive batch, waits for the writer acknowledgement, then calls `sync_update_state`.
- Account-wide `updates.differenceTooLong` and channel `updates.channelDifferenceTooLong` fail with the synthetic `ARCHIVE_DIFFERENCE_TOO_LONG` RPC error before Grammers advances its in-memory message-box checkpoint. The caller must surface this as a collector failure requiring reconciliation.

To refresh the vendored source, replace it from the selected upstream release, reapply only these changes, retain both upstream license files, and review the complete diff. Do not call `sync_update_state` before every update in the returned local batch is durably acknowledged. Archive mode must use `update_queue_limit: None`; Grammers truncates buffered output after it has advanced its in-memory update state when a finite limit is exceeded.

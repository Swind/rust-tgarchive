# Dependency decisions (Phase 0)

Checked against docs.rs `latest` pages on 2026-10-02. The project now has a minimal P1 manifest; the full Phase 1–8 dependency matrix below is a **selection**, not a claim that this project has compiled it together. Core-only tests passed, while Grammers and the future HTTP/database stack have not been compiled here.

| Area | Selection for implementation | Why / evidence |
|---|---|---|
| Toolchain | Rust/Cargo `1.98.1`, edition `2024`, `rust-version = "1.98"` | `rustc 1.98.1 (48a229cea 2026-09-01)` and Cargo 1.98.1 are installed here. Pin the same toolchain in CI and local project configuration for reproducible implementation. This deliberately does not promise support for the unverified 1.85 edition floor. |
| Async runtime | `tokio = "1.53"` | docs.rs latest is 1.53.1. Enable only `macros`, `rt-multi-thread`, `net`, `signal`, `sync`, and `time` initially; add a feature only when code needs it. Axum explicitly targets Tokio. |
| HTTP | `axum = "0.8"`; `tower-http = "0.6"` only when middleware is used | docs.rs latest Axum is 0.8.9 and says it is designed to work with Tokio; utoipa-axum 0.2.0 declares Axum `^0.8.0`. |
| CLI / serialization | `clap = { version = "4.6", features = ["derive"] }`; `serde = "1"`; `serde_json = "1"` | docs.rs latest clap is 4.6.7. Derive is the documented built-in CLI path. |
| SQLite | `sqlx = "0.8.6"`, features `runtime-tokio`, `sqlite`, `migrate`, `macros` | Pin the current cached/known-compatible SQLx release line for this project rather than adopting SQLx 0.9 before it is needed. The published 0.8.6 manifest confirms all four features; SQLite is bundled by its `sqlite` feature. No TLS backend is needed. |
| OpenAPI | `utoipa = "5.5"` with `axum_extras` and `yaml`; `utoipa-axum = "0.2"` | utoipa 5.5.0 is the latest v5 line compatible with utoipa-axum 0.2.0, whose published manifest requires utoipa `^5.0.0` and Axum `^0.8.0`. The docs.rs `latest` utoipa is 6.0.0 and is **not** compatible with utoipa-axum 0.2's declared dependency range. Use YAML serialization built into utoipa's `yaml` feature (currently backed by `serde_norway`) instead of adding an independent YAML library. |
| Telegram | `grammers-client = "0.10"`; access its re-exported `grammers_client::session` and `grammers_client::sender` types | docs.rs latest `grammers-client` and `grammers-session` are both 0.10.0. Client re-exports its compatible session and sender dependencies, avoiding a second independently selected Grammers version. Use a registry release, never Git HEAD. |
| Errors / tracing / date | `thiserror = "2"`; `tracing = "0.1"`; `chrono = { version = "0.4", features = ["serde"] }` | Match the Phase 1 domain types and original plan: domain and DTO timestamps use UTC `DateTime<Utc>`, while SQLite may store Unix seconds at the persistence boundary. Do not add `anyhow` to the public core API. |

## Resolution and repeatable verification

P1 should add only dependencies used by its skeleton/domain/ports. Axum, SQLx, utoipa, clap, and Grammers can enter in their implementation phases; this Phase 0 selection does not justify pulling the entire future stack into the initial skeleton. Once the relevant manifest entries exist, from the repository root:

```sh
rustc --version
cargo --version
cargo update
cargo tree -d
cargo check --all-targets
cargo tree -i utoipa
cargo tree -i grammers-session
```

Then commit the generated `Cargo.lock` with implementation changes. The expected checks are one resolved utoipa 5.x line shared by `utoipa-axum`, Axum 0.8.x, Grammers client/session 0.10.x (session reached via client re-export), and no SQLx TLS backend. The current lock/manifest cover only the core skeleton; resolution and compilation of the full future dependency set remain pending.

## Verified compatibility pin

Phase 6 的隔離 `cargo check` spike 曾成功編譯 Grammers client/session 0.10.0（官方 SQLite session）與 SQLx 0.8.6。然而 `cargo test` 的最終 linking 發現 libsql-ffi 與 libsqlite3-sys 的 `sqlite3_*` 重複符號。`cargo check` 不足以證明 executable 可連結，先前無 conflict 的推論已撤回。

因此停用 Grammers 的 `sqlite-storage` default feature，僅啟用 session types 的 `serde`，以最小 file-backed Session adapter 持久化 auth/DC/peer/update snapshot；寫檔採私有權限與原子替換。SQLx 是 archive 唯一 SQLite stack。此 adapter 仍需 Phase 6 本地 reopen/權限/連結測試；真帳號驗收另行進行。

Fresh resolution 發現 grammers-crypto 0.10.0 的 `glass_pumpkin` prerelease range 會選到 rc1，該版本的 bigint/check API 與 Grammers 預期不相容。因此 manifest 明確固定 `glass_pumpkin = "=2.0.0-rc0"`；使用此 pin 的 spike 編譯通過。升級 Grammers 時再檢查是否可以移除此相容性 pin。

## Source links

- [Tokio crate docs (1.53.1)](https://docs.rs/tokio/latest/tokio/)
- [Axum crate docs (0.8.9)](https://docs.rs/axum/latest/axum/)
- [SQLx 0.8.6 published manifest](https://docs.rs/crate/sqlx/0.8.6/source/Cargo.toml)
- [clap crate docs (4.6.7)](https://docs.rs/clap/latest/clap/)
- [utoipa 5.5.0 docs](https://docs.rs/utoipa/5.5.0/utoipa/), [utoipa-axum 0.2.0 manifest](https://docs.rs/crate/utoipa-axum/0.2.0/source/Cargo.toml)
- [grammers-client docs (0.10.0)](https://docs.rs/grammers-client/latest/grammers_client/), [grammers-session docs (0.10.0)](https://docs.rs/grammers-session/latest/grammers_session/)

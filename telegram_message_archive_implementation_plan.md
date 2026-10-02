# Telegram Message Archive / Collector — Implementation Plan

## 1. Project Goal

Build a Rust application that logs in to Telegram using a **normal Telegram user account** (not only the Bot API), continuously collects messages from accessible groups/channels, supports historical synchronization, and stores the collected data locally.

The application must expose the same core functionality through:

- a REST API,
- a CLI,
- background Telegram update listeners,
- background history synchronization workers.

The core business logic must not live in the REST API, CLI, Telegram adapter, or database implementation.

The project should follow **DDD-inspired boundaries and Clean Architecture**, but avoid unnecessary enterprise-style abstraction.

Primary goals:

1. Use a Telegram user account through MTProto.
2. Collect new messages in real time.
3. Synchronize historical messages.
4. Persist chats, senders, messages, attachments metadata, and synchronization state.
5. Query and search archived messages.
6. Provide a REST API.
7. Provide an automatically generated OpenAPI specification.
8. Provide a CLI exposing the same application capabilities as REST where appropriate.
9. Keep Telegram, database, REST, and CLI implementation details outside the domain/application core.
10. Keep the first version simple enough to maintain as a single-node service.

---

# 2. Scope

## 2.1 Initial Scope

The first implementation should support:

- Telegram account authentication.
- Persistent Telegram session storage.
- Listing accessible chats/dialogs.
- Supported chat types:
  - private chats,
  - groups,
  - supergroups,
  - channels.
- Historical message synchronization.
- Continuous real-time message collection.
- Message creation.
- Message editing.
- Message deletion where Telegram updates provide sufficient information.
- Sender metadata.
- Basic attachment/media metadata.
- Chat metadata.
- SQLite persistence.
- SQLite FTS5 full-text message search.
- REST API.
- OpenAPI JSON/YAML generation.
- CLI.
- Structured logging.
- Graceful shutdown.
- Automated tests.

## 2.2 Explicit Non-Goals for Version 1

Do not implement these unless required by a later task:

- PostgreSQL.
- Redis.
- Kafka/NATS/RabbitMQ.
- Elasticsearch/OpenSearch.
- Qdrant/vector search.
- LLM/RAG.
- automatic media downloading.
- OCR.
- multi-user account management.
- browser UI/frontend.
- distributed workers.
- horizontal scaling.
- complex event sourcing.
- strict CQRS infrastructure.
- dozens of separate Rust crates.
- generic plugin framework.

The architecture should allow these capabilities to be added later without redesigning the core application.

---

# 3. Technology Choices

Use the following technologies unless there is a strong technical reason to deviate.

## Runtime

**Tokio**

Reasons:

- Telegram networking is asynchronous.
- Axum integrates naturally with Tokio.
- SQLx supports Tokio.
- Background collectors and workers can run concurrently.
- Tokio provides channels, cancellation, timers, tasks, and graceful shutdown primitives.

## Telegram Client

**grammers**

Use Telegram MTProto with a real Telegram user account.

Responsibilities:

- authentication,
- session persistence,
- Telegram RPC,
- dialogs/chat enumeration,
- historical message retrieval,
- real-time updates.

Do not allow `grammers` types to leak into the domain or application layers.

Pin the selected `grammers-*` dependencies to compatible versions. If using Git dependencies, pin an exact commit instead of tracking the repository head.

## REST Framework

**Axum**

Reasons:

- small and composable,
- strong Tokio/Tower integration,
- easy extractor-based validation,
- middleware ecosystem,
- does not require business logic to be embedded in controller classes.

## OpenAPI

**utoipa**

Use Rust API definitions/types as the source of truth.

Generate:

- `/openapi.json`
- `/openapi.yml`

Optionally expose Swagger UI:

- `/swagger-ui/`

Also provide a CLI command that prints the OpenAPI specification.

Do not manually maintain a second independent OpenAPI specification.

## CLI

**clap** with derive macros.

CLI responsibilities:

- parse arguments,
- validate syntax and simple constraints,
- convert CLI arguments to application commands/queries,
- invoke application use cases,
- format output.

The CLI must not contain Telegram, SQL, or business logic.

## Persistence

**SQLite**

Reasons:

- this is primarily a single-node archival service,
- workload is dominated by message inserts and reads,
- deployment is simple,
- millions of messages are practical,
- FTS5 provides efficient text search,
- no external database server is required.

Enable WAL mode.

Recommended pragmas:

```sql
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;
PRAGMA busy_timeout = 5000;
PRAGMA synchronous = NORMAL;
```

## Database Access

**SQLx**

Reasons:

- async,
- works well with SQLite,
- low abstraction overhead,
- no ORM model leakage into the domain,
- migrations are built in,
- explicit SQL is appropriate for archive/query workloads.

## Full-Text Search

**SQLite FTS5**

Use FTS5 for lexical text search.

Do not use `LIKE '%query%'` as the primary search implementation.

## Serialization

- `serde`
- `serde_json`
- `serde_yaml` for OpenAPI YAML output

## Errors

- `thiserror` for domain/application/infrastructure typed errors.
- `anyhow` only for application startup/bootstrap or top-level executable errors.

Avoid `anyhow::Error` inside domain/application public APIs.

## Logging / Telemetry

- `tracing`
- `tracing-subscriber`
- `tower-http` tracing middleware for HTTP

All important background operations should produce structured logs.

---

# 4. Architectural Principles

Use four conceptual layers:

1. Domain
2. Application
3. Interfaces
4. Infrastructure

Dependency direction:

```text
Interfaces --------\
                   \
                    > Application ---> Domain
                   /
Infrastructure ---/

Bootstrap knows all layers.
```

More strictly:

```text
domain
  - depends on no project layer

application
  - depends on domain

interface
  - depends on application/domain

infrastructure
  - implements ports required by application
  - may depend on application/domain

bootstrap
  - wires everything together
```

Do not let:

- Domain import Axum.
- Domain import SQLx.
- Domain import grammers.
- Application import Axum.
- Application import clap.
- Application import SQLx.
- Application import grammers.

---

# 5. Suggested Source Layout

Start as a single Rust crate with module boundaries.

Do not split into many crates prematurely.

```text
src/
├── main.rs
├── bootstrap.rs
├── config.rs
│
├── domain/
│   ├── mod.rs
│   ├── chat.rs
│   ├── message.rs
│   ├── sender.rs
│   ├── attachment.rs
│   ├── pagination.rs
│   └── error.rs
│
├── application/
│   ├── mod.rs
│   │
│   ├── ports/
│   │   ├── mod.rs
│   │   ├── telegram.rs
│   │   ├── message_repository.rs
│   │   ├── chat_repository.rs
│   │   ├── sender_repository.rs
│   │   └── sync_repository.rs
│   │
│   ├── messages/
│   │   ├── mod.rs
│   │   ├── ingest.rs
│   │   ├── get.rs
│   │   ├── list.rs
│   │   └── search.rs
│   │
│   ├── chats/
│   │   ├── mod.rs
│   │   ├── get.rs
│   │   └── list.rs
│   │
│   └── sync/
│       ├── mod.rs
│       ├── chat.rs
│       ├── all.rs
│       └── status.rs
│
├── interface/
│   ├── mod.rs
│   │
│   ├── rest/
│   │   ├── mod.rs
│   │   ├── router.rs
│   │   ├── error.rs
│   │   ├── openapi.rs
│   │   ├── dto/
│   │   │   ├── mod.rs
│   │   │   ├── message.rs
│   │   │   ├── chat.rs
│   │   │   └── sync.rs
│   │   └── routes/
│   │       ├── mod.rs
│   │       ├── messages.rs
│   │       ├── chats.rs
│   │       ├── sync.rs
│   │       └── status.rs
│   │
│   └── cli/
│       ├── mod.rs
│       ├── args.rs
│       ├── output.rs
│       ├── messages.rs
│       ├── chats.rs
│       └── sync.rs
│
└── infrastructure/
    ├── mod.rs
    │
    ├── telegram/
    │   ├── mod.rs
    │   ├── client.rs
    │   ├── auth.rs
    │   ├── listener.rs
    │   ├── history.rs
    │   └── mapper.rs
    │
    └── persistence/
        ├── mod.rs
        └── sqlite/
            ├── mod.rs
            ├── database.rs
            ├── message_repository.rs
            ├── chat_repository.rs
            ├── sender_repository.rs
            ├── sync_repository.rs
            └── migrations/
```

Move to a Cargo workspace only when module-level boundaries are no longer sufficient.

---

# 6. Domain Model

Keep the domain small.

Avoid modeling every Telegram concept immediately.

## 6.1 Identifier Value Objects

Prefer explicit identifier wrappers:

```rust
pub struct ChatId(pub i64);
pub struct MessageId(pub i64);
pub struct SenderId(pub i64);
```

Consider deriving:

- `Debug`
- `Clone`
- `Copy`
- `Eq`
- `PartialEq`
- `Hash`
- serialization only where useful

Do not use raw `i64` everywhere.

## 6.2 Chat

```rust
pub struct Chat {
    pub id: ChatId,
    pub kind: ChatKind,
    pub title: Option<String>,
    pub username: Option<String>,
}

pub enum ChatKind {
    Private,
    Group,
    Supergroup,
    Channel,
}
```

Keep Telegram-specific peer objects out of this model.

## 6.3 Sender

```rust
pub struct Sender {
    pub id: SenderId,
    pub kind: SenderKind,
    pub display_name: Option<String>,
    pub username: Option<String>,
}
```

Possible sender kinds:

```rust
pub enum SenderKind {
    User,
    Chat,
    Channel,
    Unknown,
}
```

## 6.4 Message

Suggested initial model:

```rust
pub struct Message {
    pub id: MessageId,
    pub chat_id: ChatId,
    pub sender_id: Option<SenderId>,

    pub timestamp: DateTime<Utc>,
    pub edited_at: Option<DateTime<Utc>>,

    pub text: Option<String>,

    pub reply_to: Option<MessageId>,

    pub attachments: Vec<Attachment>,
}
```

Telegram message IDs are not globally unique.

Use:

```text
(chat_id, message_id)
```

as the logical unique identifier.

## 6.5 Attachment

Version 1 should store metadata only.

Example:

```rust
pub struct Attachment {
    pub kind: AttachmentKind,
    pub telegram_file_id: Option<String>,
    pub mime_type: Option<String>,
    pub file_name: Option<String>,
    pub size: Option<i64>,
}
```

Possible initial kinds:

```rust
pub enum AttachmentKind {
    Photo,
    Video,
    Audio,
    Voice,
    Document,
    Sticker,
    Animation,
    Other,
}
```

Do not implement media download in the first version.

## 6.6 Message Event

Normalize Telegram updates into application-friendly events.

```rust
pub enum MessageEvent {
    Created(Message),
    Updated(Message),
    Deleted {
        chat_id: ChatId,
        message_id: MessageId,
    },
}
```

Both real-time updates and history synchronization should eventually pass through the same ingestion pipeline.

---

# 7. Application Ports

Infrastructure adapters must implement application-defined ports.

## 7.1 Telegram Gateway

Example:

```rust
#[async_trait]
pub trait TelegramGateway: Send + Sync {
    async fn list_chats(&self) -> Result<Vec<Chat>, TelegramGatewayError>;

    async fn fetch_messages(
        &self,
        request: FetchMessagesRequest,
    ) -> Result<MessageBatch, TelegramGatewayError>;
}
```

The application layer must not expose `grammers` objects.

Suggested request/result models:

```rust
pub struct FetchMessagesRequest {
    pub chat_id: ChatId,
    pub cursor: Option<MessageCursor>,
    pub limit: usize,
}

pub struct MessageBatch {
    pub messages: Vec<Message>,
    pub next_cursor: Option<MessageCursor>,
    pub has_more: bool,
}
```

Real-time listening can be implemented as infrastructure pushing normalized events into an application ingestion service/channel rather than forcing the application to depend on a Telegram stream type.

## 7.2 Message Repository

Suggested responsibilities:

```rust
#[async_trait]
pub trait MessageRepository: Send + Sync {
    async fn upsert(&self, message: &Message) -> Result<(), RepositoryError>;

    async fn delete(
        &self,
        chat_id: ChatId,
        message_id: MessageId,
    ) -> Result<(), RepositoryError>;

    async fn get(
        &self,
        chat_id: ChatId,
        message_id: MessageId,
    ) -> Result<Option<Message>, RepositoryError>;

    async fn list(
        &self,
        query: ListMessagesQuery,
    ) -> Result<MessagePage, RepositoryError>;

    async fn search(
        &self,
        query: SearchMessagesQuery,
    ) -> Result<MessagePage, RepositoryError>;
}
```

## 7.3 Chat Repository

```rust
#[async_trait]
pub trait ChatRepository: Send + Sync {
    async fn upsert(&self, chat: &Chat) -> Result<(), RepositoryError>;
    async fn get(&self, id: ChatId) -> Result<Option<Chat>, RepositoryError>;
    async fn list(&self, query: ListChatsQuery) -> Result<Vec<Chat>, RepositoryError>;
}
```

## 7.4 Sender Repository

```rust
#[async_trait]
pub trait SenderRepository: Send + Sync {
    async fn upsert(&self, sender: &Sender) -> Result<(), RepositoryError>;
}
```

## 7.5 Sync Repository

Track per-chat synchronization state.

Possible interface:

```rust
#[async_trait]
pub trait SyncRepository: Send + Sync {
    async fn get_chat_state(
        &self,
        chat_id: ChatId,
    ) -> Result<Option<ChatSyncState>, RepositoryError>;

    async fn save_chat_state(
        &self,
        state: &ChatSyncState,
    ) -> Result<(), RepositoryError>;
}
```

---

# 8. Application Use Cases

REST and CLI must invoke these use cases instead of duplicating behavior.

## Messages

Implement:

- `IngestMessageEvent`
- `GetMessage`
- `ListMessages`
- `SearchMessages`

## Chats

Implement:

- `GetChat`
- `ListChats`
- `RefreshChats`

## Synchronization

Implement:

- `SyncChat`
- `SyncAllChats`
- `GetSyncStatus`

Potential future use cases:

- `DownloadAttachment`
- `ReindexSearch`
- `SemanticSearch`

Do not implement future use cases until needed.

---

# 9. Input Validation Rules

Validation must be separated into interface-level and business/application-level validation.

## Interface Validation

REST/CLI may validate:

- required parameter exists,
- integer parses successfully,
- enum string is valid,
- limit falls inside allowed range,
- malformed JSON,
- malformed cursor,
- invalid timestamp format.

Examples:

```text
limit < 1
limit > 1000
invalid chat_id syntax
invalid sort direction
```

## Application / Domain Validation

Application handles:

- chat does not exist,
- requested synchronization is not possible,
- invalid state transition,
- conflicting operation,
- repository failure,
- Telegram access failure,
- synchronization state inconsistency.

Example:

```text
"abc" is not a valid chat ID
=> interface rejects it.

12345 is syntactically valid but no such chat is known
=> application returns ChatNotFound.
```

---

# 10. Application API Models

REST DTOs and CLI argument structs should convert into common application commands/queries.

Example:

```rust
pub struct ListMessagesQuery {
    pub chat_id: Option<ChatId>,
    pub sender_id: Option<SenderId>,
    pub before: Option<MessageCursor>,
    pub after: Option<MessageCursor>,
    pub limit: PageSize,
}
```

REST:

```text
HTTP query params
    ↓
ListMessagesRequest
    ↓
TryFrom
    ↓
ListMessagesQuery
```

CLI:

```text
clap args
    ↓
ListMessagesArgs
    ↓
TryFrom
    ↓
ListMessagesQuery
```

Both invoke the same:

```rust
app.messages.list.execute(query).await
```

This rule applies throughout the application.

---

# 11. Pagination

Use cursor/keyset pagination.

Do not use large SQL `OFFSET` pagination.

Good API shape:

```http
GET /api/v1/messages?chat_id=-100123&before=<cursor>&limit=100
```

Possible cursor contents:

- timestamp,
- message ID,
- chat ID if needed.

For globally ordered messages, use a deterministic compound sort key such as:

```text
(timestamp, chat_id, message_id)
```

The cursor may be serialized as an opaque URL-safe Base64 value so external clients do not depend on the internal representation.

Application APIs should treat cursors as value objects.

---

# 12. SQLite Schema

Use SQLx migrations.

The exact schema may evolve, but the initial design should resemble the following.

## 12.1 Chats

```sql
CREATE TABLE chats (
    id              INTEGER PRIMARY KEY,
    kind            TEXT NOT NULL,
    title           TEXT,
    username        TEXT,

    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL
);
```

`created_at` here means the archive database record creation time unless a reliable Telegram chat creation timestamp exists.

## 12.2 Senders

```sql
CREATE TABLE senders (
    id              INTEGER PRIMARY KEY,
    kind            TEXT NOT NULL,
    display_name    TEXT,
    username        TEXT,

    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL
);
```

## 12.3 Messages

```sql
CREATE TABLE messages (
    row_id          INTEGER PRIMARY KEY AUTOINCREMENT,

    chat_id         INTEGER NOT NULL,
    message_id      INTEGER NOT NULL,

    sender_id       INTEGER,

    timestamp       INTEGER NOT NULL,
    edited_at       INTEGER,

    text            TEXT,

    reply_to        INTEGER,

    is_deleted      INTEGER NOT NULL DEFAULT 0,

    collected_at    INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,

    UNIQUE(chat_id, message_id),

    FOREIGN KEY(chat_id) REFERENCES chats(id)
);
```

Use an internal SQLite `row_id` because FTS integration is easier with a single integer row key.

## 12.4 Attachments

```sql
CREATE TABLE attachments (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,

    message_row_id      INTEGER NOT NULL,

    kind                TEXT NOT NULL,
    telegram_file_id    TEXT,
    mime_type           TEXT,
    file_name           TEXT,
    size                INTEGER,

    FOREIGN KEY(message_row_id)
        REFERENCES messages(row_id)
        ON DELETE CASCADE
);
```

## 12.5 Synchronization State

```sql
CREATE TABLE chat_sync_state (
    chat_id                 INTEGER PRIMARY KEY,

    oldest_message_id       INTEGER,
    newest_message_id       INTEGER,

    history_complete        INTEGER NOT NULL DEFAULT 0,

    last_sync_started_at    INTEGER,
    last_sync_completed_at  INTEGER,

    last_error              TEXT,

    FOREIGN KEY(chat_id) REFERENCES chats(id)
);
```

Avoid using only "last imported message ID" for all synchronization decisions. History synchronization may move backward while real-time updates move forward.

Track enough state to support both directions safely.

---

# 13. FTS5 Search

Create an FTS5 index for message text.

Example design:

```sql
CREATE VIRTUAL TABLE messages_fts
USING fts5(
    text,
    content='messages',
    content_rowid='row_id'
);
```

Keep FTS synchronized using one of:

1. SQLite triggers, or
2. explicit repository updates.

Prefer triggers if they remain simple and well tested.

Search should support at minimum:

- free text query,
- optional chat filter,
- optional sender filter,
- optional time range,
- result limit,
- cursor pagination.

Return the original `Message` application model, not raw FTS rows.

---

# 14. Telegram Authentication

The Telegram adapter must support persistent user-account authentication.

Configuration will require Telegram API credentials.

Do not hard-code credentials.

Suggested environment/config values:

```text
TELEGRAM_API_ID
TELEGRAM_API_HASH
TELEGRAM_SESSION_FILE
DATABASE_URL
SERVER_BIND
RUST_LOG
```

Authentication flow:

1. Open/load persistent Telegram session.
2. Connect.
3. Determine whether the session is authorized.
4. If not authorized:
   - request phone number,
   - request verification code,
   - handle password/2FA if required,
   - persist resulting session.
5. On future starts, reuse the session.

Interactive authentication may initially be CLI-driven.

Do not expose Telegram login codes or passwords through ordinary REST endpoints in version 1.

The Telegram session file must be treated as sensitive.

---

# 15. Telegram Mapping Layer

Create explicit mapping functions between `grammers` types and internal models.

Example conceptual interface:

```rust
fn map_message(input: &grammers::types::Message) -> Result<Message, MappingError>;
fn map_chat(...) -> Result<Chat, MappingError>;
fn map_sender(...) -> Result<Sender, MappingError>;
```

Rules:

- all Telegram-specific field interpretation belongs here,
- domain/application must not know `grammers` types,
- unsupported Telegram properties may be ignored initially,
- do not panic because a Telegram field is absent.

If preserving unsupported Telegram metadata becomes useful, introduce an explicit metadata structure later instead of allowing raw Telegram objects into the domain.

---

# 16. Real-Time Collection

The service must maintain a long-running Telegram update listener.

Conceptual pipeline:

```text
Telegram
   ↓
grammers update
   ↓
Telegram mapper
   ↓
MessageEvent
   ↓
bounded Tokio mpsc channel
   ↓
IngestMessageEvent
   ↓
repositories
   ↓
SQLite
```

Use a **bounded** Tokio channel to introduce backpressure.

Do not use an unbounded channel for the primary ingestion path.

The listener must remain lightweight.

It should not:

- execute complex SQL directly,
- perform media downloads,
- generate embeddings,
- make slow external API calls.

Those operations belong behind workers/use cases.

---

# 17. Message Ingestion

Both historical synchronization and real-time updates must reuse the same message persistence logic.

Example:

```text
Realtime update ----------\
                           \
                            > MessageEvent -> IngestMessageEvent -> repository
                           /
History synchronization --/
```

`IngestMessageEvent` handles:

### Created

- ensure referenced chat exists or update it,
- ensure sender metadata is available when possible,
- upsert message,
- persist attachment metadata,
- update relevant synchronization state if needed.

### Updated

- upsert changed message state,
- update `edited_at`,
- update text/attachments,
- keep FTS synchronized.

### Deleted

Prefer a soft-delete strategy initially:

```text
is_deleted = 1
```

instead of permanently deleting the row.

Reasons:

- preserves archive consistency,
- makes update ordering less dangerous,
- allows audit/debugging,
- Telegram deletion updates may carry limited context.

Search/list APIs should exclude deleted rows by default, with an explicit option if later needed.

---

# 18. Historical Synchronization

Historical synchronization is separate from real-time collection but uses the same ingestion path.

Conceptual flow:

```text
List Telegram chats
        ↓
select chat
        ↓
load chat sync state
        ↓
fetch message batch
        ↓
map messages
        ↓
IngestMessageEvent
        ↓
save checkpoint
        ↓
continue until complete or stopped
```

Requirements:

- resumable,
- idempotent,
- safe to restart,
- bounded batch size,
- logs progress,
- handles Telegram rate limits,
- does not reinsert duplicate messages incorrectly.

Use SQLite unique constraint:

```text
UNIQUE(chat_id, message_id)
```

and repository upsert behavior for idempotency.

Do not rely only on process memory for synchronization progress.

---

# 19. Synchronization Scheduling

Version 1 should support manual synchronization via:

```text
CLI
REST
startup option
```

Possible CLI:

```bash
telegram-archive sync chat <CHAT_ID>
telegram-archive sync all
```

Possible REST:

```http
POST /api/v1/chats/{chat_id}/sync
POST /api/v1/sync
```

A synchronization request should not require the HTTP handler to remain open for the entire operation.

Preferred design:

```text
request
  ↓
application sync coordinator
  ↓
create/enqueue sync job
  ↓
return accepted/job status
```

For version 1 this can still be implemented entirely in-process using Tokio tasks/channels.

No external job queue is required.

Expose synchronization status separately.

---

# 20. REST API

Use versioned routes:

```text
/api/v1
```

Initial API surface:

## Status

```http
GET /api/v1/status
```

Return:

- application status,
- Telegram connectivity/auth state,
- database status,
- realtime collector state.

Do not expose secrets.

## Chats

```http
GET /api/v1/chats
GET /api/v1/chats/{chat_id}
POST /api/v1/chats/refresh
POST /api/v1/chats/{chat_id}/sync
```

## Messages

```http
GET /api/v1/messages
GET /api/v1/chats/{chat_id}/messages
GET /api/v1/messages/search
GET /api/v1/chats/{chat_id}/messages/{message_id}
```

Avoid a globally ambiguous:

```text
GET /messages/{message_id}
```

because Telegram message IDs are not globally unique.

Use `(chat_id, message_id)`.

Example list query:

```http
GET /api/v1/messages?chat_id=-100123&limit=100&before=<cursor>
```

Example search:

```http
GET /api/v1/messages/search?q=chromium&chat_id=-100123&limit=50
```

## Synchronization

```http
POST /api/v1/sync
GET  /api/v1/sync/status
GET  /api/v1/chats/{chat_id}/sync/status
```

If introducing job IDs:

```http
GET /api/v1/sync/jobs/{job_id}
```

is preferable.

---

# 21. REST Handler Rule

Handlers must be thin.

A correct handler should approximately do:

```rust
async fn list_messages(
    State(app): State<AppState>,
    Query(request): Query<ListMessagesRequest>,
) -> Result<Json<ListMessagesResponse>, ApiError> {
    let query = request.try_into()?;
    let result = app.list_messages.execute(query).await?;
    Ok(Json(result.into()))
}
```

Handlers must not:

- execute SQL,
- access SQLx pools directly,
- invoke grammers directly,
- decide synchronization algorithms,
- implement business rules.

---

# 22. REST Error Model

Use a consistent JSON error response.

Example:

```json
{
  "error": {
    "code": "chat_not_found",
    "message": "Chat was not found",
    "request_id": "..."
  }
}
```

Possible error mapping:

```text
Malformed input          -> 400
Validation error         -> 400
Chat/message not found   -> 404
Conflict                 -> 409
Telegram unavailable     -> 503
Database unavailable     -> 503
Unexpected internal      -> 500
```

Do not expose internal stack traces, SQL errors, or Telegram credentials.

---

# 23. OpenAPI

OpenAPI must be generated from the Rust API implementation.

Requirements:

- `utoipa` schemas for REST request/response DTOs,
- endpoint annotations,
- error response schema,
- pagination schema,
- examples where useful.

Expose:

```text
GET /openapi.json
GET /openapi.yml
```

Optionally:

```text
GET /swagger-ui/
```

Add CLI command:

```bash
telegram-archive openapi
telegram-archive openapi --format yaml
telegram-archive openapi --format json
```

The repository may contain a generated `openapi.yml`, but it must be regenerated from code.

Recommended CI check:

```bash
cargo run -- openapi --format yaml > openapi.generated.yml
diff -u openapi.yml openapi.generated.yml
```

This prevents API implementation and committed API documentation from drifting.

---

# 24. CLI

Suggested top-level CLI:

```text
telegram-archive
├── serve
├── auth
├── chats
├── messages
├── sync
├── status
└── openapi
```

Examples:

```bash
telegram-archive auth login

telegram-archive serve

telegram-archive chats list
telegram-archive chats refresh

telegram-archive messages list \
  --chat-id -100123 \
  --limit 100

telegram-archive messages search "chromium" \
  --chat-id -100123

telegram-archive sync chat -100123
telegram-archive sync all

telegram-archive status

telegram-archive openapi --format yaml
```

CLI output formats may support:

```text
human
json
```

Example:

```bash
telegram-archive messages list --output json
```

The CLI must use the same application use cases as REST.

---

# 25. Application Composition

Provide an application facade/container.

Conceptually:

```rust
pub struct Application {
    pub get_message: Arc<GetMessage>,
    pub list_messages: Arc<ListMessages>,
    pub search_messages: Arc<SearchMessages>,
    pub list_chats: Arc<ListChats>,
    pub refresh_chats: Arc<RefreshChats>,
    pub sync_chat: Arc<SyncChat>,
    pub sync_all: Arc<SyncAllChats>,
    pub get_sync_status: Arc<GetSyncStatus>,
}
```

Exact generics may become cumbersome.

Prefer trait objects or concrete shared service structs when they simplify bootstrap code.

Do not create abstraction solely for theoretical purity.

---

# 26. Bootstrap

`bootstrap.rs` is the composition root.

It may know all implementations.

Responsibilities:

1. load config,
2. initialize tracing,
3. open SQLite,
4. apply migrations,
5. initialize repositories,
6. initialize Telegram session/client,
7. create application services,
8. create ingestion channel,
9. create workers,
10. create REST router,
11. start requested mode.

Example conceptual dependency wiring:

```text
SqlitePool
  ↓
SqliteMessageRepository
SqliteChatRepository
SqliteSenderRepository
SqliteSyncRepository

GrammersTelegramGateway
  ↓

Application services
  ↓

CLI / REST / background workers
```

No global singleton is required.

Use `Arc` for shared immutable service ownership where appropriate.

---

# 27. Runtime Modes

The executable should support at least:

## Server Mode

```bash
telegram-archive serve
```

Starts:

- REST server,
- Telegram real-time listener,
- ingestion worker,
- optional background sync coordinator.

## One-Shot CLI Commands

Examples:

```bash
telegram-archive chats list
telegram-archive messages search rust
```

These initialize required dependencies, execute the use case, then exit.

## Authentication Mode

```bash
telegram-archive auth login
```

Interactive login and session persistence.

---

# 28. Concurrency Design

Use Tokio intentionally.

Recommended tasks:

```text
Task A: Telegram realtime listener
Task B: message ingestion worker
Task C: REST server
Task D: sync coordinator
Task E+: bounded sync jobs if necessary
```

Prefer:

- bounded channels,
- cancellation tokens,
- graceful shutdown,
- explicit task ownership.

Avoid spawning detached tasks everywhere.

A central runtime/service supervisor should own important long-running tasks.

---

# 29. SQLite Write Strategy

SQLite handles concurrent readers well in WAL mode but still fundamentally serializes writes.

Do not create excessive independent write contention.

Recommended initial design:

```text
Realtime Telegram events --\
                            \
History sync events ---------> bounded ingestion queue -> ingestion worker -> repositories
                            /
future import sources -------/
```

Reads from REST/CLI may use the SQLx pool normally.

It is acceptable for repositories to perform transactions.

If ingestion throughput later becomes a bottleneck, batch writes before considering a different database.

Do not move to PostgreSQL preemptively.

---

# 30. Transactions

Use transactions when a message operation modifies multiple tables.

Example message upsert transaction:

```text
BEGIN

upsert sender
upsert message
replace/update attachment metadata
update FTS state if not trigger-based
update relevant checkpoint if required

COMMIT
```

Do not leave a message row and attachments partially updated after failure.

---

# 31. Idempotency

Message ingestion and historical synchronization must be idempotent.

Use:

```text
UNIQUE(chat_id, message_id)
```

and upsert semantics.

Repeated synchronization of the same batch must result in the same stored state.

This should be explicitly tested.

---

# 32. Configuration

Support environment variables initially.

Optional later enhancement:

- TOML configuration file.

Suggested config:

```text
TELEGRAM_API_ID
TELEGRAM_API_HASH
TELEGRAM_SESSION_FILE

DATABASE_URL=sqlite://telegram.db

SERVER_BIND=127.0.0.1:8080

RUST_LOG=info
```

Potential config struct:

```rust
pub struct Config {
    pub telegram: TelegramConfig,
    pub database: DatabaseConfig,
    pub server: ServerConfig,
}
```

Do not scatter environment access throughout the application.

Load configuration once during bootstrap.

---

# 33. Security

The service archives private Telegram content, therefore security matters.

Version 1 requirements:

- default REST bind address should be localhost unless explicitly configured,
- never log Telegram API hash,
- never log Telegram verification code,
- never log 2FA password,
- protect Telegram session file permissions,
- do not expose raw database errors over REST,
- do not expose archived messages without considering deployment access controls.

REST authentication may be deferred if the service is guaranteed to bind only to localhost.

If binding to non-loopback interfaces, implement authentication before treating the service as production-ready.

---

# 34. Observability

Use `tracing`.

Important spans/events:

```text
telegram.connect
telegram.auth
telegram.update
telegram.history_batch
message.ingest
sync.chat
sync.all
db.query failure
http.request
```

Include useful identifiers:

- chat ID,
- message ID,
- sync job ID,
- batch size.

Do not include sensitive message text in normal INFO logs.

Message content should appear only in debug logs if explicitly necessary, and ideally not by default.

---

# 35. Graceful Shutdown

Handle:

- SIGINT,
- SIGTERM.

Shutdown sequence should:

1. stop accepting new HTTP work,
2. stop scheduling new synchronization jobs,
3. cancel Telegram listener,
4. stop producers,
5. drain or cleanly stop ingestion queue,
6. flush/commit in-flight DB operations,
7. close resources.

Use Tokio cancellation primitives or `tokio-util::sync::CancellationToken`.

---

# 36. Testing Strategy

Testing is required from the beginning.

## Domain Tests

Pure unit tests for:

- identifier/value object validation,
- pagination/cursor logic,
- domain transformations if any.

## Application Tests

Use fake/in-memory ports.

Example fake:

```rust
struct FakeTelegramGateway {
    chats: Vec<Chat>,
    messages: HashMap<ChatId, Vec<Message>>,
}
```

Test:

- list messages,
- sync chat,
- retry/idempotency,
- not-found behavior,
- correct repository calls,
- sync checkpoint behavior.

These tests must not connect to Telegram.

## Persistence Integration Tests

Use temporary SQLite databases.

Test:

- migrations,
- message upsert,
- edited messages,
- soft deletion,
- `(chat_id, message_id)` uniqueness,
- FTS indexing,
- pagination,
- transactions.

## REST Tests

Test Axum router using application fakes.

Test:

- status codes,
- JSON schema,
- validation,
- error mapping,
- query parameter parsing.

REST tests must not require Telegram or a real SQLite database unless specifically testing full integration.

## Telegram Adapter Tests

Mapper tests should use fixtures/sample Telegram structures where practical.

Real Telegram integration tests should be optional/manual because they require credentials and network access.

---

# 37. Development Phases

The AI agent should implement the project in incremental phases.

Do not attempt everything in a single giant change.

## Phase 1 — Skeleton and Domain

Implement:

- Cargo project.
- module structure.
- config skeleton.
- domain IDs.
- `Chat`.
- `Sender`.
- `Message`.
- `Attachment`.
- `MessageEvent`.
- pagination value objects.
- application port traits.

Acceptance:

- compiles,
- no Telegram/SQL/Axum dependency leaks into domain/application core.

## Phase 2 — SQLite Persistence

Implement:

- SQLx setup.
- migrations.
- SQLite pragmas.
- repositories.
- repository integration tests.
- FTS5.

Acceptance:

- upsert/list/get/search works,
- edited message works,
- soft delete works,
- migrations work from empty DB,
- FTS tests pass.

## Phase 3 — Application Use Cases

Implement:

- `IngestMessageEvent`.
- `GetMessage`.
- `ListMessages`.
- `SearchMessages`.
- `ListChats`.
- `GetChat`.
- synchronization state models.

Use fake ports for tests.

Acceptance:

- application layer is fully testable without Telegram/Axum/SQLite.

## Phase 4 — CLI

Implement:

- clap hierarchy.
- list/search message commands.
- chat listing.
- status.
- output formatting.
- OpenAPI command may be added after REST phase.

Acceptance:

- CLI calls application use cases,
- no SQL/grammers logic in CLI handlers.

## Phase 5 — REST + OpenAPI

Implement:

- Axum router.
- REST DTOs.
- API error model.
- routes.
- utoipa schemas.
- OpenAPI JSON/YAML.
- Swagger UI optional.

Acceptance:

- REST and CLI share application use cases,
- `/openapi.yml` works,
- `/openapi.json` works,
- router tests pass.

## Phase 6 — Telegram Authentication + Gateway

Implement:

- grammers connection.
- session persistence.
- interactive user login.
- Telegram gateway.
- chat mapping.
- message mapping.

Acceptance:

- can authenticate using a real user account,
- session survives restart,
- can list Telegram dialogs through the gateway.

## Phase 7 — Historical Synchronization

Implement:

- chat refresh.
- `SyncChat`.
- `SyncAllChats`.
- batch history fetch.
- persistent checkpoints.
- rate-limit handling.
- progress logging.

Acceptance:

- sync can stop/restart without corrupting state,
- duplicate synchronization is idempotent.

## Phase 8 — Real-Time Collection

Implement:

- Telegram update listener.
- normalized `MessageEvent`.
- bounded ingestion channel.
- ingestion worker.
- edit/delete handling.
- graceful shutdown.

Acceptance:

- new Telegram messages appear in SQLite,
- edits update existing rows,
- deletions mark messages deleted,
- REST immediately sees persisted results.

## Phase 9 — Production Hardening

Implement:

- health/status improvements,
- request IDs,
- HTTP tracing,
- CORS if needed,
- timeouts,
- config validation,
- shutdown tests,
- README,
- generated `openapi.yml`,
- secure defaults.

Acceptance:

- `cargo test` succeeds,
- `cargo clippy` succeeds,
- `cargo fmt --check` succeeds,
- fresh setup instructions work.

---

# 38. REST / CLI Shared Behavior Examples

## List Messages

REST:

```http
GET /api/v1/messages?chat_id=-100123&limit=50
```

CLI:

```bash
telegram-archive messages list --chat-id -100123 --limit 50
```

Both map into:

```rust
ListMessagesQuery
```

and execute:

```rust
ListMessages::execute(...)
```

## Search

REST:

```http
GET /api/v1/messages/search?q=rust&chat_id=-100123
```

CLI:

```bash
telegram-archive messages search rust --chat-id -100123
```

Both map into:

```rust
SearchMessagesQuery
```

and execute:

```rust
SearchMessages::execute(...)
```

## Synchronization

REST:

```http
POST /api/v1/chats/-100123/sync
```

CLI:

```bash
telegram-archive sync chat -100123
```

Both call the same `SyncChat` use case.

---

# 39. Coding Rules for the AI Agent

The implementation agent must follow these rules.

## Rule 1

Do not put business logic in REST handlers.

## Rule 2

Do not put business logic in CLI handlers.

## Rule 3

Do not expose SQLx types outside infrastructure.

## Rule 4

Do not expose grammers types outside infrastructure.

## Rule 5

Do not make application use cases depend on Axum or clap.

## Rule 6

Do not introduce a repository abstraction that is never consumed by the application.

Every abstraction should correspond to an actual boundary.

## Rule 7

Prefer explicit code over generalized frameworks.

## Rule 8

Do not overuse generics.

If generic service types make the dependency graph unreadable, prefer:

```rust
Arc<dyn MessageRepository>
```

or another appropriately scoped trait object.

## Rule 9

Keep functions reasonably small, but do not split code solely to satisfy arbitrary line counts.

## Rule 10

Every significant new use case should have application-level tests.

## Rule 11

Database behavior should be tested against real SQLite, not mocked SQL.

## Rule 12

All migrations must be checked into the repository.

## Rule 13

OpenAPI must be generated from Rust API definitions.

## Rule 14

Do not manually duplicate API business logic between REST and CLI.

## Rule 15

Do not add infrastructure not justified by current requirements.

---

# 40. Dependency Suggestions

The exact versions should be selected at implementation time and pinned in `Cargo.lock`.

Expected dependencies include:

```toml
[dependencies]
tokio = { version = "1", features = ["full"] }

axum = "..."
tower = "..."
tower-http = { version = "...", features = ["trace", "cors", "compression-full", "request-id"] }

clap = { version = "4", features = ["derive"] }

serde = { version = "1", features = ["derive"] }
serde_json = "1"
serde_yaml = "..."

utoipa = { version = "...", features = ["axum_extras", "chrono"] }
utoipa-swagger-ui = { version = "...", features = ["axum"] }

sqlx = {
    version = "...",
    features = [
        "runtime-tokio",
        "sqlite",
        "migrate",
        "chrono"
    ]
}

async-trait = "..."
chrono = { version = "0.4", features = ["serde"] }

thiserror = "2"
anyhow = "1"

tracing = "0.1"
tracing-subscriber = { version = "...", features = ["env-filter"] }

tokio-util = "..."

grammers-client = "..."
grammers-session = "..."
```

Do not blindly copy versions from this document.

Use mutually compatible current versions and pin them through Cargo.

---

# 41. Suggested Main Runtime Flow

Server mode:

```text
main
 ↓
load config
 ↓
bootstrap
 ├── sqlite
 ├── migrations
 ├── repositories
 ├── Telegram client
 ├── application services
 ├── ingestion queue
 ├── REST router
 └── background workers
 ↓
start
 ├── REST server
 ├── Telegram listener
 ├── ingestion worker
 └── sync coordinator
 ↓
wait for shutdown
 ↓
graceful stop
```

---

# 42. Future Extension Points

The initial architecture should make these additions straightforward.

## Qdrant / Semantic Search

Add application port:

```rust
pub trait SemanticIndex {
    ...
}
```

Infrastructure:

```text
QdrantSemanticIndex
```

Worker:

```text
Message persisted
   ↓
embedding/index worker
```

Do not place Qdrant logic in `MessageRepository`.

## Media Download

Add:

```text
AttachmentStorage
TelegramMediaGateway
DownloadAttachment use case
```

Possible storage:

- filesystem,
- S3/MinIO-compatible service.

## PostgreSQL

If SQLite becomes inadequate, add:

```text
PostgresMessageRepository
PostgresChatRepository
...
```

Application APIs do not change.

## New Interfaces

Possible:

- gRPC,
- WebSocket,
- Slack integration,
- desktop UI.

They call the same application layer.

---

# 43. Definition of Done for Version 1

Version 1 is complete when all of the following are true:

1. User can authenticate a Telegram user account.
2. Session survives application restart.
3. Service can enumerate accessible Telegram chats.
4. User can trigger historical synchronization.
5. Historical synchronization can resume.
6. New Telegram messages are collected in real time.
7. Edited messages update the archive.
8. Deleted messages are represented safely.
9. Messages are stored in SQLite.
10. Messages can be listed using cursor pagination.
11. Messages can be searched with FTS5.
12. Chats can be listed.
13. REST API exposes the supported query/sync operations.
14. CLI exposes corresponding application operations.
15. REST and CLI do not duplicate core business logic.
16. `/openapi.json` works.
17. `/openapi.yml` works.
18. Generated OpenAPI can be consumed by frontend/client generation tools.
19. Database migrations work on a new installation.
20. The service shuts down cleanly.
21. Important application paths have automated tests.
22. `cargo fmt --check` passes.
23. `cargo clippy` passes without significant warnings.
24. `cargo test` passes.
25. README documents setup, Telegram credentials, login, server start, CLI examples, and OpenAPI usage.

---

# 44. Final Architectural Summary

The target architecture is:

```text
                       ┌─────────────────┐
                       │      REST       │
                       │  Axum + utoipa  │
                       └────────┬────────┘
                                │
                                │ command/query
                                ▼
┌─────────────────┐      ┌─────────────────────┐
│       CLI       │─────▶│     Application     │
│      clap       │      │                     │
└─────────────────┘      │ use cases + ports   │
                         └─────────┬───────────┘
                                   │
                                   ▼
                         ┌─────────────────────┐
                         │       Domain        │
                         │                     │
                         │ Message / Chat /    │
                         │ Sender / Attachment │
                         └─────────────────────┘

             implements ports              implements ports
                    ▲                              ▲
                    │                              │
          ┌─────────┴─────────┐          ┌─────────┴──────────┐
          │     Telegram      │          │       SQLite       │
          │     grammers      │          │   SQLx + FTS5      │
          └───────────────────┘          └────────────────────┘


Telegram realtime updates
          │
          ▼
    Telegram mapper
          │
          ▼
     MessageEvent
          │
          ▼
 bounded Tokio channel
          │
          ▼
 IngestMessageEvent
          │
          ▼
     repositories


Telegram historical sync
          │
          ▼
     MessageEvent
          │
          └───────────────> same ingestion pipeline
```

The most important design property is:

> REST, CLI, Telegram background collection, and future interfaces must all reuse the same application layer instead of reimplementing functionality.

The implementation should favor clear boundaries and testability while remaining practical and relatively small.

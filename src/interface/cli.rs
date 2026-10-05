use chrono::{DateTime, Utc};
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;
use std::net::SocketAddr;
use thiserror::Error;

use crate::{
    application::services::Application,
    application::{
        ApplicationError, ListMessagesQuery, MessageFilters, MessagePage, MessageView, PageSize,
        SearchMessagesQuery, SearchSort, SenderDetail, SenderProfile, SenderQuery, SenderSort,
        SyncJob, SyncScope, TimeRange, ValidationError,
    },
    domain::{Chat, ChatId, ChatKind, IdError, MessageId, SenderId, SenderKind},
    infrastructure::persistence::sqlite::SenderRepairReport,
};

#[derive(Debug, Parser)]
#[command(
    name = "tgarchive",
    version,
    about = "Archive and query Telegram messages"
)]
pub struct Cli {
    #[arg(long, global = true, value_enum, default_value = "human")]
    pub output: OutputFormat,
    /// Load environment variables from this file instead of ./.env (must exist; real
    /// environment variables still take precedence)
    #[arg(long, global = true, value_name = "PATH")]
    pub env_file: Option<std::path::PathBuf>,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    Human,
    Json,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Chats {
        #[command(subcommand)]
        command: ChatsCommand,
    },
    Messages {
        #[command(subcommand)]
        command: MessagesCommand,
    },
    /// Senders (users, channels): search and inspect
    Senders {
        #[command(subcommand)]
        command: SendersCommand,
    },
    /// Local data repairs (no Telegram needed)
    Repair {
        #[command(subcommand)]
        command: RepairCommand,
    },
    Status,
    Serve {
        #[arg(long)]
        bind: Option<SocketAddr>,
        #[arg(long, help = "Serve archive queries without opening Telegram")]
        query_only: bool,
    },
    Openapi {
        #[arg(long, value_enum, default_value = "json")]
        format: OpenApiCliFormat,
    },
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },
    Sync {
        #[command(subcommand)]
        command: SyncCommand,
    },
    Db {
        #[command(subcommand)]
        command: DatabaseCommand,
    },
    /// Full-text search index maintenance
    Search {
        #[command(subcommand)]
        command: SearchCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum SendersCommand {
    /// Find senders by display name / @username substring (Chinese substrings work) or ID
    Search {
        /// Text, `@username` (exact) or numeric ID
        text: String,
        #[arg(long, value_enum, default_value = "messages")]
        sort: SenderSortArg,
        #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u16).range(1..=1000))]
        limit: u16,
        /// Count deleted messages too
        #[arg(long)]
        include_deleted: bool,
    },
    /// Show one sender with its per-chat breakdown
    Get {
        /// Numeric ID, `@username`, a unique display-name substring, or `me` (the bound account)
        sender: String,
        #[arg(long)]
        include_deleted: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum RepairCommand {
    /// Fill NULL message senders that the archive itself can answer (channel posts get the
    /// channel). Private chats are never guessed: re-fetch them with `sync chat <ID> --refetch`.
    Senders {
        /// Only report what would change
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SenderSortArg {
    Messages,
    LastMessage,
    Name,
}

#[derive(Debug, Subcommand)]
pub enum SearchCommand {
    /// Re-tokenize every message and rebuild the search index (progress on stderr). Needed after
    /// upgrading an existing archive (`db init` does it automatically) or when the index state
    /// is not `ready`.
    RebuildIndex,
    /// Show the search index state and how many messages are indexed
    Status,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SearchSortArg {
    Relevance,
    Time,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OpenApiCliFormat {
    Json,
    Yaml,
}

#[derive(Debug, Subcommand)]
pub enum ChatsCommand {
    /// List known chats and whether each is tracked (collected)
    List {
        #[arg(long, help = "Only show tracked chats")]
        tracked: bool,
    },
    /// Fetch chat metadata from Telegram; never collects messages or changes tracking
    Refresh,
    /// Start collecting a known chat (idempotent)
    Track {
        #[arg(allow_hyphen_values = true)]
        chat_id: i64,
        /// After tracking, also fetch the chat's history now (like `sync chat`); needs Telegram
        /// credentials. Tracking is saved first even if the sync cannot run or is rate limited.
        #[arg(long)]
        backfill: bool,
    },
    /// Stop collecting a chat; stored messages are kept (idempotent)
    Untrack {
        #[arg(allow_hyphen_values = true)]
        chat_id: i64,
    },
    Get {
        #[arg(allow_hyphen_values = true)]
        chat_id: i64,
    },
}

#[derive(Debug, Subcommand)]
pub enum AuthCommand {
    Login {
        #[arg(long)]
        phone: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum SyncCommand {
    Chat {
        #[arg(allow_hyphen_values = true)]
        chat_id: i64,
        /// Re-download the whole history and fill NULL sender/post-author/forward/attachment
        /// metadata of archived messages (text and edits are untouched). Resumable.
        #[arg(long)]
        refetch: bool,
    },
    All,
}

#[derive(Debug, Subcommand)]
pub enum MessagesCommand {
    List(MessageFilterArgs),
    Get {
        #[arg(allow_hyphen_values = true)]
        chat_id: i64,
        #[arg(value_parser = parse_message_id)]
        message_id: MessageId,
        /// Also show the message if it was deleted.
        #[arg(long)]
        include_deleted: bool,
    },
    /// Full-text search (plain text; Chinese substrings supported)
    Search {
        query: String,
        /// `relevance`: `--before` takes the previous page's next_cursor (no `--after`);
        /// `time`: newest first with keyset cursors.
        #[arg(long, value_enum, default_value = "relevance")]
        sort: SearchSortArg,
        #[command(flatten)]
        filters: MessageFilterArgs,
    },
}

#[derive(Debug, Args, Clone)]
pub struct MessageFilterArgs {
    #[arg(long, allow_hyphen_values = true)]
    pub chat_id: Option<i64>,
    #[arg(long, allow_hyphen_values = true)]
    pub sender_id: Option<i64>,
    /// Sender: numeric ID, `@username`, a display-name substring matching exactly one sender, or
    /// `me` (the bound account). Ambiguous names list the candidates and fail.
    #[arg(long, conflicts_with = "sender_id")]
    pub sender: Option<String>,
    /// Exact channel post signature.
    #[arg(long)]
    pub post_author: Option<String>,
    #[arg(long)]
    pub before: Option<String>,
    #[arg(long)]
    pub after: Option<String>,
    #[arg(long, value_parser = parse_timestamp)]
    pub from: Option<DateTime<Utc>>,
    #[arg(long, value_parser = parse_timestamp)]
    pub to: Option<DateTime<Utc>>,
    #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u16).range(1..=1000))]
    pub limit: u16,
    /// Also include messages deleted on Telegram.
    #[arg(long)]
    pub include_deleted: bool,
}

#[derive(Debug, Subcommand)]
pub enum DatabaseCommand {
    Init {
        #[arg(long)]
        database_url: Option<String>,
    },
}

#[derive(Debug, Error)]
pub enum CliError {
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("database error: {0}")]
    Database(String),
    #[error("REST server error: {0}")]
    Server(String),
    #[error("OpenAPI export error: {0}")]
    OpenApi(String),
    #[error("Telegram operation failed: {0}")]
    Telegram(String),
    #[error("chat {0} not found; run `chats refresh` to fetch your chats from Telegram first")]
    ChatNotFound(i64),
    #[error("no sender matches {0:?}")]
    SenderNotFound(String),
    #[error("{0}")]
    AmbiguousSender(String),
    #[error("sync job failed: {0}")]
    SyncFailed(String),
    #[error("{0}")]
    Application(#[from] ApplicationError),
    #[error("{0}")]
    Identifier(#[from] IdError),
    #[error("{0}")]
    Cursor(#[from] crate::interface::cursor::CursorError),
    #[error("{0}")]
    Validation(#[from] ValidationError),
    #[error("could not encode output: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Serialize)]
struct MessagePageOutput {
    items: Vec<MessageView>,
    has_more: bool,
    next_cursor: Option<String>,
}

#[derive(Serialize)]
struct StatusOutput<'a> {
    collector: CollectorOutput<'a>,
    sync_jobs: &'a [crate::application::SyncJob],
    search_index: &'a crate::application::SearchIndexStatus,
}

#[derive(Serialize)]
struct CollectorOutput<'a> {
    state: &'a str,
    detail: &'a Option<String>,
}

pub enum PreparedInvocation {
    RebuildSearchIndex {
        output: OutputFormat,
    },
    InitializeDatabase {
        output: OutputFormat,
        database_url: Option<String>,
    },
    Query {
        output: OutputFormat,
        command: PreparedCommand,
    },
    Serve {
        bind: Option<SocketAddr>,
        query_only: bool,
    },
    OpenApi {
        format: OpenApiCliFormat,
    },
    AuthLogin {
        phone: Option<String>,
    },
    RefreshChats {
        output: OutputFormat,
    },
    SetTracking {
        chat_id: ChatId,
        tracked: bool,
        backfill: bool,
        output: OutputFormat,
    },
    Sync {
        scope: SyncScope,
        output: OutputFormat,
        refetch: bool,
    },
    RepairSenders {
        dry_run: bool,
        output: OutputFormat,
    },
}

pub enum PreparedCommand {
    ChatsList {
        tracked_only: bool,
    },
    ChatGet(ChatId),
    /// The optional text is a `--sender` reference resolved against the archive when run.
    MessagesList(ListMessagesQuery, Option<String>),
    MessageGet(ChatId, MessageId, bool),
    MessagesSearch(SearchMessagesQuery, Option<String>),
    SendersSearch(SenderQuery),
    SenderGet(String, bool),
    Status,
    SearchStatus,
}

pub fn prepare(cli: Cli) -> Result<PreparedInvocation, CliError> {
    let Cli {
        command, output, ..
    } = cli;
    match command {
        Command::Db {
            command: DatabaseCommand::Init { database_url },
        } => Ok(PreparedInvocation::InitializeDatabase {
            output,
            database_url,
        }),
        Command::Search {
            command: SearchCommand::RebuildIndex,
        } => Ok(PreparedInvocation::RebuildSearchIndex { output }),
        Command::Search {
            command: SearchCommand::Status,
        } => Ok(PreparedInvocation::Query {
            output,
            command: PreparedCommand::SearchStatus,
        }),
        Command::Serve { bind, query_only } => Ok(PreparedInvocation::Serve { bind, query_only }),
        Command::Openapi { format } => Ok(PreparedInvocation::OpenApi { format }),
        Command::Auth {
            command: AuthCommand::Login { phone },
        } => Ok(PreparedInvocation::AuthLogin { phone }),
        Command::Chats {
            command: ChatsCommand::Refresh,
        } => Ok(PreparedInvocation::RefreshChats { output }),
        Command::Chats {
            command: ChatsCommand::Track { chat_id, backfill },
        } => Ok(PreparedInvocation::SetTracking {
            chat_id: ChatId::from_marked(chat_id)?,
            tracked: true,
            backfill,
            output,
        }),
        Command::Chats {
            command: ChatsCommand::Untrack { chat_id },
        } => Ok(PreparedInvocation::SetTracking {
            chat_id: ChatId::from_marked(chat_id)?,
            tracked: false,
            backfill: false,
            output,
        }),
        Command::Sync { command } => {
            let (scope, refetch) = match command {
                SyncCommand::Chat { chat_id, refetch } => {
                    (SyncScope::Chat(ChatId::from_marked(chat_id)?), refetch)
                }
                SyncCommand::All => (SyncScope::All, false),
            };
            Ok(PreparedInvocation::Sync {
                scope,
                output,
                refetch,
            })
        }
        Command::Repair {
            command: RepairCommand::Senders { dry_run },
        } => Ok(PreparedInvocation::RepairSenders { dry_run, output }),
        command => Ok(PreparedInvocation::Query {
            output,
            command: prepare_query(command)?,
        }),
    }
}

fn prepare_query(command: Command) -> Result<PreparedCommand, CliError> {
    match command {
        Command::Chats { command } => match command {
            ChatsCommand::List { tracked } => Ok(PreparedCommand::ChatsList {
                tracked_only: tracked,
            }),
            ChatsCommand::Refresh | ChatsCommand::Track { .. } | ChatsCommand::Untrack { .. } => {
                unreachable!("chat refresh/tracking are handled before query preparation")
            }
            ChatsCommand::Get { chat_id } => {
                Ok(PreparedCommand::ChatGet(ChatId::from_marked(chat_id)?))
            }
        },
        Command::Messages { command } => match command {
            MessagesCommand::List(mut args) => {
                let sender = args.sender.take();
                Ok(PreparedCommand::MessagesList(list_query(args)?, sender))
            }
            MessagesCommand::Get {
                chat_id,
                message_id,
                include_deleted,
            } => Ok(PreparedCommand::MessageGet(
                ChatId::from_marked(chat_id)?,
                message_id,
                include_deleted,
            )),
            MessagesCommand::Search {
                query,
                sort,
                mut filters,
            } => {
                let sender = filters.sender.take();
                Ok(PreparedCommand::MessagesSearch(
                    search_query(query, sort, filters)?,
                    sender,
                ))
            }
        },
        Command::Senders { command } => match command {
            SendersCommand::Search {
                text,
                sort,
                limit,
                include_deleted,
            } => Ok(PreparedCommand::SendersSearch(SenderQuery {
                text: Some(text),
                sort: match sort {
                    SenderSortArg::Messages => SenderSort::Messages,
                    SenderSortArg::LastMessage => SenderSort::LastMessage,
                    SenderSortArg::Name => SenderSort::Name,
                },
                limit: PageSize::new(limit)?,
                offset: 0,
                include_deleted,
            })),
            SendersCommand::Get {
                sender,
                include_deleted,
            } => Ok(PreparedCommand::SenderGet(sender, include_deleted)),
        },
        Command::Status => Ok(PreparedCommand::Status),
        Command::Serve { .. }
        | Command::Openapi { .. }
        | Command::Auth { .. }
        | Command::Repair { .. }
        | Command::Sync { .. } => {
            unreachable!("non-query commands are handled during preparation")
        }
        Command::Db { .. } => unreachable!("database initialization is handled separately"),
        Command::Search { .. } => unreachable!("search index commands are handled separately"),
    }
}

pub async fn execute_prepared(
    command: PreparedCommand,
    output: OutputFormat,
    app: &Application,
) -> Result<String, CliError> {
    match command {
        PreparedCommand::ChatsList { tracked_only } => {
            let chats = app.list_chats(tracked_only).await?;
            render_chats(output, &chats)
        }
        PreparedCommand::ChatGet(chat_id) => {
            let chat = app.get_chat(chat_id).await?;
            if output == OutputFormat::Json {
                json(&chat)
            } else {
                Ok(human_chat(&chat))
            }
        }
        PreparedCommand::MessagesList(mut query, sender) => {
            if let Some(sender) = sender {
                query.filters.sender_id = Some(resolve_sender(app, &sender).await?);
            }
            render_message_page(output, app.list_messages(query).await?)
        }
        PreparedCommand::SendersSearch(query) => {
            let page = app.search_senders(query).await?;
            render_senders(output, &page.items)
        }
        PreparedCommand::SenderGet(sender, include_deleted) => {
            let id = resolve_sender(app, &sender).await?;
            let detail = app.sender_detail(id, include_deleted).await?;
            render_sender_detail(output, &detail)
        }
        PreparedCommand::MessageGet(chat_id, message_id, include_deleted) => {
            let message = app
                .get_message(chat_id, message_id, include_deleted)
                .await?;
            if output == OutputFormat::Json {
                json(&message)
            } else {
                Ok(human_message(&message))
            }
        }
        PreparedCommand::MessagesSearch(mut query, sender) => {
            if let Some(sender) = sender {
                query.filters.sender_id = Some(resolve_sender(app, &sender).await?);
            }
            render_message_page(output, app.search_messages(query).await?)
        }
        PreparedCommand::SearchStatus => {
            let status = app.sync_status().await?.search_index;
            if output == OutputFormat::Json {
                json(&status)
            } else {
                Ok(human_search_index(&status))
            }
        }
        PreparedCommand::Status => {
            let status = app.sync_status().await?;
            if output == OutputFormat::Json {
                let state = component_state(status.collector.state);
                json(&StatusOutput {
                    collector: CollectorOutput {
                        state,
                        detail: &status.collector.detail,
                    },
                    sync_jobs: &status.sync_jobs,
                    search_index: &status.search_index,
                })
            } else {
                let mut lines = vec![format!(
                    "collector\t{}",
                    component_state(status.collector.state)
                )];
                if let Some(detail) = status.collector.detail {
                    lines.push(format!("collector detail\t{detail}"));
                }
                lines.push(human_search_index(&status.search_index));
                if status.sync_jobs.is_empty() {
                    lines.push("No sync jobs.".into());
                } else {
                    lines.extend(
                        status
                            .sync_jobs
                            .iter()
                            .map(|job| format!("sync {}\t{:?}", job.id, job.state)),
                    );
                }
                Ok(lines.join("\n"))
            }
        }
    }
}

pub async fn execute_set_tracking(
    chat_id: ChatId,
    tracked: bool,
    output: OutputFormat,
    app: &Application,
) -> Result<String, CliError> {
    let result = if tracked {
        app.track_chat(chat_id).await
    } else {
        app.untrack_chat(chat_id).await
    };
    let chat = result.map_err(|error| match error {
        ApplicationError::NotFound => CliError::ChatNotFound(chat_id.get()),
        other => other.into(),
    })?;
    if output == OutputFormat::Json {
        json(&chat)
    } else if tracked {
        Ok(format!("Tracking chat {}.", chat.id.get()))
    } else {
        Ok(format!(
            "Stopped tracking chat {}; stored messages are kept.",
            chat.id.get()
        ))
    }
}

pub fn render_chats(output: OutputFormat, chats: &[Chat]) -> Result<String, CliError> {
    if output == OutputFormat::Json {
        json(&chats)
    } else if chats.is_empty() {
        Ok("No chats.".into())
    } else {
        Ok(chats.iter().map(human_chat).collect::<Vec<_>>().join("\n"))
    }
}

pub fn render_sync_job(output: OutputFormat, job: &SyncJob) -> Result<String, CliError> {
    if output == OutputFormat::Json {
        json(job)
    } else {
        let mut text = format!("sync {}\t{:?}", job.id, job.state);
        if let Some(error) = &job.summary_error {
            text.push_str("\nerror\t");
            text.push_str(error);
        }
        Ok(text)
    }
}

/// Resolves a `--sender` reference to exactly one sender; ambiguity lists the candidates.
async fn resolve_sender(app: &Application, spec: &str) -> Result<SenderId, CliError> {
    let spec = spec.trim();
    if spec.eq_ignore_ascii_case("me") {
        return app.bound_account().await?.ok_or_else(|| {
            CliError::InvalidInput(
                "the archive is not bound to a Telegram account yet (run a sync or `serve` once)"
                    .into(),
            )
        });
    }
    if let Ok(id) = spec.parse::<i64>() {
        return Ok(SenderId::from_marked(id)?);
    }
    let page = app
        .search_senders(SenderQuery {
            text: Some(spec.to_owned()),
            sort: SenderSort::Messages,
            limit: PageSize::new(20)?,
            offset: 0,
            include_deleted: true,
        })
        .await?;
    match page.items.as_slice() {
        [] => Err(CliError::SenderNotFound(spec.to_owned())),
        [only] => Ok(only.id),
        candidates => Err(CliError::AmbiguousSender(format!(
            "sender {spec:?} is ambiguous; use one of these IDs:\n{}",
            candidates
                .iter()
                .map(human_sender)
                .collect::<Vec<_>>()
                .join("\n")
        ))),
    }
}

fn sender_kind_name(kind: SenderKind) -> &'static str {
    match kind {
        SenderKind::User => "user",
        SenderKind::Chat => "chat",
        SenderKind::Channel => "channel",
        SenderKind::Unknown => "unknown",
    }
}

fn human_sender(sender: &SenderProfile) -> String {
    format!(
        "{}\t{}\t{}\t{}\t{} messages\t{} chats\t{}{}",
        sender.id.get(),
        sender_kind_name(sender.kind),
        sender.display_name.as_deref().unwrap_or(""),
        sender
            .username
            .as_deref()
            .map(|name| format!("@{name}"))
            .unwrap_or_default(),
        sender.message_count,
        sender.chat_count,
        sender
            .last_message_at
            .map(|time| time.to_rfc3339())
            .unwrap_or_default(),
        if sender.is_self { "\tself" } else { "" }
    )
}

#[derive(Serialize)]
struct SenderOutput<'a> {
    id: i64,
    kind: &'static str,
    display_name: &'a Option<String>,
    username: &'a Option<String>,
    message_count: u64,
    chat_count: u64,
    first_message_at: Option<DateTime<Utc>>,
    last_message_at: Option<DateTime<Utc>>,
    is_self: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    chats: Option<Vec<SenderChatOutput<'a>>>,
}

#[derive(Serialize)]
struct SenderChatOutput<'a> {
    chat_id: i64,
    title: &'a Option<String>,
    kind: String,
    message_count: u64,
    last_message_at: Option<DateTime<Utc>>,
}

fn sender_output<'a>(
    sender: &'a SenderProfile,
    chats: Option<Vec<SenderChatOutput<'a>>>,
) -> SenderOutput<'a> {
    SenderOutput {
        id: sender.id.get(),
        kind: sender_kind_name(sender.kind),
        display_name: &sender.display_name,
        username: &sender.username,
        message_count: sender.message_count,
        chat_count: sender.chat_count,
        first_message_at: sender.first_message_at,
        last_message_at: sender.last_message_at,
        is_self: sender.is_self,
        chats,
    }
}

fn render_senders(output: OutputFormat, senders: &[SenderProfile]) -> Result<String, CliError> {
    if output == OutputFormat::Json {
        json(
            &senders
                .iter()
                .map(|s| sender_output(s, None))
                .collect::<Vec<_>>(),
        )
    } else if senders.is_empty() {
        Ok("No senders.".into())
    } else {
        Ok(senders
            .iter()
            .map(human_sender)
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

fn render_sender_detail(output: OutputFormat, detail: &SenderDetail) -> Result<String, CliError> {
    let kind = |kind: ChatKind| format!("{kind:?}").to_lowercase();
    if output == OutputFormat::Json {
        let chats = detail
            .chats
            .iter()
            .map(|chat| SenderChatOutput {
                chat_id: chat.chat_id.get(),
                title: &chat.title,
                kind: kind(chat.kind),
                message_count: chat.message_count,
                last_message_at: chat.last_message_at,
            })
            .collect();
        json(&sender_output(&detail.profile, Some(chats)))
    } else {
        let mut lines = vec![human_sender(&detail.profile)];
        lines.extend(detail.chats.iter().map(|chat| {
            format!(
                "  chat {}\t{}\t{}\t{} messages\t{}",
                chat.chat_id.get(),
                kind(chat.kind),
                chat.title.as_deref().unwrap_or(""),
                chat.message_count,
                chat.last_message_at
                    .map(|time| time.to_rfc3339())
                    .unwrap_or_default()
            )
        }));
        Ok(lines.join("\n"))
    }
}

pub fn render_sender_repair(
    output: OutputFormat,
    report: &SenderRepairReport,
) -> Result<String, CliError> {
    if output == OutputFormat::Json {
        return json(report);
    }
    let verb = if report.dry_run { "would fix" } else { "fixed" };
    let mut lines = vec![format!(
        "rule channel_post (sender = the channel): {verb} {} rows in {} chats",
        report.channel_posts_total,
        report.channel_posts.len()
    )];
    lines.extend(report.channel_posts.iter().map(|chat| {
        format!(
            "  {}\t{}\t{} rows",
            chat.chat_id,
            chat.title.as_deref().unwrap_or(""),
            chat.rows
        )
    }));
    lines.push(format!(
        "still without sender: {} rows in {} chats (private chats cannot be told apart without \
         data; re-fetch them with `tgarchive sync chat <ID> --refetch`)",
        report.unresolved_total,
        report.unresolved.len()
    ));
    lines.extend(report.unresolved.iter().map(|chat| {
        format!(
            "  {}\t{}\t{}\t{} rows",
            chat.chat_id,
            chat.kind,
            chat.title.as_deref().unwrap_or(""),
            chat.rows
        )
    }));
    Ok(lines.join("\n"))
}

fn list_query(args: MessageFilterArgs) -> Result<ListMessagesQuery, CliError> {
    let (filters, before, after, page_size) = convert_filters(args)?;
    let query = ListMessagesQuery {
        filters,
        before,
        after,
        page_size,
    };
    query.validate()?;
    Ok(query)
}

fn search_query(
    text: String,
    sort: SearchSortArg,
    mut args: MessageFilterArgs,
) -> Result<SearchMessagesQuery, CliError> {
    let (sort, offset) = match sort {
        SearchSortArg::Time => (SearchSort::Time, 0),
        SearchSortArg::Relevance => {
            if args.after.is_some() {
                return Err(CliError::InvalidInput(
                    "--after is only valid with --sort time".into(),
                ));
            }
            let offset = args
                .before
                .take()
                .map(|value| crate::interface::cursor::decode_offset(&value))
                .transpose()?
                .unwrap_or(0);
            (SearchSort::Relevance, offset)
        }
    };
    let (filters, before, after, page_size) = convert_filters(args)?;
    let query = SearchMessagesQuery {
        text,
        filters,
        before,
        after,
        page_size,
        sort,
        offset,
    };
    query.validate()?;
    Ok(query)
}

fn convert_filters(
    args: MessageFilterArgs,
) -> Result<
    (
        MessageFilters,
        Option<crate::application::MessageCursor>,
        Option<crate::application::MessageCursor>,
        PageSize,
    ),
    CliError,
> {
    let filters = MessageFilters {
        chat_id: args.chat_id.map(ChatId::from_marked).transpose()?,
        sender_id: args.sender_id.map(SenderId::from_marked).transpose()?,
        post_author: args.post_author,
        time_range: TimeRange::new(args.from, args.to)?,
        include_deleted: args.include_deleted,
    };
    let before = args
        .before
        .as_deref()
        .map(crate::interface::cursor::decode)
        .transpose()?;
    let after = args
        .after
        .as_deref()
        .map(crate::interface::cursor::decode)
        .transpose()?;
    Ok((filters, before, after, PageSize::new(args.limit)?))
}

fn parse_message_id(value: &str) -> Result<MessageId, String> {
    value
        .parse::<i64>()
        .map_err(|_| "message ID must be an integer".to_owned())
        .and_then(|value| MessageId::new(value).map_err(|error| error.to_string()))
}

fn parse_timestamp(value: &str) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|error| format!("timestamp must be RFC 3339: {error}"))
}

fn render_message_page(output: OutputFormat, page: MessagePage) -> Result<String, CliError> {
    let next_cursor = match (page.next_cursor.as_ref(), page.next_offset) {
        (Some(cursor), _) => Some(crate::interface::cursor::encode(cursor)?),
        (None, Some(offset)) => Some(crate::interface::cursor::encode_offset(offset)?),
        (None, None) => None,
    };
    if output == OutputFormat::Json {
        json(&MessagePageOutput {
            items: page.items,
            has_more: page.has_more,
            next_cursor,
        })
    } else if page.items.is_empty() && !page.has_more {
        Ok("No messages.".into())
    } else {
        let mut lines = page.items.iter().map(human_message).collect::<Vec<_>>();
        lines.push(format!("has_more\t{}", page.has_more));
        if let Some(cursor) = next_cursor {
            lines.push(format!("next_cursor\t{cursor}"));
        }
        Ok(lines.join("\n"))
    }
}

fn render_text(output: OutputFormat, text: &str, json_value: &str) -> Result<String, CliError> {
    if output == OutputFormat::Json {
        Ok(json_value.to_owned())
    } else {
        Ok(text.to_owned())
    }
}

pub fn initialized_output(output: OutputFormat) -> Result<String, CliError> {
    render_text(
        output,
        "Initialized archive database.",
        "{\"initialized\":true}",
    )
}

fn json(value: &impl Serialize) -> Result<String, CliError> {
    serde_json::to_string(value).map_err(Into::into)
}

fn human_chat(chat: &Chat) -> String {
    format!(
        "{}\t{:?}\t{}\t{}",
        chat.id.get(),
        chat.kind,
        chat.title.as_deref().unwrap_or(""),
        if chat.tracked { "tracked" } else { "untracked" }
    )
}

fn human_message(message: &MessageView) -> String {
    let sender = message
        .sender
        .as_ref()
        .and_then(|sender| {
            sender
                .display_name
                .as_deref()
                .or(sender.username.as_deref())
        })
        .map(str::to_owned)
        .or_else(|| message.sender_id.map(|id| id.get().to_string()))
        .unwrap_or_default();
    format!(
        "{}\t{}\t{}\t{}\t{}{}",
        message.chat_id.get(),
        message.id.get(),
        message.timestamp.to_rfc3339(),
        sender,
        if message.is_deleted() {
            "[deleted] "
        } else {
            ""
        },
        message.text.as_deref().unwrap_or("")
    )
}

fn human_search_index(status: &crate::application::SearchIndexStatus) -> String {
    format!(
        "search index\t{} (version {}, {}/{} messages indexed)",
        status.state.as_str(),
        status.version,
        status.indexed,
        status.total
    )
}

fn component_state(state: crate::application::services::ComponentState) -> &'static str {
    state.as_str()
}

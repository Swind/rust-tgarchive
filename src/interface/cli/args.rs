use super::*;
use super::{parse_message_id, parse_timestamp};

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
        #[arg(
            long,
            help = "Allow a non-loopback --bind/SERVER_BIND (the API has no authentication; also TGARCHIVE_ALLOW_NON_LOOPBACK=1)"
        )]
        allow_non_loopback: bool,
    },
    /// Probe a running server's health endpoint (exit 0 when it answers 2xx); used by the Docker HEALTHCHECK
    Healthcheck {
        #[arg(
            long,
            help = "default: http://127.0.0.1:<SERVER_BIND port>/health/ready"
        )]
        url: Option<String>,
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
        /// `true`: only bots; `false`: only non-bots
        #[arg(long)]
        is_bot: Option<bool>,
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
    /// Hide messages from known bots (unknown senders are kept).
    #[arg(long)]
    pub exclude_bots: bool,
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
pub(super) struct StatusOutput<'a> {
    pub(super) collector: CollectorOutput<'a>,
    pub(super) sync_jobs: &'a [crate::application::SyncJob],
    pub(super) search_index: &'a crate::application::SearchIndexStatus,
}

#[derive(Serialize)]
pub(super) struct CollectorOutput<'a> {
    pub(super) state: &'a str,
    pub(super) detail: &'a Option<String>,
}

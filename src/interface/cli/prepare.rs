use super::*;

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
        allow_non_loopback: bool,
    },
    Healthcheck {
        url: Option<String>,
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
        Command::Serve {
            bind,
            query_only,
            allow_non_loopback,
        } => Ok(PreparedInvocation::Serve {
            bind,
            query_only,
            allow_non_loopback,
        }),
        Command::Healthcheck { url } => Ok(PreparedInvocation::Healthcheck { url }),
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

pub(super) fn prepare_query(command: Command) -> Result<PreparedCommand, CliError> {
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
                is_bot,
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
                is_bot,
            })),
            SendersCommand::Get {
                sender,
                include_deleted,
            } => Ok(PreparedCommand::SenderGet(sender, include_deleted)),
        },
        Command::Status => Ok(PreparedCommand::Status),
        Command::Serve { .. }
        | Command::Healthcheck { .. }
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

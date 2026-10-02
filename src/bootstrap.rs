use std::{future::IntoFuture, sync::Arc};

use clap::Parser;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

use crate::{
    application::{
        ArchiveWriter, PageSize, RepositoryError, SyncJob, SyncJobState, SyncRepository, SyncScope,
        ingestion_worker::{self, IngestSink},
        realtime::{ReconnectBackoff, supervise_realtime_with_status},
        services::{Application, CollectorStatusHandle, ComponentState, ComponentStatus},
        sync::{CancellationToken, SyncCoordinator, SyncEngine},
    },
    config::{Config, TelegramConfig},
    domain::Chat,
    infrastructure::persistence::sqlite::SqliteStore,
    infrastructure::telegram::{
        AuthError, LoginProgress, TelegramAdapter, realtime::AdapterRealtime,
    },
    interface::cli::{
        Cli, CliError, OpenApiCliFormat, OutputFormat, PreparedInvocation, execute_prepared,
        execute_set_tracking, initialized_output, prepare, render_chats, render_sync_job,
    },
    interface::rest::{self, OpenApiFormat},
};

/// `RUST_LOG` when set, otherwise `warn` so failures are visible by default (not only errors).
fn log_filter() -> tracing_subscriber::EnvFilter {
    tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"))
}

pub async fn run() -> Result<(), CliError> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(log_filter())
        .with_writer(std::io::stderr)
        .with_target(false)
        .try_init();

    match prepare(Cli::parse())? {
        PreparedInvocation::InitializeDatabase {
            output,
            database_url,
        } => {
            let config = Config::load(database_url);
            let store = SqliteStore::connect(&config.database_url)
                .await
                .map_err(|error| CliError::Database(error.to_string()))?;
            store.close().await;
            tracing::info!("archive database initialized");
            print_output(initialized_output(output)?);
        }
        PreparedInvocation::Query { output, command } => {
            let config = Config::load(None);
            let application = readonly_application(
                &config.database_url,
                "not tracked by the CLI; query GET /api/v1/status on the running server for realtime collector state",
            )
            .await?;
            print_output(execute_prepared(command, output, &application).await?);
        }
        PreparedInvocation::OpenApi { format } => {
            let format = match format {
                OpenApiCliFormat::Json => OpenApiFormat::Json,
                OpenApiCliFormat::Yaml => OpenApiFormat::Yaml,
            };
            let document = rest::export_openapi(format)
                .map_err(|error| CliError::OpenApi(error.to_string()))?;
            println!("{document}");
        }
        PreparedInvocation::Serve { bind, query_only } => serve(bind, query_only).await?,
        PreparedInvocation::AuthLogin { phone } => auth_login(phone).await?,
        PreparedInvocation::RefreshChats { output } => {
            let config = Config::load(None);
            let telegram = Config::telegram().map_err(CliError::InvalidInput)?;
            let adapter = open_authorized_telegram(&telegram).await?;
            let refreshed = refresh_with_adapter(&config.database_url, Arc::clone(&adapter)).await;
            let closed = adapter.shutdown().await.map_err(|_| {
                CliError::Telegram("Telegram connection did not shut down cleanly".into())
            });
            let chats = refreshed?;
            closed?;
            print_output(render_chats(output, &chats)?);
        }
        PreparedInvocation::SetTracking {
            chat_id,
            tracked,
            output,
        } => {
            let config = Config::load(None);
            let store = open_existing_store(&config.database_url).await?;
            let application = Application::new(
                store.clone(),
                store.clone(),
                store.clone(),
                store.clone(),
                None,
                ComponentStatus::disabled(),
            );
            let result = execute_set_tracking(chat_id, tracked, output, &application).await;
            store.close().await;
            print_output(result?);
        }
        PreparedInvocation::Sync { scope, output } => sync_archive(scope, output).await?,
    }
    Ok(())
}

async fn auth_login(phone: Option<String>) -> Result<(), CliError> {
    let telegram = Config::telegram().map_err(CliError::InvalidInput)?;
    telegram.validate_paths().map_err(CliError::InvalidInput)?;
    let adapter = TelegramAdapter::open(telegram.api_id, &telegram.session_file)
        .await
        .map_err(|error| CliError::Telegram(error.to_string()))?;
    let result = login_with_adapter(&telegram, &adapter, phone).await;
    let shutdown = adapter
        .shutdown()
        .await
        .map_err(|_| CliError::Telegram("Telegram connection did not shut down cleanly".into()));
    result?;
    shutdown?;
    Ok(())
}

async fn login_with_adapter(
    telegram: &TelegramConfig,
    adapter: &TelegramAdapter,
    phone: Option<String>,
) -> Result<(), CliError> {
    let authorized = adapter
        .is_authorized()
        .await
        .map_err(|_| CliError::Telegram("could not check Telegram authorization".into()))?;
    use std::io::IsTerminal;
    if login_gate(authorized, std::io::stdin().is_terminal())? == LoginGate::AlreadyAuthorized {
        println!("Telegram session is already authorized.");
        return Ok(());
    }

    let phone = match phone {
        Some(phone) if !phone.trim().is_empty() => phone,
        Some(_) => {
            return Err(CliError::InvalidInput(
                "phone number cannot be empty".into(),
            ));
        }
        None => prompt_phone()?,
    };
    let challenge = adapter
        .request_login_code(&phone, &telegram.api_hash)
        .await
        .map_err(auth_error)?;
    let code = prompt_secret("Telegram login code: ")?;
    match adapter
        .sign_in(challenge, &code)
        .await
        .map_err(auth_error)?
    {
        LoginProgress::Authenticated => println!("Telegram session authorized."),
        LoginProgress::PasswordRequired(challenge) => {
            if let Some(hint) = challenge.hint.as_deref().filter(|hint| !hint.is_empty()) {
                eprintln!("Telegram requires two-factor authentication (hint: {hint}).");
            }
            let password = prompt_secret("Telegram two-factor password: ")?;
            adapter
                .check_password(*challenge, password)
                .await
                .map_err(auth_error)?;
            println!("Telegram session authorized.");
        }
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
enum LoginGate {
    AlreadyAuthorized,
    PromptForCredentials,
}

/// A terminal is only required when credentials must actually be entered.
fn login_gate(authorized: bool, interactive: bool) -> Result<LoginGate, CliError> {
    if authorized {
        Ok(LoginGate::AlreadyAuthorized)
    } else if interactive {
        Ok(LoginGate::PromptForCredentials)
    } else {
        Err(CliError::Telegram(
            "auth login requires an interactive terminal for hidden credential input".into(),
        ))
    }
}

fn prompt_phone() -> Result<String, CliError> {
    use std::io::Write;
    print!("Phone number: ");
    std::io::stdout()
        .flush()
        .map_err(|_| CliError::Telegram("could not prompt for phone number".into()))?;
    let mut phone = String::new();
    std::io::stdin()
        .read_line(&mut phone)
        .map_err(|_| CliError::Telegram("could not read phone number".into()))?;
    let phone = phone.trim().to_owned();
    if phone.is_empty() {
        return Err(CliError::InvalidInput(
            "phone number is required; interactive input was empty".into(),
        ));
    }
    Ok(phone)
}

fn prompt_secret(prompt: &str) -> Result<String, CliError> {
    rpassword::prompt_password(prompt)
        .map_err(|_| CliError::Telegram("hidden input requires an interactive terminal".into()))
}

fn auth_error(error: AuthError) -> CliError {
    let message = match error {
        AuthError::InvalidCode => "Telegram rejected the login code".to_owned(),
        AuthError::InvalidPassword => "Telegram rejected the two-factor password".to_owned(),
        AuthError::SignUpRequired => {
            "this account must first be registered with an official Telegram client".to_owned()
        }
        AuthError::FloodWait {
            retry_after_seconds,
        } => format!("Telegram rate limited login; retry after {retry_after_seconds} seconds"),
        AuthError::Telegram(_) => {
            "Telegram login failed; check connectivity and API credentials".to_owned()
        }
    };
    CliError::Telegram(message)
}

async fn open_authorized_telegram(
    config: &TelegramConfig,
) -> Result<Arc<TelegramAdapter>, CliError> {
    let adapter = Arc::new(
        TelegramAdapter::open(config.api_id, &config.session_file)
            .await
            .map_err(|error| CliError::Telegram(error.to_string()))?,
    );
    match adapter.is_authorized().await {
        Ok(true) => Ok(adapter),
        Ok(false) => {
            let _ = adapter.shutdown().await;
            Err(CliError::Telegram(
                "Telegram session is not authorized; run `telegram-archive auth login`".into(),
            ))
        }
        Err(_) => {
            let _ = adapter.shutdown().await;
            Err(CliError::Telegram(
                "could not check Telegram authorization; verify connectivity and credentials"
                    .into(),
            ))
        }
    }
}

async fn refresh_with_adapter(
    database_url: &str,
    adapter: Arc<TelegramAdapter>,
) -> Result<Vec<Chat>, CliError> {
    let store = Arc::new(
        SqliteStore::connect(database_url)
            .await
            .map_err(|error| CliError::Database(error.to_string()))?,
    );
    let result = async {
        let account_id = adapter.authenticated_account_id().await.map_err(|_| {
            CliError::Telegram("could not resolve Telegram account identity".into())
        })?;
        store
            .bind_telegram_account(account_id)
            .await
            .map_err(|error| CliError::Database(error.to_string()))?;
        let application = Application::new(
            store.clone(),
            store.clone(),
            store.clone(),
            store.clone(),
            Some(adapter),
            ComponentStatus::disabled(),
        );
        application.refresh_chats().await.map_err(CliError::from)
    }
    .await;
    store.close().await;
    result
}

async fn readonly_application(
    database_url: &str,
    collector_detail: &str,
) -> Result<Arc<Application>, CliError> {
    let store = Arc::new(
        SqliteStore::open_existing_readonly(database_url)
            .await
            .map_err(|error| {
                CliError::Database(format!(
                    "cannot open archive database ({error}); create it with `telegram-archive db init`"
                ))
            })?,
    );
    Ok(Arc::new(Application::new(
        store.clone(),
        store.clone(),
        store.clone(),
        store,
        None,
        ComponentStatus::new(ComponentState::Disabled, Some(collector_detail.to_owned())),
    )))
}

const QUERY_ONLY_EXPLICIT_DETAIL: &str =
    "query-only mode (--query-only); realtime collector not started";
const QUERY_ONLY_UNCONFIGURED_DETAIL: &str = "Telegram not configured (TELEGRAM_API_ID/TELEGRAM_API_HASH not set); realtime collector not started";
const QUERY_ONLY_UNCONFIGURED_WARNING: &str = "TELEGRAM_API_ID/TELEGRAM_API_HASH not set; serving archive queries only, realtime collection is disabled. Load .env (set -a; . ./.env; set +a) or pass --query-only to silence this.";

/// Returns (status detail, optional stderr warning) when serving without Telegram.
fn query_only_notice(
    query_only: bool,
    telegram_requested: bool,
) -> Option<(&'static str, Option<&'static str>)> {
    if query_only {
        Some((QUERY_ONLY_EXPLICIT_DETAIL, None))
    } else if !telegram_requested {
        Some((
            QUERY_ONLY_UNCONFIGURED_DETAIL,
            Some(QUERY_ONLY_UNCONFIGURED_WARNING),
        ))
    } else {
        None
    }
}

async fn serve(bind: Option<std::net::SocketAddr>, query_only: bool) -> Result<(), CliError> {
    let config = Config::load(None);
    config
        .validate_database_path()
        .map_err(CliError::InvalidInput)?;
    let bind = Config::server_bind(bind).map_err(CliError::InvalidInput)?;
    rest::validate_loopback_bind(bind).map_err(|error| CliError::InvalidInput(error.into()))?;
    let listener = TcpListener::bind(bind)
        .await
        .map_err(|error| CliError::Server(format!("cannot bind REST server: {error}")))?;
    let telegram_requested = Config::telegram_configuration_requested();
    if let Some((detail, warning)) = query_only_notice(query_only, telegram_requested) {
        if let Some(warning) = warning {
            eprintln!("warning: {warning}");
        }
        let application = readonly_application(&config.database_url, detail).await?;
        tracing::info!(address = %listener.local_addr().unwrap_or(bind), mode = "query_only", "REST server listening");
        return serve_router(listener, rest::router(application), None).await;
    }

    let telegram = Config::telegram().map_err(CliError::InvalidInput)?;
    telegram.validate_paths().map_err(CliError::InvalidInput)?;
    let adapter = open_authorized_telegram(&telegram).await?;
    let result = serve_with_telegram(listener, &config.database_url, adapter.clone()).await;
    let shutdown = adapter
        .shutdown()
        .await
        .map_err(|_| CliError::Telegram("Telegram connection did not shut down cleanly".into()));
    result?;
    shutdown?;
    Ok(())
}

async fn serve_with_telegram(
    listener: TcpListener,
    database_url: &str,
    adapter: Arc<TelegramAdapter>,
) -> Result<(), CliError> {
    let store = Arc::new(
        SqliteStore::connect(database_url)
            .await
            .map_err(|error| CliError::Database(error.to_string()))?,
    );
    let result = async {
        let account_id = adapter.authenticated_account_id().await.map_err(|_| {
            CliError::Telegram("could not resolve Telegram account identity".into())
        })?;
        store
            .bind_telegram_account(account_id)
            .await
            .map_err(|error| CliError::Database(error.to_string()))?;
        let mut runtime = start_sync_runtime(store.clone(), adapter.clone()).await?;
        let collector =
            CollectorStatusHandle::new(ComponentStatus::new(ComponentState::Starting, None));
        runtime.start_realtime(adapter.clone(), store.clone(), collector.clone());
        let application = Arc::new(Application::new(
            store.clone(),
            store.clone(),
            store.clone(),
            store.clone(),
            Some(adapter),
            collector,
        ));
        let serving = serve_router(
            listener,
            rest::router_with_sync(application, Some(Arc::clone(&runtime.coordinator))),
            Some(&runtime),
        )
        .await;
        let stopped = runtime.shutdown().await;
        serving?;
        stopped?;
        Ok::<(), CliError>(())
    }
    .await;
    store.close().await;
    result
}

struct SyncRuntime {
    coordinator: Arc<SyncCoordinator>,
    engine: Arc<SyncEngine>,
    sink: IngestSink,
    writer: JoinHandle<Result<(), RepositoryError>>,
    realtime: Option<RealtimeTask>,
}

struct RealtimeTask {
    cancel: CancellationToken,
    handle: JoinHandle<Result<(), crate::application::ApplicationError>>,
}

impl SyncRuntime {
    /// Starts catch-up plus live updates on the same writer and engine as history sync.
    fn start_realtime(
        &mut self,
        adapter: Arc<TelegramAdapter>,
        scope: Arc<dyn crate::application::TrackingScope>,
        status: CollectorStatusHandle,
    ) {
        let cancel = CancellationToken::new();
        let engine = Arc::clone(&self.engine);
        let source = AdapterRealtime::new(adapter, self.sink.clone(), scope);
        let token = cancel.clone();
        let handle = tokio::spawn(async move {
            supervise_realtime_with_status(
                &source,
                &engine,
                ReconnectBackoff::default(),
                token,
                &status,
            )
            .await
        });
        self.realtime = Some(RealtimeTask { cancel, handle });
    }

    async fn shutdown(self) -> Result<(), CliError> {
        let Self {
            coordinator,
            engine,
            sink,
            writer,
            realtime,
        } = self;
        // Producers stop first so the writer can drain once every sender is dropped.
        let realtime_result = match realtime {
            Some(RealtimeTask { cancel, mut handle }) => {
                cancel.cancel();
                match tokio::time::timeout(std::time::Duration::from_secs(10), &mut handle).await {
                    Ok(joined) => joined
                        .map_err(|error| {
                            CliError::Telegram(format!("realtime task failed: {error}"))
                        })?
                        .map_err(|error| {
                            CliError::Telegram(format!("realtime updates failed: {error}"))
                        }),
                    Err(_) => {
                        handle.abort();
                        let _ = handle.await;
                        Err(CliError::Telegram(
                            "realtime shutdown exceeded its deadline; unacknowledged updates are re-fetched on restart".into(),
                        ))
                    }
                }
            }
            None => Ok(()),
        };
        if let Err(error) = &realtime_result {
            tracing::error!(%error, "realtime listener ended with an error");
        }
        let coordinator_result = coordinator
            .shutdown_with_timeout(std::time::Duration::from_secs(10))
            .await
            .map_err(|error| CliError::Telegram(error.to_string()));
        drop(coordinator);
        drop(engine);
        drop(sink);
        let mut writer = writer;
        let writer_result = match tokio::time::timeout(
            std::time::Duration::from_secs(10),
            &mut writer,
        )
        .await
        {
            Ok(result) => result
                .map_err(|error| {
                    CliError::Telegram(format!("ingestion writer task failed: {error}"))
                })?
                .map_err(|error| CliError::Telegram(format!("ingestion writer failed: {error}"))),
            Err(_) => {
                writer.abort();
                let _ = writer.await;
                Err(CliError::Telegram(
                    "ingestion writer shutdown exceeded its deadline; the database may require recovery on restart".into(),
                ))
            }
        };
        realtime_result?;
        coordinator_result?;
        writer_result
    }
}

async fn start_sync_runtime(
    store: Arc<SqliteStore>,
    adapter: Arc<TelegramAdapter>,
) -> Result<SyncRuntime, CliError> {
    crate::application::SyncRepository::recover_interrupted(store.as_ref())
        .await
        .map_err(|error| CliError::Database(error.to_string()))?;
    let writer: Arc<dyn ArchiveWriter> = store.clone();
    let repository: Arc<dyn SyncRepository> = store.clone();
    let chats: Arc<dyn crate::application::ChatRepository> = store;
    let gateway: Arc<dyn crate::application::TelegramGateway> = adapter;
    let (sink, writer_task) = ingestion_worker::spawn(writer, 32);
    let engine = Arc::new(SyncEngine::new(
        gateway,
        repository.clone(),
        chats,
        sink.clone(),
        PageSize::DEFAULT,
    ));
    let coordinator = SyncCoordinator::spawn(engine.clone(), repository, 16);
    Ok(SyncRuntime {
        coordinator,
        engine,
        sink,
        writer: writer_task,
        realtime: None,
    })
}

async fn serve_router(
    listener: TcpListener,
    app: axum::Router,
    runtime: Option<&SyncRuntime>,
) -> Result<(), CliError> {
    let stop = CancellationToken::new();
    let server = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(stop.clone()))
        .into_future();
    tokio::pin!(server);
    match runtime {
        Some(runtime) => loop {
            tokio::select! {
                result = &mut server => break result.map_err(|error| CliError::Server(format!("REST server failed: {error}"))),
                error = runtime.coordinator.wait_worker_failure() => {
                    stop.cancel();
                    let _ = (&mut server).await;
                    let message = error.err().map_or_else(|| "sync worker stopped unexpectedly".to_owned(), |error| error.to_string());
                    break Err(CliError::Server(message));
                }
                _ = tokio::time::sleep(std::time::Duration::from_millis(200)) => {
                    if runtime.realtime.as_ref().is_some_and(|task| task.handle.is_finished()) {
                        stop.cancel();
                        let _ = (&mut server).await;
                        break Err(CliError::Server("realtime listener stopped unexpectedly".into()));
                    }
                    if runtime.writer.is_finished() {
                        stop.cancel();
                        let _ = (&mut server).await;
                        break Err(CliError::Server("ingestion writer stopped unexpectedly".into()));
                    }
                }
            }
        },
        None => (&mut server)
            .await
            .map_err(|error| CliError::Server(format!("REST server failed: {error}"))),
    }
}

async fn shutdown_signal(stop: CancellationToken) {
    tokio::select! {
        result = tokio::signal::ctrl_c() => {
            if let Err(error) = result { tracing::error!(%error, "failed to listen for Ctrl-C"); }
        }
        _ = terminate_signal() => {}
        _ = stop.cancelled() => {}
    }
}

#[cfg(unix)]
async fn terminate_signal() {
    match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
        Ok(mut signal) => {
            let _ = signal.recv().await;
        }
        Err(error) => {
            tracing::error!(%error, "failed to listen for SIGTERM");
            std::future::pending::<()>().await;
        }
    }
}

#[cfg(not(unix))]
async fn terminate_signal() {
    std::future::pending::<()>().await;
}

async fn open_existing_store(database_url: &str) -> Result<Arc<SqliteStore>, CliError> {
    SqliteStore::open_existing(database_url)
        .await
        .map(Arc::new)
        .map_err(|error| {
            CliError::Database(format!(
                "cannot open archive database ({error}); create it with `telegram-archive db init`"
            ))
        })
}

/// Checks the tracking scope against the local database before Telegram is contacted.
/// Returns `false` when there is nothing to sync (`sync all` with no tracked chats).
async fn sync_scope_allowed(database_url: &str, scope: &SyncScope) -> Result<bool, CliError> {
    use crate::application::ChatRepository;
    let store = open_existing_store(database_url).await?;
    let result: Result<bool, CliError> = async {
        match scope {
            SyncScope::Chat(id) => match store.get(*id).await.map_err(db_error)? {
                None => Err(CliError::ChatNotFound(id.get())),
                Some(chat) if !chat.tracked => {
                    Err(crate::application::ApplicationError::NotTracked.into())
                }
                Some(_) => Ok(true),
            },
            SyncScope::All => Ok(store
                .list()
                .await
                .map_err(db_error)?
                .iter()
                .any(|chat| chat.tracked)),
        }
    }
    .await;
    store.close().await;
    result
}

fn db_error(error: crate::application::RepositoryError) -> CliError {
    CliError::Database(error.to_string())
}

async fn sync_archive(scope: SyncScope, output: OutputFormat) -> Result<(), CliError> {
    let config = Config::load(None);
    if !sync_scope_allowed(&config.database_url, &scope).await? {
        print_output(if output == OutputFormat::Json {
            "{\"tracked_chats\":0,\"synced\":false}".into()
        } else {
            "No tracked chats; nothing to sync. Track chats with `chats track <CHAT_ID>`.".into()
        });
        return Ok(());
    }
    let telegram = Config::telegram().map_err(CliError::InvalidInput)?;
    let adapter = open_authorized_telegram(&telegram).await?;
    let result = sync_with_adapter(&config.database_url, adapter.clone(), scope).await;
    let shutdown = adapter
        .shutdown()
        .await
        .map_err(|_| CliError::Telegram("Telegram connection did not shut down cleanly".into()));
    let job = result?;
    shutdown?;
    if job.state != SyncJobState::Succeeded {
        return Err(CliError::SyncFailed(
            job.summary_error
                .clone()
                .unwrap_or_else(|| format!("job ended as {:?}", job.state)),
        ));
    }
    print_output(render_sync_job(output, &job)?);
    Ok(())
}

async fn sync_with_adapter(
    database_url: &str,
    adapter: Arc<TelegramAdapter>,
    scope: SyncScope,
) -> Result<SyncJob, CliError> {
    let store = Arc::new(
        SqliteStore::connect(database_url)
            .await
            .map_err(|error| CliError::Database(error.to_string()))?,
    );
    let setup = async {
        let account_id = adapter.authenticated_account_id().await.map_err(|_| {
            CliError::Telegram("could not resolve Telegram account identity".into())
        })?;
        store
            .bind_telegram_account(account_id)
            .await
            .map_err(|error| CliError::Database(error.to_string()))?;
        Ok::<(), CliError>(())
    }
    .await;
    if let Err(error) = setup {
        store.close().await;
        return Err(error);
    }
    let runtime = match start_sync_runtime(store.clone(), adapter).await {
        Ok(runtime) => runtime,
        Err(error) => {
            store.close().await;
            return Err(error);
        }
    };
    let result = async {
        let job = runtime.coordinator.submit(scope).await?;
        runtime
            .coordinator
            .wait_job(&job.id)
            .await
            .map_err(CliError::from)
    }
    .await;
    let stopped = runtime.shutdown().await;
    store.close().await;
    let job = result?;
    stopped?;
    Ok(job)
}

fn print_output(output: String) {
    if !output.is_empty() {
        println!("{output}");
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn query_only_notice_distinguishes_explicit_from_unconfigured() {
        let (detail, warning) = query_only_notice(true, false).unwrap();
        assert!(detail.contains("--query-only") && warning.is_none());
        let (detail, warning) = query_only_notice(false, false).unwrap();
        assert!(detail.contains("not configured"));
        let warning = warning.unwrap();
        assert!(
            warning.contains("realtime collection is disabled") && warning.contains("--query-only")
        );
        assert!(query_only_notice(false, true).is_none());
    }

    use super::*;

    #[test]
    fn default_log_filter_enables_warnings_but_not_info() {
        // Not hermetic if RUST_LOG is set in the environment; only the fallback is asserted.
        if std::env::var_os("RUST_LOG").is_none() {
            assert_eq!(log_filter().to_string(), "warn");
        }
    }

    #[test]
    fn authorized_session_needs_no_terminal() {
        assert_eq!(
            login_gate(true, false).unwrap(),
            LoginGate::AlreadyAuthorized
        );
        assert_eq!(
            login_gate(true, true).unwrap(),
            LoginGate::AlreadyAuthorized
        );
    }

    #[test]
    fn unauthorized_session_requires_a_terminal() {
        assert_eq!(
            login_gate(false, true).unwrap(),
            LoginGate::PromptForCredentials
        );
        assert!(login_gate(false, false).is_err());
    }
}

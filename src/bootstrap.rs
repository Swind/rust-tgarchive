use std::sync::Arc;

use clap::Parser;
use tokio::net::TcpListener;

use crate::{
    application::services::{Application, ComponentStatus},
    config::Config,
    infrastructure::persistence::sqlite::SqliteStore,
    interface::cli::{
        Cli, CliError, OpenApiCliFormat, PreparedInvocation, execute_prepared, initialized_output,
        prepare,
    },
    interface::rest::{self, OpenApiFormat},
};

pub async fn run() -> Result<(), CliError> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
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
            let application = readonly_application(&config.database_url).await?;
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
        PreparedInvocation::Serve { bind } => serve(bind).await?,
    }
    Ok(())
}

async fn readonly_application(database_url: &str) -> Result<Arc<Application>, CliError> {
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
        ComponentStatus::disabled(),
    )))
}

async fn serve(bind: Option<std::net::SocketAddr>) -> Result<(), CliError> {
    let config = Config::load(None);
    let bind = Config::server_bind(bind).map_err(CliError::InvalidInput)?;
    rest::validate_loopback_bind(bind).map_err(|error| CliError::InvalidInput(error.into()))?;
    let application = readonly_application(&config.database_url).await?;
    let listener = TcpListener::bind(bind)
        .await
        .map_err(|error| CliError::Server(format!("cannot bind REST server: {error}")))?;
    tracing::info!(address = %listener.local_addr().unwrap_or(bind), "REST query server listening");
    axum::serve(listener, rest::router(application))
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|error| CliError::Server(format!("REST server failed: {error}")))
}

async fn shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::error!(%error, "failed to listen for Ctrl-C");
    }
}

fn print_output(output: String) {
    if !output.is_empty() {
        println!("{output}");
    }
}

use std::sync::Arc;

use clap::Parser;

use crate::{
    application::services::{Application, ComponentStatus},
    config::Config,
    infrastructure::persistence::sqlite::SqliteStore,
    interface::cli::{
        Cli, CliError, PreparedInvocation, execute_prepared, initialized_output, prepare,
    },
};

pub async fn run() -> Result<(), CliError> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .with_target(false)
        .try_init();

    let invocation = prepare(Cli::parse())?;
    let output = match invocation {
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
            initialized_output(output)?
        }
        PreparedInvocation::Query { output, command } => {
            let config = Config::load(None);
            let store = Arc::new(
                SqliteStore::open_existing_readonly(&config.database_url)
                    .await
                    .map_err(|error| {
                        CliError::Database(format!(
                            "cannot open archive database ({error}); create it with `telegram-archive db init`"
                        ))
                    })?,
            );
            let application = Application::new(
                store.clone(),
                store.clone(),
                store.clone(),
                store,
                None,
                ComponentStatus::disabled(),
            );
            execute_prepared(command, output, &application).await?
        }
    };
    if !output.is_empty() {
        println!("{output}");
    }
    Ok(())
}

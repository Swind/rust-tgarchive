//! Process entry point and CLI dispatch; subsystem wiring lives in child modules.
mod auth;
mod database;
mod runtime;
mod server;
mod sync;

use crate::{
    application::services::{Application, ComponentStatus},
    config::{Config, SyncPacing},
    infrastructure::persistence::sqlite::SqliteStore,
    interface::{
        cli::{
            Cli, CliError, OpenApiCliFormat, OutputFormat, PreparedInvocation, execute_prepared,
            execute_set_tracking, initialized_output, prepare, render_chats,
        },
        rest::{self, OpenApiFormat},
    },
};
use auth::{auth_login, open_authorized_telegram, refresh_with_adapter};
use clap::Parser;
use database::{
    open_existing_store, readonly_application, rebuild_index, rebuild_index_if_needed,
    repair_senders,
};
use server::{healthcheck, serve};
use std::sync::Arc;
use sync::{backfill_after_track, sync_archive};

/// `RUST_LOG` when set, otherwise `warn` so failures are visible by default (not only errors).
fn log_filter() -> tracing_subscriber::EnvFilter {
    tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"))
}

pub async fn run() -> Result<(), CliError> {
    let cli = Cli::parse();
    crate::config::load_dotenv(cli.env_file.as_deref()).map_err(CliError::InvalidInput)?;
    let _ = tracing_subscriber::fmt()
        .with_env_filter(log_filter())
        .with_writer(std::io::stderr)
        .with_target(false)
        .try_init();

    match prepare(cli)? {
        PreparedInvocation::InitializeDatabase {
            output,
            database_url,
        } => {
            let config = Config::load(database_url);
            let store = SqliteStore::connect(&config.database_url)
                .await
                .map_err(|error| CliError::Database(error.to_string()))?;
            let rebuilt = rebuild_index_if_needed(&store).await;
            store.close().await;
            rebuilt?;
            tracing::info!("archive database initialized");
            print_output(initialized_output(output)?);
        }
        PreparedInvocation::RebuildSearchIndex { output } => {
            let config = Config::load(None);
            let store = open_existing_store(&config.database_url).await?;
            let result = rebuild_index(&store).await;
            store.close().await;
            let progress = result?;
            print_output(if output == OutputFormat::Json {
                format!(
                    "{{\"rebuilt\":true,\"indexed\":{},\"total\":{}}}",
                    progress.indexed, progress.total
                )
            } else {
                format!(
                    "Rebuilt search index: {} messages indexed.",
                    progress.indexed
                )
            });
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
        PreparedInvocation::Serve {
            bind,
            query_only,
            allow_non_loopback,
        } => serve(bind, query_only, allow_non_loopback).await?,
        PreparedInvocation::Healthcheck { url } => healthcheck(url)?,
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
            backfill,
            output,
        } => {
            let config = Config::load(None);
            let pacing = if backfill {
                Some(SyncPacing::from_env().map_err(CliError::InvalidInput)?)
            } else {
                None
            };
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
            if pacing.is_some() {
                backfill_after_track(chat_id, output).await?;
            }
        }
        PreparedInvocation::Sync {
            scope,
            output,
            refetch,
        } => sync_archive(scope, output, refetch).await?,
        PreparedInvocation::RepairSenders { dry_run, output } => {
            repair_senders(dry_run, output).await?
        }
    }
    Ok(())
}

fn print_output(output: String) {
    if !output.is_empty() {
        println!("{output}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_log_filter_enables_warnings_but_not_info() {
        // Not hermetic if RUST_LOG is set in the environment; only the fallback is asserted.
        if std::env::var_os("RUST_LOG").is_none() {
            assert_eq!(log_filter().to_string(), "warn");
        }
    }
}

mod store;

pub use store::SqliteStore;

use std::{str::FromStr, time::Duration};

use sqlx::{
    SqlitePool,
    migrate::Migrator,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};

use crate::application::RepositoryError;

static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

pub(crate) async fn open_pool(database_url: &str) -> Result<SqlitePool, RepositoryError> {
    let options = SqliteConnectOptions::from_str(database_url)
        .map_err(storage_error)?
        .create_if_missing(true)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5))
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal);

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await
        .map_err(storage_error)?;
    MIGRATOR.run(&pool).await.map_err(storage_error)?;
    Ok(pool)
}

pub(crate) async fn open_readonly_pool(database_url: &str) -> Result<SqlitePool, RepositoryError> {
    let options = SqliteConnectOptions::from_str(database_url)
        .map_err(storage_error)?
        .read_only(true)
        .create_if_missing(false);
    SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await
        .map_err(storage_error)
}

pub(crate) fn storage_error(error: impl std::fmt::Display) -> RepositoryError {
    RepositoryError::Unavailable(error.to_string())
}

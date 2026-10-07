use super::{
    auth::{bind_account, open_authorized_telegram},
    database::{readonly_application, warn_if_index_not_ready},
    runtime::{SyncRuntime, start_sync_runtime},
};
use crate::{
    application::{
        services::{Application, CollectorStatusHandle, ComponentState, ComponentStatus},
        sync::CancellationToken,
    },
    config::{Config, SyncPacing},
    infrastructure::{persistence::sqlite::SqliteStore, telegram::TelegramAdapter},
    interface::{cli::CliError, rest},
};
use std::{future::IntoFuture, sync::Arc};
use tokio::net::TcpListener;

const QUERY_ONLY_EXPLICIT_DETAIL: &str =
    "query-only mode (--query-only); realtime collector not started";
const QUERY_ONLY_UNCONFIGURED_DETAIL: &str = "Telegram not configured (TELEGRAM_API_ID/TELEGRAM_API_HASH not set); realtime collector not started";
const QUERY_ONLY_UNCONFIGURED_WARNING: &str = "TELEGRAM_API_ID/TELEGRAM_API_HASH not set; serving archive queries only, realtime collection is disabled. Set them in the environment or in ./.env (or --env-file), or pass --query-only to silence this.";

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

/// Minimal HTTP/1.0 GET over std::net so the runtime image needs no curl.
pub(super) fn healthcheck(url: Option<String>) -> Result<(), CliError> {
    use std::io::{Read, Write};
    let url = match url {
        Some(url) => url,
        None => {
            let bind = Config::server_bind(None).map_err(CliError::InvalidInput)?;
            format!("http://127.0.0.1:{}/health/ready", bind.port())
        }
    };
    let rest = url.strip_prefix("http://").ok_or_else(|| {
        CliError::InvalidInput("healthcheck --url must start with http://".into())
    })?;
    let (host, path) = match rest.split_once('/') {
        Some((host, path)) => (host, format!("/{path}")),
        None => (rest, "/".to_owned()),
    };
    let fail = |message: String| CliError::Server(format!("healthcheck failed: {message}"));
    let timeout = std::time::Duration::from_secs(5);
    let address = std::net::ToSocketAddrs::to_socket_addrs(host)
        .map_err(|error| fail(error.to_string()))?
        .next()
        .ok_or_else(|| fail(format!("cannot resolve {host}")))?;
    let mut stream = std::net::TcpStream::connect_timeout(&address, timeout)
        .map_err(|error| fail(error.to_string()))?;
    stream
        .set_read_timeout(Some(timeout))
        .and_then(|()| stream.set_write_timeout(Some(timeout)))
        .map_err(|error| fail(error.to_string()))?;
    stream
        .write_all(format!("GET {path} HTTP/1.0\r\nHost: {host}\r\n\r\n").as_bytes())
        .map_err(|error| fail(error.to_string()))?;
    let mut head = [0u8; 32];
    let mut filled = 0;
    while filled < head.len() {
        match stream.read(&mut head[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(error) => return Err(fail(error.to_string())),
        }
    }
    let line = String::from_utf8_lossy(&head[..filled]);
    let status = line.split_whitespace().nth(1).unwrap_or("");
    if status.starts_with('2') {
        Ok(())
    } else {
        Err(fail(format!(
            "{url} answered {:?}",
            line.lines().next().unwrap_or("")
        )))
    }
}

pub(super) async fn serve(
    bind: Option<std::net::SocketAddr>,
    query_only: bool,
    allow_non_loopback: bool,
) -> Result<(), CliError> {
    let config = Config::load(None);
    config
        .validate_database_path()
        .map_err(CliError::InvalidInput)?;
    let bind = Config::server_bind(bind).map_err(CliError::InvalidInput)?;
    let cors = rest::dev_cors_from_env().map_err(CliError::InvalidInput)?;
    if allow_non_loopback || rest::allow_non_loopback_from_env() {
        if !bind.ip().is_loopback() {
            tracing::warn!(address = %bind, "{}", rest::NON_LOOPBACK_WARNING);
        }
    } else {
        rest::validate_loopback_bind(bind).map_err(|error| {
            CliError::InvalidInput(format!(
                "{error}; pass --allow-non-loopback or set {} to override",
                rest::ALLOW_NON_LOOPBACK_ENV
            ))
        })?;
    }
    let listener = TcpListener::bind(bind)
        .await
        .map_err(|error| CliError::Server(format!("cannot bind REST server: {error}")))?;
    let telegram_requested = Config::telegram_configuration_requested();
    if let Some((detail, warning)) = query_only_notice(query_only, telegram_requested) {
        if let Some(warning) = warning {
            eprintln!("warning: {warning}");
        }
        let application = readonly_application(&config.database_url, detail).await?;
        let media_store = SqliteStore::open_existing_readonly(&config.database_url)
            .await
            .map_err(|error| CliError::Database(error.to_string()))?;
        let media = rest::MediaContext {
            store: media_store,
            media_dir: config.media_directory().map_err(CliError::InvalidInput)?,
            worker_enabled: false,
        };
        tracing::info!(address = %listener.local_addr().unwrap_or(bind), mode = "query_only", "REST server listening");
        return serve_router(
            listener,
            rest::apply_dev_cors(
                rest::router_with_media(application, None, Some(media)),
                cors,
            ),
            None,
        )
        .await;
    }

    let telegram = Config::telegram().map_err(CliError::InvalidInput)?;
    telegram.validate_paths().map_err(CliError::InvalidInput)?;
    SyncPacing::from_env().map_err(CliError::InvalidInput)?;
    let media_dir = config.media_directory().map_err(CliError::InvalidInput)?;
    tokio::fs::create_dir_all(&media_dir)
        .await
        .map_err(|error| CliError::Server(format!("cannot create media directory: {error}")))?;
    let adapter = open_authorized_telegram(&telegram).await?;
    let result = serve_with_telegram(
        listener,
        &config.database_url,
        adapter.clone(),
        media_dir,
        cors,
    )
    .await;
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
    media_dir: std::path::PathBuf,
    cors: Option<tower_http::cors::CorsLayer>,
) -> Result<(), CliError> {
    let store = Arc::new(
        SqliteStore::connect(database_url)
            .await
            .map_err(|error| CliError::Database(error.to_string()))?,
    );
    warn_if_index_not_ready(&store).await;
    let result = async {
        bind_account(&store, &adapter).await?;
        let mut runtime = start_sync_runtime(store.clone(), adapter.clone()).await?;
        runtime.start_media(adapter.clone(), store.clone(), media_dir.clone());
        let collector =
            CollectorStatusHandle::new(ComponentStatus::new(ComponentState::Starting, None));
        runtime.start_realtime(adapter.clone(), store.clone(), collector.clone());
        let application = Arc::new(
            Application::new(
                store.clone(),
                store.clone(),
                store.clone(),
                store.clone(),
                Some(adapter),
                collector,
            )
            .with_rate_limit(Arc::clone(runtime.engine.pacer())),
        );
        let serving = serve_router(
            listener,
            rest::apply_dev_cors(
                rest::router_with_media(
                    application,
                    Some(Arc::clone(&runtime.coordinator)),
                    Some(rest::MediaContext {
                        store: (*store).clone(),
                        media_dir: media_dir.clone(),
                        worker_enabled: true,
                    }),
                ),
                cors,
            ),
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
                    if runtime.media.as_ref().is_some_and(|task| task.handle.is_finished()) {
                        stop.cancel();
                        let _ = (&mut server).await;
                        break Err(CliError::Server("media worker stopped unexpectedly".into()));
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

#[cfg(test)]
mod tests {
    use super::*;
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
}

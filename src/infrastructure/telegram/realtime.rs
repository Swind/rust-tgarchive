use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use grammers_client::{
    Client,
    client::{UpdateStream, UpdatesConfiguration},
    tl,
};
use grammers_session::{
    Session,
    types::{UpdateState, UpdatesState},
};
use tokio_util::sync::CancellationToken;

use crate::{
    application::{
        RepositoryError, TrackingScope,
        ingestion_worker::IngestSink,
        realtime::{GapScope, RealtimeSource, RealtimeSourceError, is_empty, restrict_to_tracked},
    },
    domain::ChatKind,
};

use super::{file_session::FileSession, mapper, session::TelegramAdapter};

#[derive(Debug, thiserror::Error)]
pub enum RealtimeError {
    #[error("realtime updates are already running or have stopped")]
    ReceiverUnavailable,
    #[error("could not initialize Telegram update stream: {0}")]
    Stream(String),
    #[error("Telegram update processing failed: {0}")]
    Telegram(#[from] grammers_client::InvocationError),
    #[error(
        "Telegram difference is too old; reconciliation is required before realtime can continue"
    )]
    DifferenceTooLong(Option<i64>),
    #[error("could not map Telegram update: {0}")]
    Mapping(#[from] mapper::MappingError),
    #[error("archive ingestion failed: {0}")]
    Archive(#[from] RepositoryError),
    #[error("could not persist acknowledged Telegram update state: {0}")]
    Checkpoint(String),
}

mod batch;
use batch::NormalizedBatch;

/// Why a connection stopped, and whether the same connection may be run again.
#[derive(Debug)]
pub struct ConnectionFailure {
    pub error: RealtimeSourceError,
    pub reusable: bool,
}

/// One live Telegram connection (client, sender pool and update stream).
#[async_trait::async_trait]
pub trait RealtimeConnection: Send {
    /// Returns `Ok` only after `cancel` fired.
    async fn run(&mut self, cancel: CancellationToken) -> Result<(), ConnectionFailure>;
    /// Stops the sender pool and waits until it, and the session owner lock, are released.
    async fn teardown(&mut self);
    /// True while the connection waits for socket updates; false while it still works through
    /// pending Telegram differences. `None` means there is nothing to wait for.
    fn live_flag(&self) -> Option<Arc<AtomicBool>> {
        None
    }
}

/// Builds a fresh connection. It is only called after the previous one was torn down.
#[async_trait::async_trait]
pub trait RealtimeConnector: Send + Sync {
    type Connection: RealtimeConnection;
    async fn connect(&self) -> Result<Self::Connection, RealtimeSourceError>;
}

/// Live-update source that rebuilds its connection when the sender pool is gone. Grammers'
/// update receiver is single-use and `Dropped` means the pool stopped, so such a connection is
/// torn down immediately and the next attempt connects anew; only a transient error on a
/// still-live stream keeps the connection.
pub struct ReconnectingSource<C: RealtimeConnector> {
    connector: C,
    connection: tokio::sync::Mutex<Option<C::Connection>>,
    live: std::sync::Mutex<Arc<AtomicBool>>,
}

impl<C: RealtimeConnector> ReconnectingSource<C> {
    pub fn with_connector(connector: C) -> Self {
        Self {
            connector,
            connection: tokio::sync::Mutex::new(None),
            live: std::sync::Mutex::new(Arc::new(AtomicBool::new(false))),
        }
    }
}

#[async_trait::async_trait]
impl<C: RealtimeConnector> RealtimeSource for ReconnectingSource<C> {
    fn is_live(&self) -> bool {
        self.live.lock().expect("live lock").load(Ordering::SeqCst)
    }

    async fn run_once(&self, cancel: CancellationToken) -> Result<(), RealtimeSourceError> {
        let mut slot = self.connection.lock().await;
        *self.live.lock().expect("live lock") = Arc::new(AtomicBool::new(false));
        if slot.is_none() {
            if cancel.is_cancelled() {
                return Ok(());
            }
            // Building a connection is short and not interrupted midway; cancellation is
            // honoured right after, so no half-built pool is abandoned.
            let mut connection = self.connector.connect().await?;
            if cancel.is_cancelled() {
                connection.teardown().await;
                return Ok(());
            }
            *slot = Some(connection);
        }
        let connection = slot.as_mut().expect("connection was just ensured");
        *self.live.lock().expect("live lock") = connection
            .live_flag()
            .unwrap_or_else(|| Arc::new(AtomicBool::new(true)));
        match connection.run(cancel).await {
            Ok(()) => Ok(()),
            Err(failure) => {
                if !failure.reusable
                    && let Some(mut dead) = slot.take()
                {
                    dead.teardown().await;
                }
                Err(failure.error)
            }
        }
    }
}

/// Real connector: the adapter owns the pool and rebuilds it over the same `FileSession`.
pub struct AdapterConnector {
    adapter: Arc<TelegramAdapter>,
    sink: IngestSink,
    scope: Arc<dyn TrackingScope>,
}

pub type AdapterRealtime = ReconnectingSource<AdapterConnector>;

impl AdapterRealtime {
    pub fn new(
        adapter: Arc<TelegramAdapter>,
        sink: IngestSink,
        scope: Arc<dyn TrackingScope>,
    ) -> Self {
        ReconnectingSource::with_connector(AdapterConnector {
            adapter,
            sink,
            scope,
        })
    }
}

pub struct AdapterConnection {
    adapter: Arc<TelegramAdapter>,
    sink: IngestSink,
    scope: Arc<dyn TrackingScope>,
    stream: UpdateStream,
}

#[async_trait::async_trait]
impl RealtimeConnector for AdapterConnector {
    type Connection = AdapterConnection;

    async fn connect(&self) -> Result<AdapterConnection, RealtimeSourceError> {
        let updates = match self.adapter.updates.lock().await.take() {
            Some(updates) => updates,
            None => {
                self.adapter.reconnect().await.map_err(|error| {
                    RealtimeSourceError::Fatal(format!(
                        "could not rebuild Telegram client: {error}"
                    ))
                })?;
                self.adapter
                    .updates
                    .lock()
                    .await
                    .take()
                    .ok_or_else(|| classify(RealtimeError::ReceiverUnavailable))?
            }
        };
        let stream = self
            .adapter
            .client()
            .stream_updates(
                updates,
                UpdatesConfiguration {
                    catch_up: true,
                    update_queue_limit: None,
                },
            )
            .await
            .map_err(|error| classify(RealtimeError::Stream(error.to_string())))?;
        Ok(AdapterConnection {
            adapter: Arc::clone(&self.adapter),
            sink: self.sink.clone(),
            scope: Arc::clone(&self.scope),
            stream,
        })
    }
}

#[async_trait::async_trait]
impl RealtimeConnection for AdapterConnection {
    /// Update state is persisted only after every item currently buffered by Grammers is
    /// committed through the shared bounded, acknowledged writer.
    async fn run(&mut self, cancel: CancellationToken) -> Result<(), ConnectionFailure> {
        let client = self.adapter.client();
        match process_stream(
            &client,
            &mut self.stream,
            &self.sink,
            self.scope.as_ref(),
            cancel,
            self.adapter.account_id(),
        )
        .await
        {
            Ok(()) => Ok(()),
            Err(RealtimeError::DifferenceTooLong(channel)) => {
                // The stream cannot continue: Grammers would request the same difference again.
                // Persist Telegram's current state, then rebuild the stream from it.
                let fresh = match channel {
                    Some(_) => None,
                    None => Some(fetch_current_state(&client).await.map_err(|error| {
                        ConnectionFailure {
                            error: classify(error),
                            reusable: false,
                        }
                    })?),
                };
                reset_session_state(&self.adapter.session, channel, fresh)
                    .await
                    .map_err(|error| ConnectionFailure {
                        error: classify(error),
                        reusable: false,
                    })?;
                Err(ConnectionFailure {
                    error: classify(RealtimeError::DifferenceTooLong(channel)),
                    reusable: false,
                })
            }
            Err(error) => {
                let dropped = matches!(
                    error,
                    RealtimeError::Telegram(grammers_client::InvocationError::Dropped)
                );
                let error = classify(error);
                let reusable = matches!(error, RealtimeSourceError::Transient(_)) && !dropped;
                Err(ConnectionFailure { error, reusable })
            }
        }
    }

    /// Stops the pool, then immediately starts a fresh one. Catch-up and history calls use the
    /// adapter's current client between sessions, so it must never be a stopped pool; `connect`
    /// picks up the fresh pool's update receiver.
    async fn teardown(&mut self) {
        if let Err(error) = self.adapter.shutdown().await {
            tracing::warn!(%error, "Telegram sender pool did not stop cleanly");
        }
        if let Err(error) = self.adapter.reconnect().await {
            tracing::warn!(%error, "could not rebuild Telegram sender pool; retrying on connect");
        }
    }

    fn live_flag(&self) -> Option<Arc<AtomicBool>> {
        Some(self.stream.live_flag())
    }
}

/// Stream end (`Dropped`), I/O and transport failures, FLOOD_WAIT and 5xx RPC errors are
/// reconnectable; everything else (auth, mapping, storage, checkpoint) is fatal.
pub fn classify(error: RealtimeError) -> RealtimeSourceError {
    use grammers_client::InvocationError as Invocation;
    let message = error.to_string();
    match error {
        RealtimeError::Telegram(
            Invocation::Dropped | Invocation::Io(_) | Invocation::Transport(_),
        ) => RealtimeSourceError::Transient(message),
        RealtimeError::Telegram(Invocation::Rpc(ref rpc)) if rpc.code == 420 || rpc.code >= 500 => {
            RealtimeSourceError::Transient(message)
        }
        RealtimeError::DifferenceTooLong(None) => RealtimeSourceError::GapReset(GapScope::Account),
        RealtimeError::DifferenceTooLong(Some(id)) => {
            match grammers_session::types::PeerId::channel(id)
                .ok_or(mapper::MappingError::InvalidPeerId)
                .and_then(mapper::chat_id)
            {
                Ok(chat) => RealtimeSourceError::GapReset(GapScope::Channel(chat)),
                Err(_) => RealtimeSourceError::GapReset(GapScope::Account),
            }
        }
        _ => RealtimeSourceError::Fatal(message),
    }
}

/// Telegram's current common update state (`updates.getState`), without channel states.
async fn fetch_current_state(client: &Client) -> Result<UpdatesState, RealtimeError> {
    let tl::enums::updates::State::State(state) =
        client.invoke(&tl::functions::updates::GetState {}).await?;
    Ok(UpdatesState {
        pts: state.pts,
        qts: state.qts,
        date: state.date,
        seq: state.seq,
        channels: Vec::new(),
    })
}

/// Persists the post-gap update state. Account-wide: the fresh common state with all channel
/// states dropped (Grammers re-initializes a channel from its next update, exactly as it does
/// for a channel it has not seen). Channel-scoped: only that channel's entry is dropped.
/// Messages in the skipped gap are recovered for tracked chats by message-level catch-up.
async fn reset_session_state(
    session: &FileSession,
    channel: Option<i64>,
    fresh: Option<UpdatesState>,
) -> Result<(), RealtimeError> {
    let checkpoint = |error: &dyn std::fmt::Display| RealtimeError::Checkpoint(error.to_string());
    let state = match (fresh, channel) {
        (Some(fresh), _) => fresh,
        (None, Some(id)) => {
            let mut state = session.updates_state().await.map_err(|e| checkpoint(&e))?;
            state.channels.retain(|c| c.id != id);
            state
        }
        (None, None) => return Err(RealtimeError::Checkpoint("no fresh state".into())),
    };
    session
        .set_update_state(UpdateState::All(state))
        .await
        .map_err(|e| checkpoint(&e))
}

async fn process_stream(
    client: &Client,
    stream: &mut UpdateStream,
    sink: &IngestSink,
    scope: &dyn TrackingScope,
    cancellation: CancellationToken,
    account: Option<crate::domain::SenderId>,
) -> Result<(), RealtimeError> {
    loop {
        let first = tokio::select! {
            _ = cancellation.cancelled() => return Ok(()),
            result = stream.next_raw() => result.map_err(map_stream_error)?,
        };
        let mut batch = NormalizedBatch::with_account(account);
        batch.add_update(client, first.0, first.1, first.2)?;

        // Grammers can expand one Updates container or difference into several updates.
        // Do not persist its aggregate state until the entire internal buffer is ACKed.
        while stream.has_pending_updates() {
            let (raw, state, peers) = stream.next_raw().await.map_err(map_stream_error)?;
            batch.add_update(client, raw, state, peers)?;
        }

        // The tracked set is read per batch, so tracking changes apply without a restart.
        // Updates of untracked chats are intentionally ignored (not lost): the batch is reduced
        // before anything is written and the update-state checkpoint below still advances.
        let batch = restrict_to_tracked(scope, batch.into_ingest_batch()).await?;
        if !is_empty(&batch) {
            tokio::select! {
                _ = cancellation.cancelled() => return Ok(()),
                result = sink.submit(batch) => result?,
            }
        }
        // Intentionally not cancellation-selectable: after the writer ACK the checkpoint must
        // finish or report failure before the runtime tears this stream down.
        stream
            .sync_update_state()
            .await
            .map_err(|error| RealtimeError::Checkpoint(error.to_string()))?;
    }
}

fn map_stream_error(error: grammers_client::InvocationError) -> RealtimeError {
    match error {
        grammers_client::InvocationError::Rpc(ref rpc)
            if rpc.name == grammers_client::client::ARCHIVE_DIFFERENCE_TOO_LONG =>
        {
            RealtimeError::DifferenceTooLong(None)
        }
        grammers_client::InvocationError::Rpc(ref rpc)
            if grammers_client::client::archive_too_long_channel_id(&rpc.name).is_some() =>
        {
            RealtimeError::DifferenceTooLong(grammers_client::client::archive_too_long_channel_id(
                &rpc.name,
            ))
        }
        error => RealtimeError::Telegram(error),
    }
}

pub(super) fn chat_kind(peer: grammers_session::types::PeerId) -> ChatKind {
    match peer.kind() {
        grammers_session::types::PeerKind::User => ChatKind::Private,
        grammers_session::types::PeerKind::Chat => ChatKind::Group,
        grammers_session::types::PeerKind::Channel => ChatKind::Channel,
    }
}

#[cfg(test)]
#[path = "realtime/tests.rs"]
mod tests;

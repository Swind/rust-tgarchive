use std::{
    collections::HashSet,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use chrono::Utc;
use grammers_client::{
    Client,
    client::{UpdateStream, UpdatesConfiguration},
    peer::PeerMap,
    tl,
    update::Update,
};
use grammers_session::{
    Session,
    types::{UpdateState, UpdatesState},
    updates::State,
};
use tokio_util::sync::CancellationToken;

use crate::{
    application::{
        AccountDeletion, IngestBatch, IngestRecord, MessageSource, RepositoryError, TrackingScope,
        ingestion_worker::IngestSink,
        realtime::{GapScope, RealtimeSource, RealtimeSourceError, is_empty, restrict_to_tracked},
    },
    domain::{Chat, ChatKind, MessageEvent, MessageId},
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

#[derive(Default)]
struct NormalizedBatch {
    chats: Vec<Chat>,
    senders: Vec<crate::domain::Sender>,
    records: Vec<IngestRecord>,
    account_deletions: Vec<AccountDeletion>,
    seen_chats: HashSet<i64>,
    seen_senders: HashSet<i64>,
    account: Option<crate::domain::SenderId>,
}

impl NormalizedBatch {
    fn include_chat(&mut self, chat: Chat) {
        if self.seen_chats.insert(chat.id.get()) {
            self.chats.push(chat);
        }
    }

    fn include_sender(&mut self, sender: crate::domain::Sender) {
        if self.seen_senders.insert(sender.id.get()) {
            self.senders.push(sender);
        }
    }

    fn add_update(
        &mut self,
        client: &Client,
        raw: tl::enums::Update,
        state: State,
        peers: PeerMap,
    ) -> Result<(), mapper::MappingError> {
        let update = Update::from_raw(client, raw, state, peers);
        let collected_at = Utc::now();
        match update {
            Update::NewMessage(message) => self.add_message(&message, false, collected_at)?,
            Update::MessageEdited(message) => self.add_message(&message, true, collected_at)?,
            Update::MessageDeleted(deletion) => {
                self.add_deletion(&deletion.raw, collected_at)?;
            }
            // This archive stores messages; acknowledge other update kinds without inventing
            // message records. Their pts still belongs to this fully processed stream batch.
            _ => {}
        }
        Ok(())
    }

    fn add_deletion(
        &mut self,
        raw: &tl::enums::Update,
        deleted_at: chrono::DateTime<Utc>,
    ) -> Result<(), mapper::MappingError> {
        match raw {
            tl::enums::Update::DeleteMessages(update) => {
                self.account_deletions.extend(
                    update
                        .messages
                        .iter()
                        .map(|id| {
                            MessageId::new(i64::from(*id)).map(|message_id| AccountDeletion {
                                message_id,
                                deleted_at,
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                );
            }
            tl::enums::Update::DeleteChannelMessages(update) => {
                let peer_id = grammers_session::types::PeerId::channel(update.channel_id)
                    .ok_or(mapper::MappingError::InvalidPeerId)?;
                let chat_id = mapper::chat_id(peer_id)?;
                for id in &update.messages {
                    self.records.push(IngestRecord {
                        event: MessageEvent::Deleted {
                            chat_id,
                            message_id: MessageId::new(i64::from(*id))?,
                            deleted_at,
                        },
                        source: MessageSource::Realtime,
                    });
                }
            }
            _ => unreachable!("only deletion updates reach add_deletion"),
        }
        Ok(())
    }

    fn add_message(
        &mut self,
        message: &grammers_client::update::Message,
        edited: bool,
        collected_at: chrono::DateTime<Utc>,
    ) -> Result<(), mapper::MappingError> {
        let chat_id = mapper::chat_id(message.peer_id())?;
        if let Some(peer) = message.peer() {
            self.include_chat(mapper::map_chat(peer)?);
        } else {
            self.include_chat(Chat {
                id: chat_id,
                kind: chat_kind(message.peer_id()),
                title: None,
                username: None,
                tracked: false,
            });
        }
        if let Some(sender_id) = message.sender_id() {
            self.include_sender(mapper::map_sender(sender_id, message.sender())?);
        }
        let mapped = mapper::map_message(message, collected_at, self.account)?;
        self.records.push(IngestRecord {
            event: if edited {
                MessageEvent::Updated(mapped)
            } else {
                MessageEvent::Created(mapped)
            },
            source: MessageSource::Realtime,
        });
        Ok(())
    }

    fn into_ingest_batch(self) -> IngestBatch {
        IngestBatch {
            chats: self.chats,
            senders: self.senders,
            records: self.records,
            account_deletions: self.account_deletions,
            ..IngestBatch::default()
        }
    }
}

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
        let mut batch = NormalizedBatch {
            account,
            ..NormalizedBatch::default()
        };
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

fn chat_kind(peer: grammers_session::types::PeerId) -> ChatKind {
    match peer.kind() {
        grammers_session::types::PeerKind::User => ChatKind::Private,
        grammers_session::types::PeerKind::Chat => ChatKind::Group,
        grammers_session::types::PeerKind::Channel => ChatKind::Channel,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    use tokio::sync::{Semaphore, oneshot};

    use crate::application::{ArchiveWriter, RepositoryError, ingestion_worker};

    use super::super::file_session::FileSession;
    use grammers_session::{Session, updates::UpdatesLike};

    /// Fixed scope; the original tests assume the updates they feed are in scope.
    #[derive(Default)]
    struct FixedScope {
        tracked: HashSet<crate::domain::ChatId>,
        common: HashSet<MessageId>,
    }

    #[async_trait::async_trait]
    impl TrackingScope for FixedScope {
        async fn tracked_chat_ids(
            &self,
        ) -> Result<HashSet<crate::domain::ChatId>, RepositoryError> {
            Ok(self.tracked.clone())
        }
        async fn archived_common_message_ids(
            &self,
            ids: &[MessageId],
        ) -> Result<HashSet<MessageId>, RepositoryError> {
            Ok(ids
                .iter()
                .copied()
                .filter(|id| self.common.contains(id))
                .collect())
        }
    }

    fn in_scope() -> FixedScope {
        FixedScope {
            tracked: [crate::domain::ChatId::from_telegram(ChatKind::Channel, 90).unwrap()].into(),
            common: [MessageId::new(7).unwrap()].into(),
        }
    }

    #[test]
    fn raw_common_delete_has_no_guessed_chat_and_channel_delete_keeps_channel_scope() {
        let now = Utc::now();
        let mut batch = NormalizedBatch::default();
        batch
            .add_deletion(
                &tl::enums::Update::DeleteMessages(tl::types::UpdateDeleteMessages {
                    messages: vec![12, 13],
                    pts: 2,
                    pts_count: 2,
                }),
                now,
            )
            .unwrap();
        assert_eq!(batch.account_deletions.len(), 2);
        assert!(batch.records.is_empty());

        batch
            .add_deletion(
                &tl::enums::Update::DeleteChannelMessages(tl::types::UpdateDeleteChannelMessages {
                    channel_id: 90,
                    messages: vec![14],
                    pts: 1,
                    pts_count: 1,
                }),
                now,
            )
            .unwrap();
        assert_eq!(batch.account_deletions.len(), 2);
        assert!(matches!(
            batch.records.as_slice(),
            [IngestRecord {
                event: MessageEvent::Deleted { chat_id, message_id, .. },
                source: MessageSource::Realtime,
            }] if chat_id.get() == -1_000_000_000_090 && message_id.get() == 14
        ));
    }

    struct GateWriter {
        entered: Semaphore,
        release: tokio::sync::Mutex<Option<oneshot::Receiver<()>>>,
        batch: Mutex<Option<IngestBatch>>,
    }

    #[async_trait::async_trait]
    impl ArchiveWriter for GateWriter {
        async fn write_batch(&self, batch: IngestBatch) -> Result<(), RepositoryError> {
            *self.batch.lock().unwrap() = Some(batch);
            self.entered.add_permits(1);
            self.release
                .lock()
                .await
                .take()
                .expect("one writer batch")
                .await
                .map_err(|_| RepositoryError::Unavailable("test ACK gate closed".into()))
        }
    }

    fn two_scope_update_batch() -> UpdatesLike {
        UpdatesLike::Updates(tl::enums::Updates::Updates(tl::types::Updates {
            updates: vec![
                tl::enums::Update::DeleteMessages(tl::types::UpdateDeleteMessages {
                    messages: vec![7],
                    pts: 1,
                    pts_count: 1,
                }),
                tl::enums::Update::DeleteChannelMessages(tl::types::UpdateDeleteChannelMessages {
                    channel_id: 90,
                    messages: vec![8],
                    pts: 1,
                    pts_count: 1,
                }),
            ],
            users: Vec::new(),
            chats: Vec::new(),
            date: 1_700_000_001,
            seq: 0,
        }))
    }

    async fn stream_fixture(
        session: Arc<FileSession>,
        updates: UpdatesLike,
    ) -> (Client, UpdateStream) {
        let pool = grammers_client::SenderPool::with_configuration(
            session,
            12345,
            super::super::session::connection_params(),
        );
        let client = Client::with_configuration(
            pool.handle.clone(),
            grammers_client::client::ClientConfiguration {
                retry_policy: Box::new(grammers_client::client::NoRetries),
                auto_cache_peers: true,
            },
        );
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        sender.send(updates).unwrap();
        let stream = client
            .stream_updates(
                receiver,
                UpdatesConfiguration {
                    catch_up: false,
                    update_queue_limit: None,
                },
            )
            .await
            .unwrap();
        (client, stream)
    }

    #[tokio::test]
    async fn aggregate_checkpoint_waits_for_full_stream_batch_archive_ack() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        let session = Arc::new(FileSession::open(&path).await.unwrap());
        let (client, mut stream) = stream_fixture(session.clone(), two_scope_update_batch()).await;
        let (release, receiver) = oneshot::channel();
        let writer = Arc::new(GateWriter {
            entered: Semaphore::new(0),
            release: tokio::sync::Mutex::new(Some(receiver)),
            batch: Mutex::new(None),
        });
        let (sink, worker) = ingestion_worker::spawn(writer.clone(), 1);
        let cancellation = CancellationToken::new();
        let scope = in_scope();
        let process = process_stream(
            &client,
            &mut stream,
            &sink,
            &scope,
            cancellation.clone(),
            None,
        );
        let observe = async {
            writer.entered.acquire().await.unwrap().forget();
            assert_eq!(session.updates_state().await.unwrap(), Default::default());
            let batch = writer.batch.lock().unwrap().clone().unwrap();
            assert_eq!(batch.account_deletions.len(), 1);
            assert_eq!(batch.records.len(), 1);
            assert!(matches!(
                batch.records[0].event,
                MessageEvent::Deleted { .. }
            ));
            release.send(()).unwrap();
        };
        let (result, ()) = tokio::join!(process, observe);
        assert!(matches!(
            result,
            Err(RealtimeError::Telegram(
                grammers_client::InvocationError::Dropped
            ))
        ));
        let state = session.updates_state().await.unwrap();
        assert_eq!(state.pts, 1);
        assert_eq!(state.channels.len(), 1);
        assert_eq!(state.channels[0].id, 90);
        assert_eq!(state.channels[0].pts, 1);

        drop(sink);
        worker.await.unwrap().unwrap();
        let _ = std::fs::remove_file(path);
    }

    fn common_delete_update() -> UpdatesLike {
        UpdatesLike::Updates(tl::enums::Updates::Updates(tl::types::Updates {
            updates: vec![tl::enums::Update::DeleteMessages(
                tl::types::UpdateDeleteMessages {
                    messages: vec![7],
                    pts: 1,
                    pts_count: 1,
                },
            )],
            users: Vec::new(),
            chats: Vec::new(),
            date: 1_700_000_001,
            seq: 0,
        }))
    }

    fn session_path() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        (dir, path)
    }

    /// Optionally parks before the commit; always reports once the real store has committed.
    struct CrashWriter {
        inner: Arc<crate::infrastructure::persistence::sqlite::SqliteStore>,
        hang_before_commit: bool,
        entered: tokio::sync::Notify,
        committed: tokio::sync::Notify,
    }

    #[async_trait::async_trait]
    impl ArchiveWriter for CrashWriter {
        async fn write_batch(&self, batch: IngestBatch) -> Result<(), RepositoryError> {
            self.entered.notify_one();
            if self.hang_before_commit {
                std::future::pending::<()>().await;
            }
            self.inner.write_batch(batch).await?;
            self.committed.notify_one();
            Ok(())
        }
    }

    async fn bound_store() -> (
        tempfile::TempDir,
        Arc<crate::infrastructure::persistence::sqlite::SqliteStore>,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let url = format!("sqlite://{}", directory.path().join("archive.db").display());
        let store = Arc::new(
            crate::infrastructure::persistence::sqlite::SqliteStore::connect(&url)
                .await
                .unwrap(),
        );
        store
            .bind_telegram_account(crate::domain::SenderId::from_marked(1).unwrap())
            .await
            .unwrap();
        (directory, store)
    }

    /// Runs one stream attempt that is abandoned (as a crash would) at the chosen point.
    async fn crashed_attempt(
        path: &std::path::Path,
        store: Arc<crate::infrastructure::persistence::sqlite::SqliteStore>,
        hang_before_commit: bool,
    ) {
        let session = Arc::new(FileSession::open(path).await.unwrap());
        let (client, mut stream) = stream_fixture(session.clone(), common_delete_update()).await;
        let writer = Arc::new(CrashWriter {
            inner: store,
            hang_before_commit,
            entered: tokio::sync::Notify::new(),
            committed: tokio::sync::Notify::new(),
        });
        let (sink, worker) = ingestion_worker::spawn(writer.clone(), 1);
        {
            let scope = in_scope();
            let process = process_stream(
                &client,
                &mut stream,
                &sink,
                &scope,
                CancellationToken::new(),
                None,
            );
            tokio::pin!(process);
            tokio::select! {
                biased;
                _ = async {
                    if hang_before_commit {
                        writer.entered.notified().await;
                    } else {
                        writer.committed.notified().await;
                    }
                } => {}
                _ = &mut process => panic!("stream finished before the simulated crash"),
            }
        }
        assert_eq!(session.updates_state().await.unwrap(), Default::default());
        worker.abort();
        let _ = worker.await;
    }

    async fn restart_and_process(
        path: &std::path::Path,
        store: Arc<crate::infrastructure::persistence::sqlite::SqliteStore>,
    ) {
        let session = Arc::new(FileSession::open(path).await.unwrap());
        assert_eq!(session.updates_state().await.unwrap(), Default::default());
        let (client, mut stream) = stream_fixture(session.clone(), common_delete_update()).await;
        let (sink, worker) = ingestion_worker::spawn(store, 1);
        let result = process_stream(
            &client,
            &mut stream,
            &sink,
            &in_scope(),
            CancellationToken::new(),
            None,
        )
        .await;
        assert!(matches!(
            result,
            Err(RealtimeError::Telegram(
                grammers_client::InvocationError::Dropped
            ))
        ));
        assert_eq!(session.updates_state().await.unwrap().pts, 1);
        drop(sink);
        worker.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn crash_before_archive_commit_keeps_checkpoint_and_restart_reprocesses() {
        let (_directory, store) = bound_store().await;
        let (_dir, path) = session_path();
        crashed_attempt(&path, store.clone(), true).await;
        assert_eq!(store.unresolved_common_deletion_count().await.unwrap(), 0);
        restart_and_process(&path, store.clone()).await;
        assert_eq!(store.unresolved_common_deletion_count().await.unwrap(), 1);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn crash_after_commit_before_checkpoint_replays_without_duplicates() {
        let (_directory, store) = bound_store().await;
        let (_dir, path) = session_path();
        crashed_attempt(&path, store.clone(), false).await;
        assert_eq!(store.unresolved_common_deletion_count().await.unwrap(), 1);
        restart_and_process(&path, store.clone()).await;
        assert_eq!(store.unresolved_common_deletion_count().await.unwrap(), 1);
        let _ = std::fs::remove_file(path);
    }

    struct CountingWriter(Mutex<Vec<IngestBatch>>);

    #[async_trait::async_trait]
    impl ArchiveWriter for CountingWriter {
        async fn write_batch(&self, batch: IngestBatch) -> Result<(), RepositoryError> {
            self.0.lock().unwrap().push(batch);
            Ok(())
        }
    }

    #[tokio::test]
    async fn untracked_updates_write_nothing_but_still_advance_the_update_checkpoint() {
        let (_dir, path) = session_path();
        let session = Arc::new(FileSession::open(&path).await.unwrap());
        let (client, mut stream) = stream_fixture(session.clone(), two_scope_update_batch()).await;
        let writer = Arc::new(CountingWriter(Mutex::new(Vec::new())));
        let (sink, worker) = ingestion_worker::spawn(writer.clone(), 1);
        let result = process_stream(
            &client,
            &mut stream,
            &sink,
            &FixedScope::default(),
            CancellationToken::new(),
            None,
        )
        .await;
        assert!(matches!(
            result,
            Err(RealtimeError::Telegram(
                grammers_client::InvocationError::Dropped
            ))
        ));
        assert!(writer.0.lock().unwrap().is_empty());
        let state = session.updates_state().await.unwrap();
        assert_eq!(state.pts, 1);
        assert_eq!(state.channels.len(), 1);
        assert_eq!(state.channels[0].pts, 1);
        drop(sink);
        worker.await.unwrap().unwrap();
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn dropped_io_and_transient_rpc_reconnect_but_auth_and_storage_are_fatal() {
        use grammers_client::InvocationError as Invocation;
        let rpc = |code, name: &str| {
            RealtimeError::Telegram(Invocation::Rpc(grammers_client::sender::RpcError {
                code,
                name: name.into(),
                value: None,
                caused_by: None,
            }))
        };
        for transient in [
            RealtimeError::Telegram(Invocation::Dropped),
            RealtimeError::Telegram(Invocation::Io(std::io::Error::other("reset"))),
            rpc(420, "FLOOD_WAIT"),
            rpc(500, "INTERNAL"),
        ] {
            assert!(matches!(
                classify(transient),
                RealtimeSourceError::Transient(_)
            ));
        }
        for fatal in [
            rpc(401, "AUTH_KEY_UNREGISTERED"),
            RealtimeError::ReceiverUnavailable,
            RealtimeError::Archive(RepositoryError::Unavailable("disk".into())),
            RealtimeError::Checkpoint("write".into()),
        ] {
            assert!(matches!(classify(fatal), RealtimeSourceError::Fatal(_)));
        }
    }

    #[test]
    fn difference_too_long_is_a_reconciliation_not_a_fatal_error() {
        // Formerly asserted fatal; it is now recoverable by resetting state and catching up.
        assert!(matches!(
            classify(RealtimeError::DifferenceTooLong(None)),
            RealtimeSourceError::GapReset(GapScope::Account)
        ));
        let channel = crate::domain::ChatId::from_telegram(ChatKind::Channel, 77).unwrap();
        assert_eq!(
            classify(RealtimeError::DifferenceTooLong(Some(77))),
            RealtimeSourceError::GapReset(GapScope::Channel(channel))
        );
    }

    #[test]
    fn synthetic_errors_map_to_their_scope() {
        use grammers_client::sender::RpcError;
        let rpc = |name: String| {
            grammers_client::InvocationError::Rpc(RpcError {
                code: 500,
                name,
                value: None,
                caused_by: None,
            })
        };
        assert!(matches!(
            map_stream_error(rpc(
                grammers_client::client::ARCHIVE_DIFFERENCE_TOO_LONG.into()
            )),
            RealtimeError::DifferenceTooLong(None)
        ));
        let name = format!(
            "{}_1234567890123",
            grammers_client::client::ARCHIVE_CHANNEL_DIFFERENCE_TOO_LONG
        );
        assert!(matches!(
            map_stream_error(rpc(name)),
            RealtimeError::DifferenceTooLong(Some(1_234_567_890_123))
        ));
    }

    fn state(pts: i32, channels: &[(i64, i32)]) -> UpdatesState {
        UpdatesState {
            pts,
            qts: 1,
            date: 10,
            seq: 2,
            channels: channels
                .iter()
                .map(|&(id, pts)| grammers_session::types::ChannelState { id, pts })
                .collect(),
        }
    }

    #[tokio::test]
    async fn account_reset_persists_fresh_state_and_drops_channel_states() {
        let (_dir, path) = session_path();
        let session = FileSession::open(&path).await.unwrap();
        session
            .set_update_state(UpdateState::All(state(5, &[(1, 3), (2, 4)])))
            .await
            .unwrap();
        reset_session_state(&session, None, Some(state(900, &[])))
            .await
            .unwrap();
        drop(session);
        let reopened = FileSession::open(&path).await.unwrap();
        assert_eq!(reopened.updates_state().await.unwrap(), state(900, &[]));
    }

    #[tokio::test]
    async fn channel_reset_drops_only_that_channel_and_persists() {
        let (_dir, path) = session_path();
        let session = FileSession::open(&path).await.unwrap();
        session
            .set_update_state(UpdateState::All(state(5, &[(1, 3), (2, 4)])))
            .await
            .unwrap();
        reset_session_state(&session, Some(1), None).await.unwrap();
        drop(session);
        let reopened = FileSession::open(&path).await.unwrap();
        assert_eq!(reopened.updates_state().await.unwrap(), state(5, &[(2, 4)]));
    }

    /// The Grammers message box drives the stream: socket updates are read only once no
    /// difference is pending. A reset that drops channel X leaves the common box intact, so
    /// after the remaining differences finish, a common-box update is yielded.
    #[test]
    fn after_a_channel_reset_pending_differences_gate_live_updates_then_common_updates_flow() {
        use grammers_session::updates::{MessageBoxes, UpdatesLike};
        let mut boxes = MessageBoxes::load(state(10, &[(2, 4)]));
        // Common and the remaining channel are still catching up: nothing is read from the socket.
        assert!(boxes.get_difference().is_some());
        boxes.apply_difference(tl::types::updates::DifferenceEmpty { date: 11, seq: 2 }.into());
        assert!(boxes.get_difference().is_none());
        assert!(boxes.get_channel_difference().is_some());
        boxes.apply_channel_difference(
            tl::types::updates::ChannelDifferenceEmpty {
                r#final: true,
                pts: 4,
                timeout: None,
            }
            .into(),
        );
        assert!(boxes.get_channel_difference().is_none());

        let update = tl::enums::Update::NewMessage(tl::types::UpdateNewMessage {
            message: tl::types::MessageEmpty {
                id: 1,
                peer_id: None,
            }
            .into(),
            pts: 11,
            pts_count: 1,
        });
        let (updates, _, _) = boxes
            .process_updates(UpdatesLike::Updates(tl::enums::Updates::Updates(
                tl::types::Updates {
                    updates: vec![update],
                    users: vec![],
                    chats: vec![],
                    date: 12,
                    seq: 0,
                },
            )))
            .unwrap();
        assert_eq!(updates.len(), 1);
        assert_eq!(boxes.session_state().pts, 11);
    }

    type Log = Arc<Mutex<Vec<String>>>;
    type Runs = Vec<Result<(), ConnectionFailure>>;
    type Script = Vec<Result<Runs, RealtimeSourceError>>;

    struct FakeConnection {
        id: usize,
        log: Log,
        runs: std::collections::VecDeque<Result<(), ConnectionFailure>>,
    }

    #[async_trait::async_trait]
    impl RealtimeConnection for FakeConnection {
        async fn run(&mut self, cancel: CancellationToken) -> Result<(), ConnectionFailure> {
            self.log.lock().unwrap().push(format!("run {}", self.id));
            match self.runs.pop_front() {
                Some(result) => result,
                None => {
                    cancel.cancelled().await;
                    Ok(())
                }
            }
        }

        async fn teardown(&mut self) {
            self.log
                .lock()
                .unwrap()
                .push(format!("teardown {}", self.id));
        }
    }

    struct FakeConnector {
        log: Log,
        next_id: Mutex<usize>,
        /// Per connect call: `Err` fails it, `Ok` lists the scripted run results.
        script: Mutex<std::collections::VecDeque<Result<Runs, RealtimeSourceError>>>,
        cancel_during_connect: Option<CancellationToken>,
    }

    #[async_trait::async_trait]
    impl RealtimeConnector for FakeConnector {
        type Connection = FakeConnection;

        async fn connect(&self) -> Result<FakeConnection, RealtimeSourceError> {
            let id = {
                let mut next = self.next_id.lock().unwrap();
                *next += 1;
                *next
            };
            self.log.lock().unwrap().push(format!("connect {id}"));
            if let Some(token) = &self.cancel_during_connect {
                token.cancel();
            }
            let runs = self
                .script
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok(vec![]))?;
            Ok(FakeConnection {
                id,
                log: self.log.clone(),
                runs: runs.into(),
            })
        }
    }

    fn source(
        script: Script,
        cancel_during_connect: Option<CancellationToken>,
    ) -> (ReconnectingSource<FakeConnector>, Log) {
        let log = Log::default();
        let source = ReconnectingSource::with_connector(FakeConnector {
            log: log.clone(),
            next_id: Mutex::new(0),
            script: Mutex::new(script.into()),
            cancel_during_connect,
        });
        (source, log)
    }

    fn failure(message: &str, reusable: bool) -> Result<(), ConnectionFailure> {
        Err(ConnectionFailure {
            error: RealtimeSourceError::Transient(message.into()),
            reusable,
        })
    }

    fn entries(log: &Log) -> Vec<String> {
        log.lock().unwrap().clone()
    }

    #[tokio::test]
    async fn dropped_connection_is_torn_down_before_a_new_one_is_built_and_run() {
        let (source, log) = source(vec![Ok(vec![failure("dropped", false)]), Ok(vec![])], None);
        let first = source.run_once(CancellationToken::new()).await;
        assert!(matches!(first, Err(RealtimeSourceError::Transient(_))));

        let cancel = CancellationToken::new();
        let token = cancel.clone();
        let second = tokio::spawn(async move { source.run_once(token).await });
        tokio::task::yield_now().await;
        cancel.cancel();
        assert_eq!(second.await.unwrap(), Ok(()));
        assert_eq!(
            entries(&log),
            ["connect 1", "run 1", "teardown 1", "connect 2", "run 2"]
        );
    }

    #[tokio::test]
    async fn transient_error_on_live_stream_keeps_the_connection() {
        let (source, log) = source(vec![Ok(vec![failure("flood", true)])], None);
        assert!(source.run_once(CancellationToken::new()).await.is_err());
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert_eq!(source.run_once(cancel).await, Ok(()));
        assert_eq!(entries(&log), ["connect 1", "run 1", "run 1"]);
    }

    #[tokio::test]
    async fn transient_rebuild_failure_is_retried_on_next_attempt() {
        let (source, log) = source(
            vec![
                Err(RealtimeSourceError::Transient("net down".into())),
                Ok(vec![]),
            ],
            None,
        );
        assert_eq!(
            source.run_once(CancellationToken::new()).await,
            Err(RealtimeSourceError::Transient("net down".into()))
        );
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        let second = tokio::spawn(async move { source.run_once(token).await });
        tokio::task::yield_now().await;
        cancel.cancel();
        assert_eq!(second.await.unwrap(), Ok(()));
        assert_eq!(entries(&log), ["connect 1", "connect 2", "run 2"]);
    }

    #[tokio::test]
    async fn fatal_rebuild_failure_propagates() {
        let (source, _log) = source(
            vec![Err(RealtimeSourceError::Fatal("unauthorized".into()))],
            None,
        );
        assert_eq!(
            source.run_once(CancellationToken::new()).await,
            Err(RealtimeSourceError::Fatal("unauthorized".into()))
        );
    }

    #[tokio::test]
    async fn cancel_during_rebuild_tears_down_and_returns_ok() {
        let cancel = CancellationToken::new();
        let (source, log) = source(vec![Ok(vec![])], Some(cancel.clone()));
        assert_eq!(source.run_once(cancel).await, Ok(()));
        assert_eq!(entries(&log), ["connect 1", "teardown 1"]);
    }
}

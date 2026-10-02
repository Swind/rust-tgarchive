use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use grammers_client::{
    Client, SenderPool,
    client::{ClientConfiguration, NoRetries},
    peer::Peer,
    sender::ConnectionParams,
};
use grammers_session::{Session, types::PeerRef, updates::UpdatesLike};
use tokio::{
    sync::{Mutex as AsyncMutex, mpsc::UnboundedReceiver},
    task::JoinHandle,
};

use crate::{application::TelegramError, domain::SenderId};

use super::file_session::FileSession;
use super::owner_lock::AccountOwnerLock;

pub struct TelegramAdapter {
    client: RwLock<Client>,
    pub(crate) session: Arc<FileSession>,
    session_path: PathBuf,
    api_id: i32,
    pub(crate) updates: AsyncMutex<Option<UnboundedReceiver<UpdatesLike>>>,
    runner: AsyncMutex<Option<JoinHandle<()>>>,
}

#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    #[error("could not acquire Telegram session ownership: {0}")]
    OwnerLock(#[source] std::io::Error),
    #[error("could not open Telegram session: {0}")]
    Session(String),
}

impl TelegramAdapter {
    /// Opens a private persistent session and starts Grammers' sender runner.
    ///
    /// Keep the returned adapter alive while performing API calls. Its worker owns the account
    /// lock until the sender runner stops, preventing concurrent use of the session by processes.
    /// The raw update receiver remains owned by the adapter for sequential realtime processing.
    pub async fn open(api_id: i32, session_path: impl AsRef<Path>) -> Result<Self, OpenError> {
        let session_path = session_path.as_ref().to_path_buf();
        let session = Arc::new(
            FileSession::open(&session_path)
                .await
                .map_err(|error| OpenError::Session(error.to_string()))?,
        );
        let (client, updates, runner) = start_pool(api_id, &session_path, &session)?;
        Ok(Self {
            client: RwLock::new(client),
            session,
            session_path,
            api_id,
            updates: AsyncMutex::new(Some(updates)),
            runner: AsyncMutex::new(Some(runner)),
        })
    }

    /// The current client. Callers must not cache it across a possible `reconnect`.
    pub(crate) fn client(&self) -> Client {
        self.client.read().expect("client lock poisoned").clone()
    }

    /// Replaces a stopped sender pool with a fresh client and update receiver over the same
    /// `FileSession`, which carries the archive-acknowledged update state. The old pool is
    /// joined first so the owner lock is released and two clients never share the session.
    pub(crate) async fn reconnect(&self) -> Result<(), OpenError> {
        self.shutdown().await.map_err(|error| {
            OpenError::Session(format!(
                "previous sender pool did not stop cleanly: {error}"
            ))
        })?;
        let (client, updates, runner) = start_pool(self.api_id, &self.session_path, &self.session)?;
        *self.client.write().expect("client lock poisoned") = client;
        *self.updates.lock().await = Some(updates);
        *self.runner.lock().await = Some(runner);
        Ok(())
    }

    pub async fn is_authorized(&self) -> Result<bool, grammers_client::InvocationError> {
        self.client().is_authorized().await
    }

    /// Resolves and validates the authenticated user identity for archive binding.
    pub async fn authenticated_account_id(&self) -> Result<SenderId, TelegramError> {
        let user = self.client().get_me().await.map_err(|error| {
            TelegramError::Unavailable(format!("could not resolve Telegram account: {error}"))
        })?;
        super::mapper::sender_id(user.id()).map_err(|error| {
            TelegramError::Unavailable(format!("invalid Telegram account identity: {error}"))
        })
    }

    pub(crate) async fn resolve_chat(
        &self,
        marked_id: i64,
    ) -> Result<(Peer, PeerRef), ResolvePeerError> {
        let peer_id = grammers_session::types::PeerId::from_bot_api_dialog_id(marked_id)
            .ok_or(ResolvePeerError::InvalidId)?;
        let peer_ref = self
            .session
            .peer_ref(peer_id)
            .await
            .map_err(|error| ResolvePeerError::Session(Box::new(error)))?
            .ok_or(ResolvePeerError::NotCached)?;
        let peer = self
            .client()
            .resolve_peer(peer_ref)
            .await
            .map_err(ResolvePeerError::Telegram)?;
        Ok((peer, peer_ref))
    }

    /// Stops the current sender runner and releases account ownership after it has shut down.
    pub async fn shutdown(&self) -> Result<(), tokio::task::JoinError> {
        self.client().disconnect();
        let mut runner = self.runner.lock().await;
        if let Some(task) = runner.as_mut() {
            let result = task.await;
            runner.take();
            result?;
        }
        Ok(())
    }
}

type Pool = (Client, UnboundedReceiver<UpdatesLike>, JoinHandle<()>);

/// Acquires the account lock and starts a sender pool; the runner task owns the lock until it ends.
fn start_pool(
    api_id: i32,
    session_path: &Path,
    session: &Arc<FileSession>,
) -> Result<Pool, OpenError> {
    let lock = AccountOwnerLock::acquire(lock_path(session_path)).map_err(OpenError::OwnerLock)?;
    let pool = SenderPool::with_configuration(Arc::clone(session), api_id, connection_params());
    let client = Client::with_configuration(
        pool.handle.clone(),
        ClientConfiguration {
            retry_policy: Box::new(NoRetries),
            auto_cache_peers: true,
        },
    );
    let pool_runner = pool.runner;
    let runner = tokio::spawn(async move {
        let _account_lock = lock;
        pool_runner.run().await;
    });
    Ok((client, pool.updates, runner))
}

/// Connection parameters built without `ConnectionParams::default()`, whose `os_info` probe spawns
/// `getconf`. A fork between `fork` and `exec` duplicates every open descriptor, including the
/// account lock's; the child then keeps the `flock` alive after the owner dropped it, so a lock
/// release (shutdown/reconnect) would not be deterministic.
pub(super) fn connection_params() -> ConnectionParams {
    ConnectionParams {
        device_model: format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
        system_version: "unknown".into(),
        app_version: env!("CARGO_PKG_VERSION").into(),
        system_lang_code: "en".into(),
        lang_code: "en".into(),
        use_ipv6: false,
        __non_exhaustive: (),
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ResolvePeerError {
    #[error("invalid archive peer identifier")]
    InvalidId,
    #[error("peer has not been learned from dialogs or another Telegram response")]
    NotCached,
    #[error("session lookup failed: {0}")]
    Session(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("Telegram request failed: {0}")]
    Telegram(#[source] grammers_client::InvocationError),
}

fn lock_path(session_path: &Path) -> PathBuf {
    let mut lock_name = session_path.as_os_str().to_owned();
    lock_name.push(".lock");
    PathBuf::from(lock_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn shutdown_waits_for_runner_then_releases_owner_lock_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        let adapter = TelegramAdapter::open(12345, &path).await.unwrap();
        assert!(AccountOwnerLock::acquire(lock_path(&path)).is_err());
        adapter.shutdown().await.unwrap();
        assert!(AccountOwnerLock::acquire(lock_path(&path)).is_ok());
        adapter.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn reconnect_replaces_pool_keeps_session_state_and_single_owner() {
        use grammers_session::{
            Session,
            types::{UpdateState, UpdatesState},
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        let adapter = TelegramAdapter::open(12345, &path).await.unwrap();
        let checkpoint = UpdatesState {
            pts: 42,
            qts: 1,
            date: 1_700_000_000,
            seq: 3,
            channels: Vec::new(),
        };
        adapter
            .session
            .set_update_state(UpdateState::All(checkpoint.clone()))
            .await
            .unwrap();
        // Single-use receiver consumed, as after a finished stream.
        adapter.updates.lock().await.take();

        adapter.reconnect().await.unwrap();

        assert!(adapter.updates.lock().await.is_some());
        assert_eq!(adapter.session.updates_state().await.unwrap(), checkpoint);
        assert!(AccountOwnerLock::acquire(lock_path(&path)).is_err());
        adapter.shutdown().await.unwrap();
        assert!(AccountOwnerLock::acquire(lock_path(&path)).is_ok());
    }
}

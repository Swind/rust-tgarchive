use std::path::{Path, PathBuf};
use std::sync::Arc;

use grammers_client::{
    Client, SenderPool,
    client::{ClientConfiguration, NoRetries},
    peer::Peer,
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
    pub(crate) client: Client,
    pub(crate) session: Arc<FileSession>,
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
        let lock =
            AccountOwnerLock::acquire(lock_path(&session_path)).map_err(OpenError::OwnerLock)?;
        let session = Arc::new(
            FileSession::open(&session_path)
                .await
                .map_err(|error| OpenError::Session(error.to_string()))?,
        );

        let pool = SenderPool::new(Arc::clone(&session), api_id);
        let client = Client::with_configuration(
            pool.handle.clone(),
            ClientConfiguration {
                retry_policy: Box::new(NoRetries),
                auto_cache_peers: true,
            },
        );
        let updates = pool.updates;
        let pool_runner = pool.runner;
        let runner = tokio::spawn(async move {
            let _account_lock = lock;
            pool_runner.run().await;
        });

        Ok(Self {
            client,
            session,
            updates: AsyncMutex::new(Some(updates)),
            runner: AsyncMutex::new(Some(runner)),
        })
    }

    pub async fn is_authorized(&self) -> Result<bool, grammers_client::InvocationError> {
        self.client.is_authorized().await
    }

    /// Resolves and validates the authenticated user identity for archive binding.
    pub async fn authenticated_account_id(&self) -> Result<SenderId, TelegramError> {
        let user = self.client.get_me().await.map_err(|error| {
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
            .client
            .resolve_peer(peer_ref)
            .await
            .map_err(ResolvePeerError::Telegram)?;
        Ok((peer, peer_ref))
    }

    /// Stops the sender runner and releases account ownership after it has shut down.
    pub async fn shutdown(&self) -> Result<(), tokio::task::JoinError> {
        self.client.disconnect();
        let mut runner = self.runner.lock().await;
        if let Some(task) = runner.as_mut() {
            let result = task.await;
            runner.take();
            result?;
        }
        Ok(())
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
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    #[tokio::test]
    async fn shutdown_waits_for_runner_then_releases_owner_lock_and_is_idempotent() {
        let path = std::env::temp_dir().join(format!(
            "telegram-adapter-session-{}-{}.json",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let adapter = TelegramAdapter::open(12345, &path).await.unwrap();
        assert!(AccountOwnerLock::acquire(lock_path(&path)).is_err());
        adapter.shutdown().await.unwrap();
        assert!(AccountOwnerLock::acquire(lock_path(&path)).is_ok());
        adapter.shutdown().await.unwrap();
        let _ = std::fs::remove_file(path);
    }
}

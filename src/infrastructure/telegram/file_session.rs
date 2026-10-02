use std::{
    collections::HashMap,
    fmt,
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};

use grammers_session::{
    BoxFuture, Session, SessionData,
    types::{ChannelState, DcOption, PeerId, PeerInfo, UpdateState, UpdatesState},
};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex as AsyncMutex;

/// Small atomic-file-backed Grammers session used to avoid linking a second bundled SQLite.
///
/// Persistent session writes are serialized and run on Tokio's blocking pool. Values are written
/// to a private sibling file, synced, atomically renamed, then the parent directory is synced.
pub struct FileSession {
    path: PathBuf,
    data: Arc<Mutex<SessionData>>,
    write_gate: Arc<AsyncMutex<()>>,
}

#[derive(Debug)]
pub struct FileSessionError(String);

impl fmt::Display for FileSessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for FileSessionError {}

#[derive(Serialize, Deserialize)]
struct PersistedSession {
    home_dc: i32,
    dc_options: Vec<DcOption>,
    peer_infos: Vec<(PeerId, PeerInfo)>,
    updates_state: UpdatesState,
}

impl FileSession {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, FileSessionError> {
        let path = path.as_ref().to_path_buf();
        let read_path = path.clone();
        let data = tokio::task::spawn_blocking(move || read_or_initialize(&read_path))
            .await
            .map_err(|error| FileSessionError(format!("session loader task failed: {error}")))??;
        Ok(Self {
            path,
            data: Arc::new(Mutex::new(data)),
            write_gate: Arc::new(AsyncMutex::new(())),
        })
    }

    fn data(&self) -> Result<MutexGuard<'_, SessionData>, FileSessionError> {
        self.data
            .lock()
            .map_err(|_| FileSessionError("session data lock is poisoned".into()))
    }

    async fn update<F>(&self, change: F) -> Result<(), FileSessionError>
    where
        F: FnOnce(&mut SessionData) + Send + 'static,
    {
        self.update_with(change, atomic_replace).await
    }

    async fn update_with<F, P>(&self, change: F, persist: P) -> Result<(), FileSessionError>
    where
        F: FnOnce(&mut SessionData) + Send + 'static,
        P: FnOnce(&Path, &[u8]) -> Result<(), FileSessionError> + Send + 'static,
    {
        let write_guard = Arc::clone(&self.write_gate).lock_owned().await;
        let data = Arc::clone(&self.data);
        let path = self.path.clone();
        let (result, _write_guard) = tokio::task::spawn_blocking(move || {
            let result = update_and_persist(&data, &path, change, persist);
            (result, write_guard)
        })
        .await
        .map_err(|error| FileSessionError(format!("session writer task failed: {error}")))?;
        result
    }
}

fn update_and_persist<F, P>(
    data: &Mutex<SessionData>,
    path: &Path,
    change: F,
    persist: P,
) -> Result<(), FileSessionError>
where
    F: FnOnce(&mut SessionData),
    P: FnOnce(&Path, &[u8]) -> Result<(), FileSessionError>,
{
    let previous = {
        let state = data
            .lock()
            .map_err(|_| FileSessionError("session data lock is poisoned".into()))?;
        encode(&state)?
    };
    let mut updated = decode(&previous)?;
    change(&mut updated);
    let bytes = encode(&updated)?;
    if let Err(error) = persist(path, &bytes) {
        // A directory sync can fail after rename already made the new file visible. Keep the
        // acknowledged in-memory mirror aligned with the file in that uncertain durability case.
        if read_existing_bytes(path).is_ok_and(|current| current == bytes) {
            *data
                .lock()
                .map_err(|_| FileSessionError("session data lock is poisoned".into()))? = updated;
        }
        return Err(error);
    }
    *data
        .lock()
        .map_err(|_| FileSessionError("session data lock is poisoned".into()))? = updated;
    Ok(())
}

fn read_existing_bytes(path: &Path) -> Result<Vec<u8>, FileSessionError> {
    let mut file = open_existing(path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

impl Session for FileSession {
    type Error = FileSessionError;

    fn home_dc_id(&self) -> Result<i32, Self::Error> {
        Ok(self.data()?.home_dc)
    }

    fn set_home_dc_id(&self, dc_id: i32) -> BoxFuture<'_, Result<(), Self::Error>> {
        Box::pin(self.update(move |data| data.home_dc = dc_id))
    }

    fn dc_option(&self, dc_id: i32) -> Result<Option<DcOption>, Self::Error> {
        Ok(self.data()?.dc_options.get(&dc_id).cloned())
    }

    fn set_dc_option(&self, dc_option: &DcOption) -> BoxFuture<'_, Result<(), Self::Error>> {
        let dc_option = dc_option.clone();
        Box::pin(self.update(move |data| {
            data.dc_options.insert(dc_option.id, dc_option);
        }))
    }

    fn peer(&self, peer: PeerId) -> BoxFuture<'_, Result<Option<PeerInfo>, Self::Error>> {
        Box::pin(async move { Ok(self.data()?.peer_infos.get(&peer).cloned()) })
    }

    fn cache_peer(&self, peer: &PeerInfo) -> BoxFuture<'_, Result<(), Self::Error>> {
        let peer = peer.clone();
        Box::pin(self.update(move |data| {
            data.peer_infos
                .entry(peer.id())
                .or_insert_with(|| peer.clone())
                .extend_info(&peer);
        }))
    }

    fn updates_state(&self) -> BoxFuture<'_, Result<UpdatesState, Self::Error>> {
        Box::pin(async move { Ok(self.data()?.updates_state.clone()) })
    }

    fn set_update_state(&self, update: UpdateState) -> BoxFuture<'_, Result<(), Self::Error>> {
        Box::pin(self.update(move |data| {
            match update {
                UpdateState::All(state) => data.updates_state = state,
                UpdateState::Primary { pts, date, seq } => {
                    data.updates_state.pts = pts;
                    data.updates_state.date = date;
                    data.updates_state.seq = seq;
                }
                UpdateState::Secondary { qts } => data.updates_state.qts = qts,
                UpdateState::Channel { id, pts } => {
                    if !data
                        .updates_state
                        .channels
                        .iter()
                        .any(|channel| channel.id == id)
                    {
                        data.updates_state.channels.push(ChannelState { id, pts });
                    }
                }
            }
        }))
    }
}

fn read_or_initialize(path: &Path) -> Result<SessionData, FileSessionError> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    match open_existing(path) {
        Ok(mut file) => {
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)?;
            if !bytes.is_empty() {
                return decode(&bytes);
            }
            let data = SessionData::default();
            atomic_replace(path, &encode(&data)?)?;
            Ok(data)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let data = SessionData::default();
            atomic_replace(path, &encode(&data)?)?;
            Ok(data)
        }
        Err(error) => Err(error.into()),
    }
}

fn open_existing(path: &Path) -> io::Result<fs::File> {
    reject_symlink(path)?;
    let file = OpenOptions::new().read(true).open(path)?;
    let opened = file.metadata()?;
    let linked = fs::symlink_metadata(path)?;
    if linked.file_type().is_symlink() || !opened.is_file() || !same_file(&opened, &linked) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Telegram session path changed while it was being opened",
        ));
    }
    set_file_private(&file)?;
    Ok(file)
}

#[cfg(unix)]
fn same_file(opened: &fs::Metadata, linked: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    opened.dev() == linked.dev() && opened.ino() == linked.ino()
}

#[cfg(not(unix))]
fn same_file(opened: &fs::Metadata, linked: &fs::Metadata) -> bool {
    opened.file_type().is_file() && linked.file_type().is_file()
}

#[cfg(unix)]
fn set_file_private(file: &fs::File) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn set_file_private(_: &fs::File) -> io::Result<()> {
    Ok(())
}

fn reject_symlink(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "refusing a symbolic-link Telegram session path",
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn encode(data: &SessionData) -> Result<Vec<u8>, FileSessionError> {
    serde_json::to_vec(&PersistedSession {
        home_dc: data.home_dc,
        dc_options: data.dc_options.values().cloned().collect(),
        peer_infos: data
            .peer_infos
            .iter()
            .map(|(id, info)| (*id, info.clone()))
            .collect(),
        updates_state: data.updates_state.clone(),
    })
    .map_err(|error| FileSessionError(format!("could not encode session: {error}")))
}

fn decode(bytes: &[u8]) -> Result<SessionData, FileSessionError> {
    let persisted: PersistedSession = serde_json::from_slice(bytes)
        .map_err(|error| FileSessionError(format!("could not decode session: {error}")))?;
    Ok(SessionData {
        home_dc: persisted.home_dc,
        dc_options: persisted
            .dc_options
            .into_iter()
            .map(|option| (option.id, option))
            .collect::<HashMap<_, _>>(),
        peer_infos: persisted.peer_infos.into_iter().collect(),
        updates_state: persisted.updates_state,
    })
}

fn atomic_replace(path: &Path, bytes: &[u8]) -> Result<(), FileSessionError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    reject_symlink(path)?;
    let (temp_path, mut file) = create_temp_file(path)?;
    let result = (|| -> io::Result<()> {
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temp_path, path)?;
        fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp_path);
    }
    result?;
    Ok(())
}

static TEMP_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn create_temp_file(path: &Path) -> io::Result<(PathBuf, fs::File)> {
    for _ in 0..100 {
        let sequence = TEMP_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut temp_name = path.as_os_str().to_os_string();
        temp_name.push(format!(".{}.{}.tmp", std::process::id(), sequence));
        let temp_path = PathBuf::from(temp_name);
        match open_private_new(&temp_path) {
            Ok(file) => return Ok((temp_path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate a private temporary session file",
    ))
}

#[cfg(unix)]
fn open_private_new(path: &Path) -> io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn open_private_new(path: &Path) -> io::Result<fs::File> {
    OpenOptions::new().create_new(true).write(true).open(path)
}

impl From<io::Error> for FileSessionError {
    fn from(error: io::Error) -> Self {
        Self(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    fn path() -> PathBuf {
        std::env::temp_dir().join(format!(
            "telegram-file-session-{}-{}.json",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[tokio::test]
    async fn session_state_survives_reopen() {
        let path = path();
        let session = FileSession::open(&path).await.unwrap();
        session.set_home_dc_id(4).await.unwrap();
        session
            .set_update_state(UpdateState::Primary {
                pts: 91,
                date: 23,
                seq: 5,
            })
            .await
            .unwrap();
        session
            .set_update_state(UpdateState::Secondary { qts: 17 })
            .await
            .unwrap();
        session
            .set_update_state(UpdateState::Channel { id: 900, pts: 7 })
            .await
            .unwrap();
        session
            .cache_peer(&PeerInfo::User {
                id: 42,
                auth: Some(grammers_session::types::PeerAuth::default()),
                bot: Some(false),
                is_self: Some(false),
            })
            .await
            .unwrap();
        drop(session);

        let reopened = FileSession::open(&path).await.unwrap();
        assert_eq!(reopened.home_dc_id().unwrap(), 4);
        assert!(
            reopened
                .peer(PeerId::user(42).unwrap())
                .await
                .unwrap()
                .is_some()
        );
        assert_eq!(
            reopened.updates_state().await.unwrap(),
            UpdatesState {
                pts: 91,
                qts: 17,
                date: 23,
                seq: 5,
                channels: vec![ChannelState { id: 900, pts: 7 }],
            }
        );
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn dialog_channel_state_initializes_only_and_cannot_advance_acknowledged_cursor() {
        let path = path();
        let session = FileSession::open(&path).await.unwrap();
        session
            .set_update_state(UpdateState::Channel { id: 900, pts: 7 })
            .await
            .unwrap();
        session
            .set_update_state(UpdateState::Channel { id: 900, pts: 99 })
            .await
            .unwrap();
        session
            .set_update_state(UpdateState::Channel { id: 901, pts: 11 })
            .await
            .unwrap();

        let state = session.updates_state().await.unwrap();
        assert_eq!(
            state.channels,
            vec![
                ChannelState { id: 900, pts: 7 },
                ChannelState { id: 901, pts: 11 }
            ]
        );
        let _ = fs::remove_file(path);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn session_and_replacement_file_are_private() {
        use std::os::unix::fs::PermissionsExt;

        let path = path();
        let session = FileSession::open(&path).await.unwrap();
        session.set_home_dc_id(2).await.unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        drop(session);
        let _reopened = FileSession::open(&path).await.unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn malformed_session_is_rejected_without_overwrite() {
        let path = path();
        fs::write(&path, b"not-json").unwrap();
        assert!(FileSession::open(&path).await.is_err());
        assert_eq!(fs::read(&path).unwrap(), b"not-json");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn cancelled_write_finishes_and_failed_write_keeps_acknowledged_state() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let path = path();
        let session = Arc::new(FileSession::open(&path).await.unwrap());
        let background = Arc::clone(&session);
        let writer_started = Arc::new(AtomicBool::new(false));
        let writer_started_signal = Arc::clone(&writer_started);
        let cancelled = tokio::spawn(async move {
            background
                .update_with(
                    |data| data.home_dc = 4,
                    move |path, bytes| {
                        writer_started_signal.store(true, Ordering::Release);
                        std::thread::sleep(std::time::Duration::from_millis(40));
                        atomic_replace(path, bytes)
                    },
                )
                .await
        });
        while !writer_started.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
        cancelled.abort();
        let _ = cancelled.await;
        tokio::time::sleep(std::time::Duration::from_millis(80)).await;
        assert_eq!(session.home_dc_id().unwrap(), 4);
        assert_eq!(
            FileSession::open(&path)
                .await
                .unwrap()
                .home_dc_id()
                .unwrap(),
            4
        );

        let failure = session
            .update_with(
                |data| data.home_dc = 5,
                |_, _| Err(FileSessionError("injected write failure".into())),
            )
            .await;
        assert!(failure.is_err());
        assert_eq!(session.home_dc_id().unwrap(), 4);
        assert_eq!(
            FileSession::open(&path)
                .await
                .unwrap()
                .home_dc_id()
                .unwrap(),
            4
        );

        let uncertain_durability = session
            .update_with(
                |data| data.home_dc = 6,
                |path, bytes| {
                    atomic_replace(path, bytes)?;
                    Err(FileSessionError("injected post-rename sync failure".into()))
                },
            )
            .await;
        assert!(uncertain_durability.is_err());
        assert_eq!(session.home_dc_id().unwrap(), 6);
        assert_eq!(
            FileSession::open(&path)
                .await
                .unwrap()
                .home_dc_id()
                .unwrap(),
            6
        );
        let _ = fs::remove_file(path);
    }
}

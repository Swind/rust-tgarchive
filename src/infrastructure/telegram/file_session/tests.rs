use super::*;
fn path() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.json");
    (dir, path)
}

#[tokio::test]
async fn session_state_survives_reopen() {
    let (_dir, path) = path();
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
    let (_dir, path) = path();
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

    let (_dir, path) = path();
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
    let (_dir, path) = path();
    fs::write(&path, b"not-json").unwrap();
    assert!(FileSession::open(&path).await.is_err());
    assert_eq!(fs::read(&path).unwrap(), b"not-json");
    let _ = fs::remove_file(path);
}

#[tokio::test]
async fn cancelled_write_finishes_and_failed_write_keeps_acknowledged_state() {
    let (_dir, path) = path();
    let session = Arc::new(FileSession::open(&path).await.unwrap());
    let background = Arc::clone(&session);
    let (writer_started, started) = tokio::sync::oneshot::channel();
    let (release_writer, released) = std::sync::mpsc::channel();
    let cancelled = tokio::spawn(async move {
        background
            .update_with(
                |data| data.home_dc = 4,
                move |path, bytes| {
                    writer_started.send(()).unwrap();
                    released.recv().unwrap();
                    atomic_replace(path, bytes)
                },
            )
            .await
    });
    started.await.unwrap();
    cancelled.abort();
    assert!(cancelled.await.unwrap_err().is_cancelled());
    release_writer.send(()).unwrap();
    // The blocking writer keeps this gate until both persistence and the in-memory update
    // finish, even when the async caller is cancelled. Wait for that boundary, not a delay.
    drop(
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            session.write_gate.lock(),
        )
        .await
        .expect("cancelled session writer did not release its gate"),
    );
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

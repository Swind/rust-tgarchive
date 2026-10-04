use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    process::{Child, Command, ExitStatus, Output, Stdio},
    thread,
    time::Duration,
};

use tempfile::TempDir;
use tgarchive::infrastructure::persistence::sqlite::SqliteStore;

const BIN: &str = env!("CARGO_BIN_EXE_tgarchive");

struct ChildGuard(Option<Child>);

impl ChildGuard {
    fn child_mut(&mut self) -> &mut Child {
        self.0.as_mut().unwrap()
    }
    fn wait_with_output(mut self) -> std::io::Result<Output> {
        self.0.take().unwrap().wait_with_output()
    }
    fn wait(mut self) -> std::io::Result<ExitStatus> {
        self.0.take().unwrap().wait()
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn database_url(directory: &TempDir) -> String {
    format!("sqlite://{}", directory.path().join("archive.db").display())
}

#[tokio::test]
async fn serve_starts_a_loopback_query_server_without_telegram_credentials() {
    let directory = tempfile::tempdir().unwrap();
    let url = database_url(&directory);
    let store = SqliteStore::connect(&url).await.unwrap();
    store.close().await;

    let reservation = TcpListener::bind("127.0.0.1:0").unwrap();
    let address: SocketAddr = reservation.local_addr().unwrap();
    drop(reservation);

    let mut server = ChildGuard(Some(
        Command::new(BIN)
            .env_clear()
            .env("TGARCHIVE_NO_DOTENV", "1")
            .env("DATABASE_URL", &url)
            .args(["serve", "--bind", &address.to_string()])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    ));

    let response = (0..50).find_map(|_| {
        if !server.child_mut().try_wait().unwrap().is_none() {
            return None;
        }
        if let Ok(mut stream) = TcpStream::connect(address) {
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            stream
                .write_all(
                    b"GET /health/ready HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).unwrap();
            return Some(response);
        }
        thread::sleep(Duration::from_millis(100));
        None
    });

    let response = response.expect("serve did not accept a loopback HTTP request");
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.contains("\"status\":\"ready\""), "{response}");
    let _ = server.child_mut().kill();
    let _ = server.wait_with_output().unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn serve_exits_cleanly_after_sigterm() {
    let directory = tempfile::tempdir().unwrap();
    let url = database_url(&directory);
    let store = SqliteStore::connect(&url).await.unwrap();
    store.close().await;
    let reservation = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = reservation.local_addr().unwrap();
    drop(reservation);

    let mut server = ChildGuard(Some(
        Command::new(BIN)
            .env_clear()
            .env("TGARCHIVE_NO_DOTENV", "1")
            .env("DATABASE_URL", &url)
            .args(["serve", "--bind", &address.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    ));
    let ready = (0..50).any(|_| {
        if server.child_mut().try_wait().unwrap().is_some() {
            return false;
        }
        if TcpStream::connect(address).is_ok() {
            return true;
        }
        thread::sleep(Duration::from_millis(100));
        false
    });
    assert!(ready, "serve never opened its loopback listener");

    let status = Command::new("kill")
        .args(["-TERM", &server.child_mut().id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    let exit = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::task::spawn_blocking(move || server.wait()),
    )
    .await
    .expect("serve did not finish graceful SIGTERM shutdown")
    .unwrap()
    .unwrap();
    assert!(exit.success(), "serve exited with {exit}");
}

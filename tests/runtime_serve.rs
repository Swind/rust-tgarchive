use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    process::{Command, Stdio},
    thread,
    time::Duration,
};

use telegram_message_archive::infrastructure::persistence::sqlite::SqliteStore;
use tempfile::TempDir;

const BIN: &str = env!("CARGO_BIN_EXE_telegram-archive");

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

    let mut server = Command::new(BIN)
        .env_clear()
        .env("DATABASE_URL", &url)
        .args(["serve", "--bind", &address.to_string()])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let response = (0..50).find_map(|_| {
        if !server.try_wait().unwrap().is_none() {
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
    let _ = server.kill();
    let _ = server.wait_with_output().unwrap();
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.contains("\"status\":\"ready\""), "{response}");
}

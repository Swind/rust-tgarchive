//! Opt-in LIVE acceptance test against a real Telegram account.
//!
//! Skipped unless `LIVE_TELEGRAM=1` plus the variables listed in `Live::from_env` are set; always
//! `#[ignore]`d. It sends, edits and deletes messages (all prefixed by a run marker) in
//! `LIVE_TEST_CHAT_ID` only. See README "Live-account acceptance test".

use std::{
    fs::File,
    net::{SocketAddr, TcpListener},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};

use grammers_client::{Client, SenderPool, message::InputMessage};
use grammers_session::types::{PeerKind, PeerRef};
use sqlx::{SqlitePool, sqlite::SqliteConnectOptions};
use tgarchive::infrastructure::telegram::file_session::FileSession;
use tokio::{io::AsyncReadExt, io::AsyncWriteExt, net::TcpStream};

const BIN: &str = env!("CARGO_BIN_EXE_tgarchive");

struct Live {
    api_id: i32,
    api_hash: String,
    driver_session: PathBuf,
    chat_id: i64,
}

impl Live {
    fn from_env() -> Option<Self> {
        if std::env::var("LIVE_TELEGRAM").ok().as_deref() != Some("1") {
            eprintln!("SKIP live_telegram: LIVE_TELEGRAM=1 not set");
            return None;
        }
        let get = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
        let mut missing = Vec::new();
        for name in [
            "TELEGRAM_API_ID",
            "TELEGRAM_API_HASH",
            "TELEGRAM_SESSION_FILE",
            "TELEGRAM_DRIVER_SESSION_FILE",
            "LIVE_TEST_CHAT_ID",
        ] {
            if get(name).is_none() {
                missing.push(name);
            }
        }
        if !missing.is_empty() {
            eprintln!("SKIP live_telegram: missing env {}", missing.join(", "));
            return None;
        }
        let (archive, driver) = (
            get("TELEGRAM_SESSION_FILE").unwrap(),
            get("TELEGRAM_DRIVER_SESSION_FILE").unwrap(),
        );
        assert_ne!(
            std::fs::canonicalize(&archive)
                .ok()
                .unwrap_or(archive.into()),
            std::fs::canonicalize(&driver)
                .ok()
                .unwrap_or(driver.clone().into()),
            "driver and archive sessions must be different files"
        );
        Some(Self {
            api_id: get("TELEGRAM_API_ID")
                .unwrap()
                .parse()
                .expect("TELEGRAM_API_ID"),
            api_hash: get("TELEGRAM_API_HASH").unwrap(),
            driver_session: driver.into(),
            chat_id: get("LIVE_TEST_CHAT_ID")
                .unwrap()
                .parse()
                .expect("LIVE_TEST_CHAT_ID"),
        })
    }
}

/// Sends/edits/deletes messages through the separate driver session. Tracks the ids it created
/// and refuses to touch any other message.
struct Driver {
    client: Client,
    peer: PeerRef,
    marker: String,
    created: Vec<i32>,
    deleted: Vec<i32>,
    _updates: tokio::sync::mpsc::UnboundedReceiver<grammers_session::updates::UpdatesLike>,
}

impl Driver {
    async fn connect(live: &Live, marker: String) -> Result<Self, String> {
        let session = Arc::new(
            FileSession::open(&live.driver_session)
                .await
                .map_err(|e| format!("open driver session: {e}"))?,
        );
        let pool = SenderPool::new(session, live.api_id);
        let client = Client::new(pool.handle.clone());
        tokio::spawn(pool.runner.run());
        if !client.is_authorized().await.map_err(|e| e.to_string())? {
            return Err("driver session is not authorized; run scripts/login-driver.sh".into());
        }
        let mut dialogs = client.iter_dialogs();
        let mut found = None;
        while let Some(dialog) = dialogs.next().await.map_err(|e| e.to_string())? {
            if dialog.peer_id().bot_api_dialog_id() == Some(live.chat_id) {
                found = Some(dialog.peer_ref());
                break;
            }
        }
        let peer = found.ok_or("LIVE_TEST_CHAT_ID not found in the driver account's dialogs")?;
        if peer.id.kind() == PeerKind::Channel
            && std::env::var("LIVE_TEST_ALLOW_CHANNEL").ok().as_deref() != Some("1")
        {
            return Err("test chat is a supergroup/channel; set LIVE_TEST_ALLOW_CHANNEL=1 to confirm it is a private test chat".into());
        }
        Ok(Self {
            client,
            peer,
            marker,
            created: Vec::new(),
            deleted: Vec::new(),
            _updates: pool.updates,
        })
    }

    fn text(&self, label: &str, rev: &str) -> String {
        format!("{} {label} {rev}", self.marker)
    }

    async fn send(&mut self, label: &str) -> Result<i32, String> {
        let text = self.text(label, "v1");
        let message = self
            .client
            .send_message(self.peer, InputMessage::new().text(text))
            .await
            .map_err(|e| format!("send {label}: {e}"))?;
        self.created.push(message.id());
        Ok(message.id())
    }

    async fn edit(&self, id: i32, label: &str) -> Result<String, String> {
        assert!(
            self.created.contains(&id),
            "refusing to edit foreign message"
        );
        let text = self.text(label, "v2-edited");
        self.client
            .edit_message(self.peer, id, InputMessage::new().text(text.clone()))
            .await
            .map_err(|e| format!("edit {label}: {e}"))?;
        Ok(text)
    }

    async fn delete(&mut self, id: i32) -> Result<(), String> {
        assert!(
            self.created.contains(&id),
            "refusing to delete foreign message"
        );
        self.client
            .delete_messages(self.peer, &[id])
            .await
            .map_err(|e| format!("delete {id}: {e}"))?;
        self.deleted.push(id);
        Ok(())
    }

    async fn cleanup(&mut self) {
        let rest: Vec<i32> = self
            .created
            .iter()
            .copied()
            .filter(|id| !self.deleted.contains(id))
            .collect();
        if !rest.is_empty() {
            let _ = self.client.delete_messages(self.peer, &rest).await;
        }
    }
}

struct Serve {
    child: Option<Child>,
    addr: SocketAddr,
}

impl Serve {
    fn start(env: &[(&str, String)], log: &Path) -> Self {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        drop(l);
        let out = File::options().create(true).append(true).open(log).unwrap();
        let child = Command::new(BIN)
            .envs(env.iter().map(|(k, v)| (*k, v)))
            .args(["serve", "--bind", &addr.to_string()])
            .stdout(Stdio::from(out.try_clone().unwrap()))
            .stderr(Stdio::from(out))
            .spawn()
            .unwrap();
        Self {
            child: Some(child),
            addr,
        }
    }

    async fn get(&self, path: &str) -> Result<serde_json::Value, String> {
        let mut s = TcpStream::connect(self.addr)
            .await
            .map_err(|e| e.to_string())?;
        let req = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
        s.write_all(req.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        let mut buf = Vec::new();
        s.read_to_end(&mut buf).await.map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&buf);
        let body = text.split_once("\r\n\r\n").ok_or("malformed response")?.1;
        serde_json::from_str(body).map_err(|e| format!("{e}: {body}"))
    }

    async fn wait_running(&mut self) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(300);
        loop {
            if let Some(status) = self.child.as_mut().unwrap().try_wait().unwrap() {
                return Err(format!("serve exited early: {status}"));
            }
            if let Ok(v) = self.get("/api/v1/status").await
                && v["collector"]["state"] == "running"
            {
                return Ok(());
            }
            if Instant::now() > deadline {
                return Err("collector never reached state running within 300s".into());
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    /// Diagnostics for a failed scenario: collector status JSON and the persisted update state
    /// counters (no identifiers, no secrets).
    async fn diagnostics(&self) {
        match self.get("/api/v1/status").await {
            Ok(v) => eprintln!("diag: status = {}", v["collector"]),
            Err(e) => eprintln!("diag: status unavailable: {e}"),
        }
        let session = std::env::var("TELEGRAM_SESSION_FILE").unwrap_or_default();
        match std::fs::read_to_string(&session)
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        {
            Some(v) => {
                let u = &v["updates_state"];
                eprintln!(
                    "diag: session pts={} qts={} date={} seq={} channels={}",
                    u["pts"],
                    u["qts"],
                    u["date"],
                    u["seq"],
                    u["channels"].as_array().map_or(0, Vec::len)
                );
            }
            None => eprintln!("diag: session state unreadable"),
        }
    }

    /// SIGTERM, then require a clean exit.
    async fn stop(&mut self) -> Result<(), String> {
        let mut child = self.child.take().unwrap();
        let ok = Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            let _ = child.kill();
            let _ = child.wait();
            return Err("kill -TERM failed".into());
        }
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                return if status.success() {
                    Ok(())
                } else {
                    Err(format!("serve exit {status}"))
                };
            }
            if Instant::now() > deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err("serve did not exit within 30s of SIGTERM".into());
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
}

impl Drop for Serve {
    fn drop(&mut self) {
        if let Some(c) = &mut self.child {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

#[derive(Debug, Clone)]
struct Row {
    id: i64,
    text: Option<String>,
    edited: bool,
    deleted: bool,
}

async fn rows(pool: &SqlitePool, chat: i64) -> Result<Vec<Row>, String> {
    let r: Vec<(i64, Option<String>, Option<i64>, i64)> = sqlx::query_as(
        "SELECT message_id, text, edited_at, is_deleted FROM messages WHERE chat_id = ? ORDER BY message_id",
    )
    .bind(chat)
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())?;
    Ok(r.into_iter()
        .map(|(id, text, e, d)| Row {
            id,
            text,
            edited: e.is_some(),
            deleted: d != 0,
        })
        .collect())
}

async fn wait_rows(
    pool: &SqlitePool,
    chat: i64,
    what: &str,
    mut ok: impl FnMut(&[Row]) -> bool,
) -> Result<Vec<Row>, String> {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let r = rows(pool, chat).await?;
        if ok(&r) {
            return Ok(r);
        }
        if Instant::now() > deadline {
            return Err(format!("timeout (60s) waiting for: {what}; rows: {r:?}"));
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

fn row(r: &[Row], id: i32) -> Option<&Row> {
    r.iter().find(|x| x.id == i64::from(id))
}

fn cli(env: &[(&str, String)], args: &[&str], log: &Path) -> Result<(), String> {
    let out = Command::new(BIN)
        .envs(env.iter().map(|(k, v)| (*k, v)))
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    let mut f = File::options().create(true).append(true).open(log).unwrap();
    use std::io::Write;
    // Only the command name and exit status: CLI output lists chat titles.
    let _ = writeln!(f, "$ {} -> {}", args.join(" "), out.status);
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("`{}` failed: {}", args.join(" "), out.status))
    }
}

fn log_tail(log: &Path, live: &Live) -> String {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    let mut secrets = vec![live.api_hash.clone()];
    secrets.extend(std::env::var("TELEGRAM_PHONE").ok());
    let lines: Vec<&str> = text.lines().collect();
    let mut tail = lines[lines.len().saturating_sub(40)..].join("\n");
    for s in secrets.iter().filter(|s| !s.is_empty()) {
        tail = tail.replace(s.as_str(), "[redacted]");
    }
    tail
}

struct Ctx<'a> {
    live: &'a Live,
    env: Vec<(&'static str, String)>,
    log: PathBuf,
    db: PathBuf,
}

impl Ctx<'_> {
    async fn pool(&self) -> Result<SqlitePool, String> {
        SqlitePool::connect_with(
            SqliteConnectOptions::new()
                .filename(&self.db)
                .read_only(true)
                .busy_timeout(Duration::from_secs(10)),
        )
        .await
        .map_err(|e| e.to_string())
    }
}

#[tokio::test]
#[ignore = "live Telegram; needs LIVE_TELEGRAM=1 and credentials"]
async fn live_archive_acceptance() {
    let Some(live) = Live::from_env() else { return };
    let run_id = format!(
        "{}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
        std::process::id()
    );
    let marker = format!("[archive-live {run_id}]");
    eprintln!("live run marker: {marker}");

    let mut driver = match Driver::connect(&live, marker.clone()).await {
        Ok(d) => d,
        Err(e) => panic!("driver setup failed (nothing was sent): {e}"),
    };
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("archive.db");
    let ctx = Ctx {
        live: &live,
        env: vec![
            ("DATABASE_URL", format!("sqlite://{}", db.display())),
            ("TELEGRAM_API_ID", live.api_id.to_string()),
            ("TELEGRAM_API_HASH", live.api_hash.clone()),
            (
                "TELEGRAM_SESSION_FILE",
                std::env::var("TELEGRAM_SESSION_FILE").unwrap(),
            ),
        ],
        log: dir.path().join("serve.log"),
        db,
    };

    let mut report: Vec<(&str, Result<(), String>, Duration)> = Vec::new();
    let result = run_scenarios(&ctx, &mut driver, &mut report).await;
    if let Err((name, e, t)) = result {
        report.push((name, Err(e), t));
    }
    // Assertions are recorded; now best-effort cleanup of this run's remaining messages.
    driver.cleanup().await;

    eprintln!("\n=== live acceptance report ===");
    for (name, r, t) in &report {
        match r {
            Ok(()) => eprintln!("PASS  {name} ({:.1}s)", t.as_secs_f32()),
            Err(e) => eprintln!("FAIL  {name} ({:.1}s): {e}", t.as_secs_f32()),
        }
    }
    if report.iter().any(|(_, r, _)| r.is_err()) {
        eprintln!("--- serve/cli log tail ---\n{}", log_tail(&ctx.log, &live));
        panic!("live acceptance failed");
    }
}

type Scenario = (&'static str, String, Duration);

async fn run_scenarios(
    ctx: &Ctx<'_>,
    d: &mut Driver,
    report: &mut Vec<(&'static str, Result<(), String>, Duration)>,
) -> Result<(), (&'static str, String, Duration)> {
    let chat = ctx.live.chat_id;
    let chat_arg = chat.to_string();
    let mut step =
        |name: &'static str, started: Instant, r: Result<(), String>| -> Result<(), Scenario> {
            let t = started.elapsed();
            match r {
                Ok(()) => {
                    report.push((name, Ok(()), t));
                    Ok(())
                }
                Err(e) => Err((name, e, t)),
            }
        };

    let t = Instant::now();
    let setup = (|| {
        cli(&ctx.env, &["db", "init"], &ctx.log)?;
        cli(&ctx.env, &["chats", "refresh"], &ctx.log)?;
        cli(&ctx.env, &["chats", "track", &chat_arg], &ctx.log)?;
        cli(&ctx.env, &["sync", "chat", &chat_arg], &ctx.log)
    })();
    step("0 setup (init/refresh/track/sync)", t, setup)?;

    let pool = ctx
        .pool()
        .await
        .map_err(|e| ("0 open db", e, Duration::ZERO))?;
    let mut serve = Serve::start(&ctx.env, &ctx.log);

    // 1. Realtime
    let t = Instant::now();
    let r = async {
        serve.wait_running().await?;
        let a = d.send("A").await?;
        let b = d.send("B").await?;
        let c = d.send("C").await?;
        let b_text = d.edit(b, "B").await?;
        d.delete(c).await?;
        let rs = wait_rows(&pool, chat, "A live, B edited, C deleted", |r| {
            row(r, a).is_some_and(|x| !x.deleted)
                && row(r, b).is_some_and(|x| x.edited && x.text.as_deref() == Some(&b_text))
                && row(r, c).is_some_and(|x| x.deleted)
        })
        .await?;
        let _ = rs;
        let page = serve
            .get(&format!("/api/v1/chats/{chat}/messages?limit=100"))
            .await?;
        let items = page["items"].as_array().ok_or("REST page has no items")?;
        let has = |id: i32| items.iter().any(|m| m["id"] == i64::from(id));
        if !(has(a) && has(b) && !has(c)) {
            return Err(format!(
                "REST view wrong: A={} B={} C(should be absent)={}",
                has(a),
                has(b),
                has(c)
            ));
        }
        Ok((a, b, b_text))
    }
    .await;
    let (a, _b, _) = match r {
        Ok(v) => v,
        Err(e) => {
            serve.diagnostics().await;
            return Err(("1 realtime send/edit/delete", e, t.elapsed()));
        }
    };
    step("1 realtime send/edit/delete", t, Ok(()))?;

    // 2. Offline gap
    let t = Instant::now();
    let r = async {
        serve
            .stop()
            .await
            .map_err(|e| format!("graceful stop: {e}"))?;
        let dd = d.send("D").await?;
        let e = d.send("E").await?;
        let a_text = d.edit(a, "A").await?;
        d.delete(e).await?;
        serve = Serve::start(&ctx.env, &ctx.log);
        serve.wait_running().await?;
        let rs = wait_rows(
            &pool,
            chat,
            "D present, A edited, E absent-or-deleted",
            |r| {
                row(r, dd).is_some_and(|x| !x.deleted)
                    && row(r, a).is_some_and(|x| x.edited && x.text.as_deref() == Some(&a_text))
                    && row(r, e).is_none_or(|x| x.deleted)
            },
        )
        .await?;
        let _ = rs;
        Ok(())
    }
    .await;
    step("2 offline gap catch-up", t, r)?;

    // 3. Restart without changes: no duplicates, no row count change.
    let t = Instant::now();
    let r = async {
        let before = rows(&pool, chat).await?;
        serve.stop().await.map_err(|e| format!("graceful stop: {e}"))?;
        serve = Serve::start(&ctx.env, &ctx.log);
        serve.wait_running().await?;
        tokio::time::sleep(Duration::from_secs(10)).await;
        let after = rows(&pool, chat).await?;
        let dups: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM (SELECT 1 FROM messages GROUP BY chat_id, message_id HAVING COUNT(*) > 1)",
        )
        .fetch_one(&pool)
        .await
        .map_err(|e| e.to_string())?;
        if dups != 0 || before.len() != after.len() {
            return Err(format!("duplicates={dups}, rows before={} after={}", before.len(), after.len()));
        }
        Ok(())
    }
    .await;
    step("3 idle restart has no duplicates", t, r)?;
    serve.stop().await.map_err(|e| {
        (
            "3 final stop",
            format!("graceful stop: {e}"),
            Duration::ZERO,
        )
    })?;

    // 4. Untracked isolation
    let t = Instant::now();
    let r = async {
        let other: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE chat_id <> ?")
            .bind(chat)
            .fetch_one(&pool)
            .await
            .map_err(|e| e.to_string())?;
        if other != 0 {
            return Err(format!("{other} messages stored for untracked chats"));
        }
        Ok(())
    }
    .await;
    step("4 untracked chats isolated", t, r)?;
    Ok(())
}

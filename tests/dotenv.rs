use std::{path::Path, process::Command};

use tempfile::TempDir;

const BIN: &str = env!("CARGO_BIN_EXE_tgarchive");

/// Runs in `cwd` with a cleared environment so the repo-root `.env` is never read.
fn run(cwd: &Path, envs: &[(&str, &str)], args: &[&str]) -> std::process::Output {
    Command::new(BIN)
        .current_dir(cwd)
        .env_clear()
        .envs(envs.iter().copied())
        .args(args)
        .output()
        .unwrap()
}

fn sqlite_url(path: &Path) -> String {
    format!("sqlite://{}", path.display())
}

#[test]
fn dotenv_in_working_directory_is_used_with_comments_and_quotes() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("from_env.db");
    std::fs::write(
        dir.path().join(".env"),
        format!(
            "# comment\n\nUNRELATED=1\nDATABASE_URL=\"{}\"  # trailing\n",
            sqlite_url(&db)
        ),
    )
    .unwrap();
    let out = run(dir.path(), &[], &["db", "init"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(db.exists());
}

#[test]
fn process_environment_wins_over_dotenv() {
    let dir = TempDir::new().unwrap();
    let from_file = dir.path().join("file.db");
    let from_env = dir.path().join("env.db");
    std::fs::write(
        dir.path().join(".env"),
        format!("DATABASE_URL={}\n", sqlite_url(&from_file)),
    )
    .unwrap();
    let url = sqlite_url(&from_env);
    let out = run(dir.path(), &[("DATABASE_URL", &url)], &["db", "init"]);
    assert!(out.status.success());
    assert!(from_env.exists() && !from_file.exists());
}

#[test]
fn opt_out_variable_disables_default_dotenv() {
    let dir = TempDir::new().unwrap();
    let from_file = dir.path().join("file.db");
    std::fs::write(
        dir.path().join(".env"),
        format!("DATABASE_URL={}\n", sqlite_url(&from_file)),
    )
    .unwrap();
    let out = run(dir.path(), &[("TGARCHIVE_NO_DOTENV", "1")], &["db", "init"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!from_file.exists());
}

#[test]
fn missing_default_dotenv_is_silent_but_missing_explicit_file_fails() {
    let dir = TempDir::new().unwrap();
    let url = sqlite_url(&dir.path().join("a.db"));
    let ok = run(dir.path(), &[("DATABASE_URL", &url)], &["db", "init"]);
    assert!(ok.status.success());
    assert!(
        ok.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&ok.stderr)
    );
    let out = run(
        dir.path(),
        &[("DATABASE_URL", &url)],
        &["--env-file", "nope.env", "db", "init"],
    );
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("nope.env"));
}

#[test]
fn explicit_env_file_is_loaded_and_never_echoes_values() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("explicit.db");
    std::fs::write(
        dir.path().join("custom.env"),
        format!("DATABASE_URL='{}'\n", sqlite_url(&db)),
    )
    .unwrap();
    let out = run(dir.path(), &[], &["db", "init", "--env-file", "custom.env"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(db.exists());

    std::fs::write(
        dir.path().join("bad.env"),
        "SECRET_TOKEN_VALUE=\"unterminated\n",
    )
    .unwrap();
    let bad = run(dir.path(), &[], &["--env-file", "bad.env", "db", "init"]);
    assert!(!bad.status.success());
    assert!(!String::from_utf8_lossy(&bad.stderr).contains("unterminated"));
}

#[test]
fn sync_all_validates_pacing_even_with_no_tracked_chats() {
    let dir = TempDir::new().unwrap();
    let url = sqlite_url(&dir.path().join("a.db"));
    let env = [
        ("DATABASE_URL", url.as_str()),
        ("SYNC_PAGE_DELAY_MS", "abc"),
    ];
    assert!(run(dir.path(), &env, &["db", "init"]).status.success());
    let out = run(dir.path(), &env, &["sync", "all"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("SYNC_PAGE_DELAY_MS"));
}

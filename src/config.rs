use std::{
    env, fmt,
    net::SocketAddr,
    path::{Path, PathBuf},
};

const DEFAULT_DATABASE_URL: &str = "sqlite://telegram.db";
const DEFAULT_SERVER_BIND: &str = "127.0.0.1:8080";
const DEFAULT_SESSION_FILE: &str = "telegram.session";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub database_url: String,
}

#[derive(Clone, PartialEq, Eq)]
pub struct TelegramConfig {
    pub api_id: i32,
    pub api_hash: String,
    pub session_file: PathBuf,
}

impl Config {
    pub fn load(database_url: Option<String>) -> Self {
        Self {
            database_url: database_url
                .or_else(|| env::var("DATABASE_URL").ok())
                .unwrap_or_else(|| DEFAULT_DATABASE_URL.to_owned()),
        }
    }

    pub fn server_bind(cli_bind: Option<SocketAddr>) -> Result<SocketAddr, String> {
        if let Some(bind) = cli_bind {
            return Ok(bind);
        }
        let value = env::var("SERVER_BIND").unwrap_or_else(|_| DEFAULT_SERVER_BIND.to_owned());
        value
            .parse()
            .map_err(|_| format!("SERVER_BIND must be an IP socket address, got {value:?}"))
    }

    pub fn telegram() -> Result<TelegramConfig, String> {
        let api_id = env::var("TELEGRAM_API_ID")
            .map_err(|_| "TELEGRAM_API_ID is required for this command".to_owned())?
            .parse::<i32>()
            .ok()
            .filter(|id| *id > 0)
            .ok_or_else(|| "TELEGRAM_API_ID must be a positive integer".to_owned())?;
        let api_hash = env::var("TELEGRAM_API_HASH")
            .map_err(|_| "TELEGRAM_API_HASH is required for this command".to_owned())?;
        if api_hash.trim().is_empty() {
            return Err("TELEGRAM_API_HASH must not be empty".to_owned());
        }
        let session_file = env::var_os("TELEGRAM_SESSION_FILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_SESSION_FILE));
        Ok(TelegramConfig {
            api_id,
            api_hash,
            session_file,
        })
    }

    pub fn telegram_configuration_requested() -> bool {
        env::var_os("TELEGRAM_API_ID").is_some() || env::var_os("TELEGRAM_API_HASH").is_some()
    }
}

impl fmt::Debug for TelegramConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TelegramConfig")
            .field("api_id", &self.api_id)
            .field("api_hash", &"<redacted>")
            .field("session_file", &self.session_file)
            .finish()
    }
}

impl TelegramConfig {
    /// The session file may not exist yet (first login), but its directory must.
    pub fn validate_paths(&self) -> Result<(), String> {
        if self.session_file.is_dir() {
            return Err(format!(
                "TELEGRAM_SESSION_FILE {} is a directory; point it at a file path such as ./telegram.session",
                self.session_file.display()
            ));
        }
        require_parent_dir("TELEGRAM_SESSION_FILE", &self.session_file)
    }
}

fn require_parent_dir(name: &str, file: &Path) -> Result<(), String> {
    match file.parent() {
        Some(parent) if !parent.as_os_str().is_empty() && !parent.is_dir() => Err(format!(
            "{name}: directory {} does not exist; create it (mkdir -p) or choose another path",
            parent.display()
        )),
        _ => Ok(()),
    }
}

impl Config {
    /// File-backed SQLite URLs need an existing parent directory; in-memory URLs are accepted.
    pub fn validate_database_path(&self) -> Result<(), String> {
        let rest = self
            .database_url
            .strip_prefix("sqlite://")
            .or_else(|| self.database_url.strip_prefix("sqlite:"))
            .ok_or_else(|| {
                format!(
                    "DATABASE_URL must start with sqlite:// (got {:?}); example: sqlite://telegram.db",
                    self.database_url
                )
            })?;
        let path = rest.split('?').next().unwrap_or(rest);
        if path.is_empty() || path == ":memory:" {
            return Ok(());
        }
        let path = Path::new(path);
        if path.is_dir() {
            return Err(format!(
                "DATABASE_URL points at a directory ({}); use a file path such as sqlite://telegram.db",
                path.display()
            ));
        }
        require_parent_dir("DATABASE_URL", path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn telegram(session: PathBuf) -> TelegramConfig {
        TelegramConfig {
            api_id: 1,
            api_hash: "super-secret-hash".into(),
            session_file: session,
        }
    }

    #[test]
    fn debug_output_redacts_api_hash() {
        let text = format!("{:?}", telegram("s.session".into()));
        assert!(!text.contains("super-secret-hash"));
        assert!(text.contains("<redacted>"));
    }

    #[test]
    fn session_path_errors_are_actionable() {
        let dir = tempfile::tempdir().unwrap();
        let err = telegram(dir.path().to_path_buf())
            .validate_paths()
            .unwrap_err();
        assert!(err.contains("is a directory"));
        let err = telegram(dir.path().join("missing/x.session"))
            .validate_paths()
            .unwrap_err();
        assert!(err.contains("mkdir -p"));
        assert!(
            telegram(dir.path().join("x.session"))
                .validate_paths()
                .is_ok()
        );
        assert!(telegram("x.session".into()).validate_paths().is_ok());
    }

    #[test]
    fn database_url_validation() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = |url: String| Config { database_url: url };
        assert!(cfg("postgres://x".into()).validate_database_path().is_err());
        assert!(
            cfg("sqlite::memory:".into())
                .validate_database_path()
                .is_ok()
        );
        assert!(
            cfg(format!("sqlite://{}", dir.path().display()))
                .validate_database_path()
                .unwrap_err()
                .contains("directory")
        );
        assert!(
            cfg(format!("sqlite://{}/nope/a.db", dir.path().display()))
                .validate_database_path()
                .unwrap_err()
                .contains("does not exist")
        );
        assert!(
            cfg(format!("sqlite://{}/a.db?mode=rwc", dir.path().display()))
                .validate_database_path()
                .is_ok()
        );
    }
}

use std::{env, net::SocketAddr, path::PathBuf};

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

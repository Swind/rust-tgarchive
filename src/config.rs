use std::{env, net::SocketAddr};

const DEFAULT_DATABASE_URL: &str = "sqlite://telegram.db";
const DEFAULT_SERVER_BIND: &str = "127.0.0.1:8080";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub database_url: String,
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
}

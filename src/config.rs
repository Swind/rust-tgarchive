use std::env;

const DEFAULT_DATABASE_URL: &str = "sqlite://telegram.db";

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
}

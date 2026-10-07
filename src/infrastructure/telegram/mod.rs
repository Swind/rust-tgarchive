//! Telegram MTProto adapter.
//!
//! This module intentionally owns only the account/session transport boundary. Realtime update
//! durability remains a separate capability and is not implied by using Grammers' session.

pub mod auth;
pub mod file_session;
pub mod media;

pub use auth::{AuthError, LoginChallenge, LoginProgress, PasswordChallenge};
pub use session::{OpenError, TelegramAdapter};
pub mod gateway;
pub mod mapper;
pub mod owner_lock;
pub mod realtime;
pub mod session;

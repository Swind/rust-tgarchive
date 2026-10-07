use chrono::{DateTime, Utc};
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;
use std::net::SocketAddr;
use thiserror::Error;

use crate::{
    application::services::Application,
    application::{
        ApplicationError, ListMessagesQuery, MessageFilters, MessagePage, MessageView, PageSize,
        SearchMessagesQuery, SearchSort, SenderDetail, SenderProfile, SenderQuery, SenderSort,
        SyncJob, SyncScope, TimeRange, ValidationError,
    },
    domain::{Chat, ChatId, ChatKind, IdError, MessageId, SenderId, SenderKind},
    infrastructure::persistence::sqlite::SenderRepairReport,
};

mod args;
mod execute;
mod prepare;
mod query;
mod render;

pub use args::*;
pub use execute::{execute_prepared, execute_set_tracking};
pub use prepare::{PreparedCommand, PreparedInvocation, prepare};
use render::{
    component_state, human_chat, human_message, human_search_index, json, render_message_page,
    render_sender_detail, render_senders, resolve_sender,
};
pub use render::{initialized_output, render_chats, render_sender_repair, render_sync_job};

use query::{list_query, parse_message_id, parse_timestamp, search_query};

mod coordinator;
pub use coordinator::SyncCoordinator;

use std::{sync::Arc, time::Duration};

pub use tokio_util::sync::CancellationToken;

use super::ingestion_worker::IngestSink;
use super::pacer::RatePacer;
use super::{
    ApplicationError, ChatRepository, HistoryBoundary, IngestBatch, MessageSource, PageSize,
    RefetchCheckpoint, SyncChatProgress, SyncJob, SyncJobState, SyncRepository, SyncScope,
    TelegramError, TelegramGateway,
};
use crate::domain::{ChatId, MessageEvent, MessageId};

pub struct SyncEngine {
    gateway: Arc<dyn TelegramGateway>,
    repository: Arc<dyn SyncRepository>,
    chats: Arc<dyn ChatRepository>,
    sink: IngestSink,
    page_size: PageSize,
    pacer: Arc<RatePacer>,
    max_flood_wait: Duration,
}

/// Longest FLOOD_WAIT that is waited out by default; longer ones pause or skip instead.
pub const DEFAULT_MAX_FLOOD_WAIT: Duration = Duration::from_secs(300);

impl SyncEngine {
    pub fn new(
        gateway: Arc<dyn TelegramGateway>,
        repository: Arc<dyn SyncRepository>,
        chats: Arc<dyn ChatRepository>,
        sink: IngestSink,
        page_size: PageSize,
    ) -> Self {
        Self {
            gateway,
            repository,
            chats,
            sink,
            page_size,
            pacer: Arc::new(RatePacer::disabled()),
            max_flood_wait: DEFAULT_MAX_FLOOD_WAIT,
        }
    }

    /// Shares one pacer between history sync and catch-up (both use this engine) and sets
    /// the longest FLOOD_WAIT that is waited out. `new` defaults to no pacing.
    pub fn with_rate_control(mut self, pacer: Arc<RatePacer>, max_flood_wait: Duration) -> Self {
        self.pacer = pacer;
        self.max_flood_wait = max_flood_wait;
        self
    }

    pub fn pacer(&self) -> &Arc<RatePacer> {
        &self.pacer
    }

    /// Refreshes metadata for every dialog (never touching tracking flags), then returns the
    /// tracked chat IDs. With nothing tracked, Telegram is not contacted at all.
    async fn tracked_snapshot(
        &self,
        cancel: &CancellationToken,
    ) -> Result<Vec<ChatId>, ApplicationError> {
        let chats = tokio::select! { _ = cancel.cancelled() => return Err(ApplicationError::Conflict), result = self.gateway.list_chats() => result? };
        self.chats.save_refresh(chats).await?;
        self.tracked_ids().await
    }

    async fn tracked_ids(&self) -> Result<Vec<ChatId>, ApplicationError> {
        let mut ids: Vec<_> = self
            .chats
            .list()
            .await?
            .into_iter()
            .filter(|chat| chat.tracked)
            .map(|chat| chat.id)
            .collect();
        ids.sort_by_key(|id| id.get());
        Ok(ids)
    }

    /// Gives tracked chats that have no catch-up baseline one (without fetching history), so a
    /// chat tracked while the server runs is covered by later reconnect catch-up rounds.
    pub async fn baseline_new_tracked(
        &self,
        cancel: &CancellationToken,
    ) -> Result<(), ApplicationError> {
        for id in self.tracked_ids().await? {
            let baselined = self
                .repository
                .get_checkpoint(id)
                .await?
                .is_some_and(|checkpoint| checkpoint.catchup_after_id.is_some());
            if baselined {
                continue;
            }
            match self.catch_up_chat(id, cancel).await {
                Ok(_) => {}
                Err(
                    error @ (ApplicationError::TelegramUnavailable(_)
                    | ApplicationError::TelegramFloodWait { .. }),
                ) => {
                    tracing::warn!(chat_id = id.get(), reason = %sanitize_reason(&error.to_string()), "baseline for newly tracked chat failed; retrying later");
                }
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    pub async fn sync_chat(
        &self,
        job_id: &str,
        chat_id: ChatId,
        cancel: &CancellationToken,
    ) -> Result<u64, ApplicationError> {
        let mut checkpoint = self
            .repository
            .get_checkpoint(chat_id)
            .await?
            .unwrap_or_default();
        if checkpoint.history_complete {
            // History is done; fill any forward gap (messages newer than the archive).
            let caught_up = self.catch_up_chat(chat_id, cancel).await?;
            self.sink
                .submit(IngestBatch {
                    job_progress: Some(SyncChatProgress {
                        job_id: job_id.to_owned(),
                        chat_id,
                        state: SyncJobState::Succeeded,
                        committed_messages: caught_up,
                        summary_error: None,
                    }),
                    ..Default::default()
                })
                .await?;
            return Ok(caught_up);
        }
        if checkpoint.catchup_after_id.is_none() && checkpoint.history_before_id.is_some() {
            // Legacy/partial state: this page is not the newest, so use the archive's top.
            checkpoint.catchup_after_id = Some(self.archived_baseline(chat_id).await?);
        }
        let mut committed = 0u64;
        loop {
            if cancel.is_cancelled() {
                return Err(ApplicationError::Conflict);
            }
            let page = self
                .fetch_with_retry(
                    chat_id,
                    HistoryBoundary {
                        before_message_id: checkpoint.history_before_id,
                        after_message_id: None,
                    },
                    self.page_size,
                    cancel,
                )
                .await?;
            let next = page.next_before_message_id;
            if !page.exhausted && !cursor_advances(checkpoint.history_before_id, next) {
                return Err(ApplicationError::Internal(
                    "Telegram history page did not advance its exclusive cursor".into(),
                ));
            }
            let count = page.records.len() as u64;
            let mut next_checkpoint = checkpoint.clone();
            next_checkpoint.history_before_id = next;
            next_checkpoint.history_complete = page.exhausted;
            if next_checkpoint.catchup_after_id.is_none() {
                // The first page of a fresh history walk holds the newest messages; anything
                // newer than it must later be caught up, never baselined past.
                next_checkpoint.catchup_after_id = Some(
                    page.records
                        .iter()
                        .map(record_message_id)
                        .max_by_key(|id| id.get())
                        .unwrap_or(MessageId::BEFORE_FIRST),
                );
            }
            let progress = SyncChatProgress {
                job_id: job_id.to_owned(),
                chat_id,
                state: if page.exhausted {
                    SyncJobState::Succeeded
                } else {
                    SyncJobState::Running
                },
                committed_messages: committed + count,
                summary_error: None,
            };
            self.sink
                .submit(IngestBatch {
                    chats: page.chats,
                    senders: page.senders,
                    records: page.records,
                    checkpoint: Some((chat_id, next_checkpoint.clone())),
                    job_progress: Some(progress),
                    ..IngestBatch::default()
                })
                .await?;
            committed += count;
            checkpoint = next_checkpoint;
            if page.exhausted {
                break;
            }
        }
        Ok(committed)
    }

    /// Re-downloads the full history newest to oldest and backfills NULL metadata (sender, post
    /// author, forward origin, attachment details) of rows that already exist; text, edit
    /// versions and deletions are never changed (see [`MessageSource::Refetch`]). Messages
    /// missing from the archive are inserted like a normal history sync.
    ///
    /// The walk has its own persisted checkpoint (`RefetchCheckpoint`), committed with every page,
    /// so a rerun after an interruption or rate limit resumes where it stopped. A finished walk
    /// clears it; the next `--refetch` starts from the newest message again.
    pub async fn refetch_chat(
        &self,
        job_id: &str,
        chat_id: ChatId,
        cancel: &CancellationToken,
    ) -> Result<u64, ApplicationError> {
        let saved = self.repository.get_refetch_checkpoint(chat_id).await?;
        let mut before = if saved.active { saved.before_id } else { None };
        let mut committed = 0u64;
        loop {
            if cancel.is_cancelled() {
                return Err(ApplicationError::Conflict);
            }
            let mut page = self
                .fetch_with_retry(
                    chat_id,
                    HistoryBoundary {
                        before_message_id: before,
                        after_message_id: None,
                    },
                    self.page_size,
                    cancel,
                )
                .await?;
            let next = page.next_before_message_id;
            if !page.exhausted && !cursor_advances(before, next) {
                return Err(ApplicationError::Internal(
                    "Telegram history page did not advance its exclusive cursor".into(),
                ));
            }
            for record in &mut page.records {
                record.source = MessageSource::Refetch;
            }
            let count = page.records.len() as u64;
            self.sink
                .submit(IngestBatch {
                    chats: page.chats,
                    senders: page.senders,
                    records: page.records,
                    refetch_checkpoint: Some((
                        chat_id,
                        RefetchCheckpoint {
                            active: !page.exhausted,
                            before_id: if page.exhausted { None } else { next },
                        },
                    )),
                    job_progress: Some(SyncChatProgress {
                        job_id: job_id.to_owned(),
                        chat_id,
                        state: if page.exhausted {
                            SyncJobState::Succeeded
                        } else {
                            SyncJobState::Running
                        },
                        committed_messages: committed + count,
                        summary_error: None,
                    }),
                    ..IngestBatch::default()
                })
                .await?;
            committed += count;
            before = next;
            if page.exhausted {
                return Ok(committed);
            }
        }
    }

    /// Newest archived ID, or "before the first message" when nothing is archived (safe: it
    /// only re-fetches, never skips).
    async fn archived_baseline(&self, chat_id: ChatId) -> Result<MessageId, ApplicationError> {
        Ok(self
            .repository
            .newest_archived_id(chat_id)
            .await?
            .unwrap_or(MessageId::BEFORE_FIRST))
    }

    /// Fetches messages newer than the persisted `catchup_after_id` for one chat, ascending,
    /// up to an upper bound fixed once at the start of this round. Each page is committed
    /// together with its checkpoint; the boundary never jumps to the live newest ID.
    pub async fn catch_up_chat(
        &self,
        chat_id: ChatId,
        cancel: &CancellationToken,
    ) -> Result<u64, ApplicationError> {
        let mut checkpoint = self
            .repository
            .get_checkpoint(chat_id)
            .await?
            .unwrap_or_default();
        let newest = self
            .fetch_with_retry(
                chat_id,
                HistoryBoundary {
                    before_message_id: None,
                    after_message_id: None,
                },
                PageSize::new(1)?,
                cancel,
            )
            .await?
            .next_before_message_id;
        let Some(upper) = newest else {
            // An empty chat has nothing to catch up on. Record a "before the first message"
            // baseline so it does not stay unbaselined forever and later messages are caught up.
            if checkpoint.catchup_after_id.is_none() {
                checkpoint.catchup_after_id = Some(MessageId::BEFORE_FIRST);
                self.sink
                    .submit(IngestBatch {
                        checkpoint: Some((chat_id, checkpoint)),
                        ..IngestBatch::default()
                    })
                    .await?;
                tracing::info!(
                    chat_id = chat_id.get(),
                    "catch-up: chat has no messages; baseline set to 0"
                );
            }
            return Ok(0);
        };
        if checkpoint.catchup_after_id.is_none()
            && (checkpoint.history_complete || checkpoint.history_before_id.is_some())
        {
            checkpoint.catchup_after_id = Some(self.archived_baseline(chat_id).await?);
        } else if checkpoint.catchup_after_id.is_none()
            && let Some(archived) = self.repository.newest_archived_id(chat_id).await?
        {
            checkpoint.catchup_after_id = Some(archived);
        }
        let Some(mut after) = checkpoint.catchup_after_id else {
            // Nothing archived and no history yet: start at this round's bound.
            checkpoint.catchup_after_id = Some(upper);
            self.sink
                .submit(IngestBatch {
                    checkpoint: Some((chat_id, checkpoint)),
                    ..IngestBatch::default()
                })
                .await?;
            return Ok(0);
        };
        let mut committed = 0u64;
        while after.get() < upper.get() {
            let page = self
                .fetch_with_retry(
                    chat_id,
                    HistoryBoundary {
                        before_message_id: None,
                        after_message_id: Some(after),
                    },
                    self.page_size,
                    cancel,
                )
                .await?;
            let reached_bound = page
                .next_after_message_id
                .is_some_and(|next| next.get() >= upper.get());
            let next = if page.exhausted || reached_bound {
                upper
            } else {
                match page.next_after_message_id {
                    Some(next) if next.get() > after.get() => next,
                    _ => {
                        return Err(ApplicationError::Internal(
                            "Telegram catch-up page did not advance its exclusive cursor".into(),
                        ));
                    }
                }
            };
            let records = page
                .records
                .into_iter()
                .filter(|record| record_message_id(record).get() <= upper.get())
                .collect::<Vec<_>>();
            let count = records.len() as u64;
            checkpoint.catchup_after_id = Some(next);
            self.sink
                .submit(IngestBatch {
                    chats: page.chats,
                    senders: page.senders,
                    records,
                    checkpoint: Some((chat_id, checkpoint.clone())),
                    ..IngestBatch::default()
                })
                .await?;
            committed += count;
            after = next;
        }
        Ok(committed)
    }

    /// One catch-up round over every tracked chat. Telegram failures for a single chat are
    /// reported in the summary; storage failures and cancellation abort the round.
    pub async fn catch_up_all(
        &self,
        cancel: &CancellationToken,
    ) -> Result<CatchUpSummary, ApplicationError> {
        self.catch_up_filtered(None, cancel).await
    }

    /// Catch-up for one chat, a no-op when it is not tracked.
    pub async fn catch_up_chat_if_tracked(
        &self,
        chat_id: ChatId,
        cancel: &CancellationToken,
    ) -> Result<CatchUpSummary, ApplicationError> {
        self.catch_up_filtered(Some(chat_id), cancel).await
    }

    async fn catch_up_filtered(
        &self,
        only: Option<ChatId>,
        cancel: &CancellationToken,
    ) -> Result<CatchUpSummary, ApplicationError> {
        let mut summary = CatchUpSummary::default();
        for chat in self
            .chats
            .list()
            .await?
            .into_iter()
            .filter(|chat| chat.tracked && only.is_none_or(|id| id == chat.id))
        {
            match self.catch_up_chat(chat.id, cancel).await {
                Ok(count) => {
                    summary.committed_messages += count;
                    summary.caught_up_chats += 1;
                }
                Err(
                    error @ (ApplicationError::TelegramUnavailable(_)
                    | ApplicationError::TelegramFloodWait { .. }),
                ) => {
                    let reason = sanitize_reason(&error.to_string());
                    tracing::warn!(chat_id = chat.id.get(), %reason, "catch-up skipped a chat; it is retried on the next round");
                    self.sink
                        .submit(IngestBatch {
                            chat_error: Some((chat.id, reason.clone())),
                            ..IngestBatch::default()
                        })
                        .await?;
                    summary.failures.push((chat.id, reason));
                }
                Err(ApplicationError::Conflict) if cancel.is_cancelled() => {
                    return Err(ApplicationError::Conflict);
                }
                Err(error) => {
                    tracing::warn!(chat_id = chat.id.get(), reason = %sanitize_reason(&error.to_string()), "catch-up failed for a chat; aborting the round");
                    return Err(error);
                }
            }
        }
        Ok(summary)
    }

    async fn fetch_with_retry(
        &self,
        chat: ChatId,
        boundary: HistoryBoundary,
        page_size: PageSize,
        cancel: &CancellationToken,
    ) -> Result<super::HistoryPage, ApplicationError> {
        let mut transient_attempt = 0;
        let mut flood_attempt = 0;
        loop {
            if cancel.is_cancelled() {
                return Err(ApplicationError::Conflict);
            }
            if self.pacer.acquire(cancel).await {
                return Err(ApplicationError::Conflict);
            }
            let response = tokio::select! { _ = cancel.cancelled() => return Err(ApplicationError::Conflict), result = self.gateway.fetch_history(chat, boundary.clone(), page_size) => result };
            match response {
                Ok(page) => {
                    self.pacer.on_success();
                    return Ok(page);
                }
                Err(TelegramError::FloodWait {
                    retry_after_seconds,
                }) => {
                    let will_wait = flood_attempt < 5
                        && Duration::from_secs(retry_after_seconds) <= self.max_flood_wait;
                    self.pacer.on_flood(retry_after_seconds, will_wait);
                    if !will_wait {
                        return Err(ApplicationError::TelegramFloodWait {
                            retry_after_seconds,
                        });
                    }
                    // The pacer now holds every history request back until the wait is over.
                    flood_attempt += 1;
                }
                Err(TelegramError::Unavailable(_)) if transient_attempt < 3 => {
                    let wait = Duration::from_millis(250 * (1 << transient_attempt));
                    transient_attempt += 1;
                    if wait_or_cancel(cancel, wait).await {
                        return Err(ApplicationError::Conflict);
                    }
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct CatchUpSummary {
    pub caught_up_chats: usize,
    pub committed_messages: u64,
    pub failures: Vec<(ChatId, String)>,
}

/// Single-line, length-bounded text that is safe to log and persist.
pub fn sanitize_reason(text: &str) -> String {
    let cleaned: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    cleaned.chars().take(200).collect()
}

fn record_message_id(record: &super::IngestRecord) -> MessageId {
    match &record.event {
        MessageEvent::Created(message) | MessageEvent::Updated(message) => message.id,
        MessageEvent::Deleted { message_id, .. } => *message_id,
    }
}

async fn wait_or_cancel(cancel: &CancellationToken, duration: Duration) -> bool {
    tokio::select! { _ = tokio::time::sleep(duration) => false, _ = cancel.cancelled() => true }
}

/// A FLOOD_WAIT longer than the configured ceiling ends the job in a resumable state.
fn failure_state(error: &ApplicationError, cancel: &CancellationToken) -> SyncJobState {
    if cancel.is_cancelled() {
        SyncJobState::Interrupted
    } else if matches!(error, ApplicationError::TelegramFloodWait { .. }) {
        SyncJobState::RateLimited
    } else {
        SyncJobState::Failed
    }
}

fn failure_summary(error: &ApplicationError) -> String {
    match error {
        ApplicationError::TelegramFloodWait {
            retry_after_seconds,
        } => format!("Telegram rate limit: retry after {retry_after_seconds} s"),
        other => sanitize_reason(&other.to_string()),
    }
}

fn cursor_advances(old: Option<MessageId>, new: Option<MessageId>) -> bool {
    match new {
        Some(next) => old.is_none_or(|old| next.get() < old.get()),
        None => false,
    }
}

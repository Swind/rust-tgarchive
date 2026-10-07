use super::*;

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct StatusDto {
    pub collector: ComponentStatusDto,
    pub sync_jobs: Vec<SyncJobDto>,
    /// Deletions that could not be attributed to a chat (kept as tombstones).
    pub unresolved_deletions: u64,
    /// Pacing of Telegram history requests; absent when this process has no Telegram access.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_limit: Option<RateLimitDto>,
    /// Full-text search index state.
    pub search_index: SearchIndexDto,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SearchIndexDto {
    /// Index format version stored in the database (0 = never built).
    pub version: u32,
    /// `ready`; `rebuilding` or `stale` (search then uses a slower substring scan until
    /// `tgarchive search rebuild-index` / `db init` completes).
    pub state: String,
    /// Messages in the index.
    pub indexed: u64,
    /// Messages with text that belong in the index.
    pub total: u64,
}

impl From<SearchIndexStatus> for SearchIndexDto {
    fn from(status: SearchIndexStatus) -> Self {
        Self {
            version: status.version,
            state: status.state.as_str().to_owned(),
            indexed: status.indexed,
            total: status.total,
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct RateLimitDto {
    /// Current minimum interval between history requests (grows after FLOOD_WAIT).
    pub interval_ms: u64,
    /// Configured base interval (`SYNC_PAGE_DELAY_MS`).
    pub base_interval_ms: u64,
    pub last_flood_wait_secs: Option<u64>,
    pub last_flood_at: Option<DateTime<Utc>>,
}

/// Response of `PUT /chats/{id}/tracking`: the chat, plus backfill info when requested.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct TrackChatDto {
    #[serde(flatten)]
    pub chat: ChatDto,
    /// History sync job covering this chat (only with `backfill=true`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backfill_job_id: Option<String>,
    /// `queued` for a newly enqueued job, `already_running` when an existing job was reused.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backfill: Option<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ComponentStatusDto {
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SyncJobDto {
    pub id: String,
    pub scope: String,
    pub state: String,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub has_error: bool,
    /// Sanitized single-line failure reason (at most 300 chars; never message text).
    pub error_summary: Option<String>,
    /// Seconds Telegram asked to wait, for `rate_limited` jobs.
    pub retry_after_secs: Option<u64>,
    /// Per-chat progress; only present on `GET /sync/jobs/{id}`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chats: Option<Vec<SyncJobChatDto>>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SyncJobChatDto {
    pub chat_id: i64,
    pub title: Option<String>,
    pub state: String,
    pub committed_count: u64,
    pub error_summary: Option<String>,
}

const MAX_SUMMARY_CHARS: usize = 300;

fn bound_summary(text: String) -> String {
    let single_line = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>();
    let single_line = single_line.split_whitespace().collect::<Vec<_>>().join(" ");
    single_line.chars().take(MAX_SUMMARY_CHARS).collect()
}

/// Parses the `retry after N s` produced for FLOOD_WAIT failures.
fn parse_retry_after(summary: &str) -> Option<u64> {
    let rest = summary.split("retry after ").nth(1)?;
    rest.split_whitespace().next()?.parse().ok()
}

impl SyncJobDto {
    pub fn with_chats(
        mut self,
        progress: Vec<SyncChatProgress>,
        titles: &HashMap<i64, String>,
    ) -> Self {
        self.chats = Some(
            progress
                .into_iter()
                .map(|p| SyncJobChatDto {
                    chat_id: p.chat_id.get(),
                    title: titles.get(&p.chat_id.get()).cloned(),
                    state: job_state(p.state),
                    committed_count: p.committed_messages,
                    error_summary: p.summary_error.map(bound_summary),
                })
                .collect(),
        );
        self
    }
}

impl From<ApplicationStatus> for StatusDto {
    fn from(status: ApplicationStatus) -> Self {
        Self {
            collector: ComponentStatusDto {
                state: status.collector.state.as_str().to_owned(),
                detail: status.collector.detail,
            },
            sync_jobs: status.sync_jobs.into_iter().map(Into::into).collect(),
            unresolved_deletions: status.unresolved_deletions,
            search_index: status.search_index.into(),
            rate_limit: status.rate_limit.map(|r| RateLimitDto {
                interval_ms: r.interval_ms,
                base_interval_ms: r.base_interval_ms,
                last_flood_wait_secs: r.last_flood_wait_secs,
                last_flood_at: r.last_flood_at,
            }),
        }
    }
}

impl From<SyncJob> for SyncJobDto {
    fn from(job: SyncJob) -> Self {
        let error_summary = job.summary_error.map(bound_summary);
        let retry_after_secs = if job.state == SyncJobState::RateLimited {
            error_summary.as_deref().and_then(parse_retry_after)
        } else {
            None
        };
        Self {
            has_error: error_summary.is_some(),
            error_summary,
            retry_after_secs,
            chats: None,
            id: job.id,
            scope: match job.scope {
                crate::application::SyncScope::All => "all".to_owned(),
                crate::application::SyncScope::Chat(id) => format!("chat:{}", id.get()),
            },
            state: job_state(job.state),
            created_at: job.created_at,
            started_at: job.started_at,
            completed_at: job.completed_at,
        }
    }
}

fn job_state(state: SyncJobState) -> String {
    match state {
        SyncJobState::Queued => "queued",
        SyncJobState::Running => "running",
        SyncJobState::Succeeded => "succeeded",
        SyncJobState::Failed => "failed",
        SyncJobState::Interrupted => "interrupted",
        SyncJobState::RateLimited => "rate_limited",
    }
    .to_owned()
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct HealthDto {
    pub status: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summaries_are_single_line_bounded_and_retry_after_is_parsed() {
        assert_eq!(bound_summary("a\nb\tc".into()), "a b c");
        assert_eq!(bound_summary("x".repeat(500)).chars().count(), 300);
        assert_eq!(
            parse_retry_after("Telegram rate limit: retry after 42 s"),
            Some(42)
        );
        assert_eq!(parse_retry_after("boom"), None);
    }
}

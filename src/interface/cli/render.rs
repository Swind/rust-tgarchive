use super::*;

pub fn render_chats(output: OutputFormat, chats: &[Chat]) -> Result<String, CliError> {
    if output == OutputFormat::Json {
        json(&chats)
    } else if chats.is_empty() {
        Ok("No chats.".into())
    } else {
        Ok(chats.iter().map(human_chat).collect::<Vec<_>>().join("\n"))
    }
}

pub fn render_sync_job(output: OutputFormat, job: &SyncJob) -> Result<String, CliError> {
    if output == OutputFormat::Json {
        json(job)
    } else {
        let mut text = format!("sync {}\t{:?}", job.id, job.state);
        if let Some(error) = &job.summary_error {
            text.push_str("\nerror\t");
            text.push_str(error);
        }
        Ok(text)
    }
}

/// Resolves a `--sender` reference to exactly one sender; ambiguity lists the candidates.
pub(super) async fn resolve_sender(app: &Application, spec: &str) -> Result<SenderId, CliError> {
    let spec = spec.trim();
    if spec.eq_ignore_ascii_case("me") {
        return app.bound_account().await?.ok_or_else(|| {
            CliError::InvalidInput(
                "the archive is not bound to a Telegram account yet (run a sync or `serve` once)"
                    .into(),
            )
        });
    }
    if let Ok(id) = spec.parse::<i64>() {
        return Ok(SenderId::from_marked(id)?);
    }
    let page = app
        .search_senders(SenderQuery {
            text: Some(spec.to_owned()),
            sort: SenderSort::Messages,
            limit: PageSize::new(20)?,
            offset: 0,
            include_deleted: true,
            is_bot: None,
        })
        .await?;
    match page.items.as_slice() {
        [] => Err(CliError::SenderNotFound(spec.to_owned())),
        [only] => Ok(only.id),
        candidates => Err(CliError::AmbiguousSender(format!(
            "sender {spec:?} is ambiguous; use one of these IDs:\n{}",
            candidates
                .iter()
                .map(human_sender)
                .collect::<Vec<_>>()
                .join("\n")
        ))),
    }
}

pub(super) fn sender_kind_name(kind: SenderKind) -> &'static str {
    match kind {
        SenderKind::User => "user",
        SenderKind::Chat => "chat",
        SenderKind::Channel => "channel",
        SenderKind::Unknown => "unknown",
    }
}

pub(super) fn human_sender(sender: &SenderProfile) -> String {
    format!(
        "{}\t{}\t{}\t{}\t{} messages\t{} chats\t{}{}",
        sender.id.get(),
        sender_kind_name(sender.kind),
        sender.display_name.as_deref().unwrap_or(""),
        sender
            .username
            .as_deref()
            .map(|name| format!("@{name}"))
            .unwrap_or_default(),
        sender.message_count,
        sender.chat_count,
        sender
            .last_message_at
            .map(|time| time.to_rfc3339())
            .unwrap_or_default(),
        if sender.is_self { "\tself" } else { "" }
    ) + if sender.is_bot == Some(true) {
        "\tbot"
    } else {
        ""
    } + &sender
        .matched_history
        .as_ref()
        .map(|old| {
            format!(
                "\t[matched old name: {}{}]",
                old.display_name.as_deref().unwrap_or(""),
                old.username
                    .as_deref()
                    .map(|name| format!(" @{name}"))
                    .unwrap_or_default()
            )
        })
        .unwrap_or_default()
}

#[derive(Serialize)]
struct SenderOutput<'a> {
    id: i64,
    kind: &'static str,
    display_name: &'a Option<String>,
    username: &'a Option<String>,
    is_bot: Option<bool>,
    matched_history: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    matched_name: Option<SenderNameOutput<'a>>,
    message_count: u64,
    chat_count: u64,
    first_message_at: Option<DateTime<Utc>>,
    last_message_at: Option<DateTime<Utc>>,
    is_self: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    chats: Option<Vec<SenderChatOutput<'a>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name_history: Option<Vec<SenderNameHistoryOutput<'a>>>,
}

#[derive(Serialize)]
struct SenderNameOutput<'a> {
    display_name: &'a Option<String>,
    username: &'a Option<String>,
}

#[derive(Serialize)]
struct SenderNameHistoryOutput<'a> {
    display_name: &'a Option<String>,
    username: &'a Option<String>,
    first_seen_at: DateTime<Utc>,
    last_seen_at: DateTime<Utc>,
}

#[derive(Serialize)]
struct SenderChatOutput<'a> {
    chat_id: i64,
    title: &'a Option<String>,
    kind: String,
    message_count: u64,
    last_message_at: Option<DateTime<Utc>>,
}

fn sender_output<'a>(
    sender: &'a SenderProfile,
    chats: Option<Vec<SenderChatOutput<'a>>>,
) -> SenderOutput<'a> {
    SenderOutput {
        id: sender.id.get(),
        kind: sender_kind_name(sender.kind),
        display_name: &sender.display_name,
        username: &sender.username,
        is_bot: sender.is_bot,
        matched_history: sender.matched_history.is_some(),
        matched_name: sender.matched_history.as_ref().map(|old| SenderNameOutput {
            display_name: &old.display_name,
            username: &old.username,
        }),
        message_count: sender.message_count,
        chat_count: sender.chat_count,
        first_message_at: sender.first_message_at,
        last_message_at: sender.last_message_at,
        is_self: sender.is_self,
        chats,
        name_history: None,
    }
}

pub(super) fn render_senders(
    output: OutputFormat,
    senders: &[SenderProfile],
) -> Result<String, CliError> {
    if output == OutputFormat::Json {
        json(
            &senders
                .iter()
                .map(|s| sender_output(s, None))
                .collect::<Vec<_>>(),
        )
    } else if senders.is_empty() {
        Ok("No senders.".into())
    } else {
        Ok(senders
            .iter()
            .map(human_sender)
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

pub(super) fn render_sender_detail(
    output: OutputFormat,
    detail: &SenderDetail,
) -> Result<String, CliError> {
    let kind = |kind: ChatKind| format!("{kind:?}").to_lowercase();
    if output == OutputFormat::Json {
        let chats = detail
            .chats
            .iter()
            .map(|chat| SenderChatOutput {
                chat_id: chat.chat_id.get(),
                title: &chat.title,
                kind: kind(chat.kind),
                message_count: chat.message_count,
                last_message_at: chat.last_message_at,
            })
            .collect();
        let mut out = sender_output(&detail.profile, Some(chats));
        out.name_history = Some(
            detail
                .name_history
                .iter()
                .map(|entry| SenderNameHistoryOutput {
                    display_name: &entry.display_name,
                    username: &entry.username,
                    first_seen_at: entry.first_seen_at,
                    last_seen_at: entry.last_seen_at,
                })
                .collect(),
        );
        json(&out)
    } else {
        let mut lines = vec![human_sender(&detail.profile)];
        lines.extend(detail.chats.iter().map(|chat| {
            format!(
                "  chat {}\t{}\t{}\t{} messages\t{}",
                chat.chat_id.get(),
                kind(chat.kind),
                chat.title.as_deref().unwrap_or(""),
                chat.message_count,
                chat.last_message_at
                    .map(|time| time.to_rfc3339())
                    .unwrap_or_default()
            )
        }));
        if !detail.name_history.is_empty() {
            lines.push(
                "  name history (newest first; times are when tgarchive observed the name):".into(),
            );
            lines.extend(detail.name_history.iter().map(|entry| {
                format!(
                    "    {}\t{}\t{} .. {}",
                    entry.display_name.as_deref().unwrap_or(""),
                    entry
                        .username
                        .as_deref()
                        .map(|name| format!("@{name}"))
                        .unwrap_or_default(),
                    entry.first_seen_at.to_rfc3339(),
                    entry.last_seen_at.to_rfc3339()
                )
            }));
        }
        Ok(lines.join("\n"))
    }
}

pub fn render_sender_repair(
    output: OutputFormat,
    report: &SenderRepairReport,
) -> Result<String, CliError> {
    if output == OutputFormat::Json {
        return json(report);
    }
    let verb = if report.dry_run { "would fix" } else { "fixed" };
    let mut lines = vec![format!(
        "rule channel_post (sender = the channel): {verb} {} rows in {} chats",
        report.channel_posts_total,
        report.channel_posts.len()
    )];
    lines.extend(report.channel_posts.iter().map(|chat| {
        format!(
            "  {}\t{}\t{} rows",
            chat.chat_id,
            chat.title.as_deref().unwrap_or(""),
            chat.rows
        )
    }));
    lines.push(format!(
        "still without sender: {} rows in {} chats (private chats cannot be told apart without \
         data; re-fetch them with `tgarchive sync chat <ID> --refetch`)",
        report.unresolved_total,
        report.unresolved.len()
    ));
    lines.extend(report.unresolved.iter().map(|chat| {
        format!(
            "  {}\t{}\t{}\t{} rows",
            chat.chat_id,
            chat.kind,
            chat.title.as_deref().unwrap_or(""),
            chat.rows
        )
    }));
    Ok(lines.join("\n"))
}

pub(super) fn render_message_page(
    output: OutputFormat,
    page: MessagePage,
) -> Result<String, CliError> {
    let next_cursor = match (page.next_cursor.as_ref(), page.next_offset) {
        (Some(cursor), _) => Some(crate::interface::cursor::encode(cursor)?),
        (None, Some(offset)) => Some(crate::interface::cursor::encode_offset(offset)?),
        (None, None) => None,
    };
    if output == OutputFormat::Json {
        json(&MessagePageOutput {
            items: page.items,
            has_more: page.has_more,
            next_cursor,
        })
    } else if page.items.is_empty() && !page.has_more {
        Ok("No messages.".into())
    } else {
        let mut lines = page.items.iter().map(human_message).collect::<Vec<_>>();
        lines.push(format!("has_more\t{}", page.has_more));
        if let Some(cursor) = next_cursor {
            lines.push(format!("next_cursor\t{cursor}"));
        }
        Ok(lines.join("\n"))
    }
}

pub(super) fn render_text(
    output: OutputFormat,
    text: &str,
    json_value: &str,
) -> Result<String, CliError> {
    if output == OutputFormat::Json {
        Ok(json_value.to_owned())
    } else {
        Ok(text.to_owned())
    }
}

pub fn initialized_output(output: OutputFormat) -> Result<String, CliError> {
    render_text(
        output,
        "Initialized archive database.",
        "{\"initialized\":true}",
    )
}

pub(super) fn json(value: &impl Serialize) -> Result<String, CliError> {
    serde_json::to_string(value).map_err(Into::into)
}

pub(super) fn human_chat(chat: &Chat) -> String {
    format!(
        "{}\t{:?}\t{}\t{}",
        chat.id.get(),
        chat.kind,
        chat.title.as_deref().unwrap_or(""),
        if chat.tracked { "tracked" } else { "untracked" }
    )
}

pub(super) fn human_message(message: &MessageView) -> String {
    let sender = message
        .sender
        .as_ref()
        .and_then(|sender| {
            sender
                .display_name
                .as_deref()
                .or(sender.username.as_deref())
        })
        .map(str::to_owned)
        .or_else(|| message.sender_id.map(|id| id.get().to_string()))
        .unwrap_or_default();
    format!(
        "{}\t{}\t{}\t{}\t{}{}",
        message.chat_id.get(),
        message.id.get(),
        message.timestamp.to_rfc3339(),
        sender,
        if message.is_deleted() {
            "[deleted] "
        } else {
            ""
        },
        message.text.as_deref().unwrap_or("")
    )
}

pub(super) fn human_search_index(status: &crate::application::SearchIndexStatus) -> String {
    format!(
        "search index\t{} (version {}, {}/{} messages indexed)",
        status.state.as_str(),
        status.version,
        status.indexed,
        status.total
    )
}

pub(super) fn component_state(state: crate::application::services::ComponentState) -> &'static str {
    state.as_str()
}

#[derive(Serialize)]
struct MessagePageOutput {
    items: Vec<MessageView>,
    has_more: bool,
    next_cursor: Option<String>,
}

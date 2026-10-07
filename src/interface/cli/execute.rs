use super::*;

pub async fn execute_prepared(
    command: PreparedCommand,
    output: OutputFormat,
    app: &Application,
) -> Result<String, CliError> {
    match command {
        PreparedCommand::ChatsList { tracked_only } => {
            let chats = app.list_chats(tracked_only).await?;
            render_chats(output, &chats)
        }
        PreparedCommand::ChatGet(chat_id) => {
            let chat = app.get_chat(chat_id).await?;
            if output == OutputFormat::Json {
                json(&chat)
            } else {
                Ok(human_chat(&chat))
            }
        }
        PreparedCommand::MessagesList(mut query, sender) => {
            if let Some(sender) = sender {
                query.filters.sender_id = Some(resolve_sender(app, &sender).await?);
            }
            render_message_page(output, app.list_messages(query).await?)
        }
        PreparedCommand::SendersSearch(query) => {
            let page = app.search_senders(query).await?;
            render_senders(output, &page.items)
        }
        PreparedCommand::SenderGet(sender, include_deleted) => {
            let id = resolve_sender(app, &sender).await?;
            let detail = app.sender_detail(id, include_deleted).await?;
            render_sender_detail(output, &detail)
        }
        PreparedCommand::MessageGet(chat_id, message_id, include_deleted) => {
            let message = app
                .get_message(chat_id, message_id, include_deleted)
                .await?;
            if output == OutputFormat::Json {
                json(&message)
            } else {
                Ok(human_message(&message))
            }
        }
        PreparedCommand::MessagesSearch(mut query, sender) => {
            if let Some(sender) = sender {
                query.filters.sender_id = Some(resolve_sender(app, &sender).await?);
            }
            render_message_page(output, app.search_messages(query).await?)
        }
        PreparedCommand::SearchStatus => {
            let status = app.sync_status().await?.search_index;
            if output == OutputFormat::Json {
                json(&status)
            } else {
                Ok(human_search_index(&status))
            }
        }
        PreparedCommand::Status => {
            let status = app.sync_status().await?;
            if output == OutputFormat::Json {
                let state = component_state(status.collector.state);
                json(&StatusOutput {
                    collector: CollectorOutput {
                        state,
                        detail: &status.collector.detail,
                    },
                    sync_jobs: &status.sync_jobs,
                    search_index: &status.search_index,
                })
            } else {
                let mut lines = vec![format!(
                    "collector\t{}",
                    component_state(status.collector.state)
                )];
                if let Some(detail) = status.collector.detail {
                    lines.push(format!("collector detail\t{detail}"));
                }
                lines.push(human_search_index(&status.search_index));
                if status.sync_jobs.is_empty() {
                    lines.push("No sync jobs.".into());
                } else {
                    lines.extend(
                        status
                            .sync_jobs
                            .iter()
                            .map(|job| format!("sync {}\t{:?}", job.id, job.state)),
                    );
                }
                Ok(lines.join("\n"))
            }
        }
    }
}

pub async fn execute_set_tracking(
    chat_id: ChatId,
    tracked: bool,
    output: OutputFormat,
    app: &Application,
) -> Result<String, CliError> {
    let result = if tracked {
        app.track_chat(chat_id).await
    } else {
        app.untrack_chat(chat_id).await
    };
    let chat = result.map_err(|error| match error {
        ApplicationError::NotFound => CliError::ChatNotFound(chat_id.get()),
        other => other.into(),
    })?;
    if output == OutputFormat::Json {
        json(&chat)
    } else if tracked {
        Ok(format!("Tracking chat {}.", chat.id.get()))
    } else {
        Ok(format!(
            "Stopped tracking chat {}; stored messages are kept.",
            chat.id.get()
        ))
    }
}

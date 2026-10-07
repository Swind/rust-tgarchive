use super::*;

pub(super) fn list_query(args: MessageFilterArgs) -> Result<ListMessagesQuery, CliError> {
    let (filters, before, after, page_size) = convert_filters(args)?;
    let query = ListMessagesQuery {
        filters,
        before,
        after,
        page_size,
    };
    query.validate()?;
    Ok(query)
}

pub(super) fn search_query(
    text: String,
    sort: SearchSortArg,
    mut args: MessageFilterArgs,
) -> Result<SearchMessagesQuery, CliError> {
    let (sort, offset) = match sort {
        SearchSortArg::Time => (SearchSort::Time, 0),
        SearchSortArg::Relevance => {
            if args.after.is_some() {
                return Err(CliError::InvalidInput(
                    "--after is only valid with --sort time".into(),
                ));
            }
            let offset = args
                .before
                .take()
                .map(|value| crate::interface::cursor::decode_offset(&value))
                .transpose()?
                .unwrap_or(0);
            (SearchSort::Relevance, offset)
        }
    };
    let (filters, before, after, page_size) = convert_filters(args)?;
    let query = SearchMessagesQuery {
        text,
        filters,
        before,
        after,
        page_size,
        sort,
        offset,
    };
    query.validate()?;
    Ok(query)
}

pub(super) fn convert_filters(
    args: MessageFilterArgs,
) -> Result<
    (
        MessageFilters,
        Option<crate::application::MessageCursor>,
        Option<crate::application::MessageCursor>,
        PageSize,
    ),
    CliError,
> {
    let filters = MessageFilters {
        chat_id: args.chat_id.map(ChatId::from_marked).transpose()?,
        sender_id: args.sender_id.map(SenderId::from_marked).transpose()?,
        post_author: args.post_author,
        time_range: TimeRange::new(args.from, args.to)?,
        include_deleted: args.include_deleted,
        exclude_bots: args.exclude_bots,
    };
    let before = args
        .before
        .as_deref()
        .map(crate::interface::cursor::decode)
        .transpose()?;
    let after = args
        .after
        .as_deref()
        .map(crate::interface::cursor::decode)
        .transpose()?;
    Ok((filters, before, after, PageSize::new(args.limit)?))
}

pub(super) fn parse_message_id(value: &str) -> Result<MessageId, String> {
    value
        .parse::<i64>()
        .map_err(|_| "message ID must be an integer".to_owned())
        .and_then(|value| MessageId::new(value).map_err(|error| error.to_string()))
}

pub(super) fn parse_timestamp(value: &str) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|error| format!("timestamp must be RFC 3339: {error}"))
}

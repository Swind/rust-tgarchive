use super::*;
use tl::{enums, types};

fn topic(id: i32) -> enums::ForumTopic {
    types::ForumTopic {
        my: false,
        closed: false,
        pinned: false,
        short: false,
        hidden: false,
        title_missing: false,
        id,
        date: 10,
        peer: types::PeerChannel { channel_id: 9 }.into(),
        title: format!("topic {id}"),
        icon_color: 0,
        icon_emoji_id: None,
        top_message: id + 100,
        read_inbox_max_id: 0,
        read_outbox_max_id: 0,
        unread_count: 0,
        unread_mentions_count: 0,
        unread_reactions_count: 0,
        unread_poll_votes_count: 0,
        from_id: types::PeerUser { user_id: 8 }.into(),
        notify_settings: types::PeerNotifySettings {
            show_previews: None,
            silent: None,
            mute_until: None,
            ios_sound: None,
            android_sound: None,
            other_sound: None,
            stories_muted: None,
            stories_hide_sender: None,
            stories_ios_sound: None,
            stories_android_sound: None,
            stories_other_sound: None,
        }
        .into(),
        draft: None,
    }
    .into()
}

#[test]
fn short_pages_use_the_correct_ordering_date_and_deduplicate_overlaps() {
    let last_message = types::MessageService {
        out: false,
        mentioned: false,
        media_unread: false,
        reactions_are_possible: false,
        silent: false,
        post: false,
        legacy: false,
        id: 123,
        from_id: None,
        peer_id: types::PeerChannel { channel_id: 9 }.into(),
        saved_peer_id: None,
        reply_to: None,
        date: 99,
        action: enums::MessageAction::PinMessage,
        reactions: None,
        ttl_period: None,
    };
    for (order_by_create_date, expected_date) in [(true, 10), (false, 99)] {
        let mut topics = Vec::new();
        let mut seen = HashSet::new();
        let mut request = tl::functions::messages::GetForumTopics {
            peer: enums::InputPeer::Empty,
            q: None,
            offset_date: 0,
            offset_id: 0,
            offset_topic: 0,
            limit: TELEGRAM_PAGE_LIMIT,
        };
        let page = |topics| types::messages::ForumTopics {
            order_by_create_date,
            count: 3,
            topics,
            messages: vec![last_message.clone().into()],
            chats: Vec::new(),
            users: Vec::new(),
            pts: 0,
        };
        assert!(append_page(page(vec![topic(23)]), &mut topics, &mut seen, &mut request).unwrap());
        assert_eq!(
            (request.offset_date, request.offset_id, request.offset_topic),
            (expected_date, 123, 23)
        );
        let last = page(vec![
            topic(23),
            types::ForumTopicDeleted { id: 30 }.into(),
            topic(11),
        ]);
        assert!(!append_page(last, &mut topics, &mut seen, &mut request).unwrap());
        assert_eq!(
            topics
                .iter()
                .map(|topic| topic.id.get())
                .collect::<Vec<_>>(),
            [23, 11]
        );
    }
}

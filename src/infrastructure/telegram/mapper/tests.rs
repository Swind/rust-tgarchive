use super::*;

fn raw_message(
    media: Option<grammers_client::tl::enums::MessageMedia>,
) -> grammers_client::tl::enums::Message {
    use grammers_client::tl::{enums::Peer, types};

    types::Message {
        out: false,
        mentioned: false,
        media_unread: false,
        silent: false,
        post: false,
        from_scheduled: false,
        legacy: false,
        edit_hide: false,
        pinned: false,
        noforwards: false,
        invert_media: false,
        offline: false,
        video_processing_pending: false,
        paid_suggested_post_stars: false,
        paid_suggested_post_ton: false,
        id: 123,
        from_id: Some(Peer::User(types::PeerUser { user_id: 99 })),
        from_boosts_applied: None,
        from_rank: None,
        peer_id: Peer::User(types::PeerUser { user_id: 42 }),
        saved_peer_id: None,
        fwd_from: None,
        via_bot_id: None,
        via_business_bot_id: None,
        guestchat_via_from: None,
        reply_to: Some(grammers_client::tl::enums::MessageReplyHeader::Header(
            types::MessageReplyHeader {
                reply_to_scheduled: false,
                forum_topic: false,
                quote: false,
                reply_to_ephemeral: false,
                reply_to_msg_id: Some(7),
                reply_to_peer_id: None,
                reply_from: None,
                reply_media: None,
                reply_to_top_id: None,
                quote_text: None,
                quote_entities: None,
                quote_offset: None,
                todo_item_id: None,
                poll_option: None,
            },
        )),
        date: 1_700_000_000,
        message: "fixture text".into(),
        media,
        reply_markup: None,
        entities: None,
        views: None,
        forwards: None,
        replies: None,
        edit_date: None,
        post_author: None,
        grouped_id: None,
        reactions: None,
        restriction_reason: None,
        ttl_period: None,
        quick_reply_shortcut_id: None,
        effect: None,
        factcheck: None,
        report_delivery_until_date: None,
        paid_message_stars: None,
        suggested_post: None,
        schedule_repeat_period: None,
        summary_from_language: None,
        rich_message: None,
    }
    .into()
}

#[test]
fn grammers_peer_ids_map_to_domain_marked_ids() {
    assert_eq!(chat_id(PeerId::user(42).unwrap()).unwrap().get(), 42);
    assert_eq!(chat_id(PeerId::chat(42).unwrap()).unwrap().get(), -42);
    assert_eq!(
        chat_id(PeerId::channel(42).unwrap()).unwrap().get(),
        -1_000_000_000_042
    );
    assert_eq!(sender_id(PeerId::user(42).unwrap()).unwrap().get(), 42);
    assert_eq!(sender_id(PeerId::chat(42).unwrap()).unwrap().get(), -42);
    assert_eq!(
        sender_id(PeerId::channel(42).unwrap()).unwrap().get(),
        -1_000_000_000_042
    );
}

#[test]
fn self_user_without_known_numeric_id_is_not_fabricated() {
    assert!(matches!(
        chat_id(PeerId::self_user()),
        Err(MappingError::InvalidPeerId)
    ));
    assert!(matches!(
        sender_id(PeerId::self_user()),
        Err(MappingError::InvalidPeerId)
    ));
}

fn raw_user(bot: bool) -> grammers_client::tl::enums::User {
    use grammers_client::tl::types::User;
    User {
        is_self: false,
        contact: false,
        mutual_contact: false,
        deleted: false,
        bot,
        bot_chat_history: false,
        bot_nochats: false,
        verified: false,
        restricted: false,
        min: false,
        bot_inline_geo: false,
        support: false,
        scam: false,
        apply_min_photo: false,
        fake: false,
        bot_attach_menu: false,
        premium: false,
        attach_menu_enabled: false,
        bot_can_edit: false,
        close_friend: false,
        stories_hidden: false,
        stories_unavailable: false,
        contact_require_premium: false,
        bot_business: false,
        bot_has_main_app: false,
        bot_forum_view: false,
        bot_forum_can_manage_topics: false,
        bot_can_manage_bots: false,
        bot_guestchat: false,
        bot_guard: false,
        id: 7,
        access_hash: None,
        first_name: Some("Helper".into()),
        last_name: None,
        username: None,
        phone: None,
        photo: None,
        status: None,
        bot_info_version: None,
        restriction_reason: None,
        bot_inline_placeholder: None,
        lang_code: None,
        emoji_status: None,
        usernames: None,
        stories_max_id: None,
        color: None,
        profile_color: None,
        bot_active_users: None,
        bot_verification_icon: None,
        send_paid_messages_stars: None,
    }
    .into()
}

#[test]
fn bot_flag_comes_from_the_user_and_is_unknown_for_empty_stubs() {
    use grammers_client::tl::{enums, types};

    assert_eq!(user_is_bot(&raw_user(true)), Some(true));
    assert_eq!(user_is_bot(&raw_user(false)), Some(false));
    let empty = enums::User::Empty(types::UserEmpty { id: 7 });
    assert_eq!(user_is_bot(&empty), None);
}

#[test]
fn display_names_trim_absent_parts_without_empty_names() {
    assert_eq!(
        display_name(Some("Ada"), Some("Lovelace")),
        Some("Ada Lovelace".into())
    );
    assert_eq!(display_name(Some("Ada"), None), Some("Ada".into()));
    assert_eq!(display_name(None, Some("")), None);
}

#[test]
fn raw_message_media_and_reply_fixture_maps_to_domain_message() {
    use grammers_client::tl::{enums, types};

    let media = enums::MessageMedia::Document(types::MessageMediaDocument {
        nopremium: false,
        spoiler: false,
        video: true,
        round: false,
        voice: false,
        document: Some(enums::Document::Document(types::Document {
            id: 456,
            access_hash: 1,
            file_reference: Vec::new(),
            date: 1_700_000_000,
            mime_type: "video/mp4".into(),
            size: 2048,
            thumbs: None,
            video_thumbs: None,
            dc_id: 2,
            attributes: Vec::new(),
        })),
        alt_documents: None,
        video_cover: None,
        video_timestamp: None,
        ttl_seconds: None,
    });
    let mapped = map_raw_message(&raw_message(Some(media)), Utc::now(), None).unwrap();

    assert_eq!(mapped.id.get(), 123);
    assert_eq!(mapped.chat_id.get(), 42);
    assert_eq!(mapped.sender_id.unwrap().get(), 99);
    assert_eq!(mapped.reply_to.unwrap().get(), 7);
    assert_eq!(mapped.text.as_deref(), Some("fixture text"));
    assert_eq!(mapped.attachments[0].kind, AttachmentKind::Video);
    assert_eq!(
        mapped.attachments[0].telegram_file_id.as_deref(),
        Some("456")
    );
    assert_eq!(
        mapped.attachments[0].mime_type.as_deref(),
        Some("video/mp4")
    );
    assert_eq!(mapped.attachments[0].size, Some(2048));
}

#[test]
fn raw_service_message_fixture_is_preserved_as_a_message_without_fake_text() {
    use grammers_client::tl::{enums::Peer, types};

    let service = types::MessageService {
        out: false,
        mentioned: false,
        media_unread: false,
        reactions_are_possible: false,
        silent: false,
        post: false,
        legacy: false,
        id: 55,
        from_id: Some(Peer::User(types::PeerUser { user_id: 5 })),
        peer_id: Peer::Chat(types::PeerChat { chat_id: 6 }),
        saved_peer_id: None,
        reply_to: None,
        date: 1_700_000_001,
        action: grammers_client::tl::enums::MessageAction::PinMessage,
        reactions: None,
        ttl_period: None,
    };
    let mapped = map_raw_message(&service.into(), Utc::now(), None).unwrap();

    assert_eq!(mapped.id.get(), 55);
    assert_eq!(mapped.chat_id.get(), -6);
    assert_eq!(mapped.sender_id.unwrap().get(), 5);
    assert_eq!(mapped.text, None);
    assert!(mapped.attachments.is_empty());
}

#[test]
fn incoming_private_message_without_from_id_is_attributed_to_the_peer() {
    use grammers_client::tl::{enums::Peer, types};

    let mut service = types::MessageService {
        out: false,
        mentioned: false,
        media_unread: false,
        reactions_are_possible: false,
        silent: false,
        post: false,
        legacy: false,
        id: 1,
        from_id: None,
        peer_id: Peer::User(types::PeerUser { user_id: 777_000 }),
        saved_peer_id: None,
        reply_to: None,
        date: 1_700_000_001,
        action: grammers_client::tl::enums::MessageAction::PinMessage,
        reactions: None,
        ttl_period: None,
    };
    let mapped = map_raw_message(&service.clone().into(), Utc::now(), None).unwrap();
    assert_eq!(mapped.sender_id.unwrap().get(), 777_000);
    service.out = true;
    let mapped = map_raw_message(&service.clone().into(), Utc::now(), None).unwrap();
    assert_eq!(mapped.sender_id, None);
    let account = SenderId::from_marked(4242).unwrap();
    let mapped = map_raw_message(&service.into(), Utc::now(), Some(account)).unwrap();
    assert_eq!(mapped.sender_id, Some(account));
}

#[test]
fn outgoing_message_without_from_id_is_attributed_to_the_account_but_posts_stay_channel() {
    use grammers_client::tl::{enums::Peer, types};

    let account = SenderId::from_marked(4242).unwrap();
    let mut raw = raw_message(None);
    let grammers_client::tl::enums::Message::Message(message) = &mut raw else {
        unreachable!()
    };
    message.from_id = None;
    message.out = true;
    let mapped = map_raw_message(&raw, Utc::now(), Some(account)).unwrap();
    assert_eq!(mapped.sender_id, Some(account));
    let grammers_client::tl::enums::Message::Message(message) = &mut raw else {
        unreachable!()
    };
    message.peer_id = Peer::Channel(types::PeerChannel { channel_id: 9 });
    message.post = true;
    let mapped = map_raw_message(&raw, Utc::now(), Some(account)).unwrap();
    assert_eq!(mapped.sender_id.unwrap().get(), -1_000_000_000_009);
}

#[test]
fn post_author_and_forward_origin_are_mapped_without_touching_the_sender() {
    use grammers_client::tl::{enums, types};

    let mut raw = raw_message(None);
    let enums::Message::Message(message) = &mut raw else {
        unreachable!()
    };
    message.post_author = Some("Editor".into());
    message.fwd_from = Some(enums::MessageFwdHeader::Header(types::MessageFwdHeader {
        imported: false,
        saved_out: false,
        from_id: Some(enums::Peer::Channel(types::PeerChannel { channel_id: 5 })),
        from_name: Some("Origin".into()),
        date: 1_600_000_000,
        channel_post: None,
        post_author: None,
        saved_from_peer: None,
        saved_from_msg_id: None,
        saved_from_id: None,
        saved_from_name: None,
        saved_date: None,
        psa_type: None,
    }));
    let mapped = map_raw_message(&raw, Utc::now(), None).unwrap();
    assert_eq!(mapped.sender_id.unwrap().get(), 99);
    assert_eq!(mapped.post_author.as_deref(), Some("Editor"));
    let forward = mapped.forward.unwrap();
    assert_eq!(forward.from_id.unwrap().get(), -1_000_000_000_005);
    assert_eq!(forward.from_name.as_deref(), Some("Origin"));
    assert_eq!(forward.date.unwrap().timestamp(), 1_600_000_000);
}

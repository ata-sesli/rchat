use crate::network::gossip::GroupMessageEnvelope;

#[derive(Debug, Clone)]
pub enum DirectMediaKind {
    Image,
    Sticker,
    Document,
    Video,
    Audio,
}

#[derive(Debug, Clone)]
pub enum NetworkCommand {
    StartPunch {
        multiaddr: String,
        target_username: String,
        my_username: String,
    },
    RequestConnection {
        peer_id: String,
    },
    RegisterShadow {
        invitee: String,
        password: String,
        my_username: String,
    },
    RegisterTemporarySession {
        chat_id: String,
        peer_id: String,
        multiaddr: String,
        is_group: bool,
    },
    EndTemporarySession {
        chat_id: String,
    },
    SubscribeGroup {
        group_id: String,
    },
    UnsubscribeGroup {
        group_id: String,
    },
    PublishGroup {
        envelope: GroupMessageEnvelope,
    },
    SendDirectText {
        target_peer_id: String,
        msg_id: String,
        timestamp: i64,
        sender_alias: Option<String>,
        content: String,
    },
    SendReadReceipt {
        target_peer_id: String,
        msg_ids: Vec<String>,
    },
    SendDirectMedia {
        kind: DirectMediaKind,
        target_peer_id: String,
        file_hash: String,
        file_name: Option<String>,
        msg_id: String,
        timestamp: i64,
    },
}

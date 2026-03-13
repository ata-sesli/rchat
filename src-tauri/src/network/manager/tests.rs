use super::{
    build_incoming_dm_db_message, build_incoming_group_db_message, classify_outgoing_error_source,
    OutgoingDialSource, PeerTransportRegistry, RecentDial,
};
use crate::network::direct_message::{DirectMessageKind, DirectMessageRequest};
use crate::network::gossip::{GroupContentType, GroupMessageEnvelope};
use libp2p::Multiaddr;
use std::collections::HashMap;

fn incoming_request(
    kind: DirectMessageKind,
    text_content: Option<&str>,
    file_hash: Option<&str>,
) -> DirectMessageRequest {
    DirectMessageRequest {
        id: "msg-1".to_string(),
        sender_id: "peer-123".to_string(),
        msg_type: kind,
        text_content: text_content.map(ToString::to_string),
        file_hash: file_hash.map(ToString::to_string),
        timestamp: 1_700_000_000,
        chunk_hash: None,
        chunk_data: None,
        chunk_list: None,
        sender_alias: Some("peer".to_string()),
    }
}

#[test]
fn dm_text_maps_to_expected_db_shape() {
    let req = incoming_request(DirectMessageKind::Text, Some("hello"), None);
    let db = build_incoming_dm_db_message(&req, "chat-a".to_string());

    assert_eq!(db.content_type, "text");
    assert_eq!(db.text_content.as_deref(), Some("hello"));
    assert!(db.file_hash.is_none());
}

#[test]
fn dm_image_maps_to_expected_db_shape() {
    let req = incoming_request(DirectMessageKind::Image, None, Some("img-hash"));
    let db = build_incoming_dm_db_message(&req, "chat-a".to_string());

    assert_eq!(db.content_type, "image");
    assert!(db.text_content.is_none());
    assert_eq!(db.file_hash.as_deref(), Some("img-hash"));
}

#[test]
fn dm_sticker_maps_to_expected_db_shape() {
    let req = incoming_request(DirectMessageKind::Sticker, None, Some("sticker-hash"));
    let db = build_incoming_dm_db_message(&req, "chat-a".to_string());

    assert_eq!(db.content_type, "sticker");
    assert!(db.text_content.is_none());
    assert_eq!(db.file_hash.as_deref(), Some("sticker-hash"));
}

#[test]
fn dm_document_maps_to_expected_db_shape() {
    let req = incoming_request(
        DirectMessageKind::Document,
        Some("spec.pdf"),
        Some("doc-hash"),
    );
    let db = build_incoming_dm_db_message(&req, "chat-a".to_string());

    assert_eq!(db.content_type, "document");
    assert_eq!(db.text_content.as_deref(), Some("spec.pdf"));
    assert_eq!(db.file_hash.as_deref(), Some("doc-hash"));
}

#[test]
fn dm_video_maps_to_expected_db_shape() {
    let req = incoming_request(
        DirectMessageKind::Video,
        Some("clip.mp4"),
        Some("video-hash"),
    );
    let db = build_incoming_dm_db_message(&req, "chat-a".to_string());

    assert_eq!(db.content_type, "video");
    assert_eq!(db.text_content.as_deref(), Some("clip.mp4"));
    assert_eq!(db.file_hash.as_deref(), Some("video-hash"));
}

#[test]
fn dm_audio_maps_to_expected_db_shape() {
    let req = incoming_request(
        DirectMessageKind::Audio,
        Some("note.m4a"),
        Some("audio-hash"),
    );
    let db = build_incoming_dm_db_message(&req, "chat-a".to_string());

    assert_eq!(db.content_type, "audio");
    assert_eq!(db.text_content.as_deref(), Some("note.m4a"));
    assert_eq!(db.file_hash.as_deref(), Some("audio-hash"));
}

#[test]
fn group_document_maps_to_expected_db_shape() {
    let envelope = GroupMessageEnvelope {
        id: "g-1".to_string(),
        group_id: "group:550e8400-e29b-41d4-a716-446655440000".to_string(),
        sender_id: "peer-2".to_string(),
        sender_alias: Some("alice".to_string()),
        timestamp: 1_700_000_000,
        content_type: GroupContentType::Document,
        text_content: Some("brief.pdf".to_string()),
        file_hash: Some("doc-hash".to_string()),
    };

    let db = build_incoming_group_db_message(&envelope);
    assert_eq!(db.chat_id, envelope.group_id);
    assert_eq!(db.peer_id, "peer-2");
    assert_eq!(db.content_type, "document");
    assert_eq!(db.text_content.as_deref(), Some("brief.pdf"));
    assert_eq!(db.file_hash.as_deref(), Some("doc-hash"));
}

#[test]
fn group_audio_maps_to_expected_db_shape() {
    let envelope = GroupMessageEnvelope {
        id: "g-2".to_string(),
        group_id: "group:550e8400-e29b-41d4-a716-446655440000".to_string(),
        sender_id: "peer-2".to_string(),
        sender_alias: Some("alice".to_string()),
        timestamp: 1_700_000_001,
        content_type: GroupContentType::Audio,
        text_content: Some("voice-note.webm".to_string()),
        file_hash: Some("audio-hash".to_string()),
    };

    let db = build_incoming_group_db_message(&envelope);
    assert_eq!(db.chat_id, envelope.group_id);
    assert_eq!(db.peer_id, "peer-2");
    assert_eq!(db.content_type, "audio");
    assert_eq!(db.text_content.as_deref(), Some("voice-note.webm"));
    assert_eq!(db.file_hash.as_deref(), Some("audio-hash"));
}

#[test]
fn peer_transport_registry_tracks_quic_and_tcp() {
    let keypair = libp2p::identity::Keypair::generate_ed25519();
    let peer = keypair.public().to_peer_id();
    let quic: Multiaddr = "/ip4/10.0.0.5/udp/4242/quic-v1".parse().unwrap();
    let tcp: Multiaddr = "/ip4/10.0.0.5/tcp/4242".parse().unwrap();

    let mut registry = PeerTransportRegistry::default();
    assert!(!registry.has_quic(&peer));

    registry.record_connected(peer, &tcp);
    assert!(!registry.has_quic(&peer));

    registry.record_connected(peer, &quic);
    assert!(registry.has_quic(&peer));

    let quic_lost = registry.record_disconnected(peer, &quic);
    assert!(quic_lost);
    assert!(!registry.has_quic(&peer));
}

#[test]
fn peer_transport_registry_handles_multiple_quic_connections() {
    let keypair = libp2p::identity::Keypair::generate_ed25519();
    let peer = keypair.public().to_peer_id();
    let quic_a: Multiaddr = "/ip4/10.0.0.5/udp/4242/quic-v1".parse().unwrap();
    let quic_b: Multiaddr = "/ip4/10.0.0.6/udp/5252/quic-v1".parse().unwrap();

    let mut registry = PeerTransportRegistry::default();
    registry.record_connected(peer, &quic_a);
    registry.record_connected(peer, &quic_b);
    assert!(registry.has_quic(&peer));

    let lost_after_first_close = registry.record_disconnected(peer, &quic_a);
    assert!(!lost_after_first_close);
    assert!(registry.has_quic(&peer));

    let lost_after_second_close = registry.record_disconnected(peer, &quic_b);
    assert!(lost_after_second_close);
    assert!(!registry.has_quic(&peer));
}

#[test]
fn outgoing_error_classifier_marks_nat_keepalive() {
    let now = std::time::Instant::now();
    let recent = HashMap::<String, RecentDial>::new();
    let source = classify_outgoing_error_source(
        "Transport([(/ip4/1.1.1.1/udp/9/quic-v1, Other(...))])",
        Some("/ip4/1.1.1.1/udp/9/quic-v1"),
        &recent,
        true,
        true,
        true,
        now,
    );
    assert_eq!(source, OutgoingDialSource::NatKeepalive);
}

#[test]
fn outgoing_error_classifier_uses_recent_mdns_dial_context() {
    let now = std::time::Instant::now();
    let mut recent = HashMap::<String, RecentDial>::new();
    recent.insert(
        "/ip4/192.168.1.10/udp/7777/quic-v1".to_string(),
        RecentDial {
            source: OutgoingDialSource::Mdns,
            at: now,
        },
    );

    let source = classify_outgoing_error_source(
        "Transport([(/ip4/192.168.1.10/udp/7777/quic-v1, Other(...))])",
        Some("/ip4/192.168.1.10/udp/7777/quic-v1"),
        &recent,
        false,
        false,
        false,
        now,
    );
    assert_eq!(source, OutgoingDialSource::Mdns);
}

#[test]
fn outgoing_error_classifier_returns_unknown_without_context() {
    let now = std::time::Instant::now();
    let recent = HashMap::<String, RecentDial>::new();
    let source = classify_outgoing_error_source(
        "Transport([(/ip4/203.0.113.4/udp/9000/quic-v1, Other(...))])",
        Some("/ip4/203.0.113.4/udp/9000/quic-v1"),
        &recent,
        false,
        false,
        false,
        now,
    );
    assert_eq!(source, OutgoingDialSource::Unknown);
}

#[test]
fn keepalive_classification_does_not_trigger_mdns_classification() {
    let now = std::time::Instant::now();
    let recent = HashMap::<String, RecentDial>::new();
    let source = classify_outgoing_error_source(
        "Transport([(/ip4/1.1.1.1/udp/9/quic-v1, Other(Custom { kind: Other, error: HandshakeTimedOut }))])",
        None,
        &recent,
        true,
        true,
        true,
        now,
    );
    assert_ne!(source, OutgoingDialSource::Mdns);
    assert_eq!(source, OutgoingDialSource::NatKeepalive);
}

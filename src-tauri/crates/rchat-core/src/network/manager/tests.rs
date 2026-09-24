use super::{
    build_incoming_dm_db_message, build_incoming_group_db_message, classify_outgoing_error_source,
    incoming_call_reject_decision, punch_target_matches_peer, quic_addresses_for_peer,
    record_group_sync_request, ringing_call_peer_liveness_reason, ActiveCall, ActiveCallPhase,
    GroupSyncProgress,
    OutgoingDialSource, PeerTransportRegistry, RecentDial, VoiceStreamEvent,
};
use crate::app_state::CallKind;
use crate::network::command::NetworkCommand;
use crate::network::direct_message::{DirectMessageKind, DirectMessageRequest};
use crate::network::gossip::{GroupContentType, GroupMessageEnvelope};
use libp2p::{Multiaddr, PeerId};
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

fn active_call(call_id: &str, kind: CallKind, phase: ActiveCallPhase) -> ActiveCall {
    let keypair = libp2p::identity::Keypair::generate_ed25519();
    let peer: PeerId = keypair.public().to_peer_id();
    ActiveCall {
        call_id: call_id.to_string(),
        kind,
        peer_chat_id: format!("lh:test-{}", peer),
        remote_peer_id: peer,
        phase,
        ring_deadline: None,
        ring_expires_at: None,
        started_at: None,
        muted: false,
        camera_enabled: false,
    }
}

#[test]
fn incoming_call_reject_accepts_stale_requested_id_for_current_incoming_voice_call() {
    let call = active_call("call-current", CallKind::Voice, ActiveCallPhase::IncomingRinging);

    let decision = incoming_call_reject_decision(Some(&call), "call-previous", CallKind::Voice)
        .expect("incoming voice call should be rejectable");

    assert_eq!(decision.call.call_id, "call-current");
    assert!(!decision.requested_call_id_matched);
}

#[test]
fn punch_targets_only_complete_for_the_matching_peer_id() {
    let expected = libp2p::identity::Keypair::generate_ed25519()
        .public()
        .to_peer_id();
    let other = libp2p::identity::Keypair::generate_ed25519()
        .public()
        .to_peer_id();

    assert!(punch_target_matches_peer(Some(expected), expected));
    assert!(!punch_target_matches_peer(Some(expected), other));
    assert!(!punch_target_matches_peer(None, expected));
}

#[test]
fn group_sync_progress_survives_empty_cursor_page_requests() {
    let group_id = "group:550e8400-e29b-41d4-a716-446655440000".to_string();
    let mut progress = GroupSyncProgress::default();
    let explicit = crate::network::gossip::GroupSyncRequest {
        version: crate::network::gossip::GROUP_PROTOCOL_VERSION,
        group_id: group_id.clone(),
        wanted_record_ids: vec!["missing-parent".to_string()],
        cursor: None,
        limit: crate::network::gossip::MAX_GROUP_SYNC_RECORDS,
    };
    record_group_sync_request(&mut progress, &explicit);
    assert_eq!(progress.explicit_rounds, 1);

    let page = crate::network::gossip::GroupSyncRequest {
        version: crate::network::gossip::GROUP_PROTOCOL_VERSION,
        group_id,
        wanted_record_ids: Vec::new(),
        cursor: None,
        limit: crate::network::gossip::MAX_GROUP_SYNC_RECORDS,
    };
    record_group_sync_request(&mut progress, &page);
    assert_eq!(progress.explicit_rounds, 1);
    assert_eq!(progress.explicit_record_ids, ["missing-parent".to_string()].into());

    let retry = crate::network::gossip::GroupSyncRequest {
        version: crate::network::gossip::GROUP_PROTOCOL_VERSION,
        group_id: "group:550e8400-e29b-41d4-a716-446655440000".to_string(),
        wanted_record_ids: vec!["missing-parent".to_string()],
        cursor: None,
        limit: crate::network::gossip::MAX_GROUP_SYNC_RECORDS,
    };
    record_group_sync_request(&mut progress, &retry);
    assert_eq!(progress.explicit_rounds, 2);
}

#[test]
fn network_command_carries_optional_camera_device_selection() {
    let command = NetworkCommand::SetVideoCallCameraDevice {
        device_id: Some("camera-b".to_string()),
    };

    assert!(matches!(
        command,
        NetworkCommand::SetVideoCallCameraDevice {
            device_id: Some(device_id)
        } if device_id == "camera-b"
    ));
}

#[test]
fn incoming_call_reject_does_not_target_active_or_wrong_kind_calls() {
    let active_voice = active_call("call-active", CallKind::Voice, ActiveCallPhase::Active);
    let incoming_video = active_call("call-video", CallKind::Video, ActiveCallPhase::IncomingRinging);

    assert!(incoming_call_reject_decision(Some(&active_voice), "call-active", CallKind::Voice)
        .is_none());
    assert!(incoming_call_reject_decision(Some(&incoming_video), "call-video", CallKind::Voice)
        .is_none());
}

#[test]
fn ringing_call_liveness_clears_when_peer_or_quic_path_is_lost() {
    assert_eq!(
        ringing_call_peer_liveness_reason(ActiveCallPhase::IncomingRinging, false, true),
        Some("peer_disconnected")
    );
    assert_eq!(
        ringing_call_peer_liveness_reason(ActiveCallPhase::OutgoingRinging, true, false),
        Some("quic_path_lost")
    );
    assert_eq!(
        ringing_call_peer_liveness_reason(ActiveCallPhase::IncomingRinging, true, true),
        None
    );
    assert_eq!(
        ringing_call_peer_liveness_reason(ActiveCallPhase::Active, false, false),
        None
    );
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
        protocol_version: None,
        signed_record_id: None,
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
        protocol_version: None,
        signed_record_id: None,
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
    let tcp_id = libp2p::swarm::ConnectionId::new_unchecked(1);
    let quic_id = libp2p::swarm::ConnectionId::new_unchecked(2);

    let mut registry = PeerTransportRegistry::default();
    assert!(!registry.has_quic(&peer));
    assert_eq!(registry.quic_connection_count(&peer), 0);
    assert_eq!(registry.tcp_connection_count(&peer), 0);

    registry.record_connected(peer, tcp_id, &tcp);
    assert!(!registry.has_quic(&peer));
    assert_eq!(registry.quic_connection_count(&peer), 0);
    assert_eq!(registry.tcp_connection_count(&peer), 1);

    registry.record_connected(peer, quic_id, &quic);
    assert!(registry.has_quic(&peer));
    assert_eq!(registry.quic_connection_count(&peer), 1);
    assert_eq!(registry.tcp_connection_count(&peer), 1);

    let quic_lost = registry.record_disconnected(peer, quic_id, &quic);
    assert!(quic_lost);
    assert!(!registry.has_quic(&peer));
    assert_eq!(registry.quic_connection_count(&peer), 0);
    assert_eq!(registry.tcp_connection_count(&peer), 1);
}

#[test]
fn peer_transport_registry_selects_quic_connection_without_dropping_tcp() {
    let keypair = libp2p::identity::Keypair::generate_ed25519();
    let peer = keypair.public().to_peer_id();
    let tcp: Multiaddr = "/ip4/10.0.0.5/tcp/4242".parse().unwrap();
    let quic: Multiaddr = "/ip4/10.0.0.5/udp/4242/quic-v1".parse().unwrap();
    let tcp_id = libp2p::swarm::ConnectionId::new_unchecked(11);
    let quic_id = libp2p::swarm::ConnectionId::new_unchecked(12);

    let mut registry = PeerTransportRegistry::default();
    registry.record_connected(peer, tcp_id, &tcp);
    registry.record_connected(peer, quic_id, &quic);

    assert_eq!(registry.newest_quic_connection_id(&peer), Some(quic_id));
    assert_eq!(registry.tcp_connection_count(&peer), 1);
}

#[test]
fn voice_stream_failure_events_carry_call_id() {
    let keypair = libp2p::identity::Keypair::generate_ed25519();
    let peer = keypair.public().to_peer_id();
    let event = VoiceStreamEvent::OutboundFailure {
        peer,
        call_id: "call-123".to_string(),
        error: "closed".to_string(),
    };

    match event {
        VoiceStreamEvent::OutboundFailure { call_id, .. } => assert_eq!(call_id, "call-123"),
        _ => panic!("expected outbound failure"),
    }
}

#[test]
fn peer_transport_registry_handles_multiple_quic_connections() {
    let keypair = libp2p::identity::Keypair::generate_ed25519();
    let peer = keypair.public().to_peer_id();
    let quic_a: Multiaddr = "/ip4/10.0.0.5/udp/4242/quic-v1".parse().unwrap();
    let quic_b: Multiaddr = "/ip4/10.0.0.6/udp/5252/quic-v1".parse().unwrap();
    let quic_a_id = libp2p::swarm::ConnectionId::new_unchecked(21);
    let quic_b_id = libp2p::swarm::ConnectionId::new_unchecked(22);

    let mut registry = PeerTransportRegistry::default();
    registry.record_connected(peer, quic_a_id, &quic_a);
    registry.record_connected(peer, quic_b_id, &quic_b);
    assert!(registry.has_quic(&peer));

    let lost_after_first_close = registry.record_disconnected(peer, quic_a_id, &quic_a);
    assert!(!lost_after_first_close);
    assert!(registry.has_quic(&peer));

    let lost_after_second_close = registry.record_disconnected(peer, quic_b_id, &quic_b);
    assert!(lost_after_second_close);
    assert!(!registry.has_quic(&peer));
}

#[test]
fn peer_transport_registry_selects_newest_quic_connection_id() {
    let keypair = libp2p::identity::Keypair::generate_ed25519();
    let peer = keypair.public().to_peer_id();
    let quic_a: Multiaddr = "/ip4/10.0.0.5/udp/4242/quic-v1".parse().unwrap();
    let quic_b: Multiaddr = "/ip4/10.0.0.6/udp/5252/quic-v1".parse().unwrap();
    let quic_a_id = libp2p::swarm::ConnectionId::new_unchecked(31);
    let quic_b_id = libp2p::swarm::ConnectionId::new_unchecked(32);

    let mut registry = PeerTransportRegistry::default();
    registry.record_connected(peer, quic_a_id, &quic_a);
    registry.record_connected(peer, quic_b_id, &quic_b);

    assert_eq!(registry.newest_quic_connection_id(&peer), Some(quic_b_id));

    registry.record_disconnected(peer, quic_b_id, &quic_b);
    assert_eq!(registry.newest_quic_connection_id(&peer), Some(quic_a_id));
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
fn outgoing_error_classifier_uses_recent_voice_quic_dial_context() {
    let now = std::time::Instant::now();
    let mut recent = HashMap::<String, RecentDial>::new();
    recent.insert(
        "/ip4/192.168.1.20/udp/9001/quic-v1".to_string(),
        RecentDial {
            source: OutgoingDialSource::VoiceQuic,
            at: now,
        },
    );

    let source = classify_outgoing_error_source(
        "Transport([(/ip4/192.168.1.20/udp/9001/quic-v1, Other(...))])",
        Some("/ip4/192.168.1.20/udp/9001/quic-v1"),
        &recent,
        false,
        false,
        false,
        now,
    );
    assert_eq!(source, OutgoingDialSource::VoiceQuic);
}

#[test]
fn quic_addresses_for_peer_filters_known_addresses_to_quic_only() {
    let keypair = libp2p::identity::Keypair::generate_ed25519();
    let peer = keypair.public().to_peer_id();
    let quic_a: Multiaddr = "/ip4/10.0.0.5/udp/4242/quic-v1".parse().unwrap();
    let tcp: Multiaddr = "/ip4/10.0.0.5/tcp/4242".parse().unwrap();
    let quic_b: Multiaddr = "/ip6/::1/udp/5252/quic-v1".parse().unwrap();
    let mut local_peers = HashMap::new();
    local_peers.insert(peer, vec![quic_a.clone(), tcp, quic_b.clone()]);

    let addrs = quic_addresses_for_peer(&local_peers, &peer);

    assert_eq!(addrs, vec![quic_a, quic_b]);
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

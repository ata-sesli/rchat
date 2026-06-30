use libp2p::{
    dcutr, gossipsub, identify, identity::Keypair, kad, ping, relay, request_response,
    swarm::NetworkBehaviour,
};

use super::direct_message::{DirectMessageRequest, DirectMessageResponse};
use crate::live::broadcast::protocol::{BroadcastFrameRequest, BroadcastFrameResponse};

#[derive(NetworkBehaviour)]
pub struct RChatBehaviour {
    // The "Town Crier" - For group chat messages
    pub gossipsub: gossipsub::Behaviour,

    // The "Phone Book" - For finding peers and storing history logs
    pub kademlia: kad::Behaviour<kad::store::MemoryStore>,

    // The "ID Card" - Exchanges version/public key info on connect
    pub identify: identify::Behaviour,

    // The "Pulse" - Keeps connections alive and measures latency
    pub ping: ping::Behaviour,

    // The "Direct Line" - For 1:1 chat messages
    pub direct_message:
        request_response::cbor::Behaviour<DirectMessageRequest, DirectMessageResponse>,

    // Dedicated voice-call media transport
    pub voice_call: crate::network::voice_stream::Behaviour,
    // Dedicated video-call media transport
    pub video_call: crate::network::voice_stream::Behaviour,
    // Dedicated DM screen-broadcast media transport
    pub broadcast_stream: crate::network::voice_stream::Behaviour,
    // Dedicated DM screen-broadcast media transport
    pub broadcast: request_response::cbor::Behaviour<BroadcastFrameRequest, BroadcastFrameResponse>,

    // Circuit Relay Client - for NAT traversal via public relays
    pub relay_client: relay::client::Behaviour,

    // DCUtR - Direct Connection Upgrade through Relay (hole punching)
    pub dcutr: dcutr::Behaviour,
}

impl RChatBehaviour {
    pub fn new(key: Keypair, relay_client: relay::client::Behaviour) -> Self {
        let peer_id = key.public().to_peer_id();

        // 1. Gossipsub (Group Chat)
        let gossipsub_config = gossipsub::Config::default();
        let gossipsub = gossipsub::Behaviour::new(
            gossipsub::MessageAuthenticity::Signed(key.clone()),
            gossipsub_config,
        )
        .expect("Invalid gossipsub config");

        // 2. Kademlia (Discovery)
        let store = kad::store::MemoryStore::new(peer_id);
        let kademlia = kad::Behaviour::new(peer_id, store);

        // 3. MDNS (Local Discovery) - REPLACED by native mdns-sd (see network/mdns_sd.rs)
        // We use native OS mDNS service to avoid UDP port 5353 conflicts and VPN routing issues.

        // 4. Identify (Handshake)
        let identify =
            identify::Behaviour::new(identify::Config::new("rchat/1.0.0".into(), key.public()));

        // 5. Ping (Health)
        let ping = ping::Behaviour::default();

        // 6. Request-Response (Direct 1:1 Messages)
        let direct_message = request_response::cbor::Behaviour::new(
            [(
                libp2p::StreamProtocol::new("/rchat/dm/1.0.0"),
                request_response::ProtocolSupport::Full,
            )],
            request_response::Config::default(),
        );

        // 6b. Stream protocol (Voice Frames)
        let voice_call = crate::network::voice_stream::Behaviour::new(libp2p::StreamProtocol::new(
            crate::live::voice::protocol::VOICE_PROTOCOL,
        ));

        // 6c. Stream protocol (Video Frames)
        let video_call = crate::network::voice_stream::Behaviour::new(libp2p::StreamProtocol::new(
            crate::live::video::video::VIDEO_PROTOCOL,
        ));

        // 6d. Stream protocol (Screen Broadcast Frames)
        let broadcast_stream = crate::network::voice_stream::Behaviour::new(
            libp2p::StreamProtocol::new(crate::live::broadcast::broadcast::BROADCAST_PROTOCOL),
        );

        // 6e. Request-Response (legacy Broadcast Frames)
        let broadcast = request_response::cbor::Behaviour::new(
            [(
                libp2p::StreamProtocol::new("/rchat/broadcast/screen/request-response/1.0.0"),
                request_response::ProtocolSupport::Full,
            )],
            request_response::Config::default(),
        );

        // 7. DCUtR (Hole Punching)
        let dcutr = dcutr::Behaviour::new(peer_id);

        Self {
            gossipsub,
            kademlia,
            identify,
            ping,
            direct_message,
            voice_call,
            video_call,
            broadcast_stream,
            broadcast,
            relay_client,
            dcutr,
        }
    }
}
